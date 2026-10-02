//! Ascription: `:!` over a value, a declared parameter as one, and a tail chain returning at the
//! outermost contract — what each retypes a value to, what the run checks, and what the load
//! types, settles and refuses; and what the load types exactly because of the retype: an exact
//! argument a slot does not admit, a call at its callee's return, a nominal type.

use crate::parse::ExpressionPart;
use crate::program::Program;
use crate::scope::{BodyShape, Site};
use crate::type_lattice::display_name;

use super::run;
use super::statics::{body, interval, let_narrowing, loaded, module_body, slot};

/// Both ends of the static type of the binder `name` in `shape`, rendered.
pub(super) fn ends(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> (String, String) {
    let typed = interval(program, shape, name);
    let render = |handle| display_name(handle, program.types(), program.symbols()).to_string();
    (render(typed.lower), render(typed.upper))
}

/// Whether the load settled the ascription `LET name = <ascription>` in the top level.
fn settled(program: &Program<'_>, name: &str) -> bool {
    let shape = program.shape();
    let Some(ExpressionPart::Expression(node)) = shape.rhs(slot(program, shape, name)) else {
        panic!("`{name}` is bound to a node");
    };
    shape.settled(Site::of_node(node.reference()))
}

/// The `WHICH` overloads each test dispatches the retyped value through.
pub(super) const WHICH: &str = "EXPR #(WHICH x :(LIST OF Number)) -> Str = #(\"numbers\")\n\
                     EXPR #(WHICH x :(LIST OF (Number | Str))) -> Str = #(\"number or str\")\n\
                     EXPR #(WHICH x :(LIST OF Any)) -> Str = #(\"any\")\n";

/// The aliases the item's criteria ascribe to.
const ALIASES: &str = "LET Loose = :((LIST OF Any) | Null)\n\
                       LET Wide = :((LIST OF (Number | Str | Bool)) | (LIST OF (Number | Str | Null)))\n";

#[test]
fn an_ascription_retypes_its_value() {
    let source = format!(
        "{ALIASES}{WHICH}\
         PRINT (WHICH ([1] :! Loose))\n\
         PRINT (WHICH ([1] :! Wide))\n\
         PRINT (WHICH ([1] :! Any))\n\
         PRINT (WHICH ([1] :! (LIST OF Any)))\n\
         PRINT (WHICH ([1] :! :(LIST OF Any)))"
    );
    assert_eq!(run(&source), "any\nnumber or str\nnumbers\nany\nany");
}

#[test]
fn an_ascription_a_value_misses_is_a_fault() {
    assert_eq!(
        run(
            "LET r = ({v = [1, \"a\"]} :! :{v :Any})\nPRINT (r.v :! (LIST OF Number))\nPRINT \"after\""
        ),
        "error: :(LIST OF :(Number | Str)) does not satisfy its ascription :(LIST OF Number)"
    );
}

#[test]
fn a_tagged_value_takes_the_application_it_lies_under() {
    let source = "NEWTYPE (Type AS Boxed)\n\
                  LET b = (Boxed 7)\n\
                  EXPR #(KIND x :(Boxed {Type = :(Number | Str)})) -> Str = #(\"number or str\")\n\
                  EXPR #(KIND x :Boxed) -> Str = #(\"boxed\")\n\
                  PRINT (KIND (b :! ((Boxed {Type = Bool}) | (Boxed {Type = :(Number | Str)}))))\n\
                  PRINT (KIND (b :! Boxed))";
    assert_eq!(run(source), "number or str\nboxed");
}

#[test]
fn a_module_ascription_arrives_with_modules() {
    assert_eq!(
        run("MODULE m = (LET x = 1)\nPRINT (m :! Any)"),
        "error: ascribing a module arrives with modules"
    );
}

#[test]
fn a_declared_parameter_is_an_ascription() {
    let source = format!(
        "{WHICH}\
         LET show = FN EXPR #(SHOW x :(LIST OF Any)) -> Str = #(WHICH x)\n\
         PRINT (SHOW [1])\n\
         PRINT (show {{x = [1]}})"
    );
    assert_eq!(run(&source), "any\nany");
    // `Elt` solves to `Str`, so `d` is retyped to `MAP Str -> Any`.
    let source = "EXPR #(KEYS d :(MAP Str -> Number)) -> Str = #(\"numbers\")\n\
                  EXPR #(KEYS d :(MAP Str -> Any)) -> Str = #(\"declared\")\n\
                  LET pick = FN EXPR FOR ALL #[Elt] #(PICK d :(MAP Elt -> Any) AT key :Elt) -> Str = \
                  #(KEYS d)\n\
                  PRINT (PICK {\"k\": 1} AT \"k\")\n\
                  PRINT (pick {d = {\"k\": 1}, key = \"k\"})";
    assert_eq!(
        run(source),
        "declared\ndeclared",
        "the call's solution is substituted"
    );
    // By name, `Elt` solves to the larger of `LIST OF Number` and `LIST OF (Number | Str)`.
    let source = format!(
        "{WHICH}\
         LET join = FN EXPR FOR ALL #[Elt] #(JOIN x :Elt TO y :Elt) -> Str = #(WHICH x)\n\
         PRINT (join {{x = [1], y = [1, \"a\"]}})"
    );
    assert_eq!(run(&source), "number or str");
}

#[test]
fn a_cyclic_value_is_retyped_as_a_plain_one() {
    let source = format!(
        "{WHICH}\
         EXPR #(WHICH x :(LIST OF :(FN :{{}} -> Any))) -> Str = #(\"own\")\n\
         EXPR #(SHOW x :(LIST OF Any)) -> Str = #(WHICH x)\n\
         LET xs = [f]\n\
         LET f = (FN :{{}} -> Any = #(xs))\n\
         PRINT (WHICH xs)\n\
         PRINT (SHOW xs)\n\
         PRINT (WHICH (xs :! (LIST OF Any)))\n\
         PRINT xs\n\
         PRINT (xs :! (LIST OF Any))"
    );
    assert_eq!(
        run(&source),
        "own\nany\nany\n[:(FN :{} -> Any)]\n[:(FN :{} -> Any)]"
    );
}

#[test]
fn a_tail_chain_returns_at_the_outermost_contract() {
    let source = format!(
        "{WHICH}\
         LET inner = FN EXPR #(INNER) -> :(LIST OF Number) = #([1])\n\
         EXPR #(OUTER) -> :(LIST OF Any) = #(INNER)\n\
         EXPR #(BY_NAME) -> :(LIST OF Any) = #(inner {{}})\n\
         PRINT (WHICH (OUTER))\n\
         PRINT (WHICH (INNER))\n\
         PRINT (WHICH (BY_NAME))"
    );
    assert_eq!(run(&source), "any\nnumbers\nany");
    assert_eq!(
        run("LET r = ({v = \"s\"} :! :{v :Any})\n\
             EXPR #(INNER) -> Number = #(r.v)\n\
             EXPR #(OUTER) -> Any = #(INNER)\n\
             PRINT (OUTER)"),
        "error: :(FN :{} -> Number) returned Str, which does not satisfy Number",
        "a miss names the innermost callee"
    );
}

#[test]
fn eval_holds_its_value_to_its_declared_return() {
    assert_eq!(
        run(&format!(
            "{WHICH}PRINT (WHICH (EVAL #([1]) -> :(LIST OF (Number | Str))))"
        )),
        "number or str",
        "the value is retyped to the declared return"
    );
    assert_eq!(
        run("PRINT (EVAL (#(x) USING {x = \"a\"}) -> Number)"),
        "error: `EVAL`'s code returned Str, which does not satisfy Number"
    );
    assert_eq!(
        run(&format!(
            "{WHICH}EXPR #(INNER) -> :(LIST OF Number) = #([1])\n\
             PRINT (WHICH (EVAL #($(INNER)) -> :(LIST OF (Number | Str))))"
        )),
        "number or str",
        "a tail out of the code hands the `EVAL`'s contract on"
    );
    assert_eq!(
        run("EXPR #(RUN c :Any) -> Any = #(EVAL c -> Any)\nPRINT \"loaded\"\nRUN 1"),
        "loaded\nerror: Number is not code for `EVAL` to run"
    );
    let run_at = "EXPR FOR ALL #[Elt] #(RUN c :Code AT x :Elt) -> Elt = #(EVAL c -> Elt)\n";
    assert_eq!(run(&format!("{run_at}PRINT (RUN #(1) AT 2)")), "1");
    assert_eq!(
        run(&format!("{run_at}PRINT (RUN #(\"a\") AT 2)")),
        "error: `EVAL`'s code returned Str, which does not satisfy Number",
        "the declared return is read under the call's solution"
    );
}

#[test]
fn an_ascription_has_its_type_at_load() {
    let source = format!(
        "{ALIASES}\
         LET exact = ([1] :! (LIST OF Any))\n\
         LET loose = ([1] :! Loose)\n\
         LET any = (1 :! Any)"
    );
    loaded(&source, |program| {
        let top = program.shape();
        let point = ":(LIST OF Any)".to_string();
        assert_eq!(ends(program, top, "exact"), (point.clone(), point));
        assert_eq!(
            ends(program, top, "loose"),
            ("Never".to_string(), ":(:(LIST OF Any) | Null)".to_string())
        );
        assert_eq!(
            ends(program, top, "any"),
            ("Never".to_string(), "Any".to_string())
        );
    });
}

#[test]
fn an_ascription_the_load_can_decide_is_settled() {
    let source = format!(
        "{ALIASES}\
         LET r = ({{v = [1, \"a\"]}} :! :{{v :Any}})\n\
         LET decided = ([1] :! Loose)\n\
         LET checked = (r.v :! (LIST OF Number))"
    );
    loaded(&source, |program| {
        assert!(settled(program, "decided"));
        assert!(!settled(program, "checked"), "`r.v` is at most `Any`");
    });
}

#[test]
fn an_ascription_that_can_never_hold_refuses_the_load() {
    assert_eq!(
        run("PRINT (\"s\" :! Number)"),
        "load: <test>:1:7: this value is Str, which can never satisfy its ascription Number"
    );
    // Load only: `STOP` recurses forever.
    loaded(
        "EXPR #(STOP) -> Never = #(STOP)\nLET z = ((STOP) :! Number)",
        |program| {
            let never = "Never".to_string();
            assert_eq!(
                ends(program, program.shape(), "z"),
                (never.clone(), never),
                "an operand that never arrives is no refusal"
            );
        },
    );
}

#[test]
fn an_exact_argument_a_slot_does_not_admit_drops_its_candidate() {
    let source = format!(
        "{WHICH}LET show = FN EXPR #(SHOW x :(LIST OF Any)) -> Str = #(\n  \
         LET which = (WHICH x)\n  \
         which\n\
         )\n"
    );
    loaded(&source, |program| {
        let shown = body(program, program.shape(), "show");
        assert_eq!(let_narrowing(program, shown, "which"), "selected");
    });
    assert_eq!(run(&format!("{source}PRINT (SHOW [1])")), "any");
}

#[test]
fn a_slot_above_no_type_an_argument_can_carry_refuses_the_load() {
    assert_eq!(
        run("LET r = ({v = 1} :! :{v :Any})\n\
             LET rec = {a = r.v}\n\
             EXPR #(GET x :{b :Number}) -> Any = #(x)\n\
             PRINT (GET rec)"),
        "load: <test>:4:7: no overload of `GET _` admits (:{a :Any})"
    );
}

#[test]
fn a_call_is_exact_at_its_callee_s_return() {
    let source = format!(
        "{WHICH}EXPR #(ANYS) -> :(LIST OF Any) = #([1])\n\
         LET anys = (FN :{{}} -> :(LIST OF Any) = #([1]))\n\
         EXPR FOR ALL #[Elt] #(SAME xs :(LIST OF Elt)) -> :(LIST OF Elt) = #(xs)\n\
         LET ys = ([1] :! (LIST OF (Number | Str)))\n\
         NEWTYPE (Type AS Boxed)\n\
         EXPR #(BOX) -> Boxed = #((Boxed 7))\n\
         LET keyworded = (ANYS)\n\
         LET by_name = (anys {{}})\n\
         LET solved = (SAME ys)\n\
         MODULE lib = (\
         (LET same = (FN FOR ALL #[Elt] :{{xs :(LIST OF Elt)}} -> :(LIST OF Elt) = #(xs))) \
         (LET named = (same {{xs = ys}})))\n\
         LET which = (WHICH keyworded)\n\
         LET box = (BOX)\n"
    );
    loaded(&source, |program| {
        let top = program.shape();
        let point = |rendered: &str| (rendered.to_string(), rendered.to_string());
        assert_eq!(ends(program, top, "keyworded"), point(":(LIST OF Any)"));
        assert_eq!(ends(program, top, "by_name"), point(":(LIST OF Any)"));
        let numbers_or_strs = point(":(LIST OF :(Number | Str))");
        assert_eq!(ends(program, top, "solved"), numbers_or_strs);
        assert_eq!(
            ends(program, module_body(program, "lib"), "named"),
            numbers_or_strs,
            "a call by name whose solve is points is exact"
        );
        assert_eq!(let_narrowing(program, top, "which"), "selected");
        assert_eq!(
            ends(program, top, "box"),
            point("Boxed"),
            "a call is exact at a nominal return"
        );
    });
    assert_eq!(
        run(&format!(
            "{source}PRINT (WHICH (ANYS))\n\
             PRINT (WHICH (anys {{}}))\n\
             PRINT (WHICH (SAME ys))"
        )),
        "any\nany\nnumber or str"
    );
}

#[test]
fn a_call_the_load_cannot_solve_exactly_is_at_most_its_return() {
    let source = "EXPR FOR ALL #[Elt] #(SINGLE x :Elt) -> :(LIST OF Elt) = #([x])\n\
                  LET use = FN EXPR #(USE u :(Number | Str) WITH f :(FN :{} -> :(LIST OF Any))) \
                  -> Any = #(\n  \
                  LET single = (SINGLE u)\n  \
                  LET called = (f {})\n  \
                  single\n\
                  )\n";
    loaded(source, |program| {
        let used = body(program, program.shape(), "use");
        let never = "Never".to_string();
        assert_eq!(
            ends(program, used, "single"),
            (never.clone(), ":(LIST OF :(Number | Str))".to_string()),
            "`Elt` solves to an interval that is no point"
        );
        assert_eq!(let_narrowing(program, used, "single"), "selected");
        assert_eq!(
            ends(program, used, "called"),
            (never, ":(LIST OF Any)".to_string()),
            "a parameter's function may declare a smaller return"
        );
    });
    // Each node carries `LIST OF Number`, which the run's check reads against its static type.
    assert_eq!(
        run(&format!(
            "{source}PRINT (USE 1 WITH (FN :{{}} -> :(LIST OF Number) = #([1])))"
        )),
        "[1]"
    );
}

#[test]
fn a_call_in_tail_position_is_typed_as_any_call() {
    let source = format!(
        "{WHICH}EXPR #(INNER) -> :(LIST OF Number) = #([1])\n\
         LET outer = FN EXPR #(OUTER) -> :(LIST OF Any) = #(INNER)\n\
         LET which = (WHICH (OUTER))\n"
    );
    loaded(&source, |program| {
        let top = program.shape();
        let tail = body(program, top, "outer")
            .statement_type(0)
            .expect("the load typed the tail");
        let render = |handle| display_name(handle, program.types(), program.symbols()).to_string();
        let numbers = ":(LIST OF Number)".to_string();
        assert_eq!(
            (render(tail.lower), render(tail.upper)),
            (numbers.clone(), numbers),
            "the node never finishes, so nothing contradicts its callee's return"
        );
        assert_eq!(let_narrowing(program, top, "which"), "selected");
    });
    assert_eq!(run(&format!("{source}PRINT which")), "any");
}

#[test]
fn a_container_or_nominal_parameter_is_exact() {
    loaded(
        "NEWTYPE (Type AS Boxed)\n\
         NEWTYPE Distance = Number\n\
         UNION Maybe = #{Some: Number, None: Null}\n\
         LET f = (FN :{xs :(LIST OF Any), r :{x :Number}, d :(MAP Str -> Number), \
         u :(Number | Str), b :(Boxed {Type = Number}), n :Distance, bb :Boxed, \
         s :(Maybe.Some), m :Maybe} -> Any = #(xs))",
        |program| {
            let f = body(program, program.shape(), "f");
            for (name, declared) in [
                ("xs", ":(LIST OF Any)"),
                ("r", ":{x :Number}"),
                ("d", ":(MAP Str -> Number)"),
                ("b", ":(Boxed {Type = Number})"),
                ("n", "Distance"),
                ("bb", "Boxed"),
                ("s", "Some"),
            ] {
                let declared = declared.to_string();
                assert_eq!(ends(program, f, name), (declared.clone(), declared));
            }
            let never = "Never".to_string();
            assert_eq!(
                ends(program, f, "u"),
                (never.clone(), ":(Number | Str)".to_string())
            );
            assert_eq!(
                ends(program, f, "m"),
                (never, ":(Some | None)".to_string()),
                "a union keeps a variant's own type"
            );
        },
    );
}

/// The `KIND` overloads a nominal test dispatches a boxed value through.
const KIND: &str = "NEWTYPE (Type AS Boxed)\n\
                    UNION Maybe = #{Some: Number, None: Null}\n\
                    EXPR #(KIND x :(Boxed {Type = :(Number | Str)})) -> Str = #(\"number or str\")\n\
                    EXPR #(KIND x :Boxed) -> Str = #(\"boxed\")\n";

#[test]
fn a_nominal_ascription_is_exact() {
    let source = format!(
        "{KIND}LET boxed = ((Boxed 7) :! Boxed)\n\
         LET maybe = ((Maybe.Some 1) :! Maybe)\n\
         LET kind = (KIND boxed)"
    );
    loaded(&source, |program| {
        let top = program.shape();
        let boxed = "Boxed".to_string();
        assert_eq!(ends(program, top, "boxed"), (boxed.clone(), boxed));
        assert_eq!(
            ends(program, top, "maybe"),
            ("Never".to_string(), ":(Some | None)".to_string()),
            "a union keeps a variant's own type"
        );
        assert_eq!(
            let_narrowing(program, top, "kind"),
            "selected",
            "the application overload is never over an exact bare family"
        );
    });
}

#[test]
fn a_nominal_parameter_selects_at_load_and_a_union_keeps_its_variant() {
    let source = format!(
        "{KIND}EXPR #(PASS b :Boxed) -> Any = #(b)\n\
         LET show = FN EXPR #(SHOW b :Boxed) -> Str = #(\n  \
         LET kind = (KIND b)\n  \
         kind\n\
         )\n\
         EXPR #(DESCRIBE x :(Maybe.Some)) -> Str = #(\"has a value\")\n\
         EXPR #(DESCRIBE x :(Maybe.None)) -> Str = #(\"empty\")\n\
         EXPR #(TAG m :Maybe) -> Any = #(m)\n"
    );
    loaded(&source, |program| {
        let shown = body(program, program.shape(), "show");
        assert_eq!(let_narrowing(program, shown, "kind"), "selected");
    });
    // `PASS`'s and `TAG`'s bodies finish with their parameter, so the run's check reads an exact
    // `Boxed` and an at-most `Maybe` against what each carries.
    assert_eq!(
        run(&format!(
            "{source}PRINT (SHOW (Boxed 7))\n\
             PRINT (KIND (PASS (Boxed 7)))\n\
             PRINT (DESCRIBE (TAG (Maybe.Some 1)))"
        )),
        "boxed\nboxed\nhas a value"
    );
}

#[test]
fn a_generic_use_over_an_exact_parameter_is_selected_at_load() {
    let source = "EXPR FOR ALL #[Elt] #(FLAT rows :(LIST OF (LIST OF Elt))) -> :(LIST OF Elt) = #(\n  \
                  PRINT Elt\n  \
                  []\n\
                  )\n\
                  LET use = FN EXPR #(USE rows :(LIST OF (LIST OF (Number | Str)))) -> \
                  :(LIST OF (Number | Str)) = #(\n  \
                  LET flat = (FLAT rows)\n  \
                  flat\n\
                  )\n";
    loaded(source, |program| {
        let used = body(program, program.shape(), "use");
        assert_eq!(let_narrowing(program, used, "flat"), "selected");
    });
    assert_eq!(
        run(&format!("{source}PRINT (USE [[1]])")),
        ":(Number | Str)\n[]",
        "the argument was retyped: without it, `Elt` solves to `Number`"
    );
}

#[test]
fn a_projection_over_an_exact_record_is_exact() {
    let source = "EXPR #(PICK r :{x :Number, y :Str}) -> Str = #(\"got xy\")\n\
                  EXPR #(PICK r :{x :Number, z :Str}) -> Str = #(\"got xz\")\n\
                  LET both = {x = 1, y = \"a\", z = \"b\"}\n\
                  LET picked = (#[x y] FROM both)\n\
                  LET which = (PICK picked)";
    loaded(source, |program| {
        let top = program.shape();
        let exact = ":{x :Number y :Str}".to_string();
        assert_eq!(ends(program, top, "picked"), (exact.clone(), exact));
        assert_eq!(
            let_narrowing(program, top, "which"),
            "selected",
            "the `{{x, z}}` overload is never: `picked` carries no `z`"
        );
    });
    assert_eq!(run(&format!("{source}\nPRINT which")), "got xy");
}
