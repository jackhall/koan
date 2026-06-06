//! Return-type expressions that reference earlier parameters (`p.T`, bare param name, `sig WITH {S = p.T}`), resolved per-call.

use crate::builtins::test_support::{lookup_fn, parse_one, run, run_one, run_root_silent};
use crate::machine::model::{KObject, KType};
use crate::machine::RuntimeArena;

/// Bare parameter-name return type: the body `(Er)` returns the bound module
/// via `BareTypeLeaf`; per-call elaboration resolves `Er` to the carried
/// module's identity through `Scope::resolve_type`.
#[test]
fn functor_return_bare_parameter_name_resolves_per_call() {
    use crate::machine::model::ReturnType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
         LET IntOrdView = (IntOrd :! OrderedSig)",
    );
    run(scope, "FN (USE_ID Er :OrderedSig) -> Er = (Er)");
    let f = lookup_fn(scope, "USE_ID");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE_ID's return type should be Deferred, got {:?}",
        f.signature.return_type,
    );
    let result = run_one(scope, parse_one("USE_ID IntOrdView"));
    match result {
        KObject::KTypeValue(KType::Module {
            module: _,
            frame: _,
        }) => {}
        other => panic!("expected KModule from USE_ID, got {:?}", other.ktype()),
    }
}

/// `Er.Type` dotted return type registers as
/// `ReturnType::Deferred(Expression(...))` rather than erroring "unbound name
/// `Er`" at FN-construction. Pins the FN-def side; the end-to-end invocation is
/// covered by [`functor_get_zero_on_opaque_view_re_tags_slot_read`].
#[test]
fn functor_return_dotted_type_member_parameter_resolves_per_call() {
    use crate::machine::model::ReturnType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG WithZero = ((LET Type = Number) (VAL zero :Type))\n\
         MODULE IntOrd = ((LET Type = Number) (LET zero = 0))\n\
         LET IntOrdView = (IntOrd :| WithZero)",
    );
    assert!(
        matches!(
            scope.resolve_type("IntOrdView"),
            Some(KType::Module {
                module: _,
                frame: _
            })
        ),
        "IntOrdView should be an opaquely-ascribed module (type-only) satisfying WithZero's \
         VAL zero slot",
    );
    run(scope, "FN (GET_ZERO Er :WithZero) -> Er.Type = (Er.zero)");
    let f = lookup_fn(scope, "GET_ZERO");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "GET_ZERO's return type should be Deferred, got {:?}",
        f.signature.return_type,
    );
}

/// End-to-end functor-on-VAL-slot call: `(GET_ZERO IntOrdView)` succeeds where
/// `IntOrdView` is an opaque (`:|`) view. The body `(Er.zero)` reads the VAL slot,
/// which ATTR re-tags with the per-call abstract identity `:|` minted for
/// `IntOrdView.Type`, so the body value satisfies the per-call return type
/// `Er.Type`. The result carries the abstract `Type` identity
/// (`ktype().name()` is "Type", a `KType::AbstractType`); unwrapping the `Wrapped`
/// carrier yields the underlying `Number(0)`.
#[test]
fn functor_get_zero_on_opaque_view_re_tags_slot_read() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG WithZero = ((LET Type = Number) (VAL zero :Type))\n\
         MODULE IntOrd = ((LET Type = Number) (LET zero = 0))\n\
         LET IntOrdView = (IntOrd :| WithZero)",
    );
    run(scope, "FN (GET_ZERO Er :WithZero) -> Er.Type = (Er.zero)");
    let result = run_one(scope, parse_one("GET_ZERO IntOrdView"));
    match result {
        KObject::Wrapped { inner, type_id } => {
            assert!(
                matches!(type_id, KType::AbstractType { .. }),
                "re-tagged slot read must carry an AbstractType identity, got {:?}",
                type_id,
            );
            assert_eq!(
                type_id.name(),
                "Type",
                "the abstract identity is the SIG-named member `Type`",
            );
            assert!(
                matches!(inner.get(), KObject::Number(n) if *n == 0.0),
                "unwrapping the carrier yields the underlying Number(0), got {:?}",
                inner.get().ktype(),
            );
        }
        other => panic!(
            "expected a re-tagged Wrapped carrier from (GET_ZERO IntOrdView), got {:?}",
            other.ktype(),
        ),
    }
}

/// `(Set WITH {Elt = Er.Type})` — the sharing-constraint
/// surface canonical for `module Make (E : ORDERED) : SET with type elt = E.t`.
/// Pins that FN-def registers `Deferred(_)` without erroring `Unbound` on `Er`;
/// the body's `MODULE Result` isn't sig-ascribed to `Set`, so end-to-end
/// invocation would reject at `Signature { .. }` membership before reaching
/// the pin check.
#[test]
fn functor_return_sig_with_parameter_ref_resolves_per_call() {
    use crate::machine::model::ReturnType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
         SIG Set = ((LET Elt = Number) (VAL insert :Number))\n\
         MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
         LET IntOrdView = (IntOrd :! OrderedSig)",
    );
    run(
        scope,
        "FN (MK Er :OrderedSig) -> :(Set WITH {Elt = Er.Type}) = \
         (MODULE Result = ((LET Elt = Number) (LET insert = 0)))",
    );
    let f = lookup_fn(scope, "MK");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "MK's return type should be Deferred, got {:?}",
        f.signature.return_type,
    );
}

/// Wrong-typed body for a per-call return type — Combine-finish runs the
/// slot check against the per-call elaboration and rejects with a diagnostic
/// mentioning "per-call return type", pinning that the rejection path is the
/// per-call check, not the static lift-time one.
#[test]
fn functor_deferred_return_type_mismatch_surfaces_per_call_diagnostic() {
    use crate::machine::execute::Scheduler;
    use crate::machine::KErrorKind;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
         LET IntOrdView = (IntOrd :| OrderedSig)",
    );
    run(scope, "FN (BAD Er :OrderedSig) -> Er.Type = (1)");
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("BAD IntOrdView"), scope);
    sched
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match sched.read_result(id) {
        Err(e) => e,
        Ok(_) => panic!("BAD should fail per-call return-type check"),
    };
    match &err.kind {
        KErrorKind::TypeMismatch { arg, expected, .. } => {
            assert_eq!(arg, "<return>");
            assert!(
                expected.contains("per-call return type"),
                "expected diagnostic to mention 'per-call return type', got `{expected}`",
            );
        }
        _ => panic!("expected TypeMismatch on <return>, got {err}"),
    }
}
