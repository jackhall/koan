//! Faults ([README § Errors](../README.md#errors)): a value of `Error` is an ordinary value, a part
//! that faults stops the parts after it, and the frames an uncaught fault passed through stay
//! unreleased until the host drops the outcome holding them. An uncaught fault prints its trace,
//! innermost first, a chain of tail hops counted per callee and call site.

use crate::program::{CellSubstrate, Outcome};
use crate::values::BuildBudget;

use super::tail::peak_of;
use super::{Koan, output, run};

/// A value of declared type `Any` that holds the string `"s"`, so the load refuses nothing a run
/// over it faults on.
const ANY_STR: &str = "LET r = ({v = \"s\"} :! :{v :Any})\n";

#[test]
fn a_value_of_error_raises_nothing() {
    assert_eq!(
        run("PRINT (Error.Refused {message = \"boom\", frames = []})\nPRINT \"after\""),
        "Refused({frames = [], message = boom})\nafter"
    );
}

#[test]
fn a_part_that_faults_stops_the_parts_after_it() {
    assert_eq!(
        run(&format!(
            "{ANY_STR}EXPR #(THREE a :Any b :Any c :Any) -> Any = #(a)\n\
             PRINT (THREE (PRINT \"a\") (1 + r.v) (PRINT \"b\"))"
        )),
        "a\nerror: no overload of `_ + _` admits (Number, Str)"
    );
}

#[test]
fn an_eager_part_of_a_tie_that_faults_stops_the_parts_after_it() {
    assert_eq!(
        run(&format!(
            "{ANY_STR}LET d = {{f = (FN :{{}} -> Any = #(d)), a = (PRINT \"a\"), \
             b = (1 + r.v), c = (PRINT \"b\")}}"
        )),
        "a\nerror: no overload of `_ + _` admits (Number, Str)"
    );
}

#[test]
fn an_uncaught_fault_keeps_its_frames_until_its_outcome_drops() {
    let source = format!(
        "{ANY_STR}EXPR #(INNER) -> Number = #((LET n = (1 + r.v)) (n))\n\
         EXPR #(MIDDLE) -> Number = #((LET m = (INNER)) (m))\n\
         EXPR #(OUTER) -> Number = #((LET o = (MIDDLE)) (o))\n\
         PRINT (OUTER)"
    );
    let mut substrate =
        CellSubstrate::load::<Koan>(&source, "<test>", 8, BuildBudget::default(), output())
            .expect("the program loads");
    substrate.with(|running| {
        let Ok(Outcome::Uncaught(uncaught)) = running.run() else {
            panic!("the fault reaches the top level");
        };
        assert!(
            uncaught.kept_live() >= 3,
            "every frame the fault passed through is kept: {}",
            uncaught.kept_live()
        );
        drop(uncaught);
        assert!(running.emptied(), "the drop released every kept cell");
    });
}

#[test]
fn a_fault_keeps_the_values_it_names() {
    // Two records holding a module, built in a frame and compared there: the fault holds both
    // records where they lie, in the frame's kept region, until the outcome drops.
    let source = "MODULE m = (LET x = 1)\n\
         EXPR #(SAME) -> Bool = \
         #((LET a = ({k = m} :! :{k :Any})) (LET b = ({k = m} :! :{k :Any})) (a == b))\n\
         PRINT (SAME)";
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output())
            .expect("the program loads");
    substrate.with(|running| {
        let Ok(Outcome::Uncaught(uncaught)) = running.run() else {
            panic!("the fault reaches the top level");
        };
        assert!(
            uncaught.kept_live() >= 1,
            "the frame holding the records is kept"
        );
        drop(uncaught);
        assert!(running.emptied(), "the drop released every kept cell");
    });
}

/// The fault [`ANY_STR`]'s `r.v` raises added to a number.
const PLUS: &str = "error: no overload of `_ + _` admits (Number, Str)";

#[test]
fn a_trace_names_each_frame_innermost_first() {
    assert_eq!(
        run(&format!(
            "{ANY_STR}EXPR #(INNER) -> Number = #((LET n = (1 + r.v)) (n))\n\
             EXPR #(OUTER) -> Number = #((LET o = (INNER)) (o))\n\
             PRINT (OUTER)"
        )),
        format!(
            "{PLUS}\n  in :(FN :{{}} -> Number) (INNER) at <test>:3:38\n  \
             in :(FN :{{}} -> Number) (OUTER) at <test>:4:7"
        )
    );
}

#[test]
fn a_fault_outside_any_call_prints_no_line() {
    assert_eq!(run(&format!("{ANY_STR}PRINT (1 + r.v)")), PLUS);
}

#[test]
fn a_frames_last_statement_owes_its_line() {
    // The frame hands its last statement off, so the hand-off owes the frame's line.
    assert_eq!(
        run(&format!(
            "{ANY_STR}EXPR #(LAST) -> Number = #((PRINT \"in\") (1 + r.v))\nPRINT (LAST)"
        )),
        format!("in\n{PLUS}\n  in :(FN :{{}} -> Number) (LAST) at <test>:3:7")
    );
}

#[test]
fn a_tail_arm_owes_its_frames_line() {
    assert_eq!(
        run(&format!(
            "{ANY_STR}EXPR #(ARM x :Any) -> Any = #(MATCH x WITH #{{:Number: (1 + r.v), :Str: (x)}})\n\
             PRINT (ARM 1)"
        )),
        format!("{PLUS}\n  in :(FN :{{x :Any}} -> Any) (ARM 1) at <test>:3:7")
    );
}

#[test]
fn an_eval_frame_names_eval() {
    assert_eq!(
        run(&format!(
            "{ANY_STR}PRINT (EVAL (#(1 + x) USING {{x = r.v}}) -> Any)"
        )),
        format!("{PLUS}\n  in `EVAL` (EVAL (#(1 + x) USING {{x = r.v}}) -> Any) at <test>:2:7")
    );
}

#[test]
fn a_return_miss_is_traced_from_the_frame_that_missed() {
    assert_eq!(
        run(&format!(
            "{ANY_STR}EXPR #(BAD) -> Number = #(r.v)\n\
             EXPR #(CALLS) -> Any = #((LET b = (BAD)) (b))\n\
             PRINT (CALLS)"
        )),
        "error: :(FN :{} -> Number) returned Str, which does not satisfy Number\n  \
         in :(FN :{} -> Number) (BAD) at <test>:3:35\n  \
         in :(FN :{} -> Any) (CALLS) at <test>:4:7"
    );
}

/// A self tail loop of `depth` hops whose last frame faults.
fn down(depth: u32) -> String {
    format!(
        "{ANY_STR}EXPR #(DOWN n :Number) -> Number = \
         #(MATCH (n == 0) WITH #{{true: (1 + r.v), false: (DOWN (n - 1))}})\n\
         PRINT (DOWN {depth})"
    )
}

#[test]
fn a_tail_loop_counts_its_hops_at_its_tail_site_in_constant_cells() {
    // The live frame is folded into its own line's count: 1000 frames at the tail site.
    assert_eq!(
        run(&down(1000)),
        format!(
            "{PLUS}\n  in :(FN :{{n :Number}} -> Number) (DOWN (n - 1)) at <test>:2:84 ×1000\n  \
             in :(FN :{{n :Number}} -> Number) (DOWN 1000) at <test>:3:7"
        )
    );
    let (deep, shallow) = if cfg!(miri) { (3, 2) } else { (1000, 4) };
    assert_eq!(peak_of(&down(deep)), peak_of(&down(shallow)));
}

#[test]
fn mutual_tail_recursion_prints_a_line_per_callee_and_call_most_recent_first() {
    assert_eq!(
        run(&format!(
            "{ANY_STR}EXPR #(ODD n :Number) -> Number = \
             #(MATCH (n == 0) WITH #{{true: (1 + r.v), false: (EVEN (n - 1))}})\n\
             EXPR #(EVEN n :Number) -> Number = \
             #(MATCH (n == 0) WITH #{{true: (1 + r.v), false: (ODD (n - 1))}})\n\
             PRINT (EVEN 5)"
        )),
        format!(
            "{PLUS}\n  in :(FN :{{n :Number}} -> Number) (ODD (n - 1)) at <test>:3:84 ×3\n  \
             in :(FN :{{n :Number}} -> Number) (EVEN (n - 1)) at <test>:2:83 ×2\n  \
             in :(FN :{{n :Number}} -> Number) (EVEN 5) at <test>:4:7"
        )
    );
}

#[test]
fn a_recursion_outside_tail_position_merges_into_one_line() {
    assert_eq!(
        run(&format!(
            "{ANY_STR}EXPR #(DEEP n :Number) -> Number = \
             #(MATCH (n == 0) WITH #{{true: (1 + r.v), false: (1 + (DEEP (n - 1)))}})\n\
             PRINT (DEEP 49)"
        )),
        format!(
            "{PLUS}\n  in :(FN :{{n :Number}} -> Number) (DEEP (n - 1)) at <test>:2:89 ×49\n  \
             in :(FN :{{n :Number}} -> Number) (DEEP 49) at <test>:3:7"
        )
    );
}
