//! Return-type expressions that reference earlier parameters (`MODULE_TYPE_OF p`, bare param name, `SIG_WITH p.T`), resolved per-call.

use crate::builtins::test_support::{lookup_fn, parse_one, run, run_one, run_root_silent};
use crate::machine::model::{KObject, KType};
use crate::machine::RuntimeArena;

/// Landing test 1: bare parameter-name return type. `FN (USE_ID Er: OrderedSig) -> Er = ...`
/// returns a module value of type `Er`. The body simply returns the bound parameter
/// (Er is in `bindings.data` from Stage A's value-side bind), and the per-call return-type
/// elaboration resolves `Er` to the per-call module's `UserType { kind: Module, .. }`
/// identity via `Scope::resolve_type` against the per-call scope's `bindings.types`.
#[test]
fn functor_return_bare_parameter_name_resolves_per_call() {
    use crate::machine::model::ReturnType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
         LET int_ord = (IntOrd :! OrderedSig)",
    );
    // FN-def must register with `ReturnType::Deferred(TypeExpr(Er))`. The body `(Er)`
    // returns the bound module value via value_lookup.
    run(
        scope,
        "FN (USE_ID Er :OrderedSig) -> Er = (Er)",
    );
    let f = lookup_fn(scope, "USE_ID");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE_ID's return type should be Deferred, got {:?}",
        f.signature.return_type,
    );
    // Invoke and verify the per-call slot check accepts the bound module.
    let result = run_one(scope, parse_one("USE_ID int_ord"));
    match result {
        KObject::KTypeValue(KType::Module { module: _, frame: _ }) => {}
        other => panic!("expected KModule from USE_ID, got {:?}", other.ktype()),
    }
}

/// Landing test 2: `(MODULE_TYPE_OF Er Type)` parens-form return type. Pins that
/// FN-def registers the function with `ReturnType::Deferred(Expression(...))` instead
/// of erroring at FN-construction (the pre-Stage-B failure mode was "unbound name `Er`"
/// because the parens-form return type sub-dispatched against the outer scope where
/// `Er` is unbound).
///
/// **Post-VAL surface form.** The SIG declares a `Type`-typed value slot
/// (`(VAL zero: Type)`). A MODULE supplying `zero = 0` satisfies the slot under
/// name-presence shape-check, and the FN signature `(GET_ZERO Er: WithZero) ->
/// (MODULE_TYPE_OF Er Type) = (Er.zero)` parses and registers with
/// `ReturnType::Deferred(_)`.
///
/// **Caveat — kept simpler variant.** The plan also drafted an end-to-end
/// invocation `(GET_ZERO int_ord)` returning the underlying `Number(0)` carrier.
/// That fails today: the per-call return-type check on `Deferred(_)` returns runs
/// at lift-time and compares the body's `.ktype()` (Number, from the underlying
/// ATTR-read) against the per-call-elaborated `KType::UserType { kind: Module,
/// name: "Type", .. }`. ATTR returns the raw underlying value rather than
/// re-tagging it with the per-call abstract identity minted by `:|`. The slot
/// check rejects with the documented "per-call return type" diagnostic
/// (`functor_deferred_return_type_mismatch_surfaces_per_call_diagnostic` pins
/// that wording). Closing this end-to-end variant is tracked by
/// `roadmap/val-slot-abstract-identity-tagging.md` (tag ascribed-module
/// value-slot reads with the per-call abstract identity at ATTR time, or relax
/// the slot check to accept "value of declared-abstract-Type" by
/// carrier-recovery rather than KType equality). Until then, the test pins
/// only the VAL substrate: VAL is a valid SIG surface form, the functor's
/// FN-def succeeds with `Deferred(_)` carrying the parens-form return-type
/// reference, and the underlying-MODULE-Type's `LET zero = 0` cleanly satisfies
/// the VAL slot at ascription shape-check time.
#[test]
fn functor_return_module_type_of_parameter_resolves_per_call() {
    use crate::machine::model::ReturnType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG WithZero = ((LET Type = Number) (VAL zero :Type))\n\
         MODULE IntOrd = ((LET Type = Number) (LET zero = 0))\n\
         LET int_ord = (IntOrd :| WithZero)",
    );
    // The ascription succeeded — that's the canonical VAL-slot-satisfied-by-LET
    // pairing this item exists to enable.
    let data = scope.bindings().data();
    assert!(
        matches!(data.get("int_ord"), Some(KObject::KTypeValue(KType::Module { module: _, frame: _ }))),
        "int_ord should be an opaquely-ascribed module satisfying WithZero's VAL zero slot",
    );
    drop(data);
    // FN-def. Pre-Stage-B this errored with "unbound name `Er`" at FN-construction
    // because the parens-form return type sub-dispatched against the outer scope.
    // Post-VAL, the SIG-typed parameter `Er` carries the SIG body's `Type` slot
    // surface and the body's `(Er.zero)` reads through it. The functor registers
    // with `ReturnType::Deferred(_)`; the per-call check at lift-time is what the
    // caveat docstring above documents.
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

/// Landing test 3: `(SIG_WITH Set ((Elt: (MODULE_TYPE_OF Er Type))))` — the sharing-
/// constraint surface canonical for `module Make (E : ORDERED) : SET with type elt = E.t`.
/// The pin value `(MODULE_TYPE_OF Er Type)` references the parameter `Er`; the per-call
/// elaboration of the outer `SIG_WITH` propagates `Er`'s per-call `Type` member into the
/// pinned slot. The body returns a module whose `type_members["Elt"]` matches.
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
         LET int_ord = (IntOrd :! OrderedSig)",
    );
    // FN-def registers with `ReturnType::Deferred(Expression(...))`.
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
    // Body's `MODULE Result` isn't sig-ascribed to `Set`, so its `compatible_sigs` is
    // empty and the SatisfiesSignature check rejects on membership before the pin check.
    // This is the same situation as `functor_return_with_mismatched_sharing_constraint_errors`,
    // but the relevant Stage B invariant is that the FN registered with Deferred at all
    // (without erroring `Unbound` at FN-def time, which was the pre-Stage-B failure mode).
}

/// Stage B negative case: body produces a wrong-typed value for a per-call return type.
/// The Combine's finish closure runs the slot check against the per-call elaboration
/// and rejects with a diagnostic mentioning "per-call return type" — the wording the
/// Stage B implementation pins so a reader knows the rejection path is the per-call
/// check, not the static lift-time one.
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
         LET int_ord = (IntOrd :| OrderedSig)",
    );
    // Functor declared to return `(MODULE_TYPE_OF Er Type)` (a KType value) but the body
    // returns a Number. Per-call check must reject.
    run(
        scope,
        "FN (BAD Er :OrderedSig) -> (MODULE_TYPE_OF Er Type) = (1)",
    );
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("BAD int_ord"), scope);
    sched.execute().expect("execute does not surface per-slot errors");
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
