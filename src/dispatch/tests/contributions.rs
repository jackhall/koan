//! Calls solved from their arguments' static types: each argument at a slot its class solves from
//! contributes the upper end of its static type, or its carried type where that is `Any`, read
//! where the call runs through type captures where it names a lexical variable.

use crate::scope::ShapeKind;

use super::generic::{expressed, last_in};
use super::run;
use super::statics::loaded;

/// `PAIR` returns the type it solved `Elt` to; `USE` calls it over two parameters declared
/// `Number | Str`.
const PAIR: &str = "EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Type = #(Elt)\n\
                    EXPR #(USE a :(Number | Str) AND b :(Number | Str)) -> Type = #(PAIR a WITH b)\n";

#[test]
fn a_declared_argument_solves_its_slot_from_its_static_type() {
    assert_eq!(
        run(&format!(
            "{PAIR}PRINT (USE 1 AND \"x\")\nPRINT (USE 1 AND 2)"
        )),
        ":(Number | Str)\n:(Number | Str)"
    );
    assert_eq!(last_in(PAIR, "USE"), "selected");
}

#[test]
fn an_argument_the_load_knows_nothing_of_contributes_its_carried_type() {
    let with = |yields: &str, call: &str| {
        format!(
            "EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Type = #(Elt)\n\
             EXPR #(USE a :(Number | Str) AND q :Code) -> Type = #(\n  \
             LET e = (EVAL q -> Any)\n  \
             {call}\n\
             )\n\
             PRINT (USE {yields})"
        )
    };
    assert_eq!(
        run(&with("1 AND #(\"s\")", "PAIR a WITH e")),
        ":(Number | Str)"
    );
    assert_eq!(
        run(&with("1 AND #(true)", "PAIR a WITH e")),
        "error: no overload of PAIR _ WITH _ admits (Number, Bool)"
    );
    assert_eq!(
        run(&with("\"s\" AND #(\"s\")", "PAIR e WITH a")),
        "Str",
        "a slot a later class admits reads the carried type"
    );
}

#[test]
fn a_static_type_that_cannot_fit_its_solving_slot_refuses_the_load() {
    let first = "EXPR FOR ALL #[Elt] #(FIRST xs :(LIST OF Elt)) -> Elt = #(1)\n\
                 EXPR #(USE m :((LIST OF Number) | Null)) -> Any = #(FIRST m)";
    let refused = run(first);
    assert!(
        refused.starts_with("load:") && refused.contains("no overload of `FIRST _`"),
        "{refused}"
    );
    let only = "EXPR FOR ALL #{Elt: Number} #(ONLY x :Elt) -> Elt = #(x)\n\
                EXPR #(USE a :(Number | Str)) -> Any = #(ONLY a)";
    assert!(run(only).starts_with("load:"), "{}", run(only));
}

/// `BOTH` calls `PAIR` over its own parameters, typed by its `Outer`; `WIDE` calls `BOTH` over two
/// parameters declared `Number | Str`.
fn both(body: &str) -> String {
    format!(
        "EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Type = #(Elt)\n\
         EXPR FOR ALL #[Outer] #(BOTH a :Outer AND b :Outer) -> Type = #(\n{body}\n)\n\
         EXPR #(WIDE p :(Number | Str) AND q :(Number | Str)) -> Type = #(BOTH p AND q)\n\
         PRINT (WIDE 1 AND \"x\")"
    )
}

#[test]
fn a_contribution_over_a_lexical_variable_is_read_where_the_call_runs() {
    assert_eq!(run(&both("  PAIR a WITH b")), ":(Number | Str)");
}

#[test]
fn a_nested_callable_reads_a_contributed_variable_through_a_type_capture() {
    assert_eq!(
        run(&both(
            "  LET go = (FN :{} -> Type = #(PAIR a WITH b))\n  go {}"
        )),
        ":(Number | Str)"
    );
    assert_eq!(
        run(&both(
            "  LET go = (FN :{} -> Type = #((FN :{} -> Type = #(PAIR a WITH b)) {}))\n  go {}"
        )),
        ":(Number | Str)",
        "a callable between the call and the variable's home passes the capture on"
    );
}

#[test]
fn a_block_between_the_call_and_the_variables_home_is_a_hop() {
    let source = "EXPR FOR ALL #[Elt] #(ONLY v :Elt) -> Elt = #(v)\n\
                  EXPR FOR ALL #{Outer: Number} #(MID x :Outer) -> Bool = #(0 < (ONLY x) < 3)\n\
                  PRINT (MID 1)";
    loaded(source, |program| {
        let mid = expressed(program, program.shape(), "MID");
        assert!(
            mid.nested_shapes()
                .iter()
                .any(|(_, nested)| nested.kind() == ShapeKind::Block),
            "the run's shared operand is hoisted into a block"
        );
    });
    assert_eq!(run(source), "true");
}

#[test]
fn a_closure_captures_a_type_only_where_a_call_in_it_contributes_it() {
    let make = |inner: &str, ret: &str| {
        format!(
            "EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Type = #(Elt)\n\
             EXPR FOR ALL #[Outer] #(MAKE a :Outer AND b :Outer) -> :(FN :{{}} -> {ret}) = \
             #(FN :{{}} -> {ret} = #({inner}))\n\
             EXPR #(WIDE p :(Number | Str) AND q :(Number | Str)) -> Bool = \
             #((MAKE p AND q) == (MAKE 1 AND 1))\n\
             PRINT (WIDE 1 AND 1)"
        )
    };
    assert_eq!(run(&make("PAIR a WITH b", "Type")), "false");
    assert_eq!(run(&make("a", "Any")), "true");
}
