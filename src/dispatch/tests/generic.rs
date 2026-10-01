//! Static types of generic code: lexical variables in a body's static types, intervals, verdicts,
//! ranking at load, an `EVAL`'s declared type, and returns read through a group's intervals.

use crate::program::Program;
use crate::scope::{BodyShape, ShapeKind, Site, Static};
use crate::type_lattice::{Interval, KType, quantifier_bounds, shape_slots};

use super::run;
use super::statics::{body, let_narrowing, lexical, loaded, rendered, slot, top, upper};

/// The body the registration of `shape` whose key leads with the keyword `lead` births.
fn expressed<'graph>(
    program: &Program<'graph>,
    shape: &'graph BodyShape<'graph>,
    lead: &str,
) -> &'graph BodyShape<'graph> {
    let registration = (shape.registrations().iter())
        .find(|registration| {
            registration
                .elements
                .iter()
                .find_map(|element| element.keyword())
                .is_some_and(|keyword| {
                    program.symbols().display(keyword.symbol()).to_string() == lead
                })
        })
        .unwrap_or_else(|| panic!("`{lead}` is registered here"));
    shape
        .births(registration.slot)
        .expect("the registration births a body")
}

/// The narrowing of the keyworded use written as the statement `index` of `shape`.
fn statement(shape: &BodyShape<'_>, index: usize) -> String {
    rendered(shape.narrowing(Site::of_node(&shape.body()[index])))
}

/// The narrowing of the keyworded use written as the last statement of the body the `index`th
/// registration of `source`'s top level leading with `lead` births.
fn last_in(source: &str, lead: &str) -> String {
    loaded(source, |program| {
        let body = expressed(program, program.shape(), lead);
        statement(body, body.body().len() - 1)
    })
}

/// The one quote's code nested in `shape`.
fn code<'graph>(shape: &'graph BodyShape<'graph>) -> &'graph BodyShape<'graph> {
    let mut codes = (shape.nested_shapes().iter())
        .filter(|(_, nested)| nested.kind() == ShapeKind::Code)
        .map(|(_, nested)| *nested);
    codes.next().expect("the shape holds a quote")
}

/// Whether the static type of the binder `name` in `shape` is exact.
fn exact_in(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> bool {
    shape
        .binder_type(slot(program, shape, name))
        .expect("the load typed the shape")
        .is_exact()
}

/// Whether the static type of the top-level binder `name` of `source` is exact.
fn exact(source: &str, name: &str) -> bool {
    loaded(source, |program| exact_in(program, program.shape(), name))
}

/// A `FOR ALL` callable whose body declares an unquantified overload over its own name.
const OUTER: &str = "EXPR FOR ALL #[Elt] #(OUTER x :Elt) -> Elt = #(\n  \
                     EXPR #(INNER y :Elt) -> Elt = #(y)\n  \
                     INNER x\n\
                     )\n";

#[test]
fn a_nested_callable_substitutes_its_enclosing_names_by_level() {
    assert_eq!(
        run("LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(\n  \
             LET g = (FN FOR ALL #[Tee] :{t :Tee} -> Elt = #(x))\n  \
             g {t = \"s\"}\n\
             ))\n\
             PRINT (f {x = 1})"),
        "1"
    );
}

#[test]
fn typing_a_quote_s_code_a_second_time_interns_no_type() {
    let quote = "#(LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(\n  \
                 LET g = (FN :{} -> Elt = #(x))\n  \
                 PRINT y\n  \
                 x\n\
                 )))";
    let count = |source: &str| loaded(source, |program| program.types().node_count());
    assert_eq!(
        count(&format!("LET q = {quote}")),
        count(&format!("LET q = {quote}\nLET r = {quote}"))
    );
}

#[test]
fn an_overload_over_an_enclosing_name_is_rigid_and_selected_without_a_solve() {
    loaded(OUTER, |program| {
        let outer = expressed(program, program.shape(), "OUTER");
        let registered = match outer.registered_type(outer.registrations()[0].slot) {
            Static::Rigid { value, .. } => value.shape,
            _ => panic!("`INNER` is rigid over `Elt`"),
        };
        let types = program.types();
        assert!(quantifier_bounds(types, registered).is_empty());
        assert_eq!(
            shape_slots(registered, types).collect::<Vec<_>>(),
            [lexical(program, 0, "Elt", KType::ANY)]
        );
        assert_eq!(statement(outer, outer.body().len() - 1), "selected");
    });
}

#[test]
fn a_nested_quantified_callable_reads_its_own_names_and_checks_its_return() {
    loaded(
        "LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(\n  \
         LET h = (FN FOR ALL #[Tee] :{t :Tee, u :Elt} -> Tee = #(t))\n  \
         x\n\
         ))",
        |program| {
            let f = body(program, program.shape(), "f");
            let h = body(program, f, "h");
            assert_eq!(
                upper(program, h, "t"),
                lexical(program, 1, "Tee", KType::ANY)
            );
        },
    );
    assert_eq!(
        run(
            "LET f = (FN FOR ALL #{Elt: Number} :{x :Elt} -> Elt = #(\n  \
             LET h = (FN FOR ALL #[Tee] :{t :Tee} -> Elt = #(\"no\"))\n  \
             x\n\
             ))"
        ),
        "load: <test>:2:50: this body returns Str, which can never satisfy its declared return \
         Number"
    );
}

#[test]
fn a_callable_born_in_a_generic_body_agrees_with_its_load_time_type() {
    assert_eq!(run(&format!("{OUTER}PRINT (OUTER 1)")), "1");
    let direct = "EXPR FOR ALL #[Elt] #(OUTER x :Elt) -> Elt = #(\n  \
                  EXPR #(INNER y :Elt) -> Elt = #(y)\n  \
                  x\n\
                  )\n\
                  PRINT (OUTER 1)";
    assert_eq!(run(direct), "1");
}

#[test]
fn a_static_type_is_exact_where_the_load_knows_the_carried_type() {
    for (source, name) in [
        ("LET n = 1", "n"),
        ("LET xs = [1, 2]", "xs"),
        ("LET q = #(x)", "q"),
        ("LET f = (FN :{} -> Any = #(1))", "f"),
    ] {
        assert!(exact(source, name), "`{source}` is exact");
    }
    let parameters = "LET f = (FN :{x :(Number | Str)} -> Any = #(x))\n\
                      LET g = (FN :{x :Number} -> Any = #(x))";
    loaded(parameters, |program| {
        let f = body(program, program.shape(), "f");
        assert!(!exact_in(program, f, "x"), "a union is not exact");
        let g = body(program, program.shape(), "g");
        assert!(exact_in(program, g, "x"), "a scalar is");
    });
    assert!(
        exact(
            "EXPR #(NUMBERS) -> :(LIST OF Number) = #([1])\nLET l = (NUMBERS)",
            "l"
        ),
        "a call running a frame is exact at its return"
    );
}

#[test]
fn a_crossing_into_code_or_out_of_it_is_read_through_bounds() {
    loaded(
        "LET f = (FN FOR ALL #{Elt: Number} :{x :Elt} -> Elt = #(\n  \
         LET q = #(PRINT $x)\n  \
         x\n\
         ))",
        |program| {
            let f = body(program, program.shape(), "f");
            let statics = code(f).statics().expect("the load typed the code");
            assert_eq!(
                statics
                    .parts
                    .iter()
                    .map(|(_, typed)| *typed)
                    .collect::<Vec<_>>(),
                [Interval::point(KType::NUMBER)]
            );
        },
    );
    assert_eq!(
        run("LET q = #(FN :{t :Tee} -> Tee = #(t))\nLET f = (EVAL q -> Number)"),
        "load: <test>:2:9: this `EVAL`'s code returns :(FN :{t :Never} -> Any), which can never \
         satisfy its declared return Number"
    );
}

#[test]
fn eval_is_its_declared_type() {
    let named = "LET q = #(1 + 2)\nLET m = ((EVAL q -> Number) + 1)";
    loaded(named, |program| {
        assert_eq!(let_narrowing(program, program.shape(), "m"), "selected");
        assert_eq!(
            code(program.shape())
                .statement_type(0)
                .map(|typed| typed.upper),
            Some(KType::ANY),
            "the code's own cell keeps the hole a `USING` may fill"
        );
    });
    assert_eq!(run(&format!("{named}\nPRINT m")), "4");
    loaded(
        "LET q = #(1 + 2)\nLET m = ((EVAL q -> Any) + 1)",
        |program| {
            assert_eq!(
                let_narrowing(program, program.shape(), "m"),
                "full",
                "the traced code's `Number` no longer types the `EVAL`"
            );
        },
    );
    assert!(exact(
        "LET l = (EVAL #([1]) -> :(LIST OF (Number | Str)))",
        "l"
    ));
    assert!(
        !exact("LET u = (EVAL #(1) -> (Number | Str))", "u"),
        "a union is not exact"
    );
    assert_eq!(top("LET q = #(1)\nLET f = (EVAL q -> Any)", "f"), "Any");
}

#[test]
fn a_quantified_candidate_is_selected_only_where_its_arguments_are_exact() {
    let pair = "EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str = #(\"pair\")\n";
    loaded(&format!("{pair}LET p = (PAIR 1 WITH 2)"), |program| {
        assert_eq!(let_narrowing(program, program.shape(), "p"), "selected");
    });
    let over = format!(
        "{pair}EXPR #(USE a :(Number | Str) AND b :(Number | Str)) -> Str = #(PAIR a WITH b)"
    );
    assert_eq!(last_in(&over, "USE"), "full");
}

#[test]
fn a_maybe_an_always_candidate_outranks_is_dropped() {
    let source = "EXPR #(SHOW x :(LIST OF Number)) -> Str = #(\"numbers\")\n\
                  EXPR FOR ALL #[Elt] #(SHOW x :(LIST OF Elt)) -> Str = #(\"any\")\n\
                  EXPR #(USE xs :(LIST OF Number)) -> Str = #(SHOW xs)";
    assert_eq!(last_in(source, "USE"), "selected");
    assert_eq!(run(&format!("{source}\nPRINT (USE [1])")), "numbers");
}

#[test]
fn a_use_with_no_maybe_candidate_is_ranked_at_load() {
    let show = "EXPR #(SHOW x :Number) -> Str = #(\"number\")\n\
                EXPR #(SHOW x :Any) -> Str = #(\"any\")\n\
                LET s = (SHOW 1)";
    loaded(show, |program| {
        assert_eq!(let_narrowing(program, program.shape(), "s"), "selected");
    });
    assert_eq!(run(&format!("{show}\nPRINT s")), "number");
    let equal = "OP #(==) OVER Any -> Bool = #(false)\nLET e = (1 == 1)";
    loaded(equal, |program| {
        assert_eq!(let_narrowing(program, program.shape(), "e"), "selected");
    });
    assert_eq!(
        run(&format!("{equal}\nPRINT e")),
        "true",
        "a builtin wins a tie"
    );
}

#[test]
fn a_call_admits_only_its_maybe_candidates() {
    let pairs = "EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str = #(\"joint\")\n\
                 EXPR FOR ALL #[First Second] #(PAIR x :First WITH y :Second) -> Str = \
                 #(\"apart\")\n";
    let used = format!(
        "{pairs}EXPR #(USE a :(Number | Str) AND b :(Number | Str)) -> Str = #(PAIR a WITH b)"
    );
    // In list order: `apart`'s two variables are each alone in their class, `joint`'s second slot
    // names the variable its first solved.
    assert_eq!(last_in(&used, "USE"), "kept always maybe");
    assert_eq!(run(&format!("{used}\nPRINT (USE 1 AND 2)")), "joint");
    assert_eq!(run(&format!("{used}\nPRINT (USE 1 AND \"a\")")), "apart");
    let exact = format!("{pairs}LET p = (PAIR 1 WITH 2)");
    loaded(&exact, |program| {
        assert_eq!(let_narrowing(program, program.shape(), "p"), "selected");
    });
    assert_eq!(run(&format!("{exact}\nPRINT p")), "joint");
}

#[test]
fn an_overload_over_a_bounded_enclosing_name_is_not_selected_for_a_literal() {
    let source = "EXPR FOR ALL #{Elt: Number} #(OUTER x :Elt) -> Elt = #(\n  \
                  EXPR #(INNER y :Elt) -> Elt = #(y)\n  \
                  LET z = (INNER 1)\n  \
                  INNER x\n\
                  )";
    loaded(source, |program| {
        let outer = expressed(program, program.shape(), "OUTER");
        assert_eq!(let_narrowing(program, outer, "z"), "full");
    });
}

#[test]
fn several_always_candidates_one_rigid_are_ranked_at_the_call() {
    let source = "EXPR #(INNER y :Number) -> Str = #(\"outer\")\n\
                  EXPR FOR ALL #{Elt: Number} #(OUTER x :Elt) -> (Elt | Str) = #(\n  \
                  EXPR #(INNER y :Elt) -> Str = #(\"inner\")\n  \
                  INNER x\n\
                  )";
    assert_eq!(last_in(source, "OUTER"), "kept always always");
    assert_eq!(
        run(&format!("{source}\nPRINT (OUTER 1)")),
        "error: ambiguous call of INNER _: 2 overloads admit (Number) and none ranks first"
    );
}

#[test]
fn a_generic_return_is_read_through_its_group_s_intervals() {
    assert_eq!(
        top(
            "LET id = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))\nLET n = (id {x = 1})",
            "n"
        ),
        "Number"
    );
    let only = "EXPR FOR ALL #[Elt] #(ONLY x :Elt) -> Elt = #(x)\n";
    let one = format!("{only}LET o = (ONLY 1)");
    assert_eq!(top(&one, "o"), "Number");
    loaded(&one, |program| {
        assert_eq!(let_narrowing(program, program.shape(), "o"), "selected");
    });
    loaded(&format!("{only}LET t = ((ONLY 1) + 1)"), |program| {
        assert_eq!(let_narrowing(program, program.shape(), "t"), "selected");
    });
    assert_eq!(
        run(&format!("{only}PRINT ((ONLY 1) + \"a\")")),
        "load: <test>:2:7: no overload of `_ + _` admits (Number, Str)"
    );
    let handler = "EXPR FOR ALL #[Elt] #(HANDLER f :(FN :{x :Elt} -> Null)) -> \
                   :(FN :{x :Elt} -> Null) = #(f)\n\
                   LET g = (FN :{x :Number} -> Null = #(null))\n\
                   LET h = (HANDLER g)";
    assert_eq!(top(handler, "h"), ":(FN :{x :Number} -> Null)");
}

#[test]
fn an_exact_argument_over_a_lexical_variable_is_no_solve_of_the_call_s_own() {
    let source = "EXPR FOR ALL #[Elt] #(TWO f :(FN :{} -> :(LIST OF Elt)) THEN y :Elt) -> Str = \
                  #(\"generic\")\n\
                  EXPR #(TWO f :Any THEN y :Number) -> Str = #(\"any\")\n\
                  EXPR FOR ALL #{Ll: :(LIST OF Number)} #(OUTER xs :Ll AND ys :Ll) -> Str = #(\n  \
                  LET g = (FN :{} -> Ll = #(xs))\n  \
                  TWO g THEN 1\n\
                  )\n\
                  PRINT (OUTER [1] AND [2])\n\
                  PRINT (OUTER [] AND [])";
    assert_eq!(run(source), "generic\nany");
    let by_name = "LET h = (FN FOR ALL #[Elt] :{f :(FN :{} -> :(LIST OF Elt))} -> \
                   :(FN :{x :Elt} -> Null) = #(FN :{x :Elt} -> Null = #(null)))\n\
                   EXPR FOR ALL #{Ll: :(LIST OF Number)} #(OUTER xs :Ll AND ys :Ll) -> Any = #(\n  \
                   LET g = (FN :{} -> Ll = #(xs))\n  \
                   LET k = (h {f = g})\n  \
                   k\n\
                   )\n\
                   PRINT (OUTER [1] AND [2])\n\
                   PRINT (OUTER [] AND [])";
    assert_eq!(
        run(by_name),
        ":(FN :{x :Number} -> Null)\n:(FN :{x :Never} -> Null)",
        "a call by name solves its group from the carried types"
    );
}

#[test]
fn two_arguments_at_one_parameter_bind_the_join_of_their_types() {
    let flat = "EXPR FOR ALL #[Elt] #(FLAT rows :(LIST OF (LIST OF Elt))) -> :(LIST OF Elt) = #(\n  \
                PRINT Elt\n  \
                []\n\
                )\n";
    assert_eq!(
        run(&format!("{flat}PRINT (FLAT [[\"a\"], [1]])")),
        ":(Str | Number)\n[]"
    );
    assert_eq!(
        top(&format!("{flat}LET r = (FLAT [[\"a\"], [1]])"), "r"),
        ":(LIST OF :(Str | Number))"
    );
}

#[test]
fn two_arguments_reaching_one_parameter_from_above_bind_the_meet_of_their_types() {
    let first = "EXPR FOR ALL #[Elt] #(FIRST fs :(LIST OF (FN :{x :Elt} -> Null))) -> \
                 :(FN :{x :Elt} -> Null) = #(FN :{x :Elt} -> Null = #(null))\n";
    let apart = format!(
        "{first}LET f = (FN :{{x :Number}} -> Null = #(null))\n\
         LET g = (FN :{{x :Str}} -> Null = #(null))\n"
    );
    assert_eq!(
        top(&format!("{apart}LET h = (FIRST [f, g])"), "h"),
        ":(FN :{x :Never} -> Null)"
    );
    assert_eq!(
        run(&format!("{apart}PRINT (FIRST [f, g])")),
        ":(FN :{x :Never} -> Null)"
    );
    let overlapping = format!(
        "{first}LET f = (FN :{{x :(Number | Str)}} -> Null = #(null))\n\
         LET g = (FN :{{x :(Number | Bool)}} -> Null = #(null))\n"
    );
    assert_eq!(
        top(&format!("{overlapping}LET h = (FIRST [f, g])"), "h"),
        ":(FN :{x :Number} -> Null)"
    );
    assert_eq!(
        run(&format!("{overlapping}PRINT (FIRST [f, g])")),
        ":(FN :{x :Number} -> Null)"
    );
}
