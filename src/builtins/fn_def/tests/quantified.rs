//! Solving a quantified shape at the call. The type side — the group's binding, alpha-equivalence,
//! the shapes it interns — is [`super::shape_surface`]'s; here is what a *call* does with it: one
//! unifier walks the arguments in order, the first position reaching a quantifier binds it, later
//! positions must agree, the body reads the solution as an ordinary type name, and the return
//! elaborates at it.

use crate::builtins::test_support::TestRun;
use crate::machine::model::KObject;
use crate::machine::{KErrorKind, program_storage, run_root_storage};

/// A `NEWTYPE (Type AS Wrap)` and the two quantified members of
/// [design/effects.md](../../../../design/effects.md)'s `Monad`, defined at the top level so a
/// call reaches them by dispatch with no view in the way.
const WRAPPED: &str = "NEWTYPE (Type AS Wrap)\n\
     EXPR FOR ALL (Elt) (PURE x :Elt) -> :(Elt AS Wrap) = (Wrap (x))\n";

/// One call solves each quantifier from the type its argument carries: the same definition answers
/// at `Number` and at `Str`, and neither call is a different overload.
#[test]
fn a_call_solves_its_quantifiers_from_the_arguments() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("EXPR FOR ALL (Elt) (IDENTITY x :Elt) -> Elt = (x)");

    let number = test_run.run_one(test_run.parse_one("IDENTITY 7"));
    assert!(matches!(number, KObject::Number(n) if *n == 7.0));
    let text = test_run.run_one(test_run.parse_one("IDENTITY \"hi\""));
    assert!(matches!(text, KObject::KString(s) if *s == "hi"));
}

/// The solution is one per call, not one per position: a second occurrence of a name already bound
/// must agree with the binding, and the error names the quantifier and what bound it.
#[test]
fn a_second_occurrence_must_agree_with_the_binding() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("EXPR FOR ALL (Elt) (PAIR a :Elt b :Elt) -> Elt = (a)");

    let agreed = test_run.run_one(test_run.parse_one("PAIR 1 2"));
    assert!(matches!(agreed, KObject::Number(n) if *n == 1.0));

    let error = test_run.run_one_err(test_run.parse_one("PAIR 1 \"two\""));
    let KErrorKind::TypeMismatch { arg, expected, got } = &error.kind else {
        panic!("a disagreeing argument is a type mismatch, got {error}");
    };
    assert_eq!(arg, "b");
    assert!(
        expected.contains("Elt") && expected.contains("Number"),
        "the mismatch names the quantifier and its binding, got `{expected}`",
    );
    assert_eq!(got, "Str");
}

/// The return elaborates at the solution: the value a call produces carries the declared return
/// with each quantified position replaced by what this call bound it to.
#[test]
fn the_return_is_substituted_at_the_solution() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(WRAPPED);

    for (call, expected) in [
        ("PRINT (TYPE OF (PURE 5))", ":(Wrap {Type = Number})"),
        ("PRINT (TYPE OF (PURE \"s\"))", ":(Wrap {Type = Str})"),
    ] {
        let KObject::KString(rendered) = test_run.run_one(test_run.parse_one(call)) else {
            panic!("PRINT returns the rendered string");
        };
        assert_eq!(*rendered, expected, "for {call}");
    }
}

/// A quantifier every argument leaves undetermined is refused **at the call**, naming it: the
/// continuation's own return is deferred until its own call, so nothing at this one determines
/// `Res`.
#[test]
fn a_quantifier_no_argument_determines_is_refused_at_the_call() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(&format!(
        "{WRAPPED}\
         EXPR FOR ALL (Elt Res) (BIND m :(Elt AS Wrap) f :(FN :{{x :Elt}} -> :(Res AS Wrap))) \
         -> :(Res AS Wrap) = (f {{x = 1}})\n\
         LET deferred = (FN :{{x :Number}} -> :(x AS Wrap) = (Wrap (x)))"
    ));
    let error = test_run.run_one_err(test_run.parse_one("BIND (PURE 5) deferred"));
    let KErrorKind::TypeMismatch { arg, got, .. } = &error.kind else {
        panic!("an unsolved quantifier is a type mismatch at the call, got {error}");
    };
    assert_eq!(arg, "Res");
    assert!(
        got.contains("determines it"),
        "the miss says no argument determines it, got `{got}`",
    );
}

/// A listed name no argument position reads could never be solved, so the **definition** is
/// refused rather than every call. The return does not count: it is what the solution is
/// substituted into.
#[test]
fn a_quantifier_no_argument_reads_is_refused_at_the_definition() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let error = test_run
        .run_one_err(test_run.parse_one("EXPR FOR ALL (Elt Res) (KEEP a :Elt) -> Elt = (a)"));
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(message)
            if message.contains("`Res` cannot be solved from the arguments")),
        "an unreferenced quantifier is refused at the definition, got {error}",
    );
}

/// A `Name :Type` pair inside a head is a call-time **type argument**, not a quantifier: it needs
/// no group, and the group is not how one is spelled.
#[test]
fn a_type_parameter_in_the_head_is_not_a_quantifier() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("EXPR (BUILD Elt :Type) -> :Elt = (1)");

    let result = test_run.run_one(test_run.parse_one("BUILD Number"));
    assert!(matches!(result, KObject::Number(n) if *n == 1.0));
}

/// The value lane: the combined statement's value half is an ordinary lambda-typed binding whose
/// type **erases** every quantified position, and a call by name solves the quantifiers exactly as
/// a dispatched call does.
#[test]
fn the_value_lane_erases_the_group_and_still_solves() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "NEWTYPE (Type AS Wrap)\n\
         LET pure = FN EXPR FOR ALL (Elt) (PURE x :Elt) -> :(Elt AS Wrap) = (Wrap (x))",
    );

    for (call, expected) in [
        (
            "PRINT (TYPE OF pure)",
            ":(FN :{x :Any} -> :(Wrap {Type = Any}))",
        ),
        ("PRINT (TYPE OF (pure {x = 5}))", ":(Wrap {Type = Number})"),
        ("PRINT (TYPE OF (pure {x = \"s\"}))", ":(Wrap {Type = Str})"),
    ] {
        let KObject::KString(rendered) = test_run.run_one(test_run.parse_one(call)) else {
            panic!("PRINT returns the rendered string");
        };
        assert_eq!(*rendered, expected, "for {call}");
    }
}
