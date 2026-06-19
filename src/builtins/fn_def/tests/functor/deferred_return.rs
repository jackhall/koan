//! Return-type expressions that reference earlier parameters (`p.T`, bare param name, `sig WITH {S = p.T}`), resolved per-call.

use crate::builtins::test_support::{
    lookup_fn, parse_one, run, run_one, run_one_type, run_root_silent,
};
use crate::machine::model::{KObject, KType};
use crate::machine::KoanRegion;

/// Bare parameter-name return type: the body `(Er)` returns the bound module
/// via `BareTypeLeaf`; per-call elaboration resolves `Er` to the carried
/// module's identity through `Scope::resolve_type`.
#[test]
fn functor_return_bare_parameter_name_resolves_per_call() {
    use crate::machine::model::ReturnType;
    let arena = KoanRegion::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Carrier = Number) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Carrier = Number) (LET compare = 7))\n\
         LET IntOrdView = (IntOrd :! OrderedSig)",
    );
    run(scope, "FN (USE_ID Er :OrderedSig) -> Er = (Er)");
    let f = lookup_fn(scope, "USE_ID");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE_ID's return type should be Deferred, got {:?}",
        f.signature.return_type,
    );
    let result = run_one_type(scope, parse_one("USE_ID IntOrdView"));
    match result {
        KType::Module {
            module: _,
            frame: _,
        } => {}
        other => panic!("expected KModule from USE_ID, got {other:?}"),
    }
}

/// `Er.Type` dotted return type registers as
/// `ReturnType::Deferred(Expression(...))` rather than erroring "unbound name
/// `Er`" at FN-construction. Pins the FN-def side; the end-to-end invocation is
/// covered by [`functor_get_zero_on_opaque_view_re_tags_slot_read`].
#[test]
fn functor_return_dotted_type_member_parameter_resolves_per_call() {
    use crate::machine::model::ReturnType;
    let arena = KoanRegion::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG WithZero = ((LET Carrier = Number) (VAL zero :Carrier))\n\
         MODULE IntOrd = ((LET Carrier = Number) (LET zero = 0))\n\
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
/// `IntOrdView.Carrier`, so the body value satisfies the per-call return type
/// `Er.Carrier`. The result carries the abstract `Carrier` identity
/// (`ktype().name()` is "Carrier", a `KType::AbstractType`); unwrapping the `Wrapped`
/// carrier yields the underlying `Number(0)`.
#[test]
fn functor_get_zero_on_opaque_view_re_tags_slot_read() {
    let arena = KoanRegion::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG WithZero = ((LET Carrier = Number) (VAL zero :Carrier))\n\
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
    let arena = KoanRegion::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Carrier = Number) (VAL compare :Number))\n\
         SIG Set = ((LET Elt = Number) (VAL insert :Number))\n\
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
    let arena = KoanRegion::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG Seq = ((LET Carrier = :(LIST OF Any)) (VAL items :Carrier))\n\
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
    let arena = KoanRegion::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG Seq = ((LET Carrier = Number) (VAL v :Number))\n\
         MODULE Ints = ((LET Carrier = Number) (LET v = 1))\n\
         LET View = (Ints :! Seq)",
    );
    run(
        scope,
        "FN (BB Er :Seq) -> Er = (Er)\n\
         FN (AA Er :Seq) -> Er = (BB Er)",
    );
    let mut sched = KoanRuntime::new();
    let id = sched.dispatch_in_scope(parse_one("AA View"), scope);
    sched
        .execute()
        .expect("execute does not surface per-slot errors");
    assert!(
        sched.read_result(id).is_ok(),
        "AA V should succeed: {:?}",
        sched.read_result(id).err(),
    );
    assert_eq!(
        sched.len(),
        1,
        "deferred-return tail chain AA -> BB -> (Er) must collapse to one slot, got {}",
        sched.len(),
    );
}

/// A chain of deferred-`Expression` functors (`-> Er.Carrier`) stays TCO-flat. The first call
/// resolves `Er.Carrier` once and tail-replaces into the body; each subsequent tail call skips
/// resolution (keep-first discards its contract) and tail-replaces, so the chain reuses frames
/// instead of accumulating a dep-finish per call. (The pre-`DeferredExprTail` lowering ran the body as
/// dep-finish dependencies, making each onward call a dep — O(n).)
#[test]
fn deferred_expression_return_tail_chain_reuses_frames() {
    use crate::machine::execute::KoanRuntime;
    let arena = KoanRegion::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG Seq = ((LET Carrier = Number) (VAL v :Number))\n\
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
    let mut sched = KoanRuntime::new();
    let id = sched.dispatch_in_scope(parse_one("AA View"), scope);
    sched
        .execute()
        .expect("execute does not surface per-slot errors");
    assert!(
        sched.read_result(id).is_ok(),
        "AA should succeed: {:?}",
        sched.read_result(id).err(),
    );
    // Subsequent calls tail-replace and reuse per-call frames rather than each spawning a dep-finish.
    assert!(
        sched.tail_reuse_count() >= 1,
        "deferred-Expression tail chain must reuse a frame (tail-replace), got {}",
        sched.tail_reuse_count(),
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
    let arena = KoanRegion::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Carrier = Number) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Carrier = Number) (LET compare = 7))\n\
         LET IntOrdView = (IntOrd :| OrderedSig)",
    );
    run(scope, "FN (BAD Er :OrderedSig) -> Er.Type = (1)");
    let mut sched = KoanRuntime::new();
    let id = sched.dispatch_in_scope(parse_one("BAD IntOrdView"), scope);
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
