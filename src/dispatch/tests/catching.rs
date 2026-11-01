//! `TRY` and `CATCH`: what the load types each as and refuses, what a run catches and passes on,
//! the `Error` it builds and the frames it lists, and that a caught fault's kept cells are released.

use crate::parse::ExpressionPart;
use crate::program::{CellSubstrate, Outcome, Program};
use crate::values::BuildBudget;

use super::caching::{PRELUDE, caching};
use super::statics::{loaded, slot, top};
use super::tail::peak_of;
use super::{Koan, output, run};

/// `BOOM`, which reads a field off whatever it is handed: `NoField` at run for a record without
/// it, and for anything else.
const BOOM: &str = "EXPR #(BOOM p :Any) -> Any = #(p.field)
";

/// A value of declared type `Any` that holds the string `"s"`, so the load refuses nothing a run
/// over it faults on.
const ANY_STR: &str = "LET r = ({v = \"s\"} :! :{v :Any})\n";

/// Whether the load settled the ascription `LET name = <ascription>` in the top level.
fn settled(program: &Program<'_>, name: &str) -> bool {
    let shape = program.shape();
    let Some(ExpressionPart::Expression(node)) = shape.rhs(slot(program, shape, name)) else {
        panic!("`{name}` is bound to a node");
    };
    program.facts().settled(shape, shape.site_of_node(node))
}

#[test]
fn a_try_is_its_body_and_arms_spanned() {
    assert_eq!(
        top("LET t = (TRY (1) WITH #{_: (\"x\")})", "t"),
        ":(Number | Str)"
    );
    assert_eq!(
        top("LET t = (TRY (1) WITH #{NoOverload: (it.message)})", "t"),
        ":(Number | Str)",
        "a label's `it` is its variant's payload"
    );
    loaded(
        "LET t = ((TRY (1) WITH #{_: (\"x\")}) :! :(Number | Str))",
        |_| (),
    );
    let refused = run("LET t = ((TRY (1) WITH #{_: (\"x\")}) :! Bool)");
    assert!(
        refused.starts_with("load: ") && refused.contains("can never satisfy"),
        "{refused}"
    );
}

#[test]
fn a_catch_is_a_result_of_its_body_and_error() {
    loaded(
        "LET c = ((CATCH (1)) :! :(Result {Ok = Number, Error = Error}))",
        |program| assert!(settled(program, "c")),
    );
}

#[test]
fn a_try_label_naming_no_kind_refuses_the_load() {
    let refused = run("TRY (1) WITH #{Bogus: (2)}");
    assert!(
        refused.starts_with("load: <test>:1:1: Bogus names no variant of "),
        "{refused}"
    );
}

#[test]
fn a_try_need_not_name_every_kind() {
    assert_eq!(
        top("LET t = (TRY (1) WITH #{NoOverload: (2)})", "t"),
        "Number"
    );
}

#[test]
fn a_try_whose_body_succeeds_yields_its_value() {
    assert_eq!(
        run("PRINT (TRY ((PRINT \"body\") (1)) WITH #{_: (2)})"),
        "body\n1"
    );
}

#[test]
fn a_try_catches_a_missing_field_with_its_record() {
    let caught = |arm: &str| {
        run(&format!(
            "{BOOM}PRINT (TRY (BOOM {{a = 1}}) WITH #{{NoField: ({arm})}})"
        ))
    };
    assert_eq!(caught("it.field"), "field");
    assert_eq!(caught("it.message"), ":{a :Number} has no field field");
    assert_eq!(caught("it.of"), "{a = 1}");
}

#[test]
fn a_caught_fault_lists_its_frames_innermost_first() {
    let source = format!(
        "{BOOM}EXPR #(INNER) -> Any = #((LET i = (BOOM {{a = 1}})) (i))\n\
         EXPR #(OUTER) -> Any = #((LET o = (INNER)) (o))\n\
         PRINT (TRY (OUTER) WITH #{{NoField: (it.frames)}})"
    );
    assert_eq!(
        run(&source),
        "[{at = <test>:2:35, call = BOOM {a = 1}, callee = :(FN :{p :Any} -> Any), count = 1}, \
         {at = <test>:3:35, call = INNER, callee = :(FN :{} -> Any), count = 1}, \
         {at = <test>:4:12, call = OUTER, callee = :(FN :{} -> Any), count = 1}]"
    );
    // A tail loop's hops merge into one line, as its trace prints them.
    let down = format!(
        "{ANY_STR}EXPR #(DOWN n :Number) -> Number = \
         #(MATCH (n == 0) WITH #{{true: (1 + r.v), false: (DOWN (n - 1))}})\n\
         PRINT (TRY (DOWN 1000) WITH #{{NoOverload: (it.frames)}})"
    );
    assert_eq!(
        run(&down),
        "[{at = <test>:2:84, call = DOWN (n - 1), callee = :(FN :{n :Number} -> Number), \
         count = 1000}, {at = <test>:3:12, call = DOWN 1000, callee = :(FN :{n :Number} -> Number), \
         count = 1}]"
    );
    // So do a recursion's frames, each kept in a cell of its own.
    let recursion = format!(
        "{ANY_STR}EXPR #(REC n :Number) -> Number = \
         #(MATCH (n == 0) WITH #{{true: (1 + r.v), false: ((LET x = (REC (n - 1))) (x))}})\n\
         PRINT (TRY (REC 3) WITH #{{_: (it.frames)}})"
    );
    assert_eq!(
        run(&recursion),
        "[{at = <test>:2:93, call = REC (n - 1), callee = :(FN :{n :Number} -> Number), \
         count = 3}, {at = <test>:3:12, call = REC 3, callee = :(FN :{n :Number} -> Number), \
         count = 1}]"
    );
}

#[test]
fn a_kind_no_arm_takes_passes_on() {
    let inner = format!("{BOOM}EXPR #(INNER) -> Any = #((LET i = (BOOM {{a = 1}})) (i))\n");
    let trace = |at: &str| {
        format!(
            "error: :{{a :Number}} has no field field\n  \
             in :(FN :{{p :Any}} -> Any) (BOOM {{a = 1}}) at <test>:2:35\n  \
             in :(FN :{{}} -> Any) (INNER) at <test>:3:{at}"
        )
    };
    assert_eq!(run(&format!("{inner}PRINT (INNER)")), trace("7"));
    assert_eq!(
        run(&format!(
            "{inner}PRINT (TRY (INNER) WITH #{{Incomparable: (1)}})"
        )),
        trace("12")
    );
}

#[test]
fn a_label_outranks_default() {
    assert_eq!(
        run(&format!(
            "{BOOM}PRINT (TRY (BOOM 1) WITH #{{NoField: (\"label\"), _: (\"default\")}})"
        )),
        "label"
    );
}

#[test]
fn default_catches_every_kind() {
    for body in ["BOOM 1", "1 + r.v", "MATCH r.v WITH #{Number: (it)}"] {
        assert_eq!(
            run(&format!(
                "{BOOM}{ANY_STR}PRINT (TRY ({body}) WITH #{{_: (\"caught\")}})"
            )),
            "caught",
            "{body}"
        );
    }
}

#[test]
fn nested_try_passes_an_uncaught_kind_outward() {
    assert_eq!(
        run(&format!(
            "{BOOM}PRINT (TRY (TRY (BOOM 1) WITH #{{Incomparable: (1)}}) \
             WITH #{{NoField: (\"outer\")}})"
        )),
        "outer"
    );
}

#[test]
fn a_fault_in_an_arm_is_not_caught_by_its_try() {
    assert_eq!(
        run(&format!(
            "{BOOM}PRINT (TRY (BOOM 1) WITH #{{NoField: (BOOM 2)}})"
        )),
        "error: Number has no field field\n  in :(FN :{p :Any} -> Any) (BOOM 2) at <test>:2:37"
    );
}

#[test]
fn a_try_body_is_a_block() {
    assert_eq!(
        run(&format!(
            "{BOOM}PRINT (TRY ((LET y = 1) (BOOM y)) WITH #{{_: ((LET z = 2) (z))}})"
        )),
        "2"
    );
    for name in ["y", "z"] {
        let refused = run(&format!(
            "{BOOM}TRY ((LET y = 1) (BOOM y)) WITH #{{_: ((LET z = 2) (z))}}\nPRINT {name}"
        ));
        assert!(refused.starts_with("load: "), "`{name}` is gone: {refused}");
    }
}

#[test]
fn a_try_body_is_not_a_tail() {
    // Were `INNER` called in tail position, it would replace `OUTER`'s frame, and its fault would
    // leave past the `TRY`.
    assert_eq!(
        run(&format!(
            "{BOOM}EXPR #(OUTER) -> Any = #(TRY (INNER) WITH #{{NoField: (\"caught\")}})\n\
             EXPR #(INNER) -> Any = #(BOOM {{a = 1}})\n\
             PRINT (OUTER)"
        )),
        "caught"
    );
}

#[test]
fn a_tail_try_arm_hops_in_constant_cells() {
    // Each round's body faults, and its arm, the frame's last statement, hops to the next frame.
    let down = |depth: u32| {
        format!(
            "{BOOM}EXPR #(DOWN n :Number) -> Number = \
             #(TRY (MATCH (n == 0) WITH #{{true: (0), false: (BOOM n)}}) \
             WITH #{{NoField: (DOWN (n - 1))}})\n\
             PRINT (DOWN {depth})"
        )
    };
    assert_eq!(run(&down(4)), "0");
    let (deep, shallow) = if cfg!(miri) { (3, 2) } else { (256, 8) };
    assert_eq!(peak_of(&down(deep)), peak_of(&down(shallow)));
}

#[test]
fn catch_yields_ok_or_error() {
    assert_eq!(
        run("PRINT (CATCH (1))"),
        ":(Ok {Error = Never, Ok = Number})(1)"
    );
    let taken = |body: &str| {
        run(&format!(
            "{BOOM}LET c = (CATCH ({body}))\n\
             PRINT (MATCH c UNDER Result WITH #{{Ok: (\"ok\"), \
             Error: (MATCH it UNDER Error WITH #{{NoField: (it.field), _: (\"other\")}})}})"
        ))
    };
    assert_eq!(taken("1"), "ok");
    assert_eq!(taken("BOOM 1"), "field");
}

#[test]
fn a_caught_value_outlives_its_released_frames() {
    // Two records holding a module, built in a frame and compared there: the fault holds both
    // where they lie, in the frame's kept region, which is released once the `Error` is built.
    let same = "MODULE m = (LET x = 1)\n\
                EXPR #(SAME) -> Bool = \
                #((LET a = ({k = m} :! :{k :Any})) (LET b = ({k = m} :! :{k :Any})) (a == b))\n";
    assert_eq!(
        run(&format!(
            "{same}LET kept = (TRY (SAME) WITH #{{Incomparable: (it.left)}})\nPRINT kept.k.x"
        )),
        "1"
    );
}

#[test]
fn a_caught_fault_releases_its_frames() {
    let source = format!(
        "{BOOM}EXPR #(INNER) -> Any = #((LET i = (BOOM {{a = 1}})) (i))\n\
         EXPR #(OUTER) -> Any = #((LET o = (INNER)) (o))\n\
         PRINT (TRY (OUTER) WITH #{{_: (\"caught\")}})\n\
         PRINT (CATCH (OUTER))"
    );
    let mut substrate =
        CellSubstrate::load::<Koan>(&source, "<test>", 8, BuildBudget::default(), output())
            .expect("the program loads");
    substrate.with(|running| {
        assert!(matches!(running.run(), Ok(Outcome::Completed)));
        assert!(running.emptied(), "every kept cell was released");
    });
}

#[test]
fn a_try_in_a_caching_body_catches() {
    // `GUARDED` views the module it is handed, so its body caches; what it catches is built on the
    // body's chain, and leaves it as any value does.
    let source = format!(
        "{BOOM}EXPR #(GUARDED m :Holder) -> Str = \
         #((LET v = (m :| Holder)) \
         (LET c = (CATCH (BOOM {{a = 1}}))) \
         (TRY (BOOM {{a = 1}}) WITH #{{NoField: (MATCH c UNDER Result WITH #{{Ok: (\"ok\"), \
         Error: (MATCH it UNDER Error WITH #{{NoField: (it.message), _: (\"other\")}})}})}}))\n\
         MODULE h = (LET held = 1)\n\
         PRINT (GUARDED h)"
    );
    assert_eq!(caching(&source), 1, "`GUARDED` caches");
    assert_eq!(
        run(&format!("{PRELUDE}{source}")),
        ":{a :Number} has no field field"
    );
}
