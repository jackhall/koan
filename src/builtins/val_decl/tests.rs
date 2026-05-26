use crate::builtins::test_support::{
    parse_one, run, run_one_err, run_root_silent,
};
use crate::machine::{KErrorKind, RuntimeArena};
use crate::machine::model::{KObject, KType};

/// Smoke test: `(VAL zero: Number)` inside a SIG body binds `zero` under the SIG's
/// decl_scope as a `KTypeValue(KType::Number)` carrier. The slot exists in
/// `bindings.data` so `ascribe::shape_check` will require it of an ascribed module.
#[test]
fn val_inside_sig_binds_typeexpr_carrier() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "SIG OrderedSig = ((VAL zero :Number))");
    let s = match scope.bindings().data().get("OrderedSig").map(|(o, _)| *o) {
        Some(KObject::KTypeValue(KType::Signature(s))) => *s,
        _ => panic!("OrderedSig must bind a KSignature"),
    };
    let zero = s.decl_scope().bindings().expect_value("zero");
    match zero {
        KObject::KTypeValue(kt) => assert_eq!(*kt, KType::Number),
        other => panic!("expected KTypeValue(Number), got {:?}", other.ktype()),
    }
}

/// SIG-local shadowing: `LET Type = Number` inside the SIG body shadows the builtin
/// `Type`. A subsequent `(VAL zero: Type)` re-elaborates against the SIG decl_scope's
/// types map and binds `zero` with `KType::Number` (the shadow), not `KType::Type`
/// (the meta-type). Pins the parking path — sibling statement order isn't
/// guaranteed, so VAL parks on LET's placeholder and resumes via Combine.
#[test]
fn val_resolves_sig_local_type_shadow() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG WithZero = ((LET Type = Number) (VAL zero :Type))",
    );
    let s = match scope.bindings().data().get("WithZero").map(|(o, _)| *o) {
        Some(KObject::KTypeValue(KType::Signature(s))) => *s,
        _ => panic!("WithZero must bind a KSignature"),
    };
    let zero = s.decl_scope().bindings().expect_value("zero");
    match zero {
        KObject::KTypeValue(kt) => assert_eq!(
            *kt, KType::Number,
            "SIG-local `LET Type = Number` must shadow the meta-type builtin",
        ),
        other => panic!("expected KTypeValue, got {:?}", other.ktype()),
    }
}

/// `VAL` outside a SIG body — at the run-root — surfaces a structured `ShapeError`
/// directing the user to `LET`. Gate is the immediate-enclosing labeled scope check.
#[test]
fn val_outside_sig_errors() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let err = run_one_err(scope, parse_one("VAL x :Number"));
    match &err.kind {
        KErrorKind::ShapeError(msg) => {
            assert!(
                msg.contains("VAL is only valid inside a SIG body"),
                "expected SIG-only diagnostic, got: {msg}",
            );
        }
        _ => panic!("expected ShapeError, got something else"),
    }
}

/// `VAL` inside a MODULE body — modules are not SIGs; surface the same diagnostic.
/// The immediate enclosing labeled scope is `"MODULE ..."`, not `"SIG ..."`.
#[test]
fn val_inside_module_errors() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let err = run_one_err(
        scope,
        parse_one("MODULE Foo = ((VAL x :Number))"),
    );
    match &err.kind {
        KErrorKind::ShapeError(msg) => {
            assert!(
                msg.contains("VAL is only valid inside a SIG body"),
                "expected SIG-only diagnostic, got: {msg}",
            );
        }
        _ => panic!("expected ShapeError, got something else"),
    }
}

/// `(VAL compare: Function<(Number, Number) -> Number>)` — structural type carrier.
/// The dispatcher's eager `from_type_expr` lowering produces
/// `KFunction { args: [Number, Number], ret: Number }`; the body accepts the result
/// directly because the structural form has no SIG-local shadow to honor.
#[test]
fn val_function_typed_slot() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((VAL compare :(Function (Number Number) -> Number)))",
    );
    let s = match scope.bindings().data().get("OrderedSig").map(|(o, _)| *o) {
        Some(KObject::KTypeValue(KType::Signature(s))) => *s,
        _ => panic!("OrderedSig must bind a KSignature"),
    };
    let compare = s.decl_scope().bindings().expect_value("compare");
    match compare {
        KObject::KTypeValue(KType::KFunction { args, ret }) => {
            assert_eq!(args.len(), 2);
            assert_eq!(args[0], KType::Number);
            assert_eq!(args[1], KType::Number);
            assert_eq!(**ret, KType::Number);
        }
        other => panic!("expected KFunction-typed slot, got {:?}", other.ktype()),
    }
}

/// VAL on a SIG body whose name is later required by ascription: the missing-member
/// shape-check still fires because `shape_check` walks `bindings.data` and VAL
/// writes there.
#[test]
fn val_slot_required_by_shape_check() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG WithCompare = ((VAL compare :Number))\n\
         MODULE Empty = (LET unrelated = 0)",
    );
    let err = run_one_err(scope, parse_one("Empty :| WithCompare"));
    match &err.kind {
        KErrorKind::ShapeError(msg) => {
            assert!(
                msg.contains("compare") && msg.contains("WithCompare"),
                "expected diagnostic naming missing `compare`, got: {msg}",
            );
        }
        _ => panic!("expected ShapeError, got something else"),
    }
}

/// A MODULE that supplies the VAL-declared slot via a regular `LET name = <value>`
/// satisfies the SIG. The shape_check is name-presence only; the VAL's declared
/// type is recorded but not yet checked against the example value's `ktype()` —
/// that's modular implicits.
#[test]
fn val_slot_satisfied_by_module_let_member() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG WithCompare = ((VAL compare :Number))\n\
         MODULE IntOrd = (LET compare = 0)\n\
         LET Ord = (IntOrd :| WithCompare)",
    );
    let data = scope.bindings().data();
    assert!(matches!(data.get("Ord").map(|(o, _)| *o), Some(KObject::KTypeValue(KType::Module { module: _, frame: _ }))));
}

/// SIG body mixing the abstract type declaration (`LET Type = Number`) with a VAL
/// slot referencing it. Pins the canonical roadmap form.
#[test]
fn val_with_abstract_type_member_declaration() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG WithZero = ((LET Type = Number) (VAL zero :Type))",
    );
    let s = match scope.bindings().data().get("WithZero").map(|(o, _)| *o) {
        Some(KObject::KTypeValue(KType::Signature(s))) => *s,
        _ => panic!("WithZero must bind a KSignature"),
    };
    // `Type` lives in the SIG's `bindings.types`; `zero` lives in `bindings.data`.
    let type_kt = s.decl_scope().bindings().expect_type("Type");
    assert_eq!(*type_kt, KType::Number);
    let zero = s.decl_scope().bindings().expect_value("zero");
    assert!(matches!(zero, KObject::KTypeValue(KType::Number)));
}
