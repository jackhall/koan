//! Static types and static selection: what the load pass types each value expression and binder
//! as, how it narrows a keyworded use's candidates, and what it refuses.

use crate::memory::Bump;
use crate::parse::ExpressionPart;
use crate::program::{CellSubstrate, Program};
use crate::scope::{BodyShape, Narrowing, ShapeKind, Slot, Static};
use crate::symbols::TypeSymbol;
use crate::type_lattice::{
    DeclaredType, Interval, KType, Parametric, Verdict, display_name, outranks,
};

use super::{Koan, output, run};
use crate::values::BuildBudget;
use crate::values::tests::reading;

/// Load `source` under dispatch and hand `inspect` its program, or the load's refusal.
pub(super) fn loaded<R>(source: &str, inspect: impl FnOnce(&Program<'_>) -> R) -> R {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output())
            .unwrap_or_else(|error| panic!("the program loads: {error}"));
    substrate.with(|running| inspect(running.program()))
}

/// The slot of the value binder `name` in `shape`.
pub(super) fn slot(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> Slot {
    (0..shape.slots())
        .map(|index| Slot(index as u32))
        .find(|slot| {
            program
                .symbols()
                .display(shape.slot_name(*slot).symbol())
                .to_string()
                == name
        })
        .unwrap_or_else(|| panic!("`{name}` is bound here"))
}

/// The static type of the binder `name` in `shape`, rendered: its upper end, or a quantified
/// callable's scheme.
pub(super) fn binder(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> String {
    let typed = (program.facts())
        .binder_type(
            &program.tiers().resting(),
            shape,
            slot(program, shape, name),
        )
        .expect("the load typed the shape");
    let rendered = match typed {
        DeclaredType::Type(interval) => DeclaredType::Type(interval.upper),
        DeclaredType::Scheme(scheme) => DeclaredType::Scheme(scheme),
    };
    display_name(
        rendered,
        &reading(program.tiers(), &Bump::new()),
        program.symbols(),
    )
    .to_string()
}

/// The static type of the binder `name` in `shape`, which binds no quantified callable.
pub(super) fn interval(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> Interval {
    (program.facts())
        .binder_type(
            &program.tiers().resting(),
            shape,
            slot(program, shape, name),
        )
        .expect("the load typed the shape")
        .as_type()
        .expect("the binder binds no quantified callable")
}

/// The static type of the top-level binder `name` of `source`, rendered.
pub(super) fn top(source: &str, name: &str) -> String {
    loaded(source, |program| binder(program, program.shape(), name))
}

/// The lexical variable at `level` named `name`, bounded by `bound`.
pub(super) fn lexical(program: &Program<'_>, level: usize, name: &str, bound: KType) -> Parametric {
    let name = TypeSymbol::declared(name, program.symbols()).expect("a Type token");
    reading(program.tiers(), &Bump::new()).lexical(level, name, bound)
}

/// The upper end of the static type of the binder `name` in `shape`.
pub(super) fn upper(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> Parametric {
    interval(program, shape, name).upper
}

/// The body the binder `name` in `shape` births.
pub(super) fn body<'graph>(
    program: &Program<'graph>,
    shape: &'graph BodyShape<'graph>,
    name: &str,
) -> &'graph BodyShape<'graph> {
    shape
        .births(slot(program, shape, name))
        .expect("the binder births a body")
}

/// The body of the top-level `MODULE` binder `name`.
pub(super) fn module_body<'graph>(
    program: &Program<'graph>,
    name: &str,
) -> &'graph BodyShape<'graph> {
    body(program, program.shape(), name)
}

/// The narrowing of the keyworded use `LET name = <use>` in the top level of `source`.
pub(super) fn narrowing(source: &str, name: &str) -> String {
    loaded(source, |program| {
        let_narrowing(program, program.shape(), name)
    })
}

/// The narrowing of the keyworded use `LET name = <use>` in `shape`.
pub(super) fn let_narrowing(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> String {
    let Some(ExpressionPart::Expression(node)) = shape.rhs(slot(program, shape, name)) else {
        panic!("`{name}` is bound to a node");
    };
    rendered(
        (program.facts())
            .use_statics(&program.tiers().resting(), shape, shape.site_of_node(node))
            .narrowing,
    )
}

/// A narrowing, rendered: `full`, `selected`, or `kept` followed by each kept candidate's verdict.
pub(super) fn rendered(narrowing: Narrowing<'_>) -> String {
    match narrowing {
        Narrowing::Full => "full".to_string(),
        Narrowing::Kept(kept) => {
            kept.entries
                .iter()
                .fold("kept".to_string(), |text, (_, verdict)| {
                    text + match verdict {
                        Verdict::Always => " always",
                        Verdict::Maybe => " maybe",
                        Verdict::Never => " never",
                    }
                })
        }
        Narrowing::Selected(_) => "selected".to_string(),
    }
}

#[test]
fn a_literal_container_or_quote_has_its_own_type() {
    assert_eq!(top("LET n = 1", "n"), "Number");
    assert_eq!(top("LET s = \"a\"", "s"), "Str");
    assert_eq!(
        top("LET xs = [1, \"a\"]", "xs"),
        ":(LIST OF :(Number | Str))"
    );
    assert_eq!(top("LET e = []", "e"), ":(LIST OF Never)");
    assert_eq!(
        top("LET r = {x = 1, y = \"a\"}", "r"),
        ":{x :Number y :Str}"
    );
    assert_eq!(top("LET t = :(LIST OF Number)", "t"), "ProperType");
    assert_eq!(
        top("LET q = #(PRINT \\y)", "q"),
        ":(Expression NEEDING #[y])"
    );
}

#[test]
fn a_call_has_its_callee_s_return() {
    assert_eq!(
        top("LET f = (FN :{x :Number} -> Str = #(\"a\"))", "f"),
        ":(FN :{x :Number} -> Str)"
    );
    assert_eq!(top("LET a = (1 + 2)", "a"), "Number");
    assert_eq!(top("LET p = (PRINT \"x\")", "p"), "Str");
    assert_eq!(
        top(
            "LET f = (FN :{x :Number} -> Str = #(\"a\"))\nLET b = (f {x = 1})",
            "b"
        ),
        "Str"
    );
    assert_eq!(
        top(
            "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> :(LIST OF Elt) = #([x])\nLET w = (WRAP 1)",
            "w"
        ),
        ":(LIST OF Number)",
        "a quantified callee's return is read through its group's intervals"
    );
}

#[test]
fn a_construction_has_the_identity_it_builds() {
    assert_eq!(
        top("NEWTYPE Meters = Number\nLET m = (Meters 3)", "m"),
        "Meters"
    );
    assert_eq!(
        top("NEWTYPE (Type AS Boxed)\nLET b = (Boxed 7)", "b"),
        ":(Boxed {Type = Number})"
    );
}

#[test]
fn a_parameter_is_read_at_its_declared_type() {
    loaded(
        "LET f = (FN :{x :Number} -> Number = #(\n  LET y = x\n  y\n))",
        |program| {
            let f = body(program, program.shape(), "f");
            assert_eq!(binder(program, f, "x"), "Number");
            assert_eq!(binder(program, f, "y"), "Number");
        },
    );
    loaded(
        "MODULE lib = (LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(\n  LET y = x\n  y\n)))",
        |program| {
            let f = body(program, module_body(program, "lib"), "f");
            assert_eq!(
                upper(program, f, "y"),
                lexical(program, 0, "Elt", KType::ANY),
                "a `FOR ALL` parameter is its group's lexical variable"
            );
        },
    );
}

#[test]
fn a_capture_keeps_its_type_along_the_chain() {
    loaded(
        "MODULE lib = (LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(\n  \
         LET g = (FN :{} -> Any = #(\n    LET y = x\n    y\n  ))\n  \
         MODULE inner OVER #[x] = (LET h = (FN FOR ALL #[Tee] :{t :Tee} -> Any = #(\n    LET z = x\n    z\n  )))\n  \
         x\n)))",
        |program| {
            let f = body(program, module_body(program, "lib"), "f");
            let variable = lexical(program, 0, "Elt", KType::ANY);
            let g = body(program, f, "g");
            assert_eq!(upper(program, g, "y"), variable);
            let h = body(program, body(program, f, "inner"), "h");
            assert_eq!(upper(program, h, "z"), variable);
        },
    );
}

/// Two overloads of `AREA`, one over each shape, and a callee that returns either.
const AREAS: &str = "NEWTYPE Circle = :{r :Number}\n\
                     NEWTYPE Square = :{s :Number}\n\
                     EXPR #(AREA c :Circle) -> Number = #(1)\n\
                     EXPR #(AREA s :Square) -> Str = #(\"square\")\n\
                     EXPR #(EITHER) -> (Circle | Square) = #(Circle {r = 1})\n";

/// The static type of each value part of `shape`'s statements the load typed, in program order.
pub(super) fn typed_parts(program: &Program<'_>, shape: &BodyShape<'_>) -> Vec<Interval> {
    let mut typed = Vec::new();
    for statement in shape.body() {
        crate::scope::each_part(statement, |part| {
            typed.extend(program.facts().value_type(
                &program.tiers().resting(),
                shape,
                shape.site(part),
            ));
        });
    }
    typed
}

#[test]
fn a_cyclic_binding_and_a_quote_s_hole_are_typed() {
    loaded("LET f = (FN :{} -> Any = #(a))\nLET a = [f]", |program| {
        let shape = program.shape();
        assert_eq!(binder(program, shape, "f"), ":(FN :{} -> Any)");
        assert_eq!(binder(program, shape, "a"), ":(LIST OF :(FN :{} -> Any))");
    });
    loaded("LET q = #(PRINT y)", |program| {
        let (_, code) = program.shape().nested_shapes()[0];
        let parts = typed_parts(program, code);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].upper, KType::ANY.into(), "a hole is `Any`");
        assert_eq!(
            program
                .facts()
                .statement_type(&program.tiers().resting(), code, 0),
            Some(Interval::within(KType::ANY)),
            "a hole a `USING` may fill is a candidate the load cannot read"
        );
    });
}

#[test]
fn a_use_with_one_admitting_candidate_selects_it() {
    let source = format!("{AREAS}LET a = (AREA (Circle {{r = 1}}))");
    assert_eq!(narrowing(&source, "a"), "selected");
    assert_eq!(top(&source, "a"), "Number");
    assert_eq!(run(&format!("{source}\nPRINT a")), "1");
}

#[test]
fn a_use_several_candidates_may_admit_joins_their_returns() {
    let source = format!("{AREAS}LET a = (AREA (EITHER))");
    assert_eq!(narrowing(&source, "a"), "kept maybe maybe");
    assert_eq!(top(&source, "a"), ":(Str | Number)");
    let source = format!("{AREAS}LET r = ({{v = 1}} :! :{{v :Any}})\nLET a = (AREA r.v)");
    assert_eq!(narrowing(&source, "a"), "kept maybe maybe");
    assert_eq!(top(&source, "a"), ":(Str | Number)");
}

#[test]
fn a_candidate_that_can_never_admit_is_dropped() {
    let source = "EXPR #(PICK x :Number) -> Str = #(\"number\")\n\
                  EXPR #(PICK x :Str) -> Str = #(\"str\")\n\
                  EXPR #(PICK x :Bool) -> Str = #(\"bool\")\n\
                  EXPR #(EITHER) -> (Number | Str) = #(1)\n\
                  LET a = (PICK (EITHER))";
    assert_eq!(narrowing(source, "a"), "kept maybe maybe");
    assert_eq!(run(&format!("{source}\nPRINT a")), "number");
}

#[test]
fn a_use_no_candidate_can_admit_refuses_the_load() {
    assert_eq!(
        run("PRINT (\"a\" + 1)"),
        "load: <test>:1:7: no overload of `_ + _` admits (Str, Number)"
    );
    assert_eq!(
        run("EXPR #(NEVER CALLED) -> Any = #(\n  PRINT (\"a\" + 1)\n)"),
        "load: <test>:2:9: no overload of `_ + _` admits (Str, Number)",
        "inside a body no call reaches"
    );
    assert_eq!(
        run("EXPR FOR ALL #{Elt: Str} #(PLUS x :Elt) -> Any = #(x + 1)"),
        "load: <test>:1:51: no overload of `_ + _` admits (Elt, Number)",
        "a rigid variable is judged through its bound"
    );
    assert_eq!(
        run("EXPR FOR ALL #[Elt] #(PLUS x :Elt) -> Any = #(x + 1)\nPRINT (PLUS 1)"),
        "2"
    );
}

#[test]
fn a_refusal_in_a_quote_s_code_is_reported_by_its_eval() {
    assert_eq!(
        run("LET q = #(PRINT $(\"a\" + 1))\nPRINT \"loaded\"\nEVAL q -> Any"),
        "loaded\nerror: <test>:1:18: no overload of `_ + _` admits (Str, Number)"
    );
}

/// A callable whose body never arrives: its one call is itself.
const DIE: &str = "EXPR #(DIE) -> Never = #(DIE)\n";

#[test]
fn an_argument_that_never_arrives_narrows_nothing() {
    assert_eq!(
        run(&format!(
            "{DIE}EXPR #(USE) -> Any = #((DIE) + 1)\nPRINT \"loaded\""
        )),
        "loaded"
    );
}

#[test]
fn a_body_that_can_never_meet_its_return_refuses_the_load() {
    assert_eq!(
        run("EXPR #(BAD) -> Str = #(1)"),
        "load: <test>:1:23: this body returns Number, which can never satisfy its declared \
         return Str"
    );
    assert_eq!(run("EXPR #(OK) -> (Number | Str) = #(1)\nPRINT (OK)"), "1");
    assert_eq!(
        run("LET r = ({v = 1} :! :{v :Any})\nEXPR #(LATE) -> Str = #(r.v)\nPRINT \"loaded\""),
        "loaded"
    );
    assert_eq!(
        run(
            "MODULE lib = ((LET id = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))) (PRINT (id {x = 1})))"
        ),
        "1"
    );
    assert_eq!(
        run("MODULE lib = (LET no = (FN FOR ALL #{Elt: Str} :{x :Elt} -> Elt = #(1)))"),
        "load: <test>:1:68: this body returns Number, which can never satisfy its declared \
         return Str"
    );
    assert_eq!(
        run(&format!(
            "{DIE}EXPR #(ON) -> Str = #(DIE)\nPRINT \"loaded\""
        )),
        "loaded",
        "a body that never arrives is not checked"
    );
    assert_eq!(
        run("LET r = ({v = 1} :! :{v :Any})\nEXPR #(DIE) -> Never = #(r.v)"),
        "load: <test>:2:25: this body returns Any, which can never satisfy its declared return \
         Never",
        "a declared `Never` is met only by a body that never arrives"
    );
    assert_eq!(
        run("LET q = #((EXPR #(BAD) -> Str = #(1)) (PRINT 2))\nPRINT \"loaded\"\nEVAL q -> Any"),
        "loaded\nerror: <test>:1:34: this body returns Number, which can never satisfy its \
         declared return Str"
    );
}

#[test]
fn an_eval_whose_code_can_never_meet_its_return_refuses_the_load() {
    assert_eq!(
        run("LET q = #(\"a\")\nEVAL q -> Number"),
        "load: <test>:2:1: this `EVAL`'s code returns Str, which can never satisfy its declared \
         return Number"
    );
    assert_eq!(
        run("EVAL #(\"a\") -> Number"),
        "load: <test>:1:1: this `EVAL`'s code returns Str, which can never satisfy its declared \
         return Number"
    );
    assert_eq!(run("LET q = #(1)\nPRINT (EVAL q -> (Number | Str))"), "1");
    assert_eq!(
        run(&format!(
            "{DIE}LET q = #($(DIE))\nEXPR #(USE) -> Str = #(EVAL q -> Str)\nPRINT \"loaded\""
        )),
        "loaded",
        "code that never arrives is not checked"
    );
}

#[test]
fn a_quantified_candidate_is_selected_when_it_always_admits() {
    let only = "EXPR FOR ALL #[Elt] #(ONLY x :Elt) -> Elt = #(x)\nLET o = (ONLY 1)";
    assert_eq!(narrowing(only, "o"), "selected");
    let source = "EXPR #(EITHER) -> Any = #(1)\n\
                  EXPR FOR ALL #[Elt] #(BOTH x :Elt AND y :Elt) -> Str = #(\"both\")\n\
                  LET b = (BOTH (EITHER) AND \"s\")";
    assert_eq!(narrowing(source, "b"), "full", "its `y` may miss the solve");
}

/// Each call of a quantified function solves its group from that call's arguments, at load.
#[test]
fn each_call_of_a_quantified_function_is_typed_by_its_arguments() {
    let pick = "(LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x)))";
    let typed = |call: &str| {
        loaded(
            &format!("MODULE lib = ({pick} (LET n = {call}))"),
            |program| binder(program, module_body(program, "lib"), "n"),
        )
    };
    assert_eq!(typed("(pick {x = 1})"), "Number");
    assert_eq!(typed("(pick {x = \"s\"})"), "Str");
}

/// The block a `USING … SCOPE` in the body of `source`'s one top-level callable builds.
pub(super) fn using_block<R>(
    source: &str,
    inspect: impl FnOnce(&Program<'_>, &BodyShape<'_>) -> R,
) -> R {
    loaded(source, |program| {
        let (_, callable) = program
            .shape()
            .nested_shapes()
            .iter()
            .find(|(_, nested)| nested.kind() == ShapeKind::Callable)
            .expect("the callable is nested in the top level");
        let (_, block) = callable
            .nested_shapes()
            .iter()
            .find(|(_, nested)| nested.kind() == ShapeKind::Block)
            .expect("the callable's body holds the block");
        inspect(program, block)
    })
}

/// A closed quantified head's return is exactly its return under the call's solution: the frame
/// retypes the value to it under the selection's own solution.
#[test]
fn a_surfaced_quantified_head_returns_exactly_its_solved_return() {
    let boxes = "SIG Boxes = #[(EXPR FOR ALL #[Elt] #(BOX _ :Elt) -> :(LIST OF Elt))]\n";
    let source = format!(
        "{boxes}EXPR #(OPEN m :Boxes) -> Any = #(USING (m :! Boxes) SCOPE (\
         (LET a = (BOX 1)) (LET b = (BOX \"s\"))))"
    );
    using_block(&source, |program, block| {
        let exact = |name| {
            let typed = interval(program, block, name);
            assert_eq!(typed.lower, typed.upper, "`{name}` is exactly its return");
            display_name(
                typed.upper,
                &reading(program.tiers(), &Bump::new()),
                program.symbols(),
            )
            .to_string()
        };
        assert_eq!(exact("a"), ":(LIST OF Number)");
        assert_eq!(exact("b"), ":(LIST OF Str)");
    });
}

#[test]
fn a_surfaced_head_reads_the_ascription_s_pins() {
    let stack = "SIG Stack FOR ALL #{Elt: Any} = #[(EXPR #(PUSH _ :Elt) -> :(LIST OF Elt))]\n";
    let using = |ascribed: &str, body: &str| {
        format!("{stack}EXPR #(OPEN s :Stack) -> Any = #(USING (s :! {ascribed}) SCOPE ({body}))")
    };
    // Pinned, the head is closed: every overload the header offers is retyped to it, so its
    // return is exact and a use it never admits is refused.
    let pinned = using("(Stack WITH {Elt = Number})", "LET a = (PUSH 1)");
    using_block(&pinned, |program, block| {
        let typed = interval(program, block, "a");
        assert_eq!(typed.lower, typed.upper, "a closed head's return is exact");
        assert_eq!(
            display_name(
                typed.upper,
                &reading(program.tiers(), &Bump::new()),
                program.symbols()
            )
            .to_string(),
            ":(LIST OF Number)"
        );
    });
    let refused = run(&using("(Stack WITH {Elt = Number})", "PUSH \"s\""));
    assert!(
        refused.contains("no overload of `PUSH _` admits (Str)"),
        "{refused}"
    );
    let open = using("Stack", "LET a = (PUSH 1)");
    using_block(&open, |program, block| {
        let typed = interval(program, block, "a");
        assert_eq!(
            display_name(
                typed.upper,
                &reading(program.tiers(), &Bump::new()),
                program.symbols()
            )
            .to_string(),
            ":(LIST OF Elt)",
            "an unpinned parameter reads as the block's own type name"
        );
    });
}

#[test]
fn a_surfaced_operator_head_is_typed_at_load() {
    let using = |body: &str| {
        format!(
            "SIG Mix = #[(OP #(<>) OVER Number)]\n\
             EXPR #(OPEN m :Mix) -> Any = #(USING (m :! Mix) SCOPE ({body}))"
        )
    };
    using_block(&using("LET a = (1 <> 2)"), |program, block| {
        assert_eq!(upper(program, block, "a"), KType::NUMBER.into());
    });
    let refused = run(&using("\"s\" <> 2"));
    assert!(
        refused.contains("no overload of `_ <> _` admits (Str, Number)"),
        "every overload the header offers is retyped to the head: {refused}"
    );
}

#[test]
fn a_surfaced_head_s_ranking_must_agree_with_its_key_s_others() {
    let source = "SIG Moves = #[(EXPR #(MOVE 2 :Number TO 1 :Number) -> Number)]\n\
                  EXPR #(OPEN m :Moves) -> Any = #(USING (m :! Moves) SCOPE (\
                  (EXPR #(MOVE a :Str TO b :Str) -> Number = #(1))))";
    assert_eq!(
        run(source),
        "load: <test>:2:59: `MOVE _ TO _` is ranked two ways here"
    );
}

/// `SIG Crates` declares `Boxes`'s `BOX` and a `size`; `TAKE` takes a `Boxes`, and `PICK` takes
/// either.
const CRATES: &str = "SIG Boxes = #[(EXPR FOR ALL #[Elt] #(BOX _ :Elt) -> :(LIST OF Elt))]\n\
                      SIG Crates = #[\
                      (EXPR FOR ALL #[Elt] #(BOX _ :Elt) -> :(LIST OF Elt)) (VAL size :Number)]\n\
                      EXPR #(TAKE m :Boxes) -> Str = #(\"boxes\")\n\
                      EXPR #(PICK m :Boxes) -> Str = #(\"boxes\")\n\
                      EXPR #(PICK m :Crates) -> Str = #(\"crates\")\n\
                      LET use = (FN :{c :Crates} -> Str = #(\
                      (LET taken = (TAKE c)) (LET picked = (PICK c)) (picked)))\n";

#[test]
fn a_signature_with_more_members_is_always_at_one_with_fewer_and_outranks_it() {
    loaded(CRATES, |program| {
        let used = body(program, program.shape(), "use");
        assert_eq!(let_narrowing(program, used, "taken"), "selected");
        assert_eq!(let_narrowing(program, used, "picked"), "selected");
        // `PICK`'s two shapes, by whether the slot's signature declares `size`.
        let pick = |sized: bool| {
            let shape = program.shape();
            shape
                .registrations()
                .iter()
                .find_map(|registration| {
                    match program.facts().registered_type(
                        &program.tiers().resting(),
                        shape,
                        registration.slot,
                    ) {
                        Static::Closed(registered) => {
                            let rendered = display_name(
                                registered.shape,
                                &reading(program.tiers(), &Bump::new()),
                                program.symbols(),
                            )
                            .to_string();
                            (rendered.contains("PICK") && rendered.contains("size") == sized)
                                .then_some(registered.shape)
                        }
                        _ => None,
                    }
                })
                .expect("both overloads are typed at load")
        };
        let (crates, boxes) = (pick(true), pick(false));
        let scratch = Bump::new();
        let (crates, boxes) = (crates.into(), boxes.into());
        assert!(outranks(
            &reading(program.tiers(), &Bump::new()),
            &scratch,
            crates,
            boxes,
            0
        ));
    });
    let module = "MODULE crate = (\
                  (EXPR FOR ALL #[Elt] #(BOX x :Elt) -> :(LIST OF Elt) = #([x])) (LET size = 1))\n";
    assert_eq!(
        run(&format!("{CRATES}{module}PRINT (use {{c = crate}})")),
        "crates"
    );
}

#[test]
fn a_fellow_in_a_knot_reads_its_seed_whatever_order_the_members_are_typed_in() {
    let knot = "LET a = {next = b, v = 1}\nLET b = {next = a, v = \"s\"}";
    let swapped = "LET b = {next = a, v = 1}\nLET a = {next = b, v = \"s\"}";
    assert_eq!(top(knot, "a"), ":{next :Any v :Number}");
    assert_eq!(top(knot, "b"), ":{next :Any v :Str}");
    assert_eq!(top(swapped, "b"), top(knot, "a"));
    assert_eq!(top(swapped, "a"), top(knot, "b"));
}

#[test]
fn an_annotated_fellow_is_read_at_its_annotation() {
    let knot = "LET a :{next :Any v :Number} = {next = b, v = 1}\nLET b = {next = a, v = \"s\"}";
    assert_eq!(top(knot, "b"), ":{next :{next :Any v :Number} v :Str}");
}

#[test]
fn a_fellow_s_callable_body_reads_the_fellow_s_own_type() {
    let source = "LET a = {next = b, v = 1}\nLET b = {next = a, f = (FN :{} -> Any = #(a))}";
    loaded(source, |program| {
        let (_, f) = (program.shape().nested_shapes().iter())
            .find(|(_, nested)| nested.kind() == ShapeKind::Callable)
            .expect("the function's body is nested in the program");
        let read = (program
            .facts()
            .statement_type(&program.tiers().resting(), f, 0))
        .expect("the load typed the body");
        let rendered = display_name(
            read.upper,
            &reading(program.tiers(), &Bump::new()),
            program.symbols(),
        )
        .to_string();
        assert_eq!(rendered, ":{next :Any v :Number}");
    });
}

/// Every fact of `program`'s top level and the bodies it births, keyed by the names binding them
/// and sorted: an independent declaration's facts are the same lines wherever it is written.
fn by_names(program: &Program<'_>) -> Vec<String> {
    let bump = Bump::new();
    let types = reading(program.tiers(), &bump);
    let (facts, symbols) = (program.facts(), program.symbols());
    let named = |typed: DeclaredType<Parametric>| display_name(typed, &types, symbols).to_string();
    let statically = |typed: crate::scope::StaticType<'_>| match typed {
        Static::Unknown => "unknown".to_string(),
        Static::Closed(closed) => format!("closed {}", named(closed.into())),
        Static::Rigid { value, .. } => format!("rigid {}", named(value.into())),
    };
    fn render(
        program: &Program<'_>,
        shape: &BodyShape<'_>,
        path: &str,
        out: &mut Vec<String>,
        line: &dyn Fn(&BodyShape<'_>, Slot) -> Option<String>,
        typed: &dyn Fn(&BodyShape<'_>, usize) -> String,
    ) {
        for at in (0..shape.slots()).map(|at| Slot(at as u32)) {
            let name = program
                .symbols()
                .display(shape.slot_name(at).symbol())
                .to_string();
            if let Some(fact) = line(shape, at) {
                out.push(format!("{path} {name} {fact}"));
            }
            if let Some(born) = shape.births(at) {
                render(program, born, &format!("{path}/{name}"), out, line, typed);
            }
        }
        if !path.is_empty() {
            for index in 0..shape.body().len() {
                out.push(format!("{path} #{index} = {}", typed(shape, index)));
            }
            if let Some((closing, _)) = program.facts().closing(shape.segment()) {
                let summary = closing.summary.map(|summary| summary.is_some());
                out.push(format!(
                    "{path} recursive {} summary {summary:?}",
                    closing.recursive
                ));
            }
        }
    }
    let line = |shape: &BodyShape<'_>, at: Slot| -> Option<String> {
        Some(match shape.slot_name(at) {
            crate::symbols::BinderSymbol::Value(_) => {
                match facts.binder_type(&program.tiers().resting(), shape, at)? {
                    DeclaredType::Type(typed) => format!("= {}", named(typed.upper.into())),
                    DeclaredType::Scheme(scheme) => format!("= {}", named(scheme.into())),
                }
            }
            crate::symbols::BinderSymbol::Type(_) => {
                format!(
                    "declares {}",
                    statically(facts.declared_type(&program.tiers().resting(), shape, at))
                )
            }
            crate::symbols::BinderSymbol::Registration(_) => {
                match facts.registered_type(&program.tiers().resting(), shape, at) {
                    Static::Unknown => "registers unknown".to_string(),
                    Static::Closed(registered) => {
                        format!("registers closed {}", named(registered.shape.into()))
                    }
                    Static::Rigid { value, .. } => {
                        format!("registers rigid {}", named(value.shape))
                    }
                }
            }
            crate::symbols::BinderSymbol::Key(_) => return None,
        })
    };
    let typed = |shape: &BodyShape<'_>, index: usize| {
        (facts.statement_type(&program.tiers().resting(), shape, index))
            .map_or("untyped".to_string(), |typed| named(typed.upper.into()))
    };
    let mut out = Vec::new();
    render(program, program.shape(), "", &mut out, &line, &typed);
    out.sort();
    out
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig {
        cases: crate::tests::case_share(1, 4),
        ..proptest::prelude::ProptestConfig::default()
    })]

    /// 10. Reordering independent declarations changes no fact: two top-level declarations whose
    /// units read nothing of each other, swapped, leave every fact each fixes as it was — a call
    /// unfolds into its callee wherever either is written.
    #[test]
    fn reordering_independent_declarations_changes_no_fact(case in super::generate::independent()) {
        let (written, swapped) = (case.render(false), case.render(true));
        let facts = |source: &str| loaded(source, by_names);
        proptest::prop_assert_eq!(facts(&written), facts(&swapped), "in\n{}\nswapped as\n{}", written, swapped);
    }
}

/// A container's type is the union of its cells' filled types, and the load types a literal whose
/// elements are exact by exactly that type.
#[test]
fn a_container_literal_is_typed_by_its_cells_filled_types() {
    let kind = "EXPR #(KIND xs :(LIST OF :{x :Number})) -> Str = #(\"narrow\")\n\
                EXPR #(KIND xs :(LIST OF (:{x :Number, y :Number} | :{x :Number, y :Null}))) \
                -> Str = #(\"filled\")\n\
                PRINT (KIND [{x = 1, y = 2}, {x = 3}])";
    assert_eq!(run(kind), "filled");
    for (source, typed) in [
        (
            "LET l = [{x = 1, y = 2}, {x = 3}]",
            ":(LIST OF :(:{x :Number y :Number} | :{x :Number y :Null}))",
        ),
        (
            "LET l = [{p = {x = 1, y = 2}}, {p = {x = 3}}]",
            ":(LIST OF :(:{p :{x :Number y :Number}} | :{p :{x :Number y :Null}}))",
        ),
        (
            "LET l = {\"a\": {x = 1, y = 2}, \"b\": {x = 3}}",
            ":(MAP Str -> :(:{x :Number y :Number} | :{x :Number y :Null}))",
        ),
    ] {
        loaded(source, |program| {
            let typed_at = interval(program, program.shape(), "l");
            assert!(typed_at.is_exact(), "{source} is exact");
            assert_eq!(binder(program, program.shape(), "l"), typed);
        });
    }
}
