//! Static types and static selection: what the load pass types each value expression and binder
//! as, how it narrows a keyworded use's candidates, and what it refuses.

use crate::parse::ExpressionPart;
use crate::program::{CellSubstrate, Program};
use crate::scope::{BodyShape, Narrowing, Site, Slot};
use crate::symbols::TypeSymbol;
use crate::type_lattice::{Interval, KType, Verdict, display_name};

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

/// The static type of the binder `name` in `shape`, rendered.
pub(super) fn binder(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> String {
    let typed = shape
        .binder_type(slot(program, shape, name))
        .expect("the load typed the shape");
    display_name(typed.upper, program.types(), program.symbols()).to_string()
}

/// The static type of the top-level binder `name` of `source`, rendered.
pub(super) fn top(source: &str, name: &str) -> String {
    loaded(source, |program| binder(program, program.shape(), name))
}

/// The lexical variable at `level` named `name`, bounded by `bound`.
pub(super) fn lexical(program: &Program<'_>, level: usize, name: &str, bound: KType) -> KType {
    let name = TypeSymbol::declared(name, program.symbols()).expect("a Type token");
    program.types().lexical(level, name, bound)
}

/// The upper end of the static type of the binder `name` in `shape`.
pub(super) fn upper(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> KType {
    shape
        .binder_type(slot(program, shape, name))
        .expect("the load typed the shape")
        .upper
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
        "LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(\n  LET y = x\n  y\n))",
        |program| {
            let f = body(program, program.shape(), "f");
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
        "LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(\n  \
         LET g = (FN :{} -> Any = #(\n    LET y = x\n    y\n  ))\n  \
         LET h = (FN FOR ALL #[Tee] :{t :Tee} -> Any = #(\n    LET z = x\n    z\n  ))\n  \
         x\n))",
        |program| {
            let f = body(program, program.shape(), "f");
            let variable = lexical(program, 0, "Elt", KType::ANY);
            let g = body(program, f, "g");
            assert_eq!(upper(program, g, "y"), variable);
            let h = body(program, f, "h");
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
        assert_eq!(statics.parts[0].1.upper, KType::ANY, "a hole is `Any`");
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
        run("LET id = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))\nPRINT (id {x = 1})"),
        "1"
    );
    assert_eq!(
        run("LET no = (FN FOR ALL #{Elt: Str} :{x :Elt} -> Elt = #(1))"),
        "load: <test>:1:54: this body returns Number, which can never satisfy its declared \
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
    let source = "EXPR #(EITHER) -> (Number | Str) = #(1)\n\
                  EXPR FOR ALL #[Elt] #(BOTH x :Elt AND y :Elt) -> Str = #(\"both\")\n\
                  LET b = (BOTH (EITHER) AND \"s\")";
    assert_eq!(narrowing(source, "b"), "full", "its `y` may miss the solve");
}
