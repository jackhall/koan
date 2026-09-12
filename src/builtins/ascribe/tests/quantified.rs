//! A quantified keyworded member across the ascription barrier: what satisfies one, what the
//! failure says, and what a call through the view solves.
//!
//! Satisfaction is **positional and solver-free** — a declared quantified position is the
//! unconstrained top, so an overload fills it by being at least as general there. The solving
//! happens one layer in, at each call through the installed view.

use crate::builtins::test_support::TestRun;
use crate::machine::KErrorKind;
use crate::machine::model::KObject;
use crate::memory::{program_storage, run_root_storage};

/// `SIG Monad` of [old_design/effects.md](../../../../old_design/effects.md): a wrapper member and the two
/// operations quantified over the element types they hold at.
const MONAD: &str = concat!(
    "SIG Monad = (\n",
    "  (TYPE (Type AS Wrap))\n",
    "  (EXPR FOR ALL (Elt) (PURE x :Elt) -> :(Elt AS Wrap))\n",
    "  (EXPR FOR ALL (Elt Res) (BIND m :(Elt AS Wrap) f :(FN :{x :Elt} -> :(Res AS Wrap)))",
    " -> :(Res AS Wrap))\n",
    ")\n",
);

/// A module implementing `Monad` the way the signature declares it — quantified, one
/// implementation holding at every element type.
const IO: &str = concat!(
    "MODULE io = (\n",
    "  (NEWTYPE (Type AS Wrap))\n",
    "  (EXPR FOR ALL (Elt) (PURE x :Elt) -> :(Elt AS Wrap) = (Wrap (x)))\n",
    "  (EXPR FOR ALL (Elt Res) (BIND m :(Elt AS Wrap) f :(FN :{x :Elt} -> :(Res AS Wrap)))",
    " -> :(Res AS Wrap) = (f {x = 1}))\n",
    ")\n",
);

/// The rendered result of `call` in a run set up with `source`.
fn rendered(source: &str, call: &str) -> String {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(source);
    match test_run.run_one(test_run.parse_one(call)) {
        KObject::KString(text) => text.to_string(),
        _ => panic!("PRINT returns the rendered string"),
    }
}

/// The `Monad` module ascribes once — not once per element type — and every call through the view
/// solves its own quantifiers. Both ascriptions take it: the interface is structural.
#[test]
fn a_quantified_module_ascribes_once_and_solves_per_call() {
    for ascription in [":|", ":!"] {
        let source = format!("{MONAD}{IO}LET view = (io {ascription} Monad)");
        assert_eq!(
            rendered(&source, "PRINT (TYPE OF (USING view SCOPE (PURE 5)))"),
            ":(Wrap {Type = Number})",
            "through a `{ascription}` view",
        );
        assert_eq!(
            rendered(&source, "PRINT (TYPE OF (USING view SCOPE (PURE \"s\")))"),
            ":(Wrap {Type = Str})",
            "through a `{ascription}` view",
        );
    }
}

/// A continuation keeps its own parameter types at the call: `BIND` binds `Elt` from `m`, so the
/// continuation must agree at `x` and supplies `Res` from its own return.
#[test]
fn a_continuation_agrees_at_the_bound_element_type() {
    let source = format!("{MONAD}{IO}LET view = (io :| Monad)");
    assert_eq!(
        rendered(
            &source,
            "PRINT (TYPE OF (USING view SCOPE (BIND (PURE 5) \
             (FN :{x :Number} -> :(Str AS view.Wrap) = (PURE \"s\")))))",
        ),
        ":(Wrap {Type = Str})",
    );
    // An `Any`-typed continuation is *more general* than the binding, so it is admitted too.
    assert_eq!(
        rendered(
            &source,
            "PRINT (TYPE OF (USING view SCOPE (BIND (PURE 5) \
             (FN :{x :Any} -> :(Str AS view.Wrap) = (PURE \"s\")))))",
        ),
        ":(Wrap {Type = Str})",
    );
}

/// A continuation naming a type the call has already bound `Elt` to something else is a call-time
/// error naming the quantifier.
#[test]
fn a_continuation_disagreeing_with_the_binding_is_refused() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(&format!("{MONAD}{IO}LET view = (io :| Monad)"));

    let error = test_run.run_one_err(test_run.parse_one(
        "USING view SCOPE (BIND (PURE 5) (FN :{x :Str} -> :(Str AS view.Wrap) = (PURE \"s\")))",
    ));
    let KErrorKind::TypeMismatch { expected, .. } = &error.kind else {
        panic!("a disagreeing continuation is a type mismatch, got {error}");
    };
    assert!(
        expected.contains("Elt") && expected.contains("Number"),
        "the mismatch names the quantifier and its binding, got `{expected}`",
    );
}

/// A module is admitted only with **one** implementation holding at every instantiation. A
/// monomorphic overload fixes the quantified position, and the failure says so by name; an
/// `Any`-typed one is at least as general, so it satisfies.
#[test]
fn a_monomorphic_overload_fails_where_an_any_typed_one_satisfies() {
    let declaration = concat!(
        "SIG Pure = (\n",
        "  (TYPE (Type AS Wrap))\n",
        "  (EXPR FOR ALL (Elt) (PURE x :Elt) -> :(Elt AS Wrap))\n",
        ")\n",
    );
    let module = |slot: &str| {
        format!(
            "MODULE m = (\n  (NEWTYPE (Type AS Wrap))\n  (EXPR (PURE x :{slot}) -> :({slot} AS Wrap) = (Wrap (x)))\n)\n"
        )
    };

    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(&format!("{declaration}{}", module("Number")));
    let error = test_run.run_one_err(test_run.parse_one("m :| Pure"));
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(message)
            if message.contains("quantifies over `Elt`") && message.contains("`Number`")),
        "a monomorphic overload is refused naming the quantifier, got {error}",
    );

    assert_eq!(
        rendered(
            &format!("{declaration}{}LET view = (m :| Pure)", module("Any")),
            "PRINT (TYPE OF (USING view SCOPE (PURE 5)))",
        ),
        ":(Wrap {Type = Number})",
    );
}
