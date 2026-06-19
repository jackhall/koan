use crate::builtins::test_support::{parse_one, run, run_one_err, run_root_silent};
use crate::machine::model::KType;
use crate::machine::{KErrorKind, KoanRegion};

/// Smoke: the VAL slot lives in `bindings.types` under its value-class name so
/// `ascribe::shape_check` will require it of an ascribed module.
#[test]
fn val_inside_sig_binds_typeexpr_carrier() {
    let arena = KoanRegion::new();
    let scope = run_root_silent(&arena);
    run(scope, "SIG OrderedSig = ((VAL zero :Number))");
    let s = match scope.resolve_type("OrderedSig") {
        Some(KType::Signature { sig, .. }) => *sig,
        _ => panic!("OrderedSig must bind a Signature KType"),
    };
    let zero = s.decl_scope().bindings().expect_type("zero");
    assert_eq!(*zero, KType::Number);
}

/// Pins the parking path: sibling statement order isn't guaranteed, so VAL parks
/// on LET's placeholder and resumes via dep-finish, picking the SIG-local shadow over
/// the meta-type builtin. The shadow binds a `Sig`-rooted `AbstractType` (so the
/// slot records that it *names* the abstract member `Type`), not the collapsed
/// underlying `Number`.
#[test]
fn val_resolves_sig_local_type_shadow() {
    use crate::machine::model::types::AbstractSource;
    let arena = KoanRegion::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG WithZero = ((LET Carrier = Number) (VAL zero :Carrier))",
    );
    let s = match scope.resolve_type("WithZero") {
        Some(KType::Signature { sig, .. }) => *sig,
        _ => panic!("WithZero must bind a Signature KType"),
    };
    let zero = s.decl_scope().bindings().expect_type("zero");
    match zero {
        KType::AbstractType {
            source: AbstractSource::Sig(_),
            name,
        } => assert_eq!(
            name, "Carrier",
            "VAL slot must record that it names the SIG-local abstract `Carrier`",
        ),
        other => panic!("expected AbstractType(Carrier), got {other:?}"),
    }
}

/// Gate fires on the immediate-enclosing labeled scope.
#[test]
fn val_outside_sig_errors() {
    let arena = KoanRegion::new();
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

/// Companion to `val_outside_sig_errors`: MODULE's enclosing labeled scope is
/// `"MODULE ..."`, not `"SIG ..."`, so the same diagnostic must fire.
#[test]
fn val_inside_module_errors() {
    let arena = KoanRegion::new();
    let scope = run_root_silent(&arena);
    let err = run_one_err(scope, parse_one("MODULE Foo = ((VAL x :Number))"));
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

/// Structural carriers (here `Function<...>`) are lifted directly — no SIG-local
/// shadow to honor, so the body skips the re-dispatch path.
#[test]
fn val_function_typed_slot() {
    let arena = KoanRegion::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((VAL compare :(FN (x :Number, y :Number) -> Number)))",
    );
    let s = match scope.resolve_type("OrderedSig") {
        Some(KType::Signature { sig, .. }) => *sig,
        _ => panic!("OrderedSig must bind a Signature KType"),
    };
    let compare = s.decl_scope().bindings().expect_type("compare");
    match compare {
        KType::KFunction { params, ret } => {
            assert_eq!(params.len(), 2);
            assert_eq!(params.get("x"), Some(&KType::Number));
            assert_eq!(params.get("y"), Some(&KType::Number));
            assert_eq!(**ret, KType::Number);
        }
        other => panic!("expected KFunction-typed slot, got {other:?}"),
    }
}

/// `shape_check` walks `bindings.data` and VAL writes there, so a missing slot
/// surfaces as a ShapeError naming both the member and the SIG.
#[test]
fn val_slot_required_by_shape_check() {
    let arena = KoanRegion::new();
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

/// Pins the name-presence-only contract: shape_check passes even though the
/// MODULE's `LET compare = 0` value isn't type-checked against the VAL's declared
/// `Number` — that's modular implicits' job, not shape_check's.
#[test]
fn val_slot_satisfied_by_module_let_member() {
    let arena = KoanRegion::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG WithCompare = ((VAL compare :Number))\n\
         MODULE IntOrd = (LET compare = 0)\n\
         LET Ord = (IntOrd :| WithCompare)",
    );
    // The `:|`-ascribed module alias `Ord` is type-only (no value-side carrier), so its
    // module identity lives in `types`.
    assert!(matches!(
        scope.resolve_type("Ord"),
        Some(KType::Module {
            module: _,
            frame: _
        })
    ));
}

/// Pins the canonical SIG form: abstract type via `LET Carrier = ...` plus a VAL
/// slot whose declared type references it. `Carrier` lives in `bindings.types`,
/// `zero` in `bindings.data`; both carry the `Sig`-rooted `AbstractType` identity
/// (so opacity threads to the per-call module's `slot_type_tags`), not the
/// collapsed underlying `Number`.
#[test]
fn val_with_abstract_type_member_declaration() {
    use crate::machine::model::types::AbstractSource;
    let arena = KoanRegion::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG WithZero = ((LET Carrier = Number) (VAL zero :Carrier))",
    );
    let s = match scope.resolve_type("WithZero") {
        Some(KType::Signature { sig, .. }) => *sig,
        _ => panic!("WithZero must bind a Signature KType"),
    };
    let type_kt = s.decl_scope().bindings().expect_type("Carrier");
    assert!(matches!(
        type_kt,
        KType::AbstractType {
            source: AbstractSource::Sig(_),
            name,
        } if name == "Carrier"
    ));
    let zero = s.decl_scope().bindings().expect_type("zero");
    assert!(matches!(
        zero,
        KType::AbstractType {
            source: AbstractSource::Sig(_),
            name,
        } if name == "Carrier"
    ));
}
