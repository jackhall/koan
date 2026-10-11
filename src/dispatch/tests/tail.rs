//! A keyworded self-call in tail position hops to the callee's frame: however deep the recursion,
//! the cells live at once stay constant.

use std::cell::Cell;

use crate::program::{CellSubstrate, KBirth, KBundle, KState, run as body};
use crate::scheduler::{Action, Placement, Step, StepError, Work};
use crate::type_lattice::Branch;

use super::super::Koan;
use super::output;

/// A program whose `WALK` follows a chain of `depth` links, each a newtype over a record naming the
/// next, to the `null` at its end — selecting its overload by the link's type, and calling itself
/// by keyword as its last statement.
fn chain(depth: u32) -> String {
    let mut source = String::from(
        "NEWTYPE Link = :{next :Any}\n\
         EXPR #(WALK link :Link) -> Str = #(WALK (ATTR link next))\n\
         EXPR #(WALK end :Null) -> Str = #(\"done\")\n\
         LET link0 = null\n",
    );
    for at in 1..=depth {
        source.push_str(&format!(
            "LET link{at} = (Link {{next = link{}}})\n",
            at - 1
        ));
    }
    source.push_str(&format!("PRINT (WALK link{depth})"));
    source
}

/// The most cells live at once while the walk of a `depth`-link chain runs.
fn peak_walking(depth: u32) -> usize {
    peak_of(&chain(depth))
}

/// The most cells live at once while `source` runs.
fn peak_of(source: &str) -> usize {
    let mut substrate = CellSubstrate::load::<Koan>(source, "<test>", 2, output())
        .unwrap_or_else(|error| panic!("the program loads: {error}"));
    substrate.with(|running| {
        let work = Work {
            step: body,
            state: KBirth::Program {
                program: running.program(),
            },
        };
        let root = running.root();
        let mut scheduler = running.scheduler();
        scheduler
            .run(work, root, Placement::Shares)
            .expect("the program runs");
        scheduler.peak_live_cells()
    })
}

#[test]
fn a_keyworded_self_call_in_tail_position_holds_its_cells_constant_however_deep() {
    // The frame tails its last statement into the evaluator under its contract; the evaluator
    // selects `WALK` again, whose declared `Str` the contract asks, and hops to its frame.
    // Under Miri a few hops past the shallow walk are enough to exercise the hop's crossing.
    let (deep, shallow) = if cfg!(miri) { (5, 2) } else { (10_000, 4) };
    assert_eq!(peak_walking(deep), peak_walking(shallow));
}

/// What each hop of a [`countdown`] runs before it hops: a module over the link it is handed, viewed opaquely,
/// so each hop holds different content and mints its view's carrier.
const VIEW: &str = "(MODULE m OVER #[link] = (LET held = link)) (LET seen = (m :| Holder))";

/// [`VIEW`]'s module, built and never viewed: the same work less the minting.
const MODULE: &str = "(MODULE m OVER #[link] = (LET held = link))";

/// A program whose `COUNTDOWN` follows a chain of `depth` links as `WALK` does, running `body`
/// before each hop. `call` false leaves the chain unwalked, so the difference prices the walk.
fn countdown(depth: u32, body: &str, call: bool) -> String {
    let mut source = format!(
        "SIG Holder FOR ALL #[Elt] = #[(VAL held :Elt)]\n\
         NEWTYPE Link = :{{next :Any}}\n\
         EXPR #(COUNTDOWN link :Link) -> Str = #({body} (COUNTDOWN (ATTR link next)))\n\
         EXPR #(COUNTDOWN end :Null) -> Str = #(\"done\")\n\
         LET link0 = null\n"
    );
    for at in 1..=depth {
        source.push_str(&format!(
            "LET link{at} = (Link {{next = link{}}})\n",
            at - 1
        ));
    }
    if call {
        source.push_str(&format!("PRINT (COUNTDOWN link{depth})"));
    }
    source
}

thread_local! {
    /// What [`branch_length`] read.
    static BRANCH: Cell<usize> = const { Cell::new(0) };
}

/// A root work over the top level's resting view: it records the root cell's branch length.
fn branch_length<'graph>(step: Step<'_, 'graph, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let KState::Born(birth @ KBirth::Inspect { .. }) = state else {
        return step.failed(StepError::Refused);
    };
    BRANCH.with(|branch| branch.set(step.header().get().map_or(0, Branch::len)));
    step.leave(birth)
}

/// What one walk of a countdown leaves: the most cells live at once, the heap allocations the
/// walk made, the root's length before and after it, and the root cell's branch length after.
struct Walk {
    peak: usize,
    allocations: u64,
    root: [usize; 2],
    branch: usize,
}

/// The heap allocations `running.run()` makes over `source`.
fn run_allocations(source: &str) -> u64 {
    let mut substrate = CellSubstrate::load::<Koan>(source, "<test>", 2, output())
        .unwrap_or_else(|error| panic!("the countdown loads: {error}"));
    let before = crate::tests::allocation_count();
    substrate
        .with(|running| running.run())
        .expect("the countdown runs");
    crate::tests::allocation_count() - before
}

/// One walk of a `depth`-link countdown running `body`, its allocations net of the same program
/// run without the walk.
fn walking(depth: u32, body: &str) -> Walk {
    let source = countdown(depth, body, true);
    let peak = peak_of(&source);
    let mut substrate = CellSubstrate::load::<Koan>(&source, "<test>", 2, output())
        .unwrap_or_else(|error| panic!("the countdown loads: {error}"));
    let before = substrate.with(|running| running.program().root().len());
    substrate
        .with(|running| running.run())
        .expect("the countdown runs");
    let after = substrate.with(|running| running.program().root().len());
    substrate
        .with(|running| running.inspect(branch_length))
        .expect("the inspection runs");
    let allocations = run_allocations(&source) - run_allocations(&countdown(depth, body, false));
    Walk {
        peak,
        allocations,
        root: [before, after],
        branch: BRANCH.with(Cell::get),
    }
}

#[test]
fn a_fresh_loop_viewing_a_new_module_each_hop_holds_its_types_flat() {
    // Each hop's frame is a cell of its own, so the carrier it mints lives in that frame's branch
    // and dies with it. A koan hop costs the heap a few allocations whatever it does, and building
    // the module more as the chain it captures deepens, so the minting is priced as the view's
    // cost over the module's: the same per hop at both depths.
    let (few, many) = if cfg!(miri) { (2, 4) } else { (64, 512) };
    let [viewed_few, viewed_many] = [few, many].map(|depth| walking(depth, VIEW));
    let [built_few, built_many] = [few, many].map(|depth| walking(depth, MODULE));
    assert_eq!(
        viewed_many.peak, viewed_few.peak,
        "a hop releases its predecessor"
    );
    assert_eq!(
        viewed_few.root[0], viewed_few.root[1],
        "the run writes no root"
    );
    assert_eq!(
        viewed_many.root, viewed_few.root,
        "the load's types are the depth's"
    );
    assert_eq!(
        [viewed_few.branch, viewed_many.branch],
        [0, 0],
        "no carrier reaches the root cell's branch"
    );
    let minting = |viewed: &Walk, built: &Walk| viewed.allocations - built.allocations;
    let (per_few, per_many) = (
        minting(&viewed_few, &built_few),
        minting(&viewed_many, &built_many),
    );
    assert!(
        per_many * u64::from(few) <= per_few * u64::from(many),
        "minting costs {per_many} over {many} hops against {per_few} over {few}"
    );
}
