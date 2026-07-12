//! Return-type expressions that reference earlier parameters (`p.T`, bare param name, `sig WITH {S = p.T}`), resolved per-call.

use crate::builtins::test_support::{lookup_fn, parse_one, run, run_one, run_root_silent};
use crate::machine::core::run_root_storage;
use crate::machine::model::{KObject, KType, Parseable};
use crate::witnessed::region_metrics;

/// Bare parameter-name return type: `-> Er` resolves per-call to the carried type via
/// `Scope::resolve_type`. The parameter is `:Signature`-kind, so `Er` resolves to a *signature* — a
/// valid return type (a concrete module identity is not; see
/// [`home_return_type`](crate::machine::execute)). The body returns a module ascribed to that
/// per-call signature (`IntOrd :| Er`), which the per-call return contract admits.
#[test]
fn functor_return_bare_parameter_name_resolves_per_call() {
    use crate::machine::model::ReturnType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "SIG OrderedSig = (VAL compare :Number)");
    run(scope, "MODULE IntOrd = (LET compare = 7)");
    run(scope, "FN (USE_ID Er :Signature) -> Er = (IntOrd :| Er)");
    let f = lookup_fn(scope, "USE_ID");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE_ID's return type should be Deferred, got {:?}",
        f.signature.return_type,
    );
    let result = run_one(scope, parse_one("USE_ID OrderedSig"));
    match result {
        KObject::Module(_) => {}
        other => {
            panic!(
                "expected the IntOrd view satisfying the per-call signature, got {}",
                other.summarize()
            )
        }
    }
}

/// `Er.Type` dotted return type registers as
/// `ReturnType::Deferred(Expression(...))` rather than erroring "unbound name
/// `Er`" at FN-construction. Pins the FN-def side; the end-to-end invocation is
/// covered by [`functor_get_zero_on_opaque_view_re_tags_slot_read`].
#[test]
fn functor_return_dotted_type_member_parameter_resolves_per_call() {
    use crate::machine::model::ReturnType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG WithZero = ((TYPE Carrier) (VAL zero :Carrier))\n\
         MODULE IntOrd = ((LET Carrier = Number) (LET zero = 0))\n\
         LET IntOrdView = (IntOrd :| WithZero)",
    );
    assert!(
        matches!(
            scope.resolve_type("IntOrdView"),
            Some(KType::Module { module: _ })
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
/// `IntOrdView.Carrier`, so the body value satisfies the per-call return type
/// `Er.Carrier`. The result carries the abstract `Carrier` identity
/// (`ktype().name()` is "Carrier", a `KType::AbstractType`); unwrapping the `Wrapped`
/// carrier yields the underlying `Number(0)`.
#[test]
fn functor_get_zero_on_opaque_view_re_tags_slot_read() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG WithZero = ((TYPE Carrier) (VAL zero :Carrier))\n\
         MODULE IntOrd = ((LET Carrier = Number) (LET zero = 0))\n\
         LET IntOrdView = (IntOrd :| WithZero)",
    );
    run(
        scope,
        "FN (GET_ZERO Er :WithZero) -> Er.Carrier = (Er.zero)",
    );
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
                "Carrier",
                "the abstract identity is the SIG-named member `Carrier`",
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
/// the body's `MODULE Generated` isn't sig-ascribed to `Set`, so end-to-end
/// invocation would reject at `Signature { .. }` membership before reaching
/// the pin check.
#[test]
fn functor_return_sig_with_parameter_ref_resolves_per_call() {
    use crate::machine::model::ReturnType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = ((TYPE Carrier) (VAL compare :Number))\n\
         SIG Set = ((TYPE Elt) (VAL insert :Number))\n\
         MODULE IntOrd = ((LET Carrier = Number) (LET compare = 7))\n\
         LET IntOrdView = (IntOrd :! OrderedSig)",
    );
    run(
        scope,
        "FN (MK Er :OrderedSig) -> :(Set WITH {Elt = Er.Type}) = \
         (MODULE Generated = ((LET Elt = Number) (LET insert = 0)))",
    );
    let f = lookup_fn(scope, "MK");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "MK's return type should be Deferred, got {:?}",
        f.signature.return_type,
    );
}

/// The deferred-return stamp coarsens a parameterized carrier. A body producing a
/// `List<Number>` whose per-call return type resolves to the transparent carrier
/// `(LIST OF Any)` lifts with the *declared* element type `Any` — the deferred-path twin
/// of the resolved-return coarsening at the lift boundary. Without the stamp the body's
/// incidental `Number` element type would leak through.
#[test]
fn functor_deferred_return_coarsens_list_carrier() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Seq = ((TYPE Carrier) (VAL items :Carrier))\n\
         MODULE Ints = ((LET Carrier = :(LIST OF Any)) (LET items = [1 2 3]))\n\
         LET IntsView = (Ints :! Seq)",
    );
    run(scope, "FN (ITEMS Er :Seq) -> Er.Carrier = (Er.items)");
    let result = run_one(scope, parse_one("ITEMS IntsView"));
    match result {
        KObject::List(_, elem) => assert!(
            matches!(elem.as_ref(), KType::Any),
            "deferred return stamped to (LIST OF Any) must coarsen the element type to Any, got {:?}",
            elem,
        ),
        other => panic!(
            "expected a List from (ITEMS IntsView), got {:?}",
            other.ktype(),
        ),
    }
}

/// TCO across deferred returns: a deferred-return FN whose body tail-calls another deferred-return
/// FN collapses to a **single scheduler slot**, exactly like a resolved-return tail chain. The
/// per-call return type rides a `ReturnContract::PerCall` on the tail-replace, so the body is a
/// proper tail call — no per-call dep-finish frame is held, so a recursive deferred body stays
/// TCO-flat. (The pre-`PerCall` dep-finish lowering held a frame per call and would not collapse.)
#[test]
fn deferred_return_tail_call_stays_tco_flat() {
    use crate::machine::execute::KoanRuntime;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    // `Er` is `:Signature`-kind, so the deferred `-> Er` return resolves per-call to a signature (a
    // valid return type, unlike a concrete module identity); each body returns a module ascribed to
    // that per-call signature, which the contract admits.
    run(
        scope,
        "SIG Seq = (VAL v :Number)\n\
         MODULE Ints = (LET v = 1)",
    );
    run(
        scope,
        "FN (BB Er :Signature) -> Er = (Ints :| Er)\n\
         FN (AA Er :Signature) -> Er = (BB Er)",
    );
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one("AA Seq"), scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    assert!(
        runtime.result_error(id).is_ok(),
        "AA Seq should succeed: {:?}",
        runtime.result_error(id).err(),
    );
    assert_eq!(
        runtime.len(),
        1,
        "deferred-return tail chain AA -> BB -> (Er) must collapse to one slot, got {}",
        runtime.len(),
    );
}

/// A chain of deferred-`Expression` functors (`-> Er.Carrier`) stays TCO-flat. The first call
/// resolves `Er.Carrier` once and tail-replaces into the body; each subsequent tail call skips
/// resolution (keep-first discards its contract) and tail-replaces, so the chain mints one region
/// per call instead of accumulating a dep-finish per call. (The pre-`DeferredExprTail` lowering ran
/// the body as dep-finish dependencies, making each onward call a dep — O(n).)
#[test]
fn deferred_expression_return_tail_chain_stays_flat() {
    use crate::machine::execute::KoanRuntime;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Seq = ((TYPE Carrier) (VAL v :Number))\n\
         MODULE Ints = ((LET Carrier = Number) (LET v = 7))\n\
         LET View = (Ints :! Seq)",
    );
    run(
        scope,
        "FN (DD Er :Seq) -> Er.Carrier = (Er.v)\n\
         FN (CC Er :Seq) -> Er.Carrier = (DD Er)\n\
         FN (BB Er :Seq) -> Er.Carrier = (CC Er)\n\
         FN (AA Er :Seq) -> Er.Carrier = (BB Er)",
    );
    let mut runtime = KoanRuntime::new();
    let minted_before = region_metrics().minted_total;
    let id = runtime.dispatch_in_scope(parse_one("AA View"), scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    assert!(
        runtime.result_error(id).is_ok(),
        "AA should succeed: {:?}",
        runtime.result_error(id).err(),
    );
    // Subsequent calls tail-replace rather than each spawning a dep-finish: a `FreshTail` mints
    // exactly one region per user-fn call (AA, BB, CC, DD), not a dep-finish's unbounded fanout.
    // `minted_total` is monotonic, so a before/after diff reads safely with no reset needed.
    let minted = region_metrics().minted_total - minted_before;
    assert_eq!(
        minted, 4,
        "deferred-Expression tail chain must tail-replace one region per call \
         (AA -> BB -> CC -> DD), got {minted}",
    );
}

/// Wrong-typed body for a per-call return type — dep-finish runs the
/// slot check against the per-call elaboration and rejects with a diagnostic
/// mentioning "per-call return type", pinning that the rejection path is the
/// per-call check, not the static lift-time one.
#[test]
fn functor_deferred_return_type_mismatch_surfaces_per_call_diagnostic() {
    use crate::machine::execute::KoanRuntime;
    use crate::machine::KErrorKind;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = ((TYPE Carrier) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Carrier = Number) (LET compare = 7))\n\
         LET IntOrdView = (IntOrd :| OrderedSig)",
    );
    // `Er.Carrier` is the SIG's abstract type member; under opaque ascription it is not
    // `Number`, so the `(1)` body fails the per-call return-type check. (Referencing a real
    // member, not the builtin `Type` name — module member access is module-own and does not
    // fall through to the builtin root.)
    run(scope, "FN (BAD Er :OrderedSig) -> Er.Carrier = (1)");
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one("BAD IntOrdView"), scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match runtime.result_error(id) {
        Err(e) => e,
        Ok(()) => panic!("BAD should fail per-call return-type check"),
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
