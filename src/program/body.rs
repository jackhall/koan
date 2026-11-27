//! The body runner: the one step that performs a body's units, in the order its shape emitted
//! them — the top level's, as a tenant of the root born as each call's root work, and a called
//! body's, the code an `EVAL` runs, or a block's, in the frame's own cell.
//!
//! The runner claims nothing ahead and no unit has a cell of its own. A unit of type binders is
//! bound to the handles the load pass fixed for it when they are closed, and otherwise declared
//! through the elaborator's door and bound in the same step; a module binder's body runs
//! inline, its activation laid down in the running region and the enclosing body's place kept in a
//! resident [`Outer`]; a component the knot ties is tied, and a refusal naming eager parts becomes
//! one evaluation per part; a lone `LET` and a statement that binds nothing are one evaluation
//! each. The runner answers each unit with what it needs ([`turn`]): at the top level each
//! evaluation is a `Fresh` child of the root, every eager part asked for at one park; in a frame
//! and in a block it is pushed onto the cell's [evaluation stack](super::stack) and run in the
//! same step. See [README.md § The body runner](README.md#the-body-runner).
//!
//! **Faults.** A unit that receives a fault, or that the program refuses — a tie or a declaration
//! refused, a callee that is no function, arguments that do not fit it — ends the body with a
//! fault: a frame or a block ends with it, owing its trace line where it owes one, and the top
//! level records it — rendering one it raised itself to the error sink — marks the run uncaught,
//! and leaves its view at rest as it always does. [`StepError::Refused`] is left for invariant
//! breaks alone.
//!
//! **A bind site makes its instance.** At its entry and at each statement seam that is a bind site
//! of its base's [binding graph](../../design/binding-graph.md) — the program's, or the
//! composition it runs under — a frame makes the instance of the site's node under what its slots
//! and captures bind, through the instances of that graph its step lends, and keeps the handle in
//! its frame's slab, which its blocks and module bodies share ([`instanced`]). A part a unit holds
//! in place is evaluated before the unit, bound in its held slot and its bind site visited, as a
//! `LET` holding it would be at a statement of its own ([`held_part`]). A base record's
//! types close under what the frame's slots, held slots and captures bind at its last bind site
//! or seam ([`frame_binding`]). An `EVAL` frame reports the first registration of its code overlapping a
//! builtin, as the load typed it.
//!
//! **An untraced body runs as code.** A call of a function whose body is untraced code runs under
//! that code's composition, binding its parameters, as an `EVAL` of the code runs under its own:
//! its frame activates the composition's root over the code's bindings and what its declaring
//! scope offered it, and a name the code reads that nothing binds faults the frame.
//!
//! **A declared parameter is an ascription.** A frame binds each value parameter to its argument
//! retyped to the parameter's declared type, with the call's solution substituted. So is an
//! annotated binder, `LET <name> <type> = <value>`: its value is checked against the type, unless
//! the load settled it, and retyped to it before it is bound.
//!
//! **A frame keeps what it was handed.** Its first step lays a table in its region holding, at the
//! true home of each, its callee and each parameter's argument; a binder whose evaluation delivered
//! a value pinned at a home of its own keeps that carrier too. A name read of the frame's body reads
//! the kept carrier beside the value, so the value crosses on from where it rests
//! ([`FrameKept`]).
//!
//! **Frames end under a contract.** A called frame owes its caller a value satisfying its declared
//! return, with its own type-parameter solution substituted, retyped to it; a miss is an error
//! value. When the frame's last unit is a statement that binds nothing, the runner asks for it in
//! **tail position**, owing that [`Contract`]: a call there whose return keeps it hops, and any
//! other value comes back to the runner, which holds it to the contract where the frame ends. A
//! frame a tail hop reached is checked against its own return and retyped to the one the hop owed,
//! so a chain of hops returns at the outermost contract. An `EVAL`'s frame owes the `EVAL`'s
//! declared return as a called frame owes its own. A block is in tail position only as a tail
//! `MATCH` arm, which owes its frame's contract.
//!
//! **A caching body interns in scratch.** Each step picks one chain ([`run`]) by its frame's
//! caching bit, read from the folds at the frame's first step and carried on its `Within` to its
//! blocks and the nodes on its stack. A caching body keeps its blocks' and its last statement's
//! values rather than forwarding them, so a value leaves it only through an [`Exit`]: the frame's
//! end, or a call out of it.

use std::cell::Cell;

use crate::knot::body_activation;
use crate::knot::module::coerce;
use crate::knot::module::layout;
use crate::knot::module::surface::surface;
use crate::knot::module::view;
use crate::knot::{
    KActivation, KActivationView, KFault, KInner, KReading, KValue, Knotted, Supplied, Tied, built,
    tie, using,
};
use crate::memory::{Bump, BumpAllocator, BumpVec, Lays, Writer, resident};
use crate::parse::{ExpressionPart, Scope, SourceRef};
use crate::scheduler::{
    Action, Held, Placement, Received, Request, Slot as Asked, Step, StepError, Taken, Use,
};
use crate::scope::Encloser;
use crate::scope::{Address, AtRun, Bare, Bound, Lands, Sealing, Tree, UnitIndex, Unknown};
use crate::scope::{Binding, CodeRefused};
use crate::scope::{BodyShape, Component, Position, ShapeKind, Site, Slot, Unit, UnitWork};
use crate::scope::{CaptureSource, ClosureBindings, Code, CodeSyntax, ComposedRoot, Facts};
use crate::scope::{CoercionRefused, Fault, Inner, Line, Trace, Untieable};
use crate::scope::{Coordinate, ParameterBinding, Retypes, Static, Target};
use crate::scope::{IMPLICIT, Payload, ShapeError, relaid_error};
use crate::symbols::{BinderSymbol, KeySymbol, Symbol, TypeSymbol};
use crate::type_lattice::{
    Collector, DeclaredType, DeclaredView, KType, Parametric, Record as TypeRecord, TypeNode,
    Variance, admits_with, substitute_quantified,
};
use crate::values::{
    CALL_ONLY, Chain, ContentDigest, DepotRef, Link, List, Record, Retype, TypeValue, Value,
    binding_at,
};

use super::bundle::{
    Entered, FrameKept, Handed, KBirth, KBundle, KScratch, KState, Keeps, Passing, Within,
};
use super::compose::{born_under, called_function};
use super::exit::Exit;
use super::record::{CallKind, Contract, Ending, Evaluated, Owed, Program, rendered};
use super::stack::BlockEntry;

/// The body runner's state between units: which body it is performing, how far it got, and where
/// it parked.
#[derive(Clone, Copy)]
pub struct Runner<'graph, 'cell> {
    program: &'graph Program<'graph>,
    /// Invariant: it binds. It rides the parked state family, which never crosses.
    activation: &'cell KActivation<'graph, 'cell>,
    unit: u32,
    /// A frame's value so far, held beside where it rests: set when the unit holding its last
    /// statement completes.
    result: Option<Held<'graph, 'cell>>,
    /// The enclosing bodies of a module body being run inline, innermost first.
    outer: Option<&'cell Outer<'graph, 'cell>>,
    level: Level,
    /// What a called frame, an `EVAL`'s or a tail arm's block owes; `None` everywhere else.
    owed: Option<Owed<'cell>>,
    /// The outermost barrier a frame was called through, which its value crosses outwards.
    barrier: Option<Barrier<'graph, 'cell>>,
    stage: Stage,
    /// How many of its units' bind sites and statement seams the runner has bound.
    seen: u32,
    /// What the body runs within: what it hands each evaluation and block it asks for
    /// ([`KBirth::Evaluate`]), and the depot slot a callable born in it is born under.
    within: Within<'graph, 'cell>,
}

/// Whether the runner performs the top level, a frame — a called body or the code an `EVAL` runs
/// — or a block.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Top,
    Frame,
    Block,
}

/// Where the runner parked.
#[derive(Clone, Copy)]
enum Stage {
    /// Nothing in flight: perform `unit` next.
    Next,
    /// One evaluation asked for the unit's value.
    Evaluation,
    /// One evaluation per eager part a refused tie named; the sites are in scratch.
    Supplying,
    /// One evaluation asked for the part at the site the unit holds in place, ahead of the unit.
    Holding(Site),
}

/// An enclosing body's place, kept while a module body it binds runs inline: written once when the
/// module body is entered, in the running region.
#[derive(Clone, Copy)]
struct Outer<'graph, 'cell> {
    activation: &'cell KActivation<'graph, 'cell>,
    unit: u32,
    result: Option<Held<'graph, 'cell>>,
    /// The module binder's slot in `activation`, bound when the body ends.
    binder: Slot,
    /// The runner's `seen` in `activation`, where its statement seams resume.
    seen: u32,
    outer: Option<&'cell Outer<'graph, 'cell>>,
}

/// Why a unit could not go on: a fault reached it or it raised one — what lies inside — or an
/// invariant broke.
pub(super) enum Stopped<'graph, 'a> {
    Raised(KInner<'graph, 'a>),
    Broken(StepError),
}

impl From<StepError> for Stopped<'_, '_> {
    fn from(error: StepError) -> Self {
        Stopped::Broken(error)
    }
}

/// What a runner is handed back where it asked for something: the value of the evaluation it
/// asked for, or the fault it ended with, or the values of the eager parts a tie named, by site.
pub(super) enum Woken<'x, 'graph, 'here> {
    Value(Held<'graph, 'here>),
    Fault(KInner<'graph, 'here>),
    Supplied(&'x [(Site, KValue<'graph, 'here>)]),
}

/// What a runner needs next: one evaluation of `node`, asked with `use_` and owing `owed` where
/// it is the body's tail; one evaluation per eager part a refused tie named, at `sites`, laid in
/// scratch; or nothing more, the body ended — or stopped.
pub(super) enum Turn<'graph, 'here, 'scratch> {
    Ask {
        node: Evaluated,
        use_: Use,
        owed: Option<Owed<'here>>,
    },
    Supply(&'scratch [Site]),
    Ended,
    Stopped(Stopped<'graph, 'here>),
}

#[cfg(test)]
thread_local! {
    /// How many times a runner has tied again with the parts a tie named supplied: what a test
    /// reads to see one retry however many parts.
    pub(super) static SUPPLIED_TIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many times a call by name's record was admitted against its callee's parameters: what
    /// a test reads to see a quantified callee's group solved once per call, in its frame.
    pub(crate) static ADMITTED_BY_NAME: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A step that took both its states.
type Taking<'a, 'graph, 'step, 'here, 'scratch, 'lent> =
    Step<'a, 'graph, 'step, 'here, 'scratch, 'lent, KBundle, Taken, Taken>;

/// The step: the one body runner. Born as the top level's root work, as a call's or an `EVAL`'s
/// frame, or as a block.
///
/// Each step runs on one chain ([type tiers](../../design/type-tiers.md#where-each-body-interns)):
/// the caching chain in a caching body, or in the own body of a routing caching frame, which holds
/// its scratch at its first step — and its region's anywhere else.
pub fn run<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let (mut step, scratch) = step.scratch();
    if let KState::Born(KBirth::Evaluate { .. }) | KState::Evaluating = state {
        return super::stack::evaluating(step, state, scratch);
    }
    // A frame of composed code born under the root — its composition was not live — bounces with
    // its syntax, which the drain builds the composition over.
    if let KState::Born(birth) = &state
        && let Some(root) = composed_root(birth)
        && step.composed() != Some(root.digest)
    {
        let KState::Born(birth) = state else {
            unreachable!("matched above")
        };
        let work = crate::scheduler::Work {
            step: run,
            state: birth,
        };
        return step.bounce(work, root);
    }
    let (code, facts) = step.door();
    // A frame reads its body's caching bit at its first step; its runner carries it after.
    let shape = match &state {
        KState::Born(KBirth::Call { callee, .. }) => {
            called_function(*callee).map(|function| function.frame_shape(&code))
        }
        KState::Born(KBirth::Eval { code: value, .. }) => (value.as_code())
            .and_then(|member| member.code())
            .map(|node| node.shape(&code)),
        KState::Born(_) | KState::Runner(_) | KState::Evaluating => None,
    };
    let caches = match &state {
        KState::Runner(runner) => runner.within.caches,
        _ => shape.is_some_and(|shape| facts.caches(shape)),
    };
    let routes = match &state {
        // A frame is a tree cell, and holds its scratch whatever it borrows.
        KState::Born(KBirth::Call { .. }) => caches && step.hold_scratch(),
        KState::Runner(runner) => runner.within.routes,
        KState::Born(_) | KState::Evaluating => false,
    };
    #[cfg(test)]
    if caches {
        let shape = match &state {
            KState::Runner(runner) => Some(code.shape(runner.activation.segment())),
            _ => shape,
        };
        let address = shape.map(|shape| std::ptr::from_ref(shape).addr());
        crate::scheduler::CACHING.set(address);
    }
    if routes || caches {
        let types = step.caching_types();
        ran(step, state, scratch, routes, caches, &types)
    } else {
        let types = step.types();
        ran(step, state, scratch, false, false, &types)
    }
}

/// The root of the composed code the frame `birth` runs: a call's callee born of it, or an
/// `EVAL`'s code composed at run time. `None` for written code and any other birth.
fn composed_root<'here>(birth: &KBirth<'_, 'here>) -> Option<ComposedRoot<'here>> {
    match birth {
        KBirth::Call { callee, .. } => called_function(*callee)?.root(),
        KBirth::Eval { code, .. } | KBirth::Composed { code, .. } => {
            code.as_code()?.code()?.flat()?.root()
        }
        _ => None,
    }
}

/// Whether a frame of `callee` binds each parameter to its argument's own field, retyped: no
/// barrier stands before the function, and it shows the type its body was checked against.
fn bare(callee: KValue<'_, '_>) -> bool {
    (callee.as_callable())
        .is_some_and(|node| node.coerced().is_none() && callee.ktype() == node.declared())
}

/// The table a frame keeps its callee, its arguments and its binders' delivered values in, laid in
/// its region with a carrier for the callee and for each parameter's argument: what `keeps` names,
/// which the wake laid beside every call's birth from what its caller handed on; kept here only
/// where the birth was copied in, which keeps afresh.
fn frame_kept<'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    activation: &KActivation<'graph, 'here>,
    (callee, arguments): (KValue<'graph, 'here>, KValue<'graph, 'here>),
    keeps: Option<&Keeps<'graph, '_>>,
) -> &'here FrameKept<'graph, 'here> {
    let (code, _) = step.door();
    let shape = code.shape(activation.view().segment());
    let slots = step.writer().fill(shape.slots(), |_| Cell::new(None));
    for slot in (0..shape.slots()).map(|slot| Slot(slot as u32)) {
        let name = match shape.slot_name(slot) {
            BinderSymbol::Value(name) if shape.position_of(slot) == Position::PARAMETER => {
                name.symbol()
            }
            BinderSymbol::Type(name) if shape.position_of(slot) == Position::PARAMETER => {
                name.symbol()
            }
            _ => continue,
        };
        let handed = keeps.and_then(|keeps| {
            let at = keeps.arguments.iter().find(|(handed, _)| *handed == name)?;
            Some(at.1)
        });
        let kept = handed.or_else(|| Some(step.kept_here(arguments.along(name)?)));
        slots[slot.index()].set(kept);
    }
    let callee = keeps.map_or_else(|| step.kept_here(callee), |keeps| keeps.callee);
    resident(
        step.writer(),
        FrameKept {
            segment: activation.view().segment(),
            callee: Some(callee),
            slots,
        },
    )
}

/// [`run`] on the chain `types` it picked; `routes` and `caches` are a call's or an `EVAL`'s, read
/// at its frame's first step.
fn ran<'graph, 'here, 'scratch>(
    mut step: Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    state: KState<'graph, 'here>,
    scratch: Option<KScratch<'graph, 'here, 'scratch>>,
    routes: bool,
    caches: bool,
    types: &Chain<'_, 'here, '_, '_>,
) -> Action<'graph, KBundle> {
    let (code, facts) = step.door();
    let refused = facts.refusal(&step.types());
    let runner = match state {
        KState::Born(KBirth::Program { program }) => {
            let writer = step.writer();
            let root = code.shapes().root();
            let activation = KActivation::of_program(writer, root, &facts, program.builtins());
            let activation = resident(writer, activation.with_handles(slab(&step, root.segment())));
            let activation = match at_entry(&step, types, activation) {
                Ok(activation) => activation,
                Err(error) => {
                    let inner = laid_refusal(&step, error);
                    return faulted(step, types, None, inner);
                }
            };
            Runner::at(program, activation, Level::Top, None)
        }
        // A frame under a composition whose code was refused raises the refusal as its fault.
        KState::Born(KBirth::Call {
            callee, call, owed, ..
        }) if let Some(error) = refused => {
            let inner = laid(&step, Fault::Refused { error });
            return faulted(step, types, Some(born_line(callee, call, owed)), inner);
        }
        KState::Born(KBirth::Call {
            program,
            callee,
            arguments,
            kind,
            contributed,
            handed,
            call,
            owed,
            kept: keeps,
            ..
        }) => {
            // An untraced body runs under its composition, as an `EVAL` of its code does: what it
            // births is born there.
            let untraced =
                called_function(callee).is_some_and(|function| function.composing().is_some());
            let build = match untraced {
                true => step.depot_ref(DepotRef::ROOT),
                false => step.depot_ref(born_under(callee)),
            };
            match frame(
                &step,
                types,
                program,
                callee,
                arguments,
                kind,
                contributed,
                handed,
                owed.map(|owed| owed.contract),
            )
            .and_then(|(activation, contract, barrier)| {
                let activation =
                    at_entry(&step, types, activation).map_err(|error| Fault::Refused { error })?;
                Ok((activation, contract, barrier))
            }) {
                Ok((activation, contract, barrier)) => Runner {
                    barrier,
                    within: Within {
                        routes,
                        caches,
                        build,
                        root: called_function(callee).and_then(|function| function.root()),
                        kept: bare(callee)
                            .then(|| frame_kept(&mut step, activation, (callee, arguments), keeps)),
                        arms: None,
                    },
                    ..Runner::at(
                        program,
                        activation,
                        Level::Frame,
                        Some(Owed {
                            contract,
                            call,
                            hops: owed.map_or(&[], |owed| owed.hops),
                        }),
                    )
                },
                Err(fault) => {
                    let inner = laid(&step, fault);
                    return faulted(step, types, Some(born_line(callee, call, owed)), inner);
                }
            }
        }
        KState::Born(KBirth::Eval {
            program,
            code,
            offered,
            contract,
            call,
        }) => {
            let owed = Owed {
                contract,
                call,
                hops: &[],
            };
            let line = Some(owed.line());
            let root = (code.as_code())
                .and_then(|member| member.code())
                .and_then(|node| node.flat()?.root());
            // Composed code is checked here, under its composition: the refusal it reached, then
            // each name it reads, once the types its facts name resolve in this frame's chain.
            if let Some(error) = refused {
                let inner = laid(&step, Fault::Refused { error });
                return faulted(step, types, line, inner);
            }
            if facts.composition().is_some() {
                let member = code.as_code().expect("an `EVAL` is handed a quote's code");
                let node = member.code().expect("a code member holds code");
                let unbound = match unbound(member, node.shape(&step.door().0), offered, types) {
                    Ok(None) => None,
                    Ok(Some(name)) => Some(CodeRefused::Unbound(name)),
                    Err(refused) => Some(refused),
                };
                if let Some(refused) = unbound {
                    let inner = laid(&step, Fault::Unrunnable(refused));
                    return faulted(step, types, line, inner);
                }
            }
            let Some(activation) = code_frame(&step, types, program, code, offered) else {
                return step.failed(StepError::Refused);
            };
            let activation = match at_entry(&step, types, activation) {
                Ok(activation) => activation,
                Err(error) => {
                    let inner = laid_refusal(&step, error);
                    return faulted(step, types, line, inner);
                }
            };
            // A callable born of composed code is born under its composition; one born of a written
            // quote's code under the root.
            let build = match facts.composition() {
                Some(_) => step.depot_ref(DepotRef::ROOT),
                None => DepotRef::ROOT,
            };
            // The first registration overlapping a builtin, as the load typed the code, faults
            // the frame before any statement runs.
            let (code, facts) = step.door();
            if let Some(error) = facts.overlap(types, code.shape(activation.segment())) {
                let inner = laid(&step, Fault::Refused { error });
                return faulted(step, types, line, inner);
            }
            Runner {
                within: Within {
                    caches,
                    build,
                    root,
                    ..Within::TOP
                },
                ..Runner::at(program, activation, Level::Frame, Some(owed))
            }
        }
        KState::Born(KBirth::Composed { code, source, .. }) => {
            let scratch = Bump::new();
            let (door, facts) = step.door();
            let member = code
                .as_code()
                .expect("`USING` and `BUILD` take a quote's code");
            let writer = step.writer();
            let finished = match source {
                None => Value::Knotted(built(writer, &door, member, types)),
                Some(source) => match using(writer, &door, &facts, member, source, types, &scratch)
                {
                    Ok(filled) => Value::Knotted(filled),
                    Err(refused) => {
                        let inner = laid(&step, Fault::Unfilled(refused));
                        return faulted(step, types, None, inner);
                    }
                },
            };
            return step.finish(finished);
        }
        KState::Runner(runner) if runner.level == Level::Top => {
            return top_level_woken(step, runner, scratch, types);
        }
        KState::Runner(runner) => return super::stack::woken(step, runner, scratch, types),
        KState::Born(_) | KState::Evaluating => return step.failed(StepError::Refused),
    };
    match runner.level {
        Level::Top => top_level(step, runner, None, types),
        Level::Frame | Level::Block => super::stack::running(step, runner, types),
    }
}

/// A block's runner, entered as `entry` says beside the view it sits in, in the asker's own cell:
/// a `USING … SCOPE` body on its module, a `MATCH` arm on its `it`, a synthesized block part bound
/// to nothing. Its activation is laid down and bound at its entry; a refusal its instance answers is
/// the fault the block stops with.
pub(super) fn entered_body<'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    program: &'graph Program<'graph>,
    entry: BlockEntry<'graph, 'here>,
) -> Result<Runner<'graph, 'here>, Stopped<'graph, 'here>> {
    let BlockEntry {
        segment,
        enclosing,
        entered,
        owed,
        within,
        ..
    } = entry;
    let writer = step.writer();
    let (code, facts) = step.door();
    let shape = code.shape(segment);
    debug_assert_eq!(shape.kind(), ShapeKind::Block);
    let enclosing = resident(writer, enclosing);
    let activation = resident(
        writer,
        KActivation::of_block(writer, shape, &facts, enclosing),
    );
    match entered {
        Entered::Plain => {}
        // A `USING` body's parameters are the module's surfaced names, which its shape and the
        // module cannot disagree on: a refusal is an invariant break.
        Entered::Surfaced(module, site) => {
            let bump = Bump::new();
            let picks = facts.picks(code.shape(enclosing.segment()), site);
            if surface(writer, module, picks, activation, shape, types, &bump).is_err() {
                return Err(Stopped::Broken(StepError::Refused));
            }
        }
        // An arm's one parameter is its `it`.
        Entered::Arm(it) => {
            let it_slot = shape.slot(BinderSymbol::Value(IMPLICIT.it.symbol()));
            let bound = it_slot.map(|(slot, _)| activation.bind(slot, it));
            if !matches!(bound, Some(Ok(()))) {
                return Err(Stopped::Broken(StepError::Refused));
            }
        }
    }
    // The block's parameters bind at its entry, as a callable's do.
    let activation = at_entry(step, types, activation)
        .map_err(|error| Stopped::Raised(laid_refusal(step, error)))?;
    Ok(Runner {
        within,
        ..Runner::at(program, activation, Level::Block, owed)
    })
}

/// The trace line a call's frame owes where it faults before its contract is known: its callee
/// as it shows, the call's extent, and the hop table its birth carries.
fn born_line<'here>(
    callee: KValue<'_, 'here>,
    call: SourceRef,
    owed: Option<Owed<'here>>,
) -> Line<'here> {
    Line {
        callee: Some(callee.ktype()),
        call,
        hops: owed.map_or(&[], |owed| owed.hops),
    }
}

/// `fault`, raised in this step, laid in its write home with every borrow it holds copied.
fn laid<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    fault: Fault<'_, KValue<'graph, 'here>>,
) -> KInner<'graph, 'here> {
    let writer = step.writer();
    Inner::Fault(resident(writer, fault.relaid(writer)))
}

/// A refusal a binding event's walk reached, laid in the cell's region already, as its fault.
fn laid_refusal<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    error: &'here ShapeError<'here>,
) -> KInner<'graph, 'here> {
    Inner::Fault(resident(step.writer(), Fault::Refused { error }))
}

/// End with the fault `inner` lies inside, owing `line`: what a frame, a block or an `EVAL`'s
/// frame ends with where it raises one or receives one. What it names is adopted into the region
/// from `types`, the chain the step ran on — a caching body's scratch tier, which resets.
fn faulted<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    line: Option<Line<'here>>,
    inner: KInner<'graph, 'here>,
) -> Action<'graph, KBundle> {
    let scratch = Bump::new();
    Exit::new(step.types(), types, &scratch).link(&crate::scope::Link { line, inner });
    step.fault(line, inner)
}

/// The coordinate of the running activation's own `slot`.
fn local(slot: Slot) -> Coordinate {
    Coordinate::Activation {
        hops: 0,
        target: Target::Local(slot),
    }
}

/// What an evaluator asks for to call `callee` over `arguments`, a record of its parameters by name
/// — and, for a quantified callee a keyworded call selected, of its type parameters, each a type
/// value: a frame running the callee's body, in a region of its own. `kind` says whether the
/// arguments were admitted before the call or are checked by the frame, and `contributed` what a
/// call by name solves each parameter from, per parameter in symbol order — empty, or `None` at a
/// parameter, where the frame reads the carried type. `call` is the extent of the call's node, and
/// `owed` what a tail owes, which the frame's hop table and contract continue, and `passing` what a
/// tail hands on through kept parts
/// ([`Passing`]).
#[allow(clippy::too_many_arguments)]
pub fn call<'graph, 'here>(
    program: &'graph Program<'graph>,
    callee: KValue<'graph, 'here>,
    arguments: KValue<'graph, 'here>,
    kind: CallKind,
    contributed: &'here [Option<KType>],
    handed: &'here [Handed],
    (call, owed): (SourceRef, Option<Owed<'here>>),
    passing: Option<&'here Passing<'graph, 'here>>,
    use_: Use,
) -> Request<'graph, 'here, KBundle> {
    Request {
        placement: Placement::Fresh,
        use_,
        work: crate::scheduler::Work {
            step: run,
            state: KBirth::Call {
                program,
                callee,
                arguments,
                kind,
                contributed,
                handed,
                call,
                owed,
                passing,
                kept: None,
            },
        },
    }
}

/// What an evaluator asks for to fill the holes of `code`, code composed at run time, from
/// `source` under `USING`, or with no source to build it under `BUILD`: a one-step frame under
/// the code's composition, where its shape lies, which shares its operands' storage.
pub fn under_composition<'graph, 'here>(
    program: &'graph Program<'graph>,
    code: KValue<'graph, 'here>,
    source: Option<KValue<'graph, 'here>>,
    use_: Use,
) -> Request<'graph, 'here, KBundle> {
    Request {
        placement: Placement::Shares,
        use_,
        work: crate::scheduler::Work {
            step: run,
            state: KBirth::Composed {
                program,
                code,
                source,
            },
        },
    }
}

/// What an evaluator asks for to run `code` under `EVAL`, `offered` a record of the names and keys
/// the `EVAL` offers — a key's field a list of its functions: a frame running the code's shape,
/// which shares its operands' storage. Refused before anything is spawned, with the fault the
/// `EVAL` raises, when the code's shape kept an error, or a name it reads — or a keyworded hole
/// some use of it selects from alone — is bound neither by a `USING` nor by `offered`, read
/// through `types`, a refusal closed in `scratch`. The frame owes `returns`, the type the `EVAL` declares, as a called frame owes
/// its own; `call` is the extent of the `EVAL`'s node. Nothing builds a shape here: the code's was
/// built where the program loaded.
#[allow(clippy::too_many_arguments)]
pub fn eval<'step: 'x, 'x, 'graph, 'here>(
    program: &'graph Program<'graph>,
    (door, facts, scratch): (
        &Code<'step>,
        &Facts<'step>,
        crate::memory::BumpAllocator<'x>,
    ),
    types: &Chain<'_, '_, '_, '_>,
    code: Knotted<'graph, 'here>,
    offered: KValue<'graph, 'here>,
    (returns, call): (KType, SourceRef),
    use_: Use,
) -> Result<Request<'graph, 'here, KBundle>, Fault<'x, KValue<'graph, 'here>>> {
    let node = code.code().expect("an `EVAL` is handed a quote's code");
    // Composed code's shape lies in its composition, where its frame checks it.
    if let Some(CodeSyntax::Written { segment, .. }) = node.flat() {
        let shape = door.shape(segment);
        // A capture the code binds to a type value is that type, as a load declaring it reads it.
        let mut captured = |capture: crate::scope::CaptureSlot| {
            let capture = shape.captures()[capture.index()];
            let binding = binding_at(node.bound(), capture.scope(), capture.name)?;
            match binding.link.resolve(code) {
                Value::Type(value) => Some(value.handle()),
                _ => None,
            }
        };
        if let Some(error) = facts.typing_refusal(types, scratch, shape, &mut captured) {
            return Err(Fault::Refused { error });
        }
        match unbound(code, shape, offered, types) {
            Ok(None) => {}
            Ok(Some(name)) => return Err(Fault::Unrunnable(CodeRefused::Unbound(name))),
            Err(refused) => return Err(Fault::Unrunnable(refused)),
        }
    }
    Ok(Request {
        placement: Placement::Shares,
        use_,
        work: crate::scheduler::Work {
            step: run,
            state: KBirth::Eval {
                program,
                code: Value::Knotted(code),
                offered,
                contract: Contract {
                    callee: None,
                    returns,
                    retype: returns,
                    restamp: None,
                    checked: false,
                },
                call,
            },
        },
    })
}

/// The first name `code`, of code shape `shape`, reads that nothing binds — a hole no `USING`
/// filled, a key some use selects from alone, a `\` name `offered` lacks, a `$` name composed code
/// carries no binding of — or `None`; refused where a quantified function is bound at its entry
/// and read anywhere but a call's head.
fn unbound(
    code: Knotted<'_, '_>,
    shape: &BodyShape<'_>,
    offered: KValue<'_, '_>,
    types: &Chain<'_, '_, '_, '_>,
) -> Result<Option<BinderSymbol>, CodeRefused> {
    let node = code.code().expect("an `EVAL` is handed a quote's code");
    let scratch = Bump::new();
    let offers = |name: BinderSymbol| offered.field(name.symbol(), types, &scratch).is_some();
    for capture in shape.captures() {
        let bound = match capture.source {
            CaptureSource::Read(_) | CaptureSource::Member { .. } => true,
            // A filled hole is a filled mark, no hole: a key a use selects from alone is unbound.
            CaptureSource::Hole => {
                matches!(capture.name, BinderSymbol::Key(key) if shape.required_holes().binary_search(&key).is_err())
            }
            CaptureSource::Offered => offers(capture.name),
            // A key binds the registrations at it, and holds no function where there are none.
            CaptureSource::Entry { .. } if matches!(capture.name, BinderSymbol::Key(_)) => true,
            CaptureSource::Entry { head } => {
                match binding_at(node.bound(), capture.scope(), capture.name) {
                    // A quantified function is read only at a call's head, as the load refuses
                    // a written `$` name read anywhere else.
                    Some(binding) if !head && quantified(binding.link.resolve(code)) => {
                        return Err(CodeRefused::QuantifiedRead(capture.name));
                    }
                    found => found.is_some(),
                }
            }
        };
        if !bound {
            return Ok(Some(capture.name));
        }
    }
    Ok(None)
}

/// Whether `value` is a quantified function not made an instance: read only at a call's head.
fn quantified(value: KValue<'_, '_>) -> bool {
    (value.as_callable().and_then(|member| member.function()))
        .is_some_and(|function| matches!(function.ktype(), DeclaredType::Scheme(_)))
}

/// Why a callee's position read under its solution is concrete: a function is born with no
/// variable outside its own group, and a run-time solution is concrete.
const SOLVED: &str = "a run-time callee holds only its own group's variables";

impl<'graph, 'cell> Runner<'graph, 'cell> {
    fn at(
        program: &'graph Program<'graph>,
        activation: &'cell KActivation<'graph, 'cell>,
        level: Level,
        owed: Option<Owed<'cell>>,
    ) -> Self {
        Runner {
            program,
            activation,
            unit: 0,
            result: None,
            outer: None,
            level,
            owed,
            barrier: None,
            stage: Stage::Next,
            seen: 0,
            within: Within::TOP,
        }
    }

    /// What the runner hands an evaluation it asks for: what it runs within.
    pub(super) fn handing(&self) -> Within<'graph, 'cell> {
        self.within
    }

    /// Whether the runner owes a contract: a frame's, or a tail arm's.
    pub(super) fn owes(&self) -> bool {
        self.owed.is_some()
    }

    /// The body's value so far, held beside where it rests.
    pub(super) fn result(&self) -> Option<Held<'graph, 'cell>> {
        self.result
    }

    /// The program the runner performs a body of.
    pub(super) fn program(&self) -> &'graph Program<'graph> {
        self.program
    }

    /// The view of the runner's activation, which every evaluation it asks for reads names
    /// through.
    pub(super) fn view(&self) -> KActivationView<'graph, 'cell> {
        self.activation.view()
    }

    /// The runner's activation, read through `step`'s door.
    fn reading<'step>(
        &self,
        step: &Taking<'step, 'graph, '_, 'cell, '_, '_>,
    ) -> KReading<'step, 'graph, 'cell> {
        let (code, facts) = step.door();
        self.activation.view().reading(&code, facts)
    }

    /// The unit the runner is at, of its activation's `shape`.
    fn current(&self, shape: &BodyShape<'_>) -> Unit {
        shape.units()[self.unit as usize]
    }

    /// Whether `unit`'s value is the body's: a frame's or a block's last statement, outside any
    /// module body.
    fn yields(&self, unit: Unit) -> bool {
        unit.last && self.level != Level::Top && self.outer.is_none()
    }

    /// Whether `unit` is a frame's last, or a block's owing a contract (a tail arm's), a statement
    /// binding nothing whose value is the body's — which the runner hands to the evaluator by a
    /// tail rather than waiting on. A frame called through a barrier never tails: its value
    /// crosses the barrier where the frame ends.
    fn tails(&self, unit: Unit, shape: &BodyShape<'_>) -> bool {
        self.tail_body()
            && self.barrier.is_none()
            && self.yields(unit)
            && self.unit as usize + 1 == shape.units().len()
            && matches!(unit.work, UnitWork::Statement(_))
    }

    /// Whether the runner runs a body whose last statement may tail: a frame's, or a tail arm's
    /// block, which owes its frame's contract.
    fn tail_body(&self) -> bool {
        self.level == Level::Frame || self.level == Level::Block && self.owed.is_some()
    }

    /// Bind `slot` and, when `unit` yields the body's value, keep it.
    fn bind(&mut self, unit: Unit, slot: Slot, value: KValue<'graph, 'cell>) {
        self.activation
            .bind(slot, value)
            .expect("a unit binds its slots once, in the shape's order");
        if self.yields(unit) && self.result.is_none() {
            self.result = Some(Held::bare(value));
        }
    }

    /// Keep, in its frame's table, the kept part `held` names where it is a whole kept value bound
    /// at `slot` of the frame's own body.
    fn keep(&self, slot: Slot, held: Held<'graph, 'cell>) {
        let (Some(table), Some(part)) = (self.within.kept, held.kept) else {
            return;
        };
        if part.path.is_empty()
            && self.outer.is_none()
            && table.segment == self.activation.view().segment()
        {
            table.slots[slot.index()].set(Some(part.kept));
        }
    }

    /// The top level's activation, under every module body being run inline.
    fn outermost(&self) -> &'cell KActivation<'graph, 'cell> {
        let mut activation = self.activation;
        let mut outer = self.outer;
        while let Some(enclosing) = outer {
            activation = enclosing.activation;
            outer = enclosing.outer;
        }
        activation
    }

    /// The trace line the runner owes where it ends with a fault: its frame's, where it owes one.
    fn line(&self) -> Option<Line<'cell>> {
        self.owed.map(|owed| owed.line())
    }
}

/// A frame's activation, laid down for `callee` with every value parameter bound from `arguments`,
/// retyped to its declared type with the call's solution substituted, and every type parameter
/// bound to its solution — the one an instance carries, the one a keyworded call's selection
/// carried in `arguments`, or, for a call by name, the one solved here while each argument is
/// admitted against its parameter's declared type — at its entry in `contributed`, per parameter in
/// symbol order, and its carried type where there is none — beside the contract the frame ends
/// under, and the barrier its value crosses out through. A digested parameter keeps the memo
/// `handed` for it where the value it binds carries the type the caller's did. The fault the frame
/// ends with when the callee is no function, the arguments do not name its parameters exactly, an
/// argument does not fit its parameter, or the group has no solution.
///
/// A callee has two types: the one it shows, `callee.ktype()`, and the one its body was checked
/// against, [`Knotted::declared`]. Where they differ — a header, or a member born at its annotation
/// — and through a barrier, the arguments are admitted and retyped at the shown parameters, the
/// node is called by name past them over the parameters it declares, and its value is retyped to
/// the shown return: through the contract, or past a barrier once it has crossed out.
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
fn frame<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    program: &'graph Program<'graph>,
    callee: KValue<'graph, 'here>,
    arguments: KValue<'graph, 'here>,
    kind: CallKind,
    contributed: &[Option<KType>],
    handed: &[Handed],
    owed: Option<Contract>,
) -> Result<
    (
        &'here KActivation<'graph, 'here>,
        Contract,
        Option<Barrier<'graph, 'here>>,
    ),
    KFault<'graph, 'here>,
> {
    let writer = step.writer();
    let shown = callee.ktype();
    let Some(node) = callee.as_callable() else {
        return Err(Fault::NotCallable { callee });
    };
    // `Bump::new` claims no chunk until something is put in it, so a callee with no group pays
    // nothing for having one in reach.
    let bump = Bump::new();
    let scratch = &bump;
    let outermost = node.coerced().is_some().then_some(node);
    // A bare barrier admits its arguments as they cross it, at the type it shows.
    let header = shown != node.declared();
    let past = outermost.is_some() || header;
    let unfit_at =
        |signature: DeclaredType<KType>, refused: Option<CoercionRefused>| Fault::Unfit {
            callee,
            signature,
            arguments,
            refused: refused.map(|refused| resident(writer, refused)),
        };
    let misnamed_at = |signature: DeclaredType<KType>| Fault::Misnamed {
        callee,
        signature,
        arguments,
    };
    // The shown side: admitted and retyped at the parameters the callee shows, and the return its
    // caller sees. A keyworded call's selection admitted its arguments at the shape shown already,
    // and carried the solution of its group beside them.
    let (arguments, contributed, shown_return) = match (past, kind) {
        (false, _) => (arguments, contributed, None),
        (true, CallKind::ByName) if !header => {
            (arguments, contributed, shown_returns(types, shown))
        }
        (true, CallKind::ByName) => {
            let (admitted, returns) =
                shown_admitted(writer, types, scratch, shown, arguments, contributed).map_err(
                    |miss| match miss {
                        Miss::Misnamed => misnamed_at(shown),
                        Miss::Unfit | Miss::Unsolved => unfit_at(shown, None),
                    },
                )?;
            (admitted, &[][..], Some(returns))
        }
        (true, CallKind::Keyworded) => {
            let returns = shown_keyworded_returns(types, scratch, shown, arguments);
            (arguments, contributed, returns)
        }
    };
    // A barrier stands before the function a call runs: the arguments cross each one inwards, and
    // the function behind them is called by name, admitting and solving as a call by name does.
    let (member, arguments, kind, contributed) = match outermost {
        Some(outermost) => {
            let crossed = crossed_in(writer, types, outermost, arguments)
                .map_err(|refusal| unfit_at(shown, Some(refusal)))?;
            (crossed.0, crossed.1, CallKind::ByName, &[][..])
        }
        None if past => (node, arguments, CallKind::ByName, &[][..]),
        None => (node, arguments, kind, contributed),
    };
    let Some(function) = member.function() else {
        return Err(Fault::NotCallable { callee });
    };
    let declared = member.declared();
    let misnamed = || misnamed_at(declared);
    let unfit = || unfit_at(declared, None);
    let unsolved = || Fault::Unsolved {
        callee,
        signature: declared,
        arguments,
    };
    arguments.as_record().ok_or_else(misnamed)?;
    let (code, facts) = step.door();
    let shape = function.frame_shape(&code);
    let quantified = matches!(declared, DeclaredType::Scheme(_));
    let view = declared.view(types);
    let TypeNode::KFunction { params, ret } = view.body else {
        unreachable!("a function's type is a function type")
    };
    // Past the shown type, the node binds only the parameters it declares: a function type's
    // parameters are a width subset of what a slot of it may show.
    let arguments = match past {
        true => restricted(writer, types, scratch, arguments, params),
        false => arguments,
    };
    let argument = |name: Symbol| arguments.field(name, types, scratch).ok_or_else(misnamed);
    let carried = |name: TypeSymbol| {
        arguments
            .field(name.symbol(), types, scratch)
            .and_then(|seen| seen.value().as_type())
            .map(|value| value.handle())
    };
    // The group's solution in group order. A keyworded call's selection carried it by name; a
    // call by name's is solved here, every value parameter's declared type against its argument's
    // carried type under one collector — which, for an unquantified callee, is the arguments'
    // admission alone. An instance's group was solved where the load made it, and is never
    // solved again.
    let mut solution: Option<BumpVec<'_, KType>> = function.instance().map(|instance| {
        let mut solved = BumpVec::with_capacity_in(instance.len(), scratch);
        solved.extend_from_slice(instance);
        solved
    });
    debug_assert!(
        solution.is_none() || kind == CallKind::ByName,
        "an instance has no registration, so only a call by name reaches it"
    );
    match kind {
        CallKind::Keyworded if quantified => {
            let mut solved = BumpVec::with_capacity_in(view.group.names.len(), scratch);
            solved.resize(view.group.names.len(), KType::NEVER);
            for (name, index) in function.quantifier_map().iter() {
                solved[index] = carried(name).ok_or_else(misnamed)?;
            }
            solution = Some(solved);
        }
        CallKind::Keyworded => {}
        // A header's arguments were admitted at the parameters it shows, which the order puts
        // under the node's own where neither type is quantified: nothing is left to solve.
        CallKind::ByName
            if header
                && outermost.is_none()
                && !quantified
                && matches!(shown, DeclaredType::Type(_)) => {}
        CallKind::ByName => {
            let solved = admitted(types, scratch, view, arguments, contributed).map_err(
                |miss| match miss {
                    Miss::Misnamed => misnamed(),
                    Miss::Unfit => unfit(),
                    Miss::Unsolved => unsolved(),
                },
            )?;
            if quantified {
                solution = solved;
            }
        }
    }
    let activation = match function.composing() {
        None => KActivation::of_callable(
            writer,
            shape,
            &facts,
            member,
            function.closure(),
            program.builtins(),
        ),
        Some(_) => KActivation::of_composed(
            writer,
            shape,
            &facts,
            member,
            untraced_closure(writer, types, function, shape)?,
            program.builtins(),
        ),
    };
    let activation = resident(writer, activation.with_handles(slab(step, shape.segment())));
    let reading = activation.view().reading(&code, facts);
    // What the frame re-stamps its arguments and value at, where the load closed their types: a
    // quantified callee's are solved per call, and an untraced body's composition closed none.
    // Where the load left a type open over a run-bound name, the frame reads how it re-stamps off
    // the type it closes, as the load's own restamp door would.
    let derives = !(quantified || function.composing().is_some());
    let retypes = match derives {
        false => Retypes::NONE,
        true => facts.retypes(types, scratch, shape),
    };
    let derived = |declared| derives.then(|| Retype::of(types, scratch, declared));
    let mut parameters = 0;
    for slot in (0..shape.slots()).map(|slot| Slot(slot as u32)) {
        let name = shape.slot_name(slot);
        if shape.position_of(slot) != Position::PARAMETER {
            continue;
        }
        let value = match name {
            // A declared parameter is an ascription: the argument is retyped to its declared
            // type, with the call's solution substituted, so the body dispatches on what the
            // parameter declares.
            BinderSymbol::Value(name) => {
                parameters += 1;
                let argument = argument(name.symbol())?;
                let declared = params
                    .get(name.symbol())
                    .expect("every value parameter is declared");
                let declared = match &solution {
                    Some(solution) => substitute_quantified(types, scratch, declared, solution),
                    None => declared,
                };
                let declared = types.concrete(declared).expect(SOLVED);
                debug_assert!(
                    argument.satisfies(declared, types, scratch),
                    "a frame binds an argument its parameter admits"
                );
                let retype =
                    (retypes.parameter(types, name.symbol())).or_else(|| derived(declared));
                let value = argument
                    .seen_as(declared, retype, types, scratch)
                    .restamped(writer, types);
                // A memo handed for a value carrying the type the parameter binds is that value's
                // digest: the frame laid no header over it, so the slot holds the caller's value.
                if let Some(handed) = (handed.iter()).find(|each| each.name == name.symbol())
                    && handed.carried == value.ktype()
                    && let Some(memo) = reading.digest_memo(Coordinate::Activation {
                        hops: 0,
                        target: Target::Local(slot),
                    })
                {
                    memo.set(Some(handed.digest));
                }
                value
            }
            // A `:Type` parameter — a type-channel parameter no `FOR ALL` group declares — is an
            // argument like any other, passed by keyword or by name.
            BinderSymbol::Type(name) if function.quantifier_index(name).is_none() => {
                parameters += 1;
                argument(name.symbol())?.value()
            }
            // A `FOR ALL` name is bound by **name**: the shape's type channel reaches here
            // symbol-sorted, not in the order the group was written, so a positional read would
            // hand one variable another's solution. Only a keyworded call's arguments carry one, so
            // a call by name's that names one does not name its parameters.
            BinderSymbol::Type(name) => {
                if kind == CallKind::Keyworded && carried(name).is_some() {
                    parameters += 1;
                }
                let index = function
                    .quantifier_index(name)
                    .expect("a name no group declares is a `:Type` parameter");
                let solved = solution
                    .as_ref()
                    .and_then(|solution| solution.get(index).copied())
                    .expect("a quantified callee's group is solved");
                Value::Type(TypeValue::new(writer, solved, types))
            }
            BinderSymbol::Registration(_) | BinderSymbol::Key(_) => {
                unreachable!("a parameter is a written name")
            }
        };
        activation
            .bind(slot, value)
            .expect("a fresh frame binds each parameter once");
    }
    if parameters
        != arguments
            .surface(types, scratch)
            .map_or(0, |record| record.len())
    {
        return Err(misnamed());
    }
    let returns = match &solution {
        Some(solution) => substitute_quantified(types, scratch, ret, solution),
        None => ret,
    };
    let returns = types.concrete(returns).expect(SOLVED);
    // A barrier's value is retyped to the shown return once it crosses out; any other callee's
    // through its contract, where the order does not promise the node's return lies under the
    // shown one when either is quantified.
    let (retype, checked) = match (outermost, shown_return) {
        (None, Some(shown_return)) => (
            owed.map_or(shown_return, |outer| outer.retype),
            quantified || matches!(shown, DeclaredType::Scheme(_)),
        ),
        _ => (owed.map_or(returns, |outer| outer.retype), false),
    };
    // What the value is re-stamped at, read off the load's record where it closed the return the
    // value is retyped to: a barrier's shown return is no record's.
    let restamp = match (outermost, shown_return) {
        (None, Some(_)) => owed.and_then(|outer| outer.restamp),
        _ => owed.map_or_else(
            || retypes.returned(types).or_else(|| derived(returns)),
            |outer| outer.restamp,
        ),
    };
    let contract = Contract {
        callee: Some(declared),
        returns,
        retype,
        restamp,
        checked,
    };
    let barrier = outermost.map(|outermost| Barrier {
        outermost,
        returns: shown_return,
    });
    Ok((activation, contract, barrier))
}

/// The return `shown`, a callee's type, gives every call where its group leaves it concrete: what
/// a barrier's value is retyped to once it crosses out. `None` for a quantified return, which the
/// barrier's own crossing types.
fn shown_returns(types: &Chain<'_, '_, '_, '_>, shown: DeclaredType<KType>) -> Option<KType> {
    match shown.view(types).body {
        TypeNode::ExpressionShape { ret, .. } | TypeNode::KFunction { ret, .. } => {
            types.concrete(ret)
        }
        _ => None,
    }
}

/// The return `shown`, the expression shape a keyworded call selected a header or a barrier at,
/// gives the call over `arguments`: at a scheme, under the solution the selection carried by the
/// shape's own names. `None` where it carried none.
fn shown_keyworded_returns(
    types: &Chain<'_, '_, '_, '_>,
    scratch: &Bump,
    shown: DeclaredType<KType>,
    arguments: KValue<'_, '_>,
) -> Option<KType> {
    let DeclaredType::Scheme(_) = shown else {
        return shown_returns(types, shown);
    };
    let view = shown.view(types);
    let mut solution = BumpVec::with_capacity_in(view.group.names.len(), scratch);
    for name in view.group.names {
        let seen = arguments.field(name.symbol(), types, scratch)?;
        solution.push(seen.value().as_type()?.handle());
    }
    let TypeNode::ExpressionShape { ret, .. } = view.body else {
        return None;
    };
    types.concrete(substitute_quantified(types, scratch, ret, &solution))
}

/// Why arguments do not reach a frame: they do not name its parameters, one does not fit its
/// parameter, or the group has no solution.
pub(super) enum Miss {
    Misnamed,
    Unfit,
    Unsolved,
}

/// Admit `arguments`, a call by name's record, against the parameters of `view`, a function type,
/// every value parameter's declared type against its argument's carried type — or what the load
/// contributed for it, per parameter in symbol order — under one collector: the group's solution
/// where it is quantified.
pub(super) fn admitted<'x, 'tier: 'x>(
    types: &Chain<'tier, '_, '_, '_>,
    scratch: &'x Bump,
    view: DeclaredView<'_>,
    arguments: KValue<'_, '_>,
    contributed: &[Option<KType>],
) -> Result<Option<BumpVec<'x, KType>>, Miss> {
    let TypeNode::KFunction { params, .. } = view.body else {
        return Err(Miss::Misnamed);
    };
    #[cfg(test)]
    ADMITTED_BY_NAME.with(|admitted| admitted.set(admitted.get() + 1));
    let mut collector = Collector::<KType>::new(types, scratch, view.group.bounds);
    // The load laid the parameters of the callee it saw out in symbol order, and records
    // contributions only where its argument names no field but those: a record as long as both
    // that list and this callee's parameters, naming each (read below), names the same ones. Any
    // other call is misnamed.
    let named = (arguments.surface(types, scratch)).map_or(0, |record| record.len());
    if !contributed.is_empty() && (contributed.len() != params.len() || named != params.len()) {
        return Err(Miss::Misnamed);
    }
    for (parameter, declared) in params.iter() {
        let argument = arguments
            .field(parameter.symbol(), types, scratch)
            .ok_or(Miss::Misnamed)?;
        let carried = argument.ktype().as_type().expect(CALL_ONLY);
        let solved_from = match contributed {
            [] => carried,
            // The load laid the parameters out in symbol order: this one's place is how many
            // sort before it.
            _ => contributed[params
                .keys()
                .filter(|other| other.symbol() < parameter.symbol())
                .count()]
            .unwrap_or(carried),
        };
        admits_with(
            types,
            scratch,
            declared,
            solved_from,
            Variance::Co,
            &mut collector,
        )
        .map_err(|_| Miss::Unfit)?;
    }
    if view.group.names.is_empty() {
        return Ok(None);
    }
    collector.solve(types).map(Some).map_err(|_| Miss::Unsolved)
}

/// `arguments` admitted at `shown`, the function type a callee shows, each retyped to its shown
/// parameter under the solution, beside the shown return under it: what a call by name hands a
/// callee past the type it shows.
fn shown_admitted<'graph, 'here>(
    writer: Writer<'here, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    scratch: &Bump,
    shown: DeclaredType<KType>,
    arguments: KValue<'graph, 'here>,
    contributed: &[Option<KType>],
) -> Result<(KValue<'graph, 'here>, KType), Miss> {
    let view = shown.view(types);
    let TypeNode::KFunction { params, ret } = view.body else {
        return Err(Miss::Misnamed);
    };
    arguments.as_record().ok_or(Miss::Misnamed)?;
    let named = (arguments.surface(types, scratch)).map_or(0, |record| record.len());
    if named != params.len() {
        return Err(Miss::Misnamed);
    }
    let solution = admitted(types, scratch, view, arguments, contributed)?;
    let solved = |declared: Parametric| {
        let declared = match &solution {
            Some(solution) => substitute_quantified(types, scratch, declared, solution),
            None => declared,
        };
        types.concrete(declared).expect(SOLVED)
    };
    let mut fields = BumpVec::with_capacity_in(params.len(), scratch);
    for (parameter, declared) in params.iter() {
        let argument = arguments
            .field(parameter.symbol(), types, scratch)
            .ok_or(Miss::Misnamed)?;
        let retyped = argument
            .seen_at(solved(declared), types, scratch)
            .restamped(writer, types);
        fields.push((parameter, retyped));
    }
    let record = Record::new(writer, &fields, types, scratch);
    Ok((Value::Record(record), solved(ret)))
}

/// `arguments`, a record, holding only the fields `params` names: what a node called past the type
/// it shows binds, since it may declare fewer parameters than its slot shows.
fn restricted<'graph, 'here>(
    writer: Writer<'here, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    scratch: &Bump,
    arguments: KValue<'graph, 'here>,
    params: TypeRecord<'_, Parametric>,
) -> KValue<'graph, 'here> {
    let named = (arguments.surface(types, scratch)).map_or(0, |record| record.len());
    if named == params.len() {
        return arguments;
    }
    let mut fields = BumpVec::with_capacity_in(params.len(), scratch);
    for (parameter, _) in params.iter() {
        if let Some(seen) = arguments.field(parameter.symbol(), types, scratch) {
            fields.push((parameter, seen.restamped(writer, types)));
        }
    }
    Value::Record(Record::new(writer, &fields, types, scratch))
}

/// The barrier a frame was called through, which its value crosses outwards, beside the return
/// its caller sees, which the crossed value is retyped to.
#[derive(Clone, Copy)]
struct Barrier<'graph, 'cell> {
    outermost: Knotted<'graph, 'cell>,
    returns: Option<KType>,
}

/// `arguments`, handed to a call through the barrier `outermost`, crossed inwards through it and
/// every barrier stacked behind it: the function behind them all, beside the record it takes.
fn crossed_in<'graph, 'here>(
    writer: Writer<'here, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    outermost: Knotted<'graph, 'here>,
    arguments: KValue<'graph, 'here>,
) -> Result<(Knotted<'graph, 'here>, KValue<'graph, 'here>), CoercionRefused> {
    let bump = Bump::new();
    // A keyworded member's slots bind the names the function behind every barrier registers.
    let innermost = outermost.behind_barriers();
    let parameters = innermost
        .function()
        .and_then(|function| function.registered())
        .map_or(ParameterBinding::Named(&[]), |registered| {
            registered.parameters
        });
    let mut arguments = arguments;
    for barrier in outermost.barriers() {
        arguments = coerce::inward(writer, barrier, arguments, parameters, types, &bump)?;
    }
    Ok((innermost, arguments))
}

/// `value`, what the function behind the barrier `outermost` returned, crossed outwards through
/// every barrier stacked there, innermost first. A refusal is a fault.
fn crossed_out<'graph, 'here>(
    writer: Writer<'here, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    outermost: Knotted<'graph, 'here>,
    value: KValue<'graph, 'here>,
) -> Result<KValue<'graph, 'here>, KFault<'graph, 'here>> {
    let bump = Bump::new();
    let mut stacked = BumpVec::new_in(&bump);
    for barrier in outermost.barriers() {
        stacked.push(barrier);
    }
    let mut value = value;
    for barrier in stacked.iter().rev() {
        value = coerce::outward(writer, barrier, value, types, &bump).map_err(Fault::CrossedOut)?;
    }
    Ok(value)
}

/// The closure of a frame of `function`, whose untraced body's code runs as `shape`, its
/// composition's root: the code's closure run ([`code_closure`]) over what the scope the function
/// was born in offers it. The fault the frame ends with where a name the code reads is bound
/// nowhere — a hole, a `\` name the offer lacks — or where a quantified function is read anywhere
/// but a call's head.
fn untraced_closure<'graph, 'here>(
    writer: Writer<'here, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    function: &crate::knot::Function<'graph, 'here, Knotted<'graph, 'here>>,
    shape: &BodyShape<'_>,
) -> Result<&'here ClosureBindings<'here, Knotted<'graph, 'here>>, KFault<'graph, 'here>> {
    let &[Link::Value(code), Link::Value(offered)] = function.closure().links() else {
        unreachable!("an untraced body's closure is its code and what it is offered")
    };
    let member = code.as_code().expect("an untraced body's code is code");
    let refused = match unbound(member, shape, offered, types) {
        Ok(None) => None,
        Ok(Some(name)) => Some(CodeRefused::Unbound(name)),
        Err(refused) => Some(refused),
    };
    if let Some(refused) = refused {
        return Err(Fault::Unrunnable(refused));
    }
    let bump = Bump::new();
    let links = code_closure(writer, types, member, shape, offered, &bump)
        .expect("a name `unbound` finds bound is bound");
    // The frame's callables are born under the composition.
    Ok(ClosureBindings::of(
        writer,
        &links,
        DepotRef::ROOT,
        Encloser::default(),
    ))
}

/// An `EVAL`'s frame: the code's activation over a closure run assembled in its shape's capture
/// order ([`code_closure`]). `None` when a name is bound nowhere, which [`eval`] refuses first.
fn code_frame<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    program: &'graph Program<'graph>,
    code: KValue<'graph, 'here>,
    offered: KValue<'graph, 'here>,
) -> Option<&'here KActivation<'graph, 'here>> {
    let member = code.as_code()?;
    let (door, facts) = step.door();
    let shape = member.code()?.shape(&door);
    let bump = Bump::new();
    let writer = step.writer();
    let links = code_closure(writer, types, member, shape, offered, &bump)?;
    // A quote's frame is placed under no depot slot of its own.
    let closure = ClosureBindings::of(writer, &links, DepotRef::ROOT, Encloser::default());
    let activation = KActivation::of_code(writer, shape, &facts, closure, program.builtins());
    Some(resident(
        writer,
        activation.with_handles(slab(step, shape.segment())),
    ))
}

/// The handle slab of a frame of the body at `segment`: a word for each word of the handles of the
/// nodes of its base's binding graph, laid in the frame's region. Empty over a base of no graph.
fn slab<'here>(
    step: &Taking<'_, '_, '_, 'here, '_, '_>,
    segment: crate::scope::Segment,
) -> &'here [Cell<u64>] {
    let Some(instances) = step.instances() else {
        return &[];
    };
    let laid = instances.graph();
    let Some(Some(body)) = laid.facts.bodies.get(segment.index() as usize) else {
        return &[];
    };
    step.writer()
        .fill(laid.layout.width(*body), |_| Cell::new(0))
}

/// `activation` at its entry bind site ([`instanced`]): closing a base record's types under what its
/// frame's slots bind there. A refusal its instance answers is the frame's fault.
fn at_entry<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    activation: &'here KActivation<'graph, 'here>,
) -> Result<&'here KActivation<'graph, 'here>, &'here ShapeError<'here>> {
    let view = activation.view();
    // The entry binds what a base record's types read, though no bind site's node sits there.
    let binding = instanced(step, types, view, UnitIndex::ENTRY)?.or_else(|| {
        step.instances()
            .is_some()
            .then(|| frame_binding(step, types, view))
    });
    Ok(match binding {
        Some(binding) => resident(step.writer(), activation.binding(Some(binding))),
        None => activation,
    })
}

/// What a frame's slot holding `value` binds its unknown to: a type value its type, a module value
/// its carried type over its bare member's content, by digest, beside that member as a view over
/// it reads it ([`Bare`]), retyped where a header carries it: a view over it reads the bare
/// member, as the run's does. Any other value, the type it carries, where it carries one. `memo`, the slot's digest memo where the load marked it digested, holds the digest
/// of the value the slot holds: it is read for a bare member and filled where it is empty.
pub(crate) fn bound_of<'o>(
    lay: impl Lays<'o>,
    value: KValue<'_, '_>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    memo: Option<&Cell<Option<ContentDigest>>>,
) -> Option<Bound<'o>> {
    if let Some(named) = value.as_type() {
        return Some(Bound::Type(named.handle()));
    }
    // A quantified callable carries a scheme, no type: no name stands for it.
    if value.as_module().is_none() {
        return value.ktype().as_type().map(Bound::Carried);
    }
    let member = value.knot_member()?;
    let carried = value.concrete_ktype();
    // The record's carriers are laid beside the tree, which outlives the frame's slot.
    let bare = view::bare_of(member, types, scratch);
    let sealing = bare.sealing.map(|sealing| Sealing {
        record: sealing.record.laid(lay),
        ..sealing
    });
    let as_view = Some(resident(
        lay,
        Bare {
            signature: bare.signature,
            sealing,
        },
    ));
    let bare = Value::Knotted(member);
    // A module whose content is known keeps its digest; else the value's is walked.
    let digest = || {
        let kept = member.module().and_then(|module| module.digest());
        debug_assert!(kept.is_none_or(|kept| kept == bare.digest(types, scratch)));
        kept.unwrap_or_else(|| bare.digest(types, scratch))
    };
    // A header's digest is its type over its bare member's, which its memo cannot give back.
    let tree = match (bare.ktype() == value.ktype(), memo.and_then(Cell::get)) {
        (true, Some(digest)) => Tree::Digest(digest, as_view),
        (true, None) => Tree::Digest(digest(), as_view),
        (false, _) => {
            let source = resident(lay, Tree::Digest(digest(), as_view));
            // A module keeps its digest under a root-tier type it is viewed at.
            let kept = (member.module())
                .filter(|_| types.in_root(carried))
                .and_then(|module| {
                    module
                        .retyped_digest(carried, || Tree::retyped_closed(carried, source).digest())
                });
            match kept {
                Some(digest) => Tree::retyped_known(carried, source, digest),
                None => Tree::retyped_closed(carried, source),
            }
        }
    };
    memo.filter(|memo| memo.get().is_none())
        .inspect(|memo| memo.set(tree.digest()));
    Some(Bound::Module {
        carried,
        tree: resident(lay, tree),
    })
}

/// What the slots and captures of the frame `view` runs, and of the views around it, bind: each
/// type value its type, and each module value its module ([`bound_of`]), laid in the frame's
/// region. The binding a base record's types are closed under.
fn frame_binding<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    view: KActivationView<'graph, 'here>,
) -> Binding<'here> {
    let (code, facts) = step.door();
    let (lay, scratch) = (step.writer(), Bump::new());
    let shapes = code.shapes();
    let mut staged: BumpVec<'_, (Unknown, Bound<'here>)> = BumpVec::new_in(&scratch);
    for view in std::iter::successors(Some(&view), |view| view.enclosing()) {
        let segment = view.segment();
        let shape = code.shape(segment);
        let slots = (0..shape.slots() as u32).map(|at| Address::Slot(Slot(at)));
        let captures = (0..view.closure().len() as u32)
            .map(|at| Address::Capture(crate::scope::CaptureSlot(at)));
        for at in slots.chain(captures) {
            let (held, unknown) = match at {
                Address::Held(_) => unreachable!("a frame's slots and captures"),
                Address::Slot(slot) => (view.bound(slot), Some(Unknown { segment, at })),
                Address::Capture(capture) => {
                    let held = match view.closure().get(capture) {
                        Link::Value(value) => Some(value),
                        Link::Edge(_) => None,
                    };
                    // A type capture past the builder captures holds the name of the body around
                    // it its closure layout reads, at that name's own unknown; a binder capture
                    // past those, the binder it reads.
                    let past = (capture.index()).checked_sub(shape.captures().len());
                    let layout = shapes.layout(segment);
                    let unknown = match past {
                        None => Some(Unknown { segment, at }),
                        Some(past) => match layout.get(past) {
                            Some((level, _)) => shapes.unknown_of_level(segment, *level),
                            None => facts.capture_binder(shape, past - layout.len()),
                        },
                    };
                    (held, unknown)
                }
            };
            let Some(unknown) = unknown else {
                continue;
            };
            let Some(value) = held else {
                continue;
            };
            if staged.iter().any(|(each, _)| *each == unknown) {
                continue;
            }
            // A module, or a value declared `Any` whose name is the type it carries, is bound
            // where a unit's frontier holds it, as the layers bind one: a parameter its entry
            // waits on, a seam whose content the load holds as a leaf alone, or a module capture.
            // Elsewhere its types are written where the load wrote them.
            let mut memo = None;
            if value.as_type().is_none() {
                let waited = facts.entry_waits(shape).contains(&at);
                let seam = match at {
                    Address::Slot(slot) if !waited => {
                        let seams = facts.statement_seams(shape);
                        let rooting = facts.rooting_seams(shape).binary_search(&slot).is_ok();
                        let seen = (facts.content(shape, slot)).is_some_and(|tree| {
                            !matches!(tree, Tree::Leaf(_)) && (!rooting || tree.closed())
                        });
                        let closed =
                            matches!(facts.declared_type(types, shape, slot), Static::Closed(_));
                        seams.binary_search(&slot).is_ok() && !seen && !closed
                    }
                    Address::Capture(_) => value.as_module().is_some(),
                    _ => false,
                };
                if !waited && !seam {
                    continue;
                }
                if let Address::Slot(slot) = at
                    && value.as_module().is_some()
                {
                    memo = view.reading(&code, facts).digest_memo(local(slot));
                }
            }
            let Some(bound) = bound_of(lay, value, types, &scratch, memo) else {
                continue;
            };
            staged.push((unknown, bound));
            // A module a callable captures is bound at the encloser's binder it reads, too: the
            // callable's records name a path through it by the binder rooting its chain.
            if let Address::Capture(capture) = at
                && capture.index() < shape.captures().len()
                && value.as_module().is_some()
                && let Some(Lands::Binder { at: home, slot }) = shapes.follow(
                    segment,
                    Coordinate::Activation {
                        hops: 0,
                        target: Target::Capture(capture),
                    },
                )
                && home != segment
            {
                let binder = Unknown {
                    segment: home,
                    at: Address::Slot(slot),
                };
                if !staged.iter().any(|(each, _)| *each == binder) {
                    staged.push((binder, bound));
                }
            }
        }
        // Each part its body holds in place and has bound, at its held site.
        let reading = view.reading(&code, facts);
        for (index, site) in facts.held_sites(shape).iter().enumerate() {
            let Some(value) = view.held(index) else {
                continue;
            };
            let unknown = Unknown {
                segment,
                at: Address::Held(*site),
            };
            let memo = Some(reading.held_memo(index));
            if let Some(bound) = bound_of(lay, value, types, &scratch, memo) {
                staged.push((unknown, bound));
            }
        }
    }
    Binding::laid(lay, &mut staged)
}

/// The bind site of the binding graph at the unit `unit` of the body `view` runs reached: what the
/// frame's slots and captures bind there ([`frame_binding`]), which a base record's types are
/// closed under until the next; and the instance of its node under what they bind the names of
/// its body's bind sites up to it, found filed or made through the instances of its base's graph
/// the step lends, its handle written into the frame's slab. Nothing where no node sits there or
/// the step lends no instances; no instance where its node owns no question. The first question
/// the node owns that the instance refuses is refused, worded under the binding: the frame's
/// fault at this site.
fn instanced<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    view: KActivationView<'graph, 'here>,
    unit: crate::scope::UnitIndex,
) -> Result<Option<Binding<'here>>, &'here ShapeError<'here>> {
    let Some(instances) = step.instances() else {
        return Ok(None);
    };
    let shapes = step.door().0.shapes();
    let site = instances.graph().facts.site(shapes, view.segment(), unit);
    instanced_at(step, types, view, site)
}

/// The bind site of the part held in place at `site` of the body `view` runs, its held slot bound:
/// its node's instance, made as [`instanced`] makes a unit's, under what the frame binds with the
/// part beside. That binding, which the unit it is held in runs under; `None` where no node sits
/// there.
fn held_part<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    view: KActivationView<'graph, 'here>,
    site: Site,
) -> Result<Option<Binding<'here>>, KInner<'graph, 'here>> {
    let Some(instances) = step.instances() else {
        return Ok(None);
    };
    let Some(at) = instances.graph().facts.held_site(view.segment(), site) else {
        return Ok(None);
    };
    instanced_at(step, types, view, at).map_err(|error| laid_refusal(step, error))
}

/// The value bound for the part `at`'s body holds in place at `site`, once its unit hoisted it.
pub(super) fn held_value<'graph, 'here>(
    at: &KReading<'_, 'graph, 'here>,
    site: Site,
) -> Option<KValue<'graph, 'here>> {
    at.held(at.facts().held_index(at.shape(), site)?)
}

/// The next part the unit at run index `unit` of `at`'s body holds in place and has not bound:
/// inner parts first, then left to right, as a `LET` spelling would bind them.
fn next_held(at: &KReading<'_, '_, '_>, unit: u32) -> Option<Site> {
    let shape = at.shape();
    let index = UnitIndex::of_run(unit as usize);
    let mut waiting = (at.facts().held_sites(shape).iter().enumerate())
        .filter(|(held, site)| shape.unit_of_site(**site) == index && at.held(*held).is_none())
        .map(|(_, site)| *site);
    // Sites number an outer part before the parts inside it.
    let mut next = waiting.next()?;
    for site in waiting {
        if shape.within(next, site) {
            next = site;
        }
    }
    Some(next)
}

/// [`instanced`] at the site `site` of the graph.
fn instanced_at<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    view: KActivationView<'graph, 'here>,
    site: bindgraph::Site,
) -> Result<Option<Binding<'here>>, &'here ShapeError<'here>> {
    let Some(instances) = step.instances() else {
        return Ok(None);
    };
    let laid = instances.graph();
    let shapes = step.door().0.shapes();
    let Some(node) = laid.graph.node_at(site) else {
        return Ok(None);
    };
    let scratch = Bump::new();
    let mut bound = BumpVec::new_in(&scratch);
    let mut read = BumpVec::new_in(&scratch);
    let binding = frame_binding(step, types, view);
    // The names of its body's bind sites up to it, and of its enclosers' bodies.
    let earlier = (laid.graph.nodes().iter()).filter(|held| {
        let body = held.site().body();
        (body == site.body() && laid.graph.site_le(held.site(), site))
            || std::iter::successors(laid.graph.encloser(site.body()), |body| {
                laid.graph.encloser(*body)
            })
            .any(|encloser| encloser == body)
    });
    for name in earlier.flat_map(|held| held.names().iter().copied()) {
        let crate::scope::NameAddress::Site(unknown) = laid.facts.name(name).address else {
            continue;
        };
        // A name the frame's binding binds is bound as it binds it.
        if let Some(at) = binding.bound(unknown) {
            bound.push((name, at));
            continue;
        }
        let captured =
            |view: &KActivationView<'graph, 'here>, capture| match view.closure().get(capture) {
                Link::Value(value) => Some(value),
                Link::Edge(_) => None,
            };
        // A type name is read where the frame reads its level: its own slot, a block's around it,
        // or its closure's type capture of an encloser's name.
        let coordinate = (shapes.level(unknown.segment, unknown.at))
            .and_then(|level| shapes.coordinate_of(view.segment(), level));
        let held = match coordinate {
            Some(Coordinate::Activation { hops, target }) => {
                let at = (0..hops).try_fold(&view, |view, _| view.enclosing());
                at.and_then(|at| match target {
                    Target::Local(slot) => at.bound(slot),
                    Target::Capture(capture) => captured(at, capture),
                })
            }
            _ => std::iter::successors(Some(&view), |view| view.enclosing())
                .find(|view| view.segment() == unknown.segment)
                .and_then(|view| match unknown.at {
                    Address::Slot(slot) => view.bound(slot),
                    Address::Capture(capture) => captured(view, capture),
                    Address::Held(site) => {
                        let (code, facts) = step.door();
                        let shape = code.shape(view.segment());
                        (facts.held_index(shape, site)).and_then(|at| view.held(at))
                    }
                }),
        };
        if let Some(value) = held
            && let Some(at) = bound_of(&scratch, value, types, &scratch, None)
        {
            bound.push((name, at));
            if !read.iter().any(|(each, _)| *each == unknown) {
                read.push((unknown, value));
            }
        }
    }
    // What the instance binds off the frame's slots, its readers close a base record's types
    // under too.
    let binding = match read.is_empty() {
        true => binding,
        false => {
            let mut staged =
                BumpVec::with_capacity_in(binding.entries().len() + read.len(), &scratch);
            staged.extend(binding.entries().iter().copied());
            for (unknown, value) in read.iter() {
                if let Some(at) = bound_of(step.writer(), *value, types, &scratch, None) {
                    staged.push((*unknown, at));
                }
            }
            Binding::laid(step.writer(), &mut staged)
        }
    };
    // A binding that leaves a name the node reads open answers less than the frame knows: the
    // node holds no instance, and its readers read the load's records.
    let open = (laid.graph.relevant(node).iter())
        .any(|name| !laid.graph.is_answered(*name) && !bound.iter().any(|(each, _)| each == name));
    if open || view.handles().is_empty() || laid.graph.node(node).handle_width() == 0 {
        return Ok(Some(binding));
    }
    let range = laid.layout.range(laid.graph, node);
    let cells = &view.handles()[range];
    let mut words = BumpVec::new_in(&scratch);
    words.extend(cells.iter().map(Cell::get));
    let refused = instances.make(node, &bound, types, &scratch, &mut words);
    for (cell, word) in cells.iter().zip(words.iter()) {
        cell.set(*word);
    }
    view.handles()[laid.layout.made(node)].set(1);
    #[cfg(test)]
    if let Some(instanced) = INSTANCED.with(Cell::get) {
        let mut typed = BumpVec::new_in(&scratch);
        typed.extend(bound.iter().filter_map(|(name, at)| match at {
            crate::scope::Bound::Type(ty) => Some((*name, *ty)),
            _ => None,
        }));
        instanced(node, &words, &typed);
    }
    // A question the instance refuses faults the frame here, worded under its binding.
    if let Some((question, arm)) = refused {
        let stored = laid.facts.question(question).stored();
        let Some(Payload::Refusal(refusal)) = stored.map(|stored| stored.arms[usize::from(arm)])
        else {
            panic!("a refused answer names its refusal's template");
        };
        let facts = (step.door().1).framed(view.handle_words(), Some(binding));
        let refusal = facts.bound_refusal(types, &scratch, view.segment(), refusal);
        return Err(relaid_error(step.writer(), refusal));
    }
    Ok(Some(binding))
}

/// What a test hands each instance a bind site holds: its node, its handle's words, and each type
/// name the instance binds beside the type the frame binds it to.
#[cfg(test)]
pub(crate) type Instanced = fn(bindgraph::NodeId, &[u64], &[(bindgraph::NameId, KType)]);

#[cfg(test)]
thread_local! {
    /// What each bind site's instance is handed to while a test sets it.
    pub(crate) static INSTANCED: Cell<Option<Instanced>> = const { Cell::new(None) };
}

/// The closure run of `member`'s code, run as `shape`, in its capture order: each `$` name and
/// filled mark from the binding the code carries at its mark's scope and name, a filled key from
/// its list there, each `$(…)` key from the registrations it carries at that scope with that key,
/// each `\` name from `offered` — every edge resolved to the member it names, so the run holds
/// value words only — then each type capture the code carries. A keyworded hole holds no function.
/// `None` when a name is bound nowhere.
fn code_closure<'graph, 'here, 'x>(
    writer: Writer<'here, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    member: Knotted<'graph, 'here>,
    shape: &BodyShape<'_>,
    offered: KValue<'graph, 'here>,
    bump: &'x Bump,
) -> Option<BumpVec<'x, Link<'here, Knotted<'graph, 'here>>>> {
    let node = member.code()?;
    offered.as_record()?;
    let find = |run, scope, name| Some(binding_at(run, scope, name)?.link.resolve(member));
    let mut links = BumpVec::with_capacity_in(shape.captures().len(), bump);
    for capture in shape.captures() {
        let value = match capture.source {
            CaptureSource::Read(_) | CaptureSource::Member { .. } => {
                find(node.bound(), capture.scope(), capture.name)?
            }
            // A key a `USING` filled is bound to its list; a `$(…)` key, to the registrations at it.
            CaptureSource::Entry { .. } => match (
                find(node.bound(), capture.scope(), capture.name),
                capture.name,
            ) {
                (Some(filled), _) => filled,
                (None, BinderSymbol::Key(key)) => {
                    registrations_at(writer, member, (capture.scope(), key), types, bump)
                }
                (None, _) => return None,
            },
            // A hole is never bound at an `EVAL`; a key's holds no function.
            CaptureSource::Hole => match capture.name {
                BinderSymbol::Key(_) => Value::List(List::new(writer, [].into_iter(), types, bump)),
                _ => return None,
            },
            CaptureSource::Offered => offered.field(capture.name.symbol(), types, bump)?.value(),
        };
        links.push(Link::Value(value));
    }
    links.extend(
        node.layout()
            .iter()
            .map(|capture| Link::Value(capture.value)),
    );
    Some(links)
}

/// The list of the registrations the code `member` carries at `scope` whose key is `key`, built
/// in `writer`'s region: what an entry capture of a `$(…)` use's key binds — the empty list where
/// the code carries none. Grouped here, where a chain resolves each registration's key, so a code
/// value's run keeps its registrations one by one.
fn registrations_at<'graph, 'here>(
    writer: Writer<'here, '_>,
    member: Knotted<'graph, 'here>,
    (scope, key): (Scope, KeySymbol),
    types: &Chain<'_, 'here, '_, '_>,
    scratch: &Bump,
) -> KValue<'graph, 'here> {
    let node = member.code().expect("a code member holds code");
    let mut found = BumpVec::new_in(scratch);
    for binding in node.bound() {
        if binding.scope != scope || !matches!(binding.name, BinderSymbol::Registration(_)) {
            continue;
        }
        let function = binding.link.resolve(member);
        let shape = layout::registered_shape(function).expect("a registration is a function");
        if layout::key_of(shape, types) == key {
            found.push(function);
        }
    }
    if found.is_empty() {
        return Value::List(List::new(writer, [].into_iter(), types, scratch));
    }
    Value::List(List::of_candidates(writer, found.iter().copied(), types))
}

/// A runner handed back what it asked for: the evaluation's value bound or kept, or the parts a
/// tie named supplied and the tie made. A fault handed back stops it. In a caching body every value
/// carries its types in the body's chain `types`.
fn woke<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    runner: &mut Runner<'graph, 'here>,
    woken: Woken<'_, 'graph, 'here>,
    types: &Chain<'_, 'here, '_, '_>,
) -> Result<(), Stopped<'graph, 'here>> {
    let at = runner.reading(step);
    let shape = at.shape();
    let unit = runner.current(shape);
    match (runner.stage, woken) {
        (_, Woken::Fault(inner)) => return Err(Stopped::Raised(inner)),
        // The part is bound and its bind site visited; the unit it is held in runs next.
        (Stage::Holding(site), Woken::Value(received)) => {
            let index = (at.facts().held_index(shape, site)).expect("a hoisted part is held");
            (runner.activation.hold(index, received.value))
                .expect("a part is held once, before its unit runs");
            let view = runner.activation.view();
            let binding = match held_part(step, types, view, site).map_err(Stopped::Raised)? {
                Some(binding) => Some(binding),
                // A part no bind site sits at is a seam, as a `LET` binder holding it is: a base
                // record's types close under what the frame binds now, the part beside.
                None => step.instances().map(|_| frame_binding(step, types, view)),
            };
            if let Some(binding) = binding {
                runner.activation =
                    resident(step.writer(), runner.activation.binding(Some(binding)));
            }
            runner.stage = Stage::Next;
            return Ok(());
        }
        (Stage::Evaluation, Woken::Value(received)) => match unit.work {
            UnitWork::Component(component) => {
                let slot = shape.components()[component.index()].members[0];
                let value = held(step, types, runner, slot, received.value)?;
                runner.bind(unit, slot, value);
                runner.keep(slot, received.becoming(value));
                if runner.yields(unit) {
                    runner.result = Some(received.becoming(value));
                }
            }
            UnitWork::Statement(_) => {
                if runner.yields(unit) {
                    runner.result = Some(received);
                }
            }
        },
        (Stage::Supplying, Woken::Supplied(supplied)) => {
            #[cfg(test)]
            SUPPLIED_TIES.with(|ties| ties.set(ties.get() + 1));
            let UnitWork::Component(component) = unit.work else {
                unreachable!("only a component's tie names eager parts")
            };
            let component = &shape.components()[component.index()];
            tied(step, types, runner, unit, component, supplied)?;
        }
        _ => return Err(Stopped::Broken(StepError::Unredeemable)),
    }
    runner.unit += 1;
    runner.stage = Stage::Next;
    Ok(())
}

/// A tie the program refused, raised as the fault the body stops with.
fn untied<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    error: &Untieable<'_>,
) -> Stopped<'graph, 'here> {
    Stopped::Raised(laid(step, Fault::Untied(*error)))
}

/// Tie `component` again with `supplied` answering every eager part it names by site, and bind
/// every member from the knot.
fn tied<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    runner: &mut Runner<'graph, 'here>,
    unit: Unit,
    component: &Component<'_>,
    supplied: &[(Site, KValue<'graph, 'here>)],
) -> Result<(), Stopped<'graph, 'here>> {
    let scratch = Bump::new();
    let reading = runner.reading(step);
    let written = (runner.program).written_under(reading.facts(), runner.within.root);
    let tied = tie(
        step.writer(),
        &reading,
        written,
        component,
        runner.within.born(),
        types,
        &scratch,
        &mut |site, _| {
            if let Some(value) = held_value(&reading, site) {
                return Some(Supplied::Value(value));
            }
            let at = supplied
                .binary_search_by_key(&site, |(supplied, _)| *supplied)
                .ok()?;
            Some(Supplied::Value(supplied[at].1))
        },
    );
    match tied {
        Ok(tied) => {
            bound_members(step, types, runner, unit, component, &tied)?;
            Ok(())
        }
        Err(Untieable::Eager { .. }) => {
            unreachable!("a tie asks for every eager part at once, and every one was supplied")
        }
        Err(error) => Err(untied(step, &error)),
    }
}

/// Bind every member of `component` from `tied`, each held to its annotation as the tie resolved
/// it. The tie births an annotated member at its annotation, so each slot binds the bare member.
fn bound_members<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    runner: &mut Runner<'graph, 'here>,
    unit: Unit,
    component: &Component<'_>,
    tied: &Tied<'graph, 'here, '_>,
) -> Result<(), Stopped<'graph, 'here>> {
    let at = runner.reading(step);
    let shape = at.shape();
    for (index, slot) in component.members.iter().enumerate() {
        let member = Value::Knotted(Knotted::of(tied.knot, index));
        let resolved = tied.annotations.get(index).copied().flatten();
        let value = match (shape.annotation(*slot), resolved) {
            (Some(written), Some(annotation)) => {
                held_at(step, types, runner, written, annotation, member)?
            }
            // An annotation the tie could not resolve faults as `held` resolves it.
            _ => held(step, types, runner, *slot, member)?,
        };
        debug_assert!(
            !matches!(value, Value::Retyped(_)),
            "a member is born at its annotation"
        );
        runner.bind(unit, *slot, value);
    }
    Ok(())
}

/// `value`, about to be bound at `slot`, held to the type its `LET <name> <type> = …` states, if
/// it states one ([`Program::annotated`]).
fn held<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    runner: &Runner<'graph, 'here>,
    slot: Slot,
    value: KValue<'graph, 'here>,
) -> Result<KValue<'graph, 'here>, Stopped<'graph, 'here>> {
    let at = runner.reading(step);
    let Some(written) = at.shape().annotation(slot) else {
        return Ok(value);
    };
    let facts = at.facts();
    match facts.denoted(types, at.shape(), at.shape().site(written)) {
        Ok(annotation) => held_at(step, types, runner, written, annotation, value),
        Err(refused) => Err(Stopped::Raised(laid(step, Fault::Unelaborated(refused)))),
    }
}

/// `value` held to `annotation`, what the annotation `written` denotes.
fn held_at<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    runner: &Runner<'graph, 'here>,
    written: &ExpressionPart<'_>,
    annotation: KType,
    value: KValue<'graph, 'here>,
) -> Result<KValue<'graph, 'here>, Stopped<'graph, 'here>> {
    let at = runner.reading(step);
    let (facts, site) = (at.facts(), at.shape().site(written));
    let settled = facts.settled(at.shape(), site);
    let retype = facts.retype(types, at.shape(), site);
    (runner.program)
        .annotated(step.writer(), types, value, (annotation, retype), settled)
        .map_err(|fault| Stopped::Raised(laid(step, fault)))
}

/// Perform units, `woken` first handed back to the unit that asked for it, until one asks for
/// something or the body ends.
pub(super) fn turn<'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    runner: &mut Runner<'graph, 'here>,
    woken: Option<Woken<'_, 'graph, 'here>>,
    types: &Chain<'_, 'here, '_, '_>,
) -> Turn<'graph, 'here, 'scratch> {
    if let Some(woken) = woken
        && let Err(stopped) = woke(step, runner, woken, types)
    {
        return Turn::Stopped(stopped);
    }
    loop {
        if let Err(stopped) = bound_at_seams(step, types, runner) {
            return Turn::Stopped(stopped);
        }
        let at = runner.reading(step);
        let shape = at.shape();
        if runner.unit as usize == shape.units().len() {
            match runner.outer {
                Some(outer) => match left(step, types, *runner, outer) {
                    Ok(resumed) => *runner = resumed,
                    Err(stopped) => return Turn::Stopped(stopped),
                },
                None => return Turn::Ended,
            }
            continue;
        }
        let unit = runner.current(shape);
        // Each part the unit holds in place is evaluated and bound first, as a `LET` holding it
        // would be at a statement of its own.
        if let Some(site) = next_held(&at, runner.unit) {
            let threads = runner.owed.is_some()
                && runner.tails(unit, shape)
                && shape.threaded_parts().binary_search(&site).is_ok();
            runner.stage = Stage::Holding(site);
            return Turn::Ask {
                node: Evaluated::Part(site),
                use_: if threads { Use::Threads } else { Use::Keeps },
                owed: None,
            };
        }
        let component = match unit.work {
            UnitWork::Statement(statement) => {
                let node = Evaluated::Statement(statement);
                // A caching body keeps its value, so it leaves only where the frame adopts it; a
                // tail owes the body's contract.
                let use_ = match runner.yields(unit) {
                    true if runner.within.caches => Use::Keeps,
                    true => Use::Forwards,
                    false => Use::Reads,
                };
                let owed = runner.owed.filter(|_| runner.tails(unit, shape));
                runner.stage = Stage::Evaluation;
                return Turn::Ask { node, use_, owed };
            }
            UnitWork::Component(component) => &shape.components()[component.index()],
        };
        let types_only = component
            .members
            .iter()
            .any(|slot| matches!(shape.slot_name(*slot), BinderSymbol::Type(_)));
        if types_only {
            if let Err(stopped) = declared(step, types, runner, unit, component) {
                return Turn::Stopped(stopped);
            }
            runner.unit += 1;
            continue;
        }
        if let [slot] = component.members
            && shape
                .births(*slot)
                .is_some_and(|body| body.kind() == ShapeKind::Module)
        {
            *runner = entered(step, *runner, *slot);
            continue;
        }
        let births = component
            .members
            .iter()
            .all(|slot| shape.births(*slot).is_some());
        if component.cyclic || births {
            match first_tie(step, types, runner, unit, component) {
                Ok(None) => {
                    runner.unit += 1;
                    continue;
                }
                Ok(Some(sites)) => {
                    runner.stage = Stage::Supplying;
                    return Turn::Supply(sites);
                }
                Err(stopped) => return Turn::Stopped(stopped),
            }
        }
        let [slot] = component.members else {
            return Turn::Stopped(Stopped::Broken(StepError::Refused));
        };
        let Some(rhs) = shape.rhs(*slot) else {
            return Turn::Stopped(Stopped::Broken(StepError::Refused));
        };
        // A binder the frame's tail call hands on is built where it outlives the hop.
        let threads = runner.tail_body()
            && runner.outer.is_none()
            && shape.threaded().binary_search(slot).is_ok();
        runner.stage = Stage::Evaluation;
        return Turn::Ask {
            node: Evaluated::Part(shape.site(rhs)),
            use_: if threads { Use::Threads } else { Use::Keeps },
            owed: None,
        };
    }
}

/// The top level's runner: each evaluation it asks for a `Fresh` tree child of the root, so
/// everything it allocates beside its value dies with it, and each eager part a tie names one
/// such child, all at one park.
fn top_level<'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    mut runner: Runner<'graph, 'here>,
    woken: Option<Woken<'_, 'graph, 'here>>,
    types: &Chain<'_, 'here, '_, '_>,
) -> Action<'graph, KBundle> {
    match turn(&mut step, &mut runner, woken, types) {
        Turn::Ask { node, use_, .. } => {
            let asked = ask(&mut step, &runner, node, use_);
            step.park(asked, run, KState::Runner(runner), None)
        }
        Turn::Supply(sites) => {
            let mut asked = None;
            for site in sites {
                asked = Some(ask(&mut step, &runner, Evaluated::Part(*site), Use::Keeps));
            }
            let asked = asked.expect("a tie refused on an eager part named one");
            let sites = Some(KScratch::Supplying(sites));
            step.park(asked, run, KState::Runner(runner), sites)
        }
        Turn::Ended => ended(step, runner, types),
        Turn::Stopped(stopped) => stop(step, runner, stopped, types),
    }
}

/// The top level woken: what its children delivered, as its runner is handed it back — the one
/// evaluation it asked for, or every eager part at `sites`, by site.
fn top_level_woken<'graph, 'here, 'scratch>(
    mut step: Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    runner: Runner<'graph, 'here>,
    scratch: Option<KScratch<'graph, 'here, 'scratch>>,
    types: &Chain<'_, 'here, '_, '_>,
) -> Action<'graph, KBundle> {
    let local = Bump::new();
    let mut supplied: BumpVec<'_, (Site, KValue<'graph, 'here>)> = BumpVec::new_in(&local);
    let first = match runner.stage {
        Stage::Evaluation | Stage::Holding(_) => step.results_with(types).next(),
        _ => None,
    };
    let woken = match (runner.stage, scratch) {
        (Stage::Supplying, Some(KScratch::Supplying(sites))) => {
            let mut received: BumpVec<'_, _> = BumpVec::with_capacity_in(sites.len(), &local);
            received.extend(step.results_with(types));
            let mut fault = None;
            for (site, received) in sites.iter().zip(received) {
                match received {
                    Ok(Received::Here(held)) => supplied.push((*site, held.value)),
                    Ok(Received::Fault(inner)) => fault = Some(inner),
                    Ok(Received::Scratch(_)) => {
                        unreachable!("an eager part is asked for with `Keeps`")
                    }
                    Err(error) => return step.failed(error),
                }
            }
            supplied.sort_unstable_by_key(|(site, _)| *site);
            match fault {
                Some(inner) => Woken::Fault(inner),
                None => Woken::Supplied(&supplied),
            }
        }
        (Stage::Evaluation | Stage::Holding(_), _) => match first {
            Some(Ok(Received::Here(held))) => Woken::Value(held),
            Some(Ok(Received::Fault(inner))) => Woken::Fault(inner),
            _ => return step.failed(StepError::Unredeemable),
        },
        _ => return step.failed(StepError::Unredeemable),
    };
    top_level(step, runner, Some(woken), types)
}

/// The bind sites and statement seams of the body the runner performs, a block's and an inline
/// module body's included, among the units it ran since it last looked: each bind site makes its
/// instance ([`instanced`]); and where a seam's slot, which some unit's frontier holds and its
/// facts leave unbound, binds a type or a module, a base record's types close under what the
/// frame binds now ([`frame_binding`]).
fn bound_at_seams<'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    runner: &mut Runner<'graph, 'here>,
) -> Result<(), Stopped<'graph, 'here>> {
    let (from, to) = (runner.seen as usize, runner.unit as usize);
    runner.seen = runner.unit;
    if from == to {
        return Ok(());
    }
    for ran in from..to {
        let view = runner.activation.view();
        let binding = instanced(step, types, view, UnitIndex::of_run(ran))
            .map_err(|error| Stopped::Raised(laid_refusal(step, error)))?;
        if let Some(binding) = binding {
            runner.activation = resident(step.writer(), runner.activation.binding(Some(binding)));
        }
    }
    let at = runner.reading(step);
    let (shape, facts, view) = (at.shape(), *at.facts(), at.view());
    let seams = facts.statement_seams(shape);
    if seams.is_empty() {
        return Ok(());
    }
    let scratch = Bump::new();
    let mut bound = false;
    for slot in seams {
        let ran = shape.unit_of_slot(*slot).run_index();
        if !ran.is_some_and(|ran| (from..to).contains(&ran)) {
            continue;
        }
        let unknown = Unknown {
            segment: shape.segment(),
            at: Address::Slot(*slot),
        };
        // A seam is a binder whose content the facts hold only as a leaf: an action's result, a
        // call that does not unfold. One whose tree they build, closed or over other binders, or
        // whose declaration they close, was decided where they were — save one an open path is
        // rooted at over an open tree, which the path reads off what the run binds.
        let rooting = facts.rooting_seams(shape).binary_search(slot).is_ok();
        let seen = (facts.content(shape, *slot))
            .is_some_and(|tree| !matches!(tree, Tree::Leaf(_)) && (!rooting || tree.closed()));
        let closed = seen || matches!(facts.declared_type(types, shape, *slot), Static::Closed(_));
        if closed || facts.binds(unknown) {
            continue;
        }
        let memo = at.digest_memo(local(*slot));
        bound |= (view.bound(*slot))
            .and_then(|value| bound_of(&scratch, value, types, &scratch, memo))
            .is_some();
    }
    // The seam's slots are bound: a base record's types close under what the frame binds now.
    if bound && step.instances().is_some() {
        let binding = frame_binding(step, types, view);
        runner.activation = resident(step.writer(), runner.activation.binding(Some(binding)));
    }
    Ok(())
}

/// Ask the top level's evaluation of `node` through the program's `evaluate`: a `Fresh` tree child
/// of the root.
fn ask<'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    runner: &Runner<'graph, 'here>,
    node: Evaluated,
    use_: Use,
) -> Asked {
    let work = runner
        .program
        .evaluate(node, runner.activation.view(), runner.handing());
    step.spawn(Request {
        placement: Placement::Fresh,
        use_,
        work,
    })
}

/// A component's first tie: bound, or the sites of the eager parts it named, laid in scratch.
fn first_tie<'graph, 'here, 'scratch, 'lent>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, 'lent>,
    types: &Chain<'_, 'here, '_, '_>,
    runner: &mut Runner<'graph, 'here>,
    unit: Unit,
    component: &Component<'_>,
) -> Result<Option<&'scratch [Site]>, Stopped<'graph, 'here>> {
    let scratch = Bump::new();
    let mut missed: BumpVec<'_, Site> = BumpVec::new_in(&scratch);
    let reading = runner.reading(step);
    let written = (runner.program).written_under(reading.facts(), runner.within.root);
    let tied = tie(
        step.writer(),
        &reading,
        written,
        component,
        runner.within.born(),
        types,
        &scratch,
        &mut |site, part| {
            if let Some(value) = held_value(&reading, site) {
                return Some(Supplied::Value(value));
            }
            missed.extend(part.map(|_| site));
            None
        },
    );
    match tied {
        Ok(tied) => {
            bound_members(step, types, runner, unit, component, &tied)?;
            Ok(None)
        }
        Err(Untieable::Eager { .. }) if !missed.is_empty() => Ok(Some(
            step.scratch_writer().fill(missed.len(), |at| missed[at]),
        )),
        Err(error) => Err(untied(step, &error)),
    }
}

/// Bind each member of a component of type binders to the handle the load, or a layer the runner
/// reads, fixed for it — a nominal one an activation's walk declared interned through the frame's
/// chain — or raise the refusal every run of it meets.
fn declared<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    runner: &mut Runner<'graph, 'here>,
    unit: Unit,
    component: &Component<'_>,
) -> Result<(), Stopped<'graph, 'here>> {
    let writer = step.writer();
    let at = runner.reading(step);
    let (facts, shape) = (at.facts(), at.shape());
    for slot in component.members {
        let declaration =
            (shape.declarations(*slot)).expect("a type binder records its declaration");
        let site = shape.site_of_node(declaration);
        let declared = facts.declared_type(types, shape, *slot);
        let handle = match facts.at_run(types, shape, site, declared) {
            AtRun::Closed(handle) => handle,
            AtRun::Refused(refused) => {
                return Err(Stopped::Raised(laid(step, Fault::Unelaborated(refused))));
            }
            AtRun::Open => unreachable!("a type binder's declaration is fixed where it runs"),
        };
        runner.bind(
            unit,
            *slot,
            Value::Type(TypeValue::new(writer, handle, types)),
        );
    }
    Ok(())
}

/// Enter the body the module binder `slot` births: its activation laid down in the running region,
/// the enclosing body's place kept beside it.
fn entered<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    runner: Runner<'graph, 'here>,
    slot: Slot,
) -> Runner<'graph, 'here> {
    let writer = step.writer();
    let scratch = Bump::new();
    let body = resident(
        writer,
        body_activation(writer, &runner.reading(step), slot, &scratch),
    );
    let outer = resident(
        writer,
        Outer {
            activation: runner.activation,
            unit: runner.unit,
            result: runner.result,
            binder: slot,
            seen: runner.seen,
            outer: runner.outer,
        },
    );
    Runner {
        activation: body,
        unit: 0,
        seen: 0,
        result: None,
        outer: Some(outer),
        ..runner
    }
}

/// A module body ended: tie its binder from the finished body and resume the enclosing body past
/// it.
fn left<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    body: Runner<'graph, 'here>,
    outer: &'here Outer<'graph, 'here>,
) -> Result<Runner<'graph, 'here>, Stopped<'graph, 'here>> {
    let scratch = Bump::new();
    let activation = outer.activation;
    let (code, facts) = step.door();
    let reading = activation.view().reading(&code, facts);
    let written = (body.program).written_under(&facts, body.within.root);
    let knot = tie(
        step.writer(),
        &reading,
        written,
        reading.shape().component_of(outer.binder),
        body.within.born(),
        types,
        &scratch,
        &mut |_, _| Some(Supplied::Body(body.activation)),
    )
    .map_err(|error| untied(step, &error))?;
    let mut resumed = Runner {
        activation,
        unit: outer.unit,
        result: outer.result,
        seen: outer.seen,
        outer: outer.outer,
        ..body
    };
    let unit = resumed.current(reading.shape());
    resumed.bind(
        unit,
        outer.binder,
        Value::Knotted(Knotted::of(knot.knot, 0)),
    );
    resumed.unit += 1;
    Ok(resumed)
}

/// The body's end: the top level leaves its activation's view at rest for a later root work, and a
/// called frame or an `EVAL`'s finishes with its value held to its contract, crossed out through
/// its barrier where it was called through one, and handed on from where it rests otherwise.
pub(super) fn ended<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    runner: Runner<'graph, 'here>,
    types: &Chain<'_, 'here, '_, '_>,
) -> Action<'graph, KBundle> {
    let held = runner.result.unwrap_or(Held::bare(Value::Null));
    let caches = runner.within.caches;
    match (runner.level, runner.owed) {
        (Level::Top, _) => step.leave(KBirth::Inspect {
            program: runner.program,
            view: runner.activation.view(),
        }),
        (Level::Frame | Level::Block, None) if caches => step.finish_with(held, types),
        (Level::Frame | Level::Block, None) => step.finish(held),
        (Level::Frame | Level::Block, Some(owed)) => {
            let scratch = Bump::new();
            let fulfilled = (runner.program)
                .fulfilled(step.writer(), types, held.value, owed.contract)
                .and_then(|value| match runner.barrier {
                    Some(barrier) => {
                        let crossed = crossed_out(step.writer(), types, barrier.outermost, value)?;
                        Ok(Held::bare(match barrier.returns {
                            Some(returns) => {
                                crossed.retyped(step.writer(), returns, types, &scratch)
                            }
                            None => crossed,
                        }))
                    }
                    None => Ok(held.becoming(value)),
                });
            let held = match fulfilled {
                Ok(held) => held,
                Err(fault) => {
                    let inner = laid(&step, fault);
                    return faulted(step, types, Some(owed.line()), inner);
                }
            };
            if caches {
                Exit::new(step.types(), types, &scratch).value(&held.value);
            }
            step.finish(held)
        }
    }
}

/// A unit stopped. A fault ends the body — a frame or a block ends with it, owing its line where
/// it owes one — and a broken invariant fails the step. At the top level a fault a cell below kept
/// is recorded for the run to render once the drain ends; one the top level raised itself is
/// rendered here, through `types`, to the error sink. Either way the run is marked uncaught and the
/// top level leaves its view at rest.
pub(super) fn stop<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    runner: Runner<'graph, 'here>,
    stopped: Stopped<'graph, 'here>,
    types: &Chain<'_, 'here, '_, '_>,
) -> Action<'graph, KBundle> {
    let inner = match stopped {
        Stopped::Raised(inner) => inner,
        Stopped::Broken(error) => return step.failed(error),
    };
    let program = runner.program;
    if runner.level != Level::Top {
        return faulted(step, types, runner.line(), inner);
    }
    let scratch = Bump::new();
    let mut trace = Trace::new(&scratch);
    let ending = match trace.gathered(inner, program.symbols(), types) {
        Some(kept) => {
            debug_assert!(trace.is_bare(), "the top level's own region owes no line");
            Ending::Kept(kept)
        }
        None => {
            (program.output().error)(rendered(step.writer(), &trace));
            Ending::Rendered
        }
    };
    program.ending().set(ending);
    step.leave(KBirth::Inspect {
        program,
        view: runner.outermost().view(),
    })
}
