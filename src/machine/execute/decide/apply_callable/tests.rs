//! The named type-application surface: `:(Ctor {Param = Type, …})`.
//!
//! A record-literal body on a type-constructor head — a `NEWTYPE`-declared family, the builtin
//! `Result`, or a SIG's abstract constructor slot — binds the family's declared parameters by
//! name and yields a `TypeNode::ConstructorApply`. `AS` is its arity-1 sugar. These run the real
//! dispatcher, so they cover the sub-dispatch parking path and the key-check diagnostics.

use crate::builtins::test_support::TestRun;
use crate::machine::KErrorKind;
use crate::machine::model::RunRegistries;
use crate::machine::model::{KType, Record, TypeNode};
use crate::machine::{program_storage, run_root_storage};

/// The `(name, arg)` pairs of a `ConstructorApply`, in the order the args record carries them —
/// the constructor's declared parameter order.
fn applied_args(
    kt: KType,
    registries: &RunRegistries,
) -> Vec<(crate::machine::model::BinderSymbol, KType)> {
    match registries.types.node(kt) {
        TypeNode::ConstructorApply { arguments, .. } => {
            arguments.iter().map(|(name, arg)| (name, *arg)).collect()
        }
        _ => panic!("expected a ConstructorApply, got {}", kt.name(registries)),
    }
}

/// `:(Result {Ok = Number, Error = Str})` applies the builtin two-parameter family, and the args
/// record carries `Ok` before `Error` — `Result`'s declared order.
#[test]
fn result_applies_named_type_arguments() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let applied = test_run.run_one_type(test_run.parse_one(":(Result {Ok = Number, Error = Str})"));
    assert_eq!(
        applied_args(applied, test_run.registries()),
        vec![
            (
                crate::builtins::test_support::binder_token("Ok"),
                KType::NUMBER
            ),
            (
                crate::builtins::test_support::binder_token("Error"),
                KType::STR
            ),
        ],
    );
}

/// A user-declared family applies by its own parameter name.
#[test]
fn user_family_applies_named_type_argument() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("NEWTYPE (Elem AS Wrap)");
    let applied = test_run.run_one_type(test_run.parse_one(":(Wrap {Elem = Number})"));
    assert_eq!(
        applied_args(applied, test_run.registries()),
        vec![(
            crate::builtins::test_support::binder_token("Elem"),
            KType::NUMBER
        )],
    );
}

/// A compound argument is not a bare leaf, so it sub-dispatches and the application parks until
/// the argument's own type expression lands.
#[test]
fn compound_type_argument_sub_dispatches() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("NEWTYPE (Elem AS Wrap)");
    let applied = test_run.run_one_type(test_run.parse_one(":(Wrap {Elem = (LIST OF Number)})"));
    assert_eq!(
        applied_args(applied, test_run.registries()),
        vec![(
            crate::builtins::test_support::binder_token("Elem"),
            test_run.types().list(KType::NUMBER)
        )],
    );
}

/// `AS` is arity-1 sugar for the named form: both fill the family's sole parameter, so the two
/// elaborate to one type.
#[test]
fn as_sugar_equals_named_application() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("NEWTYPE (Elem AS Wrap)");
    let sugared = test_run.run_one_type(test_run.parse_one(":(Number AS Wrap)"));
    let named = test_run.run_one_type(test_run.parse_one(":(Wrap {Elem = Number})"));
    assert_eq!(sugared.digest(), named.digest());
    assert_eq!(sugared, named);
}

/// The args record's identity is its name-to-type map, so writing the parameters in either order
/// names one application.
#[test]
fn named_application_is_order_blind() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let declared =
        test_run.run_one_type(test_run.parse_one(":(Result {Ok = Number, Error = Str})"));
    let reversed =
        test_run.run_one_type(test_run.parse_one(":(Result {Error = Str, Ok = Number})"));
    assert_eq!(declared.digest(), reversed.digest());
    assert_eq!(declared, reversed);
}

/// `KType::name()` renders the application in the constructor's declared order, and that
/// rendering re-parses to the same type. A composite argument renders as a `:(…)` sigil in
/// the record's value position, which the brace-literal parser reads back as a type.
#[test]
fn constructor_apply_name_round_trips() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("NEWTYPE (Elem AS Wrap)");
    for source in [
        ":(Wrap {Elem = Number})",
        ":(Result {Ok = Number, Error = Str})",
        ":(Wrap {Elem = (LIST OF Number)})",
        ":(Wrap {Elem = :(LIST OF Number)})",
        ":(Result {Ok = (LIST OF Number), Error = Str})",
    ] {
        let applied = test_run.run_one_type(test_run.parse_one(source));
        let rendered = applied.name(test_run.registries());
        let reparsed = test_run.run_one_type(test_run.parse_one(&rendered));
        assert_eq!(
            applied.digest(),
            reparsed.digest(),
            "`{rendered}` must re-parse to the type it renders",
        );
    }
}

/// An application that omits a declared parameter names the one it is missing.
#[test]
fn missing_type_parameter_is_named() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let error = test_run.run_one_err(test_run.parse_one(":(Result {Ok = Number})"));
    match &error.kind {
        KErrorKind::ShapeError(message) => {
            assert!(
                message.contains("missing `Error`"),
                "the error must name the missing parameter, got: {message}",
            );
            assert!(
                message.contains("`Ok`"),
                "the error must list the declared parameters, got: {message}",
            );
        }
        _ => panic!("expected a ShapeError"),
    }
}

/// An application supplying a name the family does not declare names the unknown key alongside
/// the parameter it left unfilled.
#[test]
fn unknown_type_parameter_is_named() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("NEWTYPE (Elem AS Wrap)");
    let error = test_run.run_one_err(test_run.parse_one(":(Wrap {Item = Number})"));
    match &error.kind {
        KErrorKind::ShapeError(message) => {
            assert!(
                message.contains("unknown `Item`") && message.contains("missing `Elem`"),
                "the error must name both the unknown and the missing key, got: {message}",
            );
        }
        _ => panic!("expected a ShapeError"),
    }
}

/// A SIG's abstract constructor slot applies by name inside the same SIG's value slots, and a
/// module whose supplied family declares the same parameter name satisfies it.
#[test]
fn abstract_slot_applies_named_type_argument() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "NEWTYPE (Elem AS Wrapper)\n\
         SIG Boxy = ((TYPE (Elem AS Wrap)) \
         (VAL make :(FN (x :Number) -> :(Wrap {Elem = Number}))))\n\
         MODULE id_box = ((LET Wrap = Wrapper) \
         (LET make = FN (MAKEBOX x :Number) -> :(Wrapper {Elem = Number}) = (Wrapper (x))))",
    );
    test_run.run("LET view = (id_box :| Boxy)");
    assert!(
        crate::builtins::test_support::binds_module(scope, "view"),
        "a module supplying a same-named family must satisfy a named-application value slot",
    );
}

/// An arity-2 abstract slot is satisfied end to end by a module binding the builtin `Result`,
/// whose parameters are named `Ok` and `Error`.
#[test]
fn arity_two_abstract_slot_satisfied_by_result() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Bifunctor = ((TYPE (Ok Error AS Result2)))\n\
         MODULE result_bifunctor = ((LET Result2 = Result))",
    );
    test_run.run("LET view = (result_bifunctor :| Bifunctor)");
    assert!(
        crate::builtins::test_support::binds_module(scope, "view"),
        "`LET Result2 = Result` must satisfy `TYPE (Ok Error AS Result2)`",
    );
}

/// An identity wrapper infers one type argument from the one value it wraps, so a wider family
/// has no value-construction surface yet.
#[test]
fn multi_parameter_family_rejects_value_construction() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("NEWTYPE (One Two AS Pair)");
    let error = test_run.run_one_err(test_run.parse_one("(Pair 3.0)"));
    match &error.kind {
        KErrorKind::ShapeError(message) => {
            assert!(
                message.contains("`Pair` takes 2 type parameters")
                    && message.contains("not yet supported"),
                "unexpected message: {message}",
            );
        }
        _ => panic!("expected a ShapeError"),
    }
    // The type-application surface stays open for the same family.
    let applied = test_run.run_one_type(test_run.parse_one(":(Pair {One = Number, Two = Str})"));
    assert_eq!(
        applied_args(applied, test_run.registries()),
        vec![
            (
                crate::builtins::test_support::binder_token("One"),
                KType::NUMBER
            ),
            (
                crate::builtins::test_support::binder_token("Two"),
                KType::STR
            ),
        ],
    );
}

/// An `Result (Ok v)` carrier erases its type arguments, so admission against a named application
/// reads the tag's parameter out of the args record directly.
#[test]
fn erased_result_carrier_admits_named_application() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("LET wrapped = (Result (Ok 3.0))");
    let admitting =
        test_run.run_one_type(test_run.parse_one(":(Result {Ok = Number, Error = Any})"));
    let refusing = test_run.run_one_type(test_run.parse_one(":(Result {Ok = Str, Error = Any})"));
    let value = scope.expect_value("wrapped");
    let types = test_run.registry_handle();
    assert!(
        admitting.matches_value(value, types.registries()),
        "an `Ok` carrier of a Number must inhabit `:(Result {{Ok = Number, Error = Any}})`",
    );
    assert!(
        !refusing.matches_value(value, types.registries()),
        "the same carrier must not inhabit an application binding `Ok` to Str",
    );
}

/// A type argument that resolves to a runtime value rather than a type is a slot mismatch named
/// by its parameter.
#[test]
fn value_type_argument_is_refused() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("NEWTYPE (Elem AS Wrap)");
    test_run.run("LET n = 3.0");
    let error = test_run.run_one_err(test_run.parse_one(":(Wrap {Elem = (n)})"));
    match &error.kind {
        KErrorKind::TypeMismatch { arg, expected, .. } => {
            assert_eq!(arg, "Elem");
            assert_eq!(expected, "Type");
        }
        _ => panic!("expected a TypeMismatch"),
    }
}

/// A `ConstructorApply` over a SIG's abstract constructor slot classifies into the constructor
/// family, so an `OfKind` slot expecting a type constructor admits it.
#[test]
fn constructor_apply_over_abstract_slot_is_a_type_constructor() {
    use crate::machine::core::ScopeId;
    use crate::machine::model::KKind;
    let registries = RunRegistries::new();
    let types = &registries.types;
    let wrap = crate::builtins::test_support::type_name("Wrap", &registries);
    let elem = crate::builtins::test_support::type_name("Elem", &registries);
    let ctor = types.intern(TypeNode::AbstractType {
        source: ScopeId::from_raw(0, 0xB0B),
        name: wrap,
        param_names: vec![elem],
        nonce: None,
    });
    let applied = types.constructor_apply(
        ctor,
        Record::from_pairs([(
            crate::builtins::test_support::binder_token("Elem"),
            KType::NUMBER,
        )]),
    );
    assert_eq!(applied.kind_of(types), KKind::TypeConstructor);
}
