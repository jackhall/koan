//! Return-type expressions that reference earlier parameters (`MODULE_TYPE_OF p`, bare param name, `SIG_WITH p.T`), resolved per-call.

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

/// `(MODULE_TYPE_OF Er Type)` parens-form return type registers as
/// `ReturnType::Deferred(Expression(...))` rather than erroring "unbound name
/// `Er`" at FN-construction.
///
/// Pins only the FN-def side. End-to-end invocation `(GET_ZERO IntOrdView)`
/// is gated on `roadmap/type_language/val-slot-attr-retagging.md`: ATTR
/// returns the raw underlying carrier (`Number`) rather than re-tagging it
/// with the per-call abstract identity minted by `:|`, so the lift-time slot
/// check against the per-call `KType::UserType { kind: Module, name: "Type",
/// .. }` rejects with the "per-call return type" diagnostic.
#[test]
fn functor_return_module_type_of_parameter_resolves_per_call() {
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
    run(
        scope,
        "FN (GET_ZERO Er :WithZero) -> (MODULE_TYPE_OF Er Type) = (Er.zero)",
    );
    let f = lookup_fn(scope, "GET_ZERO");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "GET_ZERO's return type should be Deferred, got {:?}",
        f.signature.return_type,
    );
}

/// `(SIG_WITH Set ((Elt: (MODULE_TYPE_OF Er Type))))` — the sharing-constraint
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
        "FN (MK Er :OrderedSig) -> (SIG_WITH Set ((Elt (MODULE_TYPE_OF Er Type)))) = \
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
    run(
        scope,
        "FN (BAD Er :OrderedSig) -> (MODULE_TYPE_OF Er Type) = (1)",
    );
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
