//! The evaluation stack: what a cell evaluating a node holds across its steps, and the machine that
//! hands each node in flight to the language's evaluator and acts on the instruction it answers
//! with ([README.md § The evaluation stack](README.md#the-evaluation-stack)).
//!
//! The evaluator never ends a step. Handed the step, a [`Node`] and what was last
//! [`Delivered`] to it, it answers with what the node needs next ([`Next`]): a part evaluated, a
//! block run, a frame it spawned and waits on, a tail, a kept cell read beside, its value or a
//! fault. Everything here is `Copy` and `Drop`-free, so a node rests in the cell's scratch habitat
//! across a park, laid as a [`Pending`] entry and parked as the cell's scratch state
//! ([`KScratch::Stack`]), and its dead entries never sit in a region.
//!
//! The machine runs every called frame and the frame of the code an `EVAL` runs, its body runner
//! at the bottom, and a top-level statement's cell, born holding [`KBirth::Evaluate`], whose own
//! node is its bottom and goes to the cell's home. A part, a block or a tie's eager parts are
//! pushed above the node that asks for them and run in the same step; the cell parks only on a
//! frame a node spawned.

use std::cell::Cell;

use crate::knot::{KActivationView, KInner, KValue};
use crate::memory::{Bump, BumpVec, resident};
use crate::scheduler::{
    Action, Held, Received, Seek, Slot as Asked, Step, StepError, Taken, Use, Work,
};
use crate::scope::{Link, Segment, Site};
use crate::values::{ArenaRef, Chain, KeptCell, Value};

use super::body::{Runner, Stopped, Turn, Woken, ended, entered_body, run, stop, turn};
use super::bundle::{Catching, Entered, KBirth, KBundle, KScratch, KState, Within};
use super::exit::Exit;
use super::record::{Evaluated, Owed, Program};

/// A node in flight: what it evaluates and the view it reads names through, what it owes a frame
/// it finishes and what it runs within, the [`Use`] its value is placed by, relative to the cell
/// ([`Use::within`]), and how far it got — the evaluator's stage, the parts delivered to it so
/// far, one slot each in the order asked, and what a `TRY` or `CATCH` has copied out of a fault.
#[derive(Clone, Copy)]
pub struct Node<'graph, 'here, 'scratch> {
    pub program: &'graph Program<'graph>,
    pub node: Evaluated,
    pub view: KActivationView<'graph, 'here>,
    pub owed: Option<Owed<'here>>,
    pub within: Within<'graph, 'here>,
    pub use_: Use,
    pub stage: u32,
    /// Laid through the cell's scratch writer at the first part asked; empty until then.
    pub received: &'scratch [Cell<Option<Held<'graph, 'here>>>],
    pub caught: Option<Catching<'graph, 'here>>,
}

impl<'graph, 'here> Node<'graph, 'here, '_> {
    /// `node`, read through `view`, owing `owed`, its value placed by `use_`, before anything is
    /// asked of it.
    pub fn born(
        program: &'graph Program<'graph>,
        node: Evaluated,
        view: KActivationView<'graph, 'here>,
        (owed, within): (Option<Owed<'here>>, Within<'graph, 'here>),
        use_: Use,
    ) -> Self {
        Node {
            program,
            node,
            view,
            owed,
            within,
            use_,
            stage: 0,
            received: &[],
            caught: None,
        }
    }
}

/// What was delivered to a node: the value of what it asked for, or the fault that ended it.
#[derive(Clone, Copy)]
pub enum Delivered<'graph, 'here> {
    Value(Held<'graph, 'here>),
    Fault(KInner<'graph, 'here>),
}

/// What a node needs next: the evaluator's answer.
pub enum Next<'graph, 'here> {
    /// The value of the part at `site`, asked with `use_`, relative to this node.
    Part { site: Site, use_: Use },
    /// A block run: a hoisted operand, a `USING` body, a `MATCH` or `TRY` arm, or a `TRY` or
    /// `CATCH` body.
    Block(BlockEntry<'graph, 'here>),
    /// The node spawned a frame — a call's, an `EVAL`'s, or a `USING` or `BUILD` over composed
    /// code — and waits on it.
    Spawned(Asked),
    /// The node's value, placed where its `Use` says.
    Value(Held<'graph, 'here>),
    /// The fault the node ends with: what lies inside it.
    Raised(KInner<'graph, 'here>),
    /// The node is in tail position and its call keeps the contract: the cell hops.
    Tail(Tailed<'graph, 'here>),
    /// A `TRY` or `CATCH` reads the kept cell beside, and the node resumes beside it.
    Beside(KeptCell),
    /// The machinery's own failure.
    Failed(StepError),
}

/// How a tail hops into a call's frame: sought, or under the frame's own settlement.
#[expect(
    clippy::large_enum_variant,
    reason = "a seek carries what a miss settles; boxing it would allocate per tail call"
)]
pub enum Tailed<'graph, 'here> {
    Seeking(Work<'graph, 'here, KBundle>, Seek<'graph>),
    Under(Work<'graph, 'here, KBundle>, ArenaRef),
}

/// A block a node runs: its segment, the view it sits in, how its parameters are bound, what it
/// owes where it is a tail arm, the use its value is asked with, relative to the node, and what it
/// runs within.
#[derive(Clone, Copy)]
pub struct BlockEntry<'graph, 'here> {
    pub segment: Segment,
    pub enclosing: KActivationView<'graph, 'here>,
    pub entered: Entered<'graph, 'here>,
    pub owed: Option<Owed<'here>>,
    pub use_: Use,
    pub within: Within<'graph, 'here>,
}

/// The language's evaluator: what the stack calls on a node in flight.
pub type Evaluator<'graph> = for<'a, 'step, 'here, 'scratch, 'lent> fn(
    &mut Step<'a, 'graph, 'step, 'here, 'scratch, 'lent, KBundle, Taken, Taken>,
    &mut Node<'graph, 'here, 'scratch>,
    Option<Delivered<'graph, 'here>>,
) -> Next<'graph, 'here>;

/// One entry of the stack, laid in the cell's scratch habitat, above the entry below it.
#[derive(Clone, Copy)]
pub struct Pending<'graph, 'here, 'scratch> {
    pub task: Task<'graph, 'here, 'scratch>,
    pub below: Option<&'scratch Pending<'graph, 'here, 'scratch>>,
}

/// What a stack entry holds: a node in flight, an inline block's runner beside the use its value
/// is asked with, relative to the cell, or a tie's eager parts being gathered.
#[derive(Clone, Copy)]
pub enum Task<'graph, 'here, 'scratch> {
    Node(Node<'graph, 'here, 'scratch>),
    Body {
        runner: Runner<'graph, 'here>,
        use_: Use,
    },
    Supply(Supply<'graph, 'here, 'scratch>),
}

/// The eager parts a refused tie named, gathered one at a time before the tie is retried: each
/// part's site, read through the runner's view within what it runs within, and a slot for each
/// value, filled in the order asked.
#[derive(Clone, Copy)]
pub struct Supply<'graph, 'here, 'scratch> {
    pub program: &'graph Program<'graph>,
    pub view: KActivationView<'graph, 'here>,
    pub within: Within<'graph, 'here>,
    pub sites: &'scratch [Site],
    pub values: &'scratch [Cell<Option<KValue<'graph, 'here>>>],
}

/// What the stack's bottom is: the cell's own body runner, resting in its continuation — a
/// frame's, or the code an `EVAL` runs — or, in a top-level statement's evaluation cell, nothing,
/// its first node its own.
#[derive(Clone, Copy)]
#[expect(
    clippy::large_enum_variant,
    reason = "a stack's bottom lives one step, and boxing a runner would allocate per step"
)]
enum Bottom<'graph, 'here> {
    Runner(Runner<'graph, 'here>),
    Statement,
}

/// What an entry hands the entry below it as it pops: a value, a fault, or the eager parts a tie
/// named, every one supplied.
enum Outcome<'graph, 'here, 'scratch> {
    Value(Held<'graph, 'here>),
    Fault(KInner<'graph, 'here>),
    Supplied(Supply<'graph, 'here, 'scratch>),
}

/// A cell's stack: its bottom, the entry on top, and the entries laid below it, innermost first.
struct Stack<'graph, 'here, 'scratch> {
    bottom: Bottom<'graph, 'here>,
    top: Option<Task<'graph, 'here, 'scratch>>,
    below: Option<&'scratch Pending<'graph, 'here, 'scratch>>,
}

impl<'graph, 'here, 'scratch> Stack<'graph, 'here, 'scratch> {
    /// The stack a cell parked, its top read off the entry it laid.
    fn rested(
        bottom: Bottom<'graph, 'here>,
        laid: &'scratch Pending<'graph, 'here, 'scratch>,
    ) -> Self {
        Stack {
            bottom,
            top: Some(laid.task),
            below: laid.below,
        }
    }

    /// Put `task` on top, laying the entry it covers in scratch.
    fn push(
        &mut self,
        step: &Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
        task: Task<'graph, 'here, 'scratch>,
    ) {
        if let Some(top) = self.top.take() {
            self.below = Some(laid(step, top, self.below));
        }
        self.top = Some(task);
    }

    /// Take the top off: the entry below it is on top again.
    fn pop(&mut self) {
        self.top = self.below.map(|pending| pending.task);
        self.below = self.below.and_then(|pending| pending.below);
    }

    /// What the cell parks with: its bottom in its continuation, and everything above it laid in
    /// scratch.
    fn parked(
        &self,
        step: &Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    ) -> (
        KState<'graph, 'here>,
        Option<KScratch<'graph, 'here, 'scratch>>,
    ) {
        let state = match self.bottom {
            Bottom::Runner(runner) => KState::Runner(runner),
            Bottom::Statement => KState::Evaluating,
        };
        let laid = self
            .top
            .map(|top| KScratch::Stack(laid(step, top, self.below)));
        (state, laid)
    }
}

/// A step that took both its states.
type Taking<'a, 'graph, 'step, 'here, 'scratch, 'lent> =
    Step<'a, 'graph, 'step, 'here, 'scratch, 'lent, KBundle, Taken, Taken>;

/// A frame's first step, once its runner is laid down: the stack, its runner at the bottom, run
/// on the frame's chain `types`.
pub(super) fn running<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    runner: Runner<'graph, 'here>,
    types: &Chain<'_, 'here, '_, '_>,
) -> Action<'graph, KBundle> {
    let stack = Stack {
        bottom: Bottom::Runner(runner),
        top: None,
        below: None,
    };
    ran(step, stack, None, types)
}

/// A frame woken, its runner at the bottom of the stack it parked in scratch: what the frame its
/// top node spawned delivered handed to that node.
pub(super) fn woken<'graph, 'here, 'scratch>(
    mut step: Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    runner: Runner<'graph, 'here>,
    scratch: Option<KScratch<'graph, 'here, 'scratch>>,
    types: &Chain<'_, 'here, '_, '_>,
) -> Action<'graph, KBundle> {
    let Some(KScratch::Stack(laid)) = scratch else {
        return step.failed(StepError::Refused);
    };
    let outcome = match received(&mut step, types) {
        Ok(outcome) => outcome,
        Err(error) => return step.failed(error),
    };
    ran(
        step,
        Stack::rested(Bottom::Runner(runner), laid),
        outcome,
        types,
    )
}

/// The step of a top-level statement's evaluation cell: its node born from its birth, or its stack
/// read off scratch, run on one chain — the caching chain where it routes or its body caches, its
/// region's elsewhere.
pub(super) fn evaluating<'graph, 'here, 'scratch>(
    step: Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    state: KState<'graph, 'here>,
    scratch: Option<KScratch<'graph, 'here, 'scratch>>,
) -> Action<'graph, KBundle> {
    let (stack, woken) = match (state, scratch) {
        (
            KState::Born(KBirth::Evaluate {
                program,
                node,
                view,
                within,
            }),
            _,
        ) => {
            // The cell's own value goes to its home.
            let born = Node::born(program, node, view, (None, within), Use::Forwards);
            let stack = Stack {
                bottom: Bottom::Statement,
                top: Some(Task::Node(born)),
                below: None,
            };
            (stack, false)
        }
        (KState::Evaluating, Some(KScratch::Stack(laid))) => {
            (Stack::rested(Bottom::Statement, laid), true)
        }
        _ => return step.failed(StepError::Refused),
    };
    let (code, facts) = step.door();
    let Some(segment) = stack.top.map(|top| segment_of(&top)) else {
        return step.failed(StepError::Refused);
    };
    let within = stack.top.map(|top| within_of(&top));
    let caches = facts.caches(code.shape(segment));
    let routes = within.is_some_and(|within| within.routes);
    if routes || caches {
        let types = step.caching_types();
        statement(step, stack, woken, &types)
    } else {
        let types = step.types();
        statement(step, stack, woken, &types)
    }
}

/// [`evaluating`] on the chain `types` it picked.
fn statement<'graph, 'here, 'scratch>(
    mut step: Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    stack: Stack<'graph, 'here, 'scratch>,
    woken: bool,
    types: &Chain<'_, 'here, '_, '_>,
) -> Action<'graph, KBundle> {
    let outcome = match woken {
        true => match received(&mut step, types) {
            Ok(outcome) => outcome,
            Err(error) => return step.failed(error),
        },
        false => None,
    };
    ran(step, stack, outcome, types)
}

/// The segment of the body an entry runs in.
fn segment_of(task: &Task<'_, '_, '_>) -> Segment {
    match task {
        Task::Node(node) => node.view.segment(),
        Task::Body { runner, .. } => runner.view().segment(),
        Task::Supply(supply) => supply.view.segment(),
    }
}

/// What an entry runs within.
fn within_of<'graph, 'here>(task: &Task<'graph, 'here, '_>) -> Within<'graph, 'here> {
    match task {
        Task::Node(node) => node.within,
        Task::Body { runner, .. } => runner.handing(),
        Task::Supply(supply) => supply.within,
    }
}

/// What the frame a woken cell's top node spawned delivered: nothing for a step entered beside a
/// kept cell.
fn received<'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    types: &Chain<'_, 'here, '_, '_>,
) -> Result<Option<Outcome<'graph, 'here, 'scratch>>, StepError> {
    if step.entered_beside() {
        return Ok(None);
    }
    match step.results_with(types).next() {
        Some(Ok(Received::Here(held))) => Ok(Some(Outcome::Value(held))),
        Some(Ok(Received::Fault(inner))) => Ok(Some(Outcome::Fault(inner))),
        Some(Ok(Received::Scratch(_))) | None => Err(StepError::Unredeemable),
        Some(Err(error)) => Err(error),
    }
}

/// Run the stack until a node parks, reads beside, tails, or the bottom ends the cell: each round
/// hands the top entry `outcome`, what the entry above it last handed down, and acts on its answer.
fn ran<'graph, 'here, 'scratch>(
    mut step: Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    mut stack: Stack<'graph, 'here, 'scratch>,
    mut outcome: Option<Outcome<'graph, 'here, 'scratch>>,
    types: &Chain<'_, 'here, '_, '_>,
) -> Action<'graph, KBundle> {
    let caches = {
        let (code, facts) = step.door();
        let segment = match (stack.bottom, stack.top) {
            (Bottom::Runner(runner), _) => runner.view().segment(),
            (Bottom::Statement, Some(top)) => segment_of(&top),
            (Bottom::Statement, None) => return step.failed(StepError::Refused),
        };
        facts.caches(code.shape(segment))
    };
    loop {
        match stack.top {
            Some(Task::Node(mut node)) => {
                let delivered = match outcome.take() {
                    None => None,
                    Some(Outcome::Value(held)) => Some(Delivered::Value(held)),
                    Some(Outcome::Fault(inner)) => Some(Delivered::Fault(inner)),
                    Some(Outcome::Supplied(_)) => return step.failed(StepError::Unredeemable),
                };
                let next = (node.program.evaluator())(&mut step, &mut node, delivered);
                stack.top = Some(Task::Node(node));
                match next {
                    Next::Part { site, use_ } => {
                        let part = Node::born(
                            node.program,
                            Evaluated::Part(site),
                            node.view,
                            (None, node.within),
                            use_.within(node.use_),
                        );
                        stack.push(&step, Task::Node(part));
                    }
                    Next::Block(entry) => {
                        #[cfg(debug_assertions)]
                        {
                            let (code, facts) = step.door();
                            debug_assert_eq!(
                                facts.caches(code.shape(entry.segment)),
                                caches,
                                "a block is classed with the body that owns it"
                            );
                        }
                        match entered_body(&mut step, types, node.program, entry) {
                            Ok(runner) => {
                                let use_ = entry.use_.within(node.use_);
                                stack.push(&step, Task::Body { runner, use_ });
                            }
                            Err(Stopped::Raised(inner)) => outcome = Some(Outcome::Fault(inner)),
                            Err(Stopped::Broken(error)) => return step.failed(error),
                        }
                    }
                    Next::Spawned(asked) => {
                        let (state, scratch) = stack.parked(&step);
                        return step.park(asked, run, state, scratch);
                    }
                    Next::Value(held) => {
                        stack.pop();
                        outcome = Some(Outcome::Value(held));
                    }
                    Next::Raised(inner) => {
                        stack.pop();
                        outcome = Some(Outcome::Fault(inner));
                    }
                    // A node in tail position: everything below it only hands its value on, so it
                    // dies with the cell.
                    Next::Tail(Tailed::Seeking(work, seek)) => {
                        return step.tail_seeking(work, seek);
                    }
                    Next::Tail(Tailed::Under(work, build)) => return step.tail_under(work, build),
                    Next::Beside(kept) => {
                        let (state, scratch) = stack.parked(&step);
                        return step.read_beside(kept, run, state, scratch);
                    }
                    Next::Failed(error) => return step.failed(error),
                }
            }
            Some(Task::Body { mut runner, use_ }) => {
                let local = Bump::new();
                let woken = match woken_of(outcome.take(), &local) {
                    Ok(woken) => woken,
                    Err(error) => return step.failed(error),
                };
                let turned = turn(&mut step, &mut runner, woken, types);
                stack.top = Some(Task::Body { runner, use_ });
                match turned {
                    Turn::Ask {
                        node,
                        use_: asked,
                        owed,
                    } => {
                        let view = runner.view();
                        let within = runner.handing();
                        let use_ = asked.within(use_);
                        let born = Node::born(runner.program(), node, view, (owed, within), use_);
                        stack.push(&step, Task::Node(born));
                    }
                    Turn::Supply(sites) => {
                        let supply = supplied(&step, &runner, sites);
                        stack.push(&step, Task::Supply(supply));
                    }
                    // A block's value stays in the body it runs in, whose chain reads its types; a
                    // tail arm's is handed on as it is held, to the frame's end, which alone owes
                    // the contract.
                    Turn::Ended => {
                        let held = runner.result().unwrap_or(Held::bare(Value::Null));
                        if runner.owes() {
                            stack.pop();
                            outcome = Some(Outcome::Value(held));
                            continue;
                        }
                        let check = caches.then_some(types);
                        let Some(held) = step.crossed_as(use_, held, check.map(|types| types as _))
                        else {
                            return step.failed(StepError::Stale);
                        };
                        stack.pop();
                        outcome = Some(Outcome::Value(held));
                    }
                    Turn::Stopped(Stopped::Raised(inner)) => {
                        stack.pop();
                        outcome = Some(Outcome::Fault(inner));
                    }
                    Turn::Stopped(Stopped::Broken(error)) => return step.failed(error),
                }
            }
            Some(Task::Supply(supply)) => {
                match outcome.take() {
                    None => {}
                    Some(Outcome::Value(held)) => {
                        match supply.values.iter().find(|slot| slot.get().is_none()) {
                            Some(slot) => slot.set(Some(held.value)),
                            None => return step.failed(StepError::Unredeemable),
                        }
                    }
                    Some(Outcome::Fault(inner)) => {
                        stack.pop();
                        outcome = Some(Outcome::Fault(inner));
                        continue;
                    }
                    Some(Outcome::Supplied(_)) => return step.failed(StepError::Unredeemable),
                }
                match supply.values.iter().position(|slot| slot.get().is_none()) {
                    Some(at) => {
                        let part = Node::born(
                            supply.program,
                            Evaluated::Part(supply.sites[at]),
                            supply.view,
                            (None, supply.within),
                            Use::Keeps,
                        );
                        stack.push(&step, Task::Node(part));
                    }
                    None => {
                        stack.pop();
                        outcome = Some(Outcome::Supplied(supply));
                    }
                }
            }
            None => match stack.bottom {
                Bottom::Runner(mut runner) => {
                    let local = Bump::new();
                    let woken = match woken_of(outcome.take(), &local) {
                        Ok(woken) => woken,
                        Err(error) => return step.failed(error),
                    };
                    let turned = turn(&mut step, &mut runner, woken, types);
                    stack.bottom = Bottom::Runner(runner);
                    match turned {
                        Turn::Ask { node, use_, owed } => {
                            let view = runner.view();
                            let within = runner.handing();
                            let born =
                                Node::born(runner.program(), node, view, (owed, within), use_);
                            stack.push(&step, Task::Node(born));
                        }
                        Turn::Supply(sites) => {
                            let supply = supplied(&step, &runner, sites);
                            stack.push(&step, Task::Supply(supply));
                        }
                        Turn::Ended => return ended(step, runner, types),
                        Turn::Stopped(stopped) => return stop(step, runner, stopped, types),
                    }
                }
                Bottom::Statement => {
                    return match outcome {
                        Some(Outcome::Value(held)) => step.finish(held),
                        Some(Outcome::Fault(inner)) => {
                            if caches {
                                let link = Link { line: None, inner };
                                Exit::new(step.types(), types, &Bump::new()).link(&link);
                            }
                            step.fault(None, inner)
                        }
                        _ => step.failed(StepError::Unredeemable),
                    };
                }
            },
        }
    }
}

/// What a runner is handed back of `outcome`: eager parts supplied by site, sorted, laid in
/// `local`.
fn woken_of<'x, 'graph, 'here>(
    outcome: Option<Outcome<'graph, 'here, '_>>,
    local: &'x Bump,
) -> Result<Option<Woken<'x, 'graph, 'here>>, StepError> {
    Ok(match outcome {
        None => None,
        Some(Outcome::Value(held)) => Some(Woken::Value(held)),
        Some(Outcome::Fault(inner)) => Some(Woken::Fault(inner)),
        Some(Outcome::Supplied(supply)) => {
            let mut supplied: BumpVec<'x, (Site, KValue<'graph, 'here>)> =
                BumpVec::with_capacity_in(supply.sites.len(), local);
            for (site, value) in supply.sites.iter().zip(supply.values) {
                supplied.push((*site, value.get().ok_or(StepError::Unredeemable)?));
            }
            supplied.sort_unstable_by_key(|(site, _)| *site);
            Some(Woken::Supplied(supplied.leak()))
        }
    })
}

/// The eager parts at `sites`, which `runner`'s tie named, to gather: a slot for each laid in
/// scratch.
fn supplied<'graph, 'here, 'scratch>(
    step: &Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    runner: &Runner<'graph, 'here>,
    sites: &'scratch [Site],
) -> Supply<'graph, 'here, 'scratch> {
    Supply {
        program: runner.program(),
        view: runner.view(),
        within: runner.handing(),
        sites,
        values: step.scratch_writer().fill(sites.len(), |_| Cell::new(None)),
    }
}

/// `task` laid in the cell's scratch habitat above `below`.
fn laid<'graph, 'here, 'scratch>(
    step: &Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    task: Task<'graph, 'here, 'scratch>,
    below: Option<&'scratch Pending<'graph, 'here, 'scratch>>,
) -> &'scratch Pending<'graph, 'here, 'scratch> {
    resident(step.scratch_writer(), Pending { task, below })
}
