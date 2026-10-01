//! Priority classes: a bucket declaration ranks the definitions that see it, two declarations of
//! one ranking are one, a ranking is part of the shape type, and two rankings that meet are
//! refused wherever they meet.

use std::cell::RefCell;

use crate::memory::Bump;
use crate::program::{CellSubstrate, KBirth, KBundle, KState};
use crate::scheduler::{Action, Step, StepError};
use crate::scope::{Coordinate, Target};
use crate::type_lattice::{FitsFailure, sig_fits};

use super::super::Koan;
use super::{output, run};

/// Two `MOVE` definitions that rank opposite ways under the two orders.
const MOVES: &str = "EXPR #(MOVE x :Any TO y :Number) -> Str = #(\"to a number\")\n\
                     EXPR #(MOVE x :Number TO y :Any) -> Str = #(\"from a number\")\n\
                     PRINT (MOVE 1 TO 1)";

#[test]
fn a_definition_takes_the_ranking_of_the_declaration_it_sees() {
    assert_eq!(run(MOVES), "from a number", "written order ranks `x` first");
    assert_eq!(
        run(&format!("EXPR #(MOVE 2 TO 1)\n{MOVES}")),
        "to a number",
        "the declaration ranks `y` first"
    );
}

#[test]
fn two_declarations_of_one_ranking_are_one() {
    assert_eq!(
        run(&format!(
            "EXPR #(MOVE 2 TO 1)\nEXPR #(MOVE 20 TO 10)\n{MOVES}"
        )),
        "to a number"
    );
}

#[test]
fn a_declaration_is_no_candidate() {
    assert_eq!(
        run("EXPR #(MOVE 2 TO 1)\nMOVE 1 TO 2"),
        "load: <test>:2:1: `MOVE _ TO _` has no overload visible here"
    );
}

#[test]
fn rankings_that_disagree_are_refused_where_they_meet() {
    let refused = |line: u32, column: u32| {
        format!("load: <test>:{line}:{column}: `MOVE _ TO _` is ranked two ways here")
    };
    assert_eq!(
        run(&format!(
            "EXPR #(MOVE 2 TO 1)\nEXPR #(MOVE 1 TO 2)\n{MOVES}"
        )),
        refused(2, 1),
        "a declaration that sees another"
    );
    assert_eq!(
        run(&format!("{MOVES}\nEXPR #(MOVE 2 TO 1)")),
        refused(4, 1),
        "a declaration that sees a definition carrying written order"
    );
    assert_eq!(
        run(
            "EXPR #(OUTER) -> Any = #(\n  EXPR #(MOVE 2 TO 1)\n  MOVE 1 TO 2\n)\n\
             EXPR #(MOVE x :Any TO y :Any) -> Any = #(x)"
        ),
        refused(2, 3),
        "an inner declaration does not shadow an outer definition"
    );
    assert_eq!(
        run("MODULE m = (EXPR #(MOVE x :Any TO y :Any) -> Any = #(y))\n\
             LET q = #((EXPR #(MOVE 2 TO 1)) (EXPR #(MOVE x :Str TO y :Str) -> Any = #(x)) \
             (MOVE 1 TO 2))\n\
             PRINT (EVAL (q USING m) -> Any)"),
        "error: MOVE _ TO _ is ranked two ways",
        "a `USING` fill"
    );
}

#[test]
fn a_ranking_is_part_of_the_shape_type() {
    let source = "PRINT (:(EXPR #(MOVE 2 :Any TO 1 :Any) -> Any) == \
                  :(EXPR #(MOVE 20 :Any TO 10 :Any) -> Any))\n\
                  PRINT (:(EXPR #(MOVE 2 :Any TO 1 :Any) -> Any) == \
                  :(EXPR #(MOVE _ :Any TO _ :Any) -> Any))\n\
                  PRINT :(EXPR #(MOVE 2 :Any TO 1 :Any) -> Any)";
    assert_eq!(
        run(source),
        "true\nfalse\n:(EXPR #(MOVE 2 :Any TO 1 :Any) -> Any)"
    );
}

thread_local! {
    /// What [`ascribing`] found.
    static ASCRIBED: RefCell<String> = const { RefCell::new(String::new()) };
}

/// A root work over the top level's resting view: whether module `m` satisfies signature `Mover`.
fn ascribing<'graph>(step: Step<'_, 'graph, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let KState::Born(birth @ KBirth::Inspect { program, view }) = state else {
        return step.failed(StepError::Refused);
    };
    let read = |name: &str| {
        let slot = program.binding(name).expect("a top-level binding");
        view.read(Coordinate::Activation {
            hops: 0,
            target: Target::Local(slot),
        })
    };
    let types = program.types();
    let module = read("m").ktype();
    let mover = read("Mover").as_type().expect("a type").handle();
    let verdict = match sig_fits(types, &Bump::new(), module, mover) {
        Ok(()) => String::from("satisfies"),
        Err(FitsFailure::RankingMismatch { .. }) => String::from("ranked otherwise"),
        Err(_) => String::from("fails otherwise"),
    };
    ASCRIBED.with(|ascribed| *ascribed.borrow_mut() = verdict);
    step.leave(birth)
}

#[test]
fn a_module_ranked_in_written_order_does_not_satisfy_a_member_ranked_otherwise() {
    let verdict = |module: &str| {
        let source =
            format!("SIG Mover = #[(EXPR #(MOVE 2 :Any TO 1 :Any) -> Any)]\nMODULE m = ({module})");
        let mut substrate = CellSubstrate::load::<Koan>(&source, "<test>", 8, output())
            .unwrap_or_else(|error| panic!("`{source}` loads: {error}"));
        substrate.with(|running| {
            running.run().expect("the program runs");
            running.inspect(ascribing).expect("the inspection runs");
        });
        ASCRIBED.with(|ascribed| ascribed.borrow().clone())
    };
    let definition = "EXPR #(MOVE x :Any TO y :Any) -> Any = #(x)";
    assert_eq!(verdict(definition), "ranked otherwise");
    assert_eq!(
        verdict(&format!("(EXPR #(MOVE 2 TO 1)) ({definition})")),
        "satisfies"
    );
}
