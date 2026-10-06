//! Static types and static selection: what the load pass types each value expression and binder
//! as, how it narrows a keyworded use's candidates, and what it refuses.

use crate::memory::Bump;
use crate::parse::ExpressionPart;
use crate::program::{CellSubstrate, Program};
use crate::scope::{BodyShape, Narrowing, ShapeKind, Site, Slot, Static};
use crate::symbols::TypeSymbol;
use crate::type_lattice::{
    DeclaredType, Interval, KType, Parametric, Verdict, class_at_least, display_name,
};

use super::{Koan, output, run};

/// Load `source` under dispatch and hand `inspect` its program, or the load's refusal.
pub(super) fn loaded<R>(source: &str, inspect: impl FnOnce(&Program<'_>) -> R) -> R {
    let mut substrate = CellSubstrate::load::<Koan>(source, "<test>", 8, output())
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
    let typed = shape
        .binder_type(slot(program, shape, name))
        .expect("the load typed the shape");
    let rendered = match typed {
        DeclaredType::Type(interval) => DeclaredType::Type(interval.upper),
        DeclaredType::Scheme(scheme) => DeclaredType::Scheme(scheme),
    };
    display_name(rendered, program.types(), program.symbols()).to_string()
}

/// The static type of the binder `name` in `shape`, which binds no quantified callable.
pub(super) fn interval(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> Interval {
    shape
        .binder_type(slot(program, shape, name))
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
    program.types().lexical(level, name, bound)
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
fn narrowing(source: &str, name: &str) -> String {
    loaded(source, |program| {
        let_narrowing(program, program.shape(), name)
    })
}

/// The narrowing of the keyworded use `LET name = <use>` in `shape`.
pub(super) fn let_narrowing(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> String {
    let Some(ExpressionPart::Expression(node)) = shape.rhs(slot(program, shape, name)) else {
        panic!("`{name}` is bound to a node");
    };
    rendered(shape.narrowing(Site::of_node(node.reference())))
}

/// A narrowing, rendered: `full`, `selected`, or `kept` followed by each kept candidate's verdict.
pub(super) fn rendered(narrowing: Narrowing<'_>) -> String {
    match narrowing {
        Narrowing::Full => "full".to_string(),
        Narrowing::Kept(kept) => kept.iter().fold("kept".to_string(), |text, (_, verdict)| {
            text + match verdict {
                Verdict::Always => " always",
                Verdict::Maybe => " maybe",
                Verdict::Never => " never",
            }
        }),
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

#[test]
fn a_cyclic_binding_and_a_quote_s_hole_are_typed() {
    loaded("LET f = (FN :{} -> Any = #(a))\nLET a = [f]", |program| {
        let shape = program.shape();
        assert_eq!(binder(program, shape, "f"), ":(FN :{} -> Any)");
        assert_eq!(binder(program, shape, "a"), ":(LIST OF :(FN :{} -> Any))");
    });
    loaded("LET q = #(PRINT y)", |program| {
        let (_, code) = program.shape().nested_shapes()[0];
        let statics = code.statics().expect("the load typed the code");
        assert_eq!(statics.parts.len(), 1);
        assert_eq!(
            statics.parts[0].1.upper,
            KType::ANY.into(),
            "a hole is `Any`"
        );
        assert_eq!(
            statics.statements,
            [Interval::within(KType::ANY)],
            "a hole a `USING` may fill is a candidate the load cannot read"
        );
    });
}

/// Two overloads of `AREA`, one over each shape, and a callee that returns either.
const AREAS: &str = "NEWTYPE Circle = :{r :Number}\n\
                     NEWTYPE Square = :{s :Number}\n\
                     EXPR #(AREA c :Circle) -> Number = #(1)\n\
                     EXPR #(AREA s :Square) -> Str = #(\"square\")\n\
                     EXPR #(EITHER) -> (Circle | Square) = #(Circle {r = 1})\n";

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
    assert_eq!(narrowing(&source, "a"), "full");
    assert_eq!(top(&source, "a"), ":(Str | Number)");
    let source = format!("{AREAS}LET r = ({{v = 1}} :! :{{v :Any}})\nLET a = (AREA r.v)");
    assert_eq!(narrowing(&source, "a"), "full");
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

#[test]
fn a_surfaced_head_is_typed_at_load_and_returns_at_most() {
    let boxes = "SIG Boxes = #[(EXPR FOR ALL #[Elt] #(BOX _ :Elt) -> :(LIST OF Elt))]\n";
    let source = format!(
        "{boxes}EXPR #(OPEN m :Boxes) -> Any = #(USING (m :! Boxes) SCOPE (\
         (LET a = (BOX 1)) (LET b = (BOX \"s\"))))"
    );
    using_block(&source, |program, block| {
        let at_most = |name| {
            let typed = interval(program, block, name);
            assert_eq!(
                typed.lower,
                KType::NEVER.into(),
                "`{name}` is at most its return"
            );
            display_name(typed.upper, program.types(), program.symbols()).to_string()
        };
        assert_eq!(at_most("a"), ":(LIST OF Number)");
        assert_eq!(at_most("b"), ":(LIST OF Str)");
    });
}

#[test]
fn a_surfaced_head_reads_the_ascription_s_pins() {
    let stack = "SIG Stack FOR ALL #{Elt: Any} = #[(EXPR #(PUSH _ :Elt) -> :(LIST OF Elt))]\n";
    let using = |ascribed: &str, body: &str| {
        format!("{stack}EXPR #(OPEN s :Stack) -> Any = #(USING (s :! {ascribed}) SCOPE ({body}))")
    };
    let pinned = using("(Stack WITH {Elt = Number})", "LET a = (PUSH 1)");
    using_block(&pinned, |program, block| {
        let typed = interval(program, block, "a");
        assert_eq!(typed.lower, KType::NEVER.into());
        assert_eq!(
            display_name(typed.upper, program.types(), program.symbols()).to_string(),
            ":(LIST OF Number)"
        );
    });
    // The key holds the module's own overloads, which may admit more than the head does.
    assert_eq!(
        run(&using("(Stack WITH {Elt = Number})", "PUSH \"s\"")),
        "",
        "a use the head alone never admits loads"
    );
    let open = using("Stack", "LET a = (PUSH 1)");
    using_block(&open, |program, block| {
        let typed = interval(program, block, "a");
        assert_eq!(
            display_name(typed.upper, program.types(), program.symbols()).to_string(),
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
    assert_eq!(
        run(&using("\"s\" <> 2")),
        "",
        "the key holds the module's own overloads, which may admit more than the head"
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
                .find_map(
                    |registration| match shape.registered_type(registration.slot) {
                        Static::Closed(registered) => {
                            let rendered =
                                display_name(registered.shape, program.types(), program.symbols())
                                    .to_string();
                            (rendered.contains("PICK") && rendered.contains("size") == sized)
                                .then_some(registered.shape)
                        }
                        _ => None,
                    },
                )
                .expect("both overloads are typed at load")
        };
        let (crates, boxes) = (pick(true), pick(false));
        let scratch = Bump::new();
        let (crates, boxes) = (crates.into(), boxes.into());
        assert!(class_at_least(program.types(), &scratch, crates, boxes, 0));
        assert!(!class_at_least(program.types(), &scratch, boxes, crates, 0));
    });
    let module = "MODULE crate = (\
                  (EXPR FOR ALL #[Elt] #(BOX x :Elt) -> :(LIST OF Elt) = #([x])) (LET size = 1))\n";
    assert_eq!(
        run(&format!("{CRATES}{module}PRINT (use {{c = crate}})")),
        "crates"
    );
}
