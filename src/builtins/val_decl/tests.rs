use crate::builtins::test_support::{binds_module, parse_one, run, run_one_err, run_root_silent};
use crate::machine::model::KType;
use crate::machine::run_root_storage;
use crate::machine::KErrorKind;

/// Smoke: the VAL slot lives in the signature's stored schema (`value_slots`) under its
/// value-class name so `ascribe::check_satisfies` will require it of an ascribed module.
#[test]
fn val_inside_sig_binds_typeexpr_carrier() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "SIG Ordered = ((VAL zero :Number))");
    let content = match scope.resolve_type("Ordered") {
        Some(KType::Signature { content, .. }) => content,
        _ => panic!("Ordered must bind a Signature KType"),
    };
    let zero = content
        .schema
        .value_slots
        .get("zero")
        .expect("zero must live in Ordered's stored schema value_slots");
    assert_eq!(*zero, KType::Number);
}

/// Pins the parking path: sibling statement order isn't guaranteed, so VAL parks
/// on TYPE's placeholder and resumes via dep-finish, picking the SIG-local shadow over
/// the meta-type builtin. The shadow binds an `AbstractType` sourced at the SIG decl scope (so
/// the slot records that it *names* the abstract member `Carrier`).
#[test]
fn val_resolves_sig_local_type_shadow() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "SIG WithZero = ((TYPE Carrier) (VAL zero :Carrier))");
    let content = match scope.resolve_type("WithZero") {
        Some(KType::Signature { content, .. }) => content,
        _ => panic!("WithZero must bind a Signature KType"),
    };
    let zero = content
        .schema
        .value_slots
        .get("zero")
        .expect("zero must live in WithZero's stored schema value_slots");
    match zero {
        KType::AbstractType { source, name, .. } => {
            assert_eq!(
                name, "Carrier",
                "VAL slot must record that it names the SIG-local abstract `Carrier`",
            );
            assert_eq!(*source, content.sig_id);
        }
        other => panic!("expected AbstractType(Carrier), got {other:?}"),
    }
}

/// Duplicate `VAL x` within one SIG body errors `Rebind` naming `x` at the slot insert (VAL
/// installs no dispatch-time placeholder, so the collision surfaces one step later than a
/// placeholder-backed binder would) and the enclosing SIG binds nothing.
#[test]
fn duplicate_val_slot_name_is_rebind() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let err = run_one_err(
        scope,
        parse_one("SIG SigDup = ((VAL x :Number) (VAL x :Str))"),
    );
    assert!(
        matches!(&err.kind, KErrorKind::Rebind { name } if name == "x"),
        "expected Rebind naming `x`, got {err}",
    );
    assert!(
        scope.resolve_type("SigDup").is_none(),
        "the colliding signature binds nothing",
    );
}

/// Gate fires on the immediate-enclosing labeled scope.
#[test]
fn val_outside_sig_errors() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
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
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let err = run_one_err(scope, parse_one("MODULE foo = ((VAL x :Number))"));
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
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = ((VAL compare :(FN (x :Number, y :Number) -> Number)))",
    );
    let content = match scope.resolve_type("Ordered") {
        Some(KType::Signature { content, .. }) => content,
        _ => panic!("Ordered must bind a Signature KType"),
    };
    let compare = content
        .schema
        .value_slots
        .get("compare")
        .expect("compare must live in Ordered's stored schema value_slots");
    match compare {
        KType::KFunction { params, ret, .. } => {
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
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG WithCompare = ((VAL compare :Number))\n\
         MODULE empty = (LET unrelated = 0)",
    );
    let err = run_one_err(scope, parse_one("empty :| WithCompare"));
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
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG WithCompare = ((VAL compare :Number))\n\
         MODULE int_ord = (LET compare = 0)\n\
         LET ord = (int_ord :| WithCompare)",
    );
    // The `:|`-ascribed view `ord` is a module value, so it binds on the value channel.
    assert!(binds_module(scope, "ord"));
}

/// Pins the canonical SIG form: abstract type via `TYPE Carrier` plus a VAL
/// slot whose declared type references it. `Carrier` lives in the decl scope's type table,
/// `zero` in the signature's stored schema `value_slots`; both carry the same `AbstractType`
/// identity, sourced at the SIG decl scope (so opacity threads to the per-call module's
/// `slot_type_tags`), not the collapsed underlying `Number`.
#[test]
fn val_with_abstract_type_member_declaration() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "SIG WithZero = ((TYPE Carrier) (VAL zero :Carrier))");
    let content = match scope.resolve_type("WithZero") {
        Some(KType::Signature { content, .. }) => content,
        _ => panic!("WithZero must bind a Signature KType"),
    };
    let decl_id = content.sig_id;
    let type_kt = content
        .schema
        .abstract_members
        .get("Carrier")
        .expect("Carrier must live in WithZero's stored schema abstract_members");
    assert!(matches!(
        type_kt,
        KType::AbstractType { source, name, .. } if *source == decl_id && name == "Carrier"
    ));
    let zero = content
        .schema
        .value_slots
        .get("zero")
        .expect("zero must live in WithZero's stored schema value_slots");
    assert!(matches!(
        zero,
        KType::AbstractType { source, name, .. } if *source == decl_id && name == "Carrier"
    ));
    assert_eq!(*type_kt, *zero, "both name the same abstract identity");
}
