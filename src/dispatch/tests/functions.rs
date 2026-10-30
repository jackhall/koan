//! A function retyped at its slot: a function-typed parameter, an annotation, a barrier's return.
//! A call through a function retyped to a type sees exactly that type, so the load reads its
//! return exactly and the run retypes the value to it.

use super::ascription::ends;
use super::run;
use super::statics::{body, loaded};

/// `wide`, which returns a field a narrower function type hides.
const WIDE: &str = "LET wide = (FN :{} -> :{x :Number, y :Str} = #({x = 1, y = \"hidden\"}))\n";

#[test]
fn a_hidden_field_of_a_retyped_functions_return_refuses_the_load() {
    let source = format!(
        "{WIDE}LET read = (FN :{{g :(FN :{{}} -> :{{x :Number}})}} -> Any = #(ATTR (g {{}}) y))\n\
         PRINT (read {{g = wide}})"
    );
    assert!(
        run(&source).contains(":{x :Number} has no field y"),
        "{}",
        run(&source)
    );
}

#[test]
fn a_call_through_a_retyped_function_is_exactly_its_declared_return() {
    let source = format!(
        "{WIDE}LET read = (FN :{{g :(FN :{{}} -> :{{x :Number}})}} -> Any = #(\n  \
         LET got = (g {{}})\n  \
         got\n\
         ))\n\
         PRINT (read {{g = wide}})"
    );
    loaded(&source, |program| {
        let read = body(program, program.shape(), "read");
        let exact = ":{x :Number}".to_string();
        assert_eq!(ends(program, read, "got"), (exact.clone(), exact));
    });
    assert_eq!(run(&source), "{x = 1}");
}

#[test]
fn an_overload_over_the_hidden_fields_is_never_selected() {
    let source = format!(
        "{WIDE}EXPR #(PICK r :{{x :Number, y :Str}}) -> Str = #(\"wide\")\n\
         EXPR #(PICK r :{{x :Number}}) -> Str = #(\"narrow\")\n\
         LET pick = (FN :{{g :(FN :{{}} -> :{{x :Number}})}} -> Str = #(PICK (g {{}})))\n\
         PRINT (pick {{g = wide}})"
    );
    assert_eq!(run(&source), "narrow");
}

#[test]
fn a_function_passed_through_a_function_typed_slot_equals_itself() {
    let source = format!(
        "{WIDE}LET same = (FN :{{g :(FN :{{}} -> :{{x :Number}})}} -> Bool = #(g == wide))\n\
         PRINT (same {{g = wide}})"
    );
    assert_eq!(run(&source), "true");
}

#[test]
fn a_retype_of_a_retyped_function_is_one_header_over_the_same_node() {
    let source = format!(
        "{WIDE}LET outer = (FN :{{g :(FN :{{}} -> Any)}} -> Any = #(g))\n\
         LET inner = (FN :{{g :(FN :{{}} -> :{{x :Number}})}} -> Any = #(outer {{g = g}}))\n\
         LET passed = (inner {{g = wide}})\n\
         PRINT passed\n\
         PRINT (passed == wide)\n\
         PRINT (passed {{}})"
    );
    assert_eq!(run(&source), ":(FN :{} -> Any)\ntrue\n{x = 1, y = hidden}");
}

#[test]
fn a_function_declaring_fewer_parameters_than_its_slot_shows_runs() {
    let source = "LET narrow = (FN :{a :Number} -> Number = #(a))\n\
                  LET call = (FN :{g :(FN :{a :Number, b :Str} -> Number)} -> Number = \
                  #(g {a = 1, b = \"s\"}))\n\
                  PRINT (call {g = narrow})";
    assert_eq!(run(source), "1");
}

#[test]
fn an_annotated_function_in_a_cycle_is_born_at_its_annotation() {
    let source = "LET f :(FN :{} -> Any) = (FN :{} -> :(FN :{} -> Any) = #(f))\n\
                  PRINT (f {})";
    assert_eq!(run(source), ":(FN :{} -> Any)");
}

#[test]
fn a_tail_chain_through_a_retyped_function_returns_the_outermost_return() {
    let source = "LET inner = (FN :{} -> :(LIST OF Number) = #([1]))\n\
                  EXPR #(WHICH x :(LIST OF Number)) -> Str = #(\"numbers\")\n\
                  EXPR #(WHICH x :Any) -> Str = #(\"any\")\n\
                  LET outer = (FN :{g :(FN :{} -> :(LIST OF Any))} -> :(LIST OF Any) = #(g {}))\n\
                  PRINT (WHICH (outer {g = inner}))";
    assert_eq!(run(source), "any");
}

#[test]
fn functions_of_one_code_and_type_compare_by_their_captures() {
    let source = "LET make = (FN :{c :Number} -> Any = #(FN :{x :Number} -> Number = #(c)))\n\
                  LET other = (FN :{c :Number} -> Any = #(FN :{x :Number} -> Number = #(c)))\n\
                  PRINT ((make {c = 1}) == (make {c = 1}))\n\
                  PRINT ((make {c = 1}) == (other {c = 1}))\n\
                  PRINT ((make {c = 1}) == (make {c = 2}))";
    assert_eq!(run(source), "true\ntrue\nfalse");
}
