//! A keyworded self-call in tail position hops to the callee's frame: however deep the recursion,
//! the cells live at once stay constant.

use crate::program::{CellSubstrate, KBirth, run as body};
use crate::scheduler::{Placement, Work};

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
    let source = chain(depth);
    let mut substrate = CellSubstrate::load::<Koan>(&source, "<test>", 2, output())
        .unwrap_or_else(|error| panic!("the chain loads: {error}"));
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
