//! Targeted Miri coverage for the unsafe sites in this file. Each test pins down a
//! specific aliasing/lifetime shape under tree borrows; logical assertions are minimal
//! — these tests fail when Miri reports UB, not on values.

use super::*;
use crate::builtins::default_scope;
use crate::machine::model::types::KType;
use crate::machine::BindingIndex;

/// `scope_bounded` re-anchors the child scope with a borrow bounded by the `&Rc` witness.
/// The good path: read it within the witness borrow. The over-anchor and covariance
/// compile-error properties were confirmed by the C0 spike (see
/// scratch/type-enforced-frame-reanchor-plan.md § C0 verdict); they are structural —
/// `scope_bounded`'s `'step` borrow cannot widen to a free `'a`, and `Scope<'a>` is invariant.
#[test]
fn scope_bounded_reanchors_within_witness_borrow() {
    let region = KoanRegion::new();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let frame: Rc<CallFrame> = CallFrame::new(scope, None);
    let bounded: &Scope<'_> = frame.scope_bounded();
    // Same underlying child scope as the unbounded accessors, just a shorter borrow.
    assert_eq!(bounded.id, frame.scope().id);
    assert_eq!(bounded.id, frame.scope_for_bind().id);
}

/// `CallFrame::scope`'s re-borrow stays valid when the region is mutated through a
/// sibling pointer afterward — `frame.scope()` and `frame.region().alloc(...)`
/// must coexist soundly under tree borrows.
#[test]
fn call_arena_scope_survives_subsequent_alloc() {
    let region = KoanRegion::new();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let frame = CallFrame::new(scope, None);
    let s = frame.scope();
    let _new = frame.region().alloc_object(KObject::Number(1.0));
    assert!(std::ptr::eq(s.region, frame.region()));
}

/// Raw-pointer roundtrip: lifetime-anchor an extracted `*const KoanRegion` and
/// `*const Scope<'_>` from the same frame, then mutate via one ref while the other
/// stays live.
#[test]
fn call_arena_scope_survives_subsequent_alloc_via_raw_ptr_roundtrip() {
    let region = KoanRegion::new();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let frame: Rc<CallFrame> = CallFrame::new(scope, None);
    let region_ptr: *const KoanRegion = frame.region();
    let scope_ptr: *const Scope<'_> = frame.scope();
    let inner_region: &KoanRegion = unsafe { &*(region_ptr as *const _) };
    let child: &Scope<'_> = unsafe { &*(scope_ptr as *const _) };
    let it_obj: &KObject<'_> = inner_region.alloc_object(KObject::Number(42.0));
    child
        .bind_value("it".to_string(), it_obj, BindingIndex::BUILTIN)
        .unwrap();
    assert!(matches!(child.lookup("it"), Some(KObject::Number(n)) if *n == 42.0));
}

/// Repeated `frame.scope()` calls produce aliasing shared refs that must be
/// concurrently readable.
#[test]
fn call_arena_scope_repeated_calls_alias() {
    let region = KoanRegion::new();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let frame = CallFrame::new(scope, None);
    let s1 = frame.scope();
    let s2 = frame.scope();
    let s3 = frame.scope();
    assert!(std::ptr::eq(s1, s2));
    assert!(std::ptr::eq(s2, s3));
    assert!(s1.outer().is_some());
}

/// Two-deep chain: dropping the local `outer` handle leaves only `inner`'s `FrameStorage.outer`
/// keeping the outer region alive while we read through `inner.scope().outer`.
#[test]
fn call_arena_chained_outer_frame_walkable() {
    let region = KoanRegion::new();
    let run_scope = default_scope(&region, Box::new(std::io::sink()));
    let outer = CallFrame::new(run_scope, None);
    let inner = CallFrame::new(outer.scope(), Some(outer.storage_rc()));
    drop(outer);
    let outer_scope = inner
        .scope()
        .outer()
        .expect("inner.scope().outer must be Some");
    assert!(std::ptr::eq(
        outer_scope.region,
        inner.scope().outer().unwrap().region
    ));
    assert!(outer_scope.outer().is_some());
}

/// In-struct Rc must keep the region alive for a re-anchored `&Scope` stored alongside
/// it once the local Rc handle is dropped.
#[test]
fn call_arena_scope_re_anchored_into_struct_alongside_rc() {
    struct Holder<'a> {
        s: &'a Scope<'a>,
        _f: Rc<CallFrame>,
    }

    let region = KoanRegion::new();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let h = {
        let f = CallFrame::new(scope, None);
        let s: &Scope<'_> = unsafe { std::mem::transmute::<&Scope<'_>, &Scope<'_>>(f.scope()) };
        Holder { s, _f: f }
    };
    assert!(h.s.outer().is_some());
}

/// Allocating records the stored address into the `membership` side-table via
/// `RefCell::borrow_mut` while a prior `&KObject` from the same region is shared-borrowed.
/// Pins that tree-borrows shape.
#[test]
fn runtime_arena_alloc_while_prior_ref_live() {
    let a = KoanRegion::new();
    let r1 = a.alloc_object(KObject::Number(1.0));
    let r2 = a.alloc_object(KObject::Number(2.0));
    assert!(matches!(r1, KObject::Number(n) if *n == 1.0));
    assert!(matches!(r2, KObject::Number(n) if *n == 2.0));
}

/// `alloc_ktype` returns an region-lifetime `&KType` and bumps `alloc_count` by one.
#[test]
fn alloc_ktype_returns_arena_lifetime_ref_and_counts() {
    let a = KoanRegion::new();
    let baseline = a.alloc_count();
    let t: &KType = a.alloc_ktype(KType::Number);
    assert!(matches!(t, KType::Number));
    assert_eq!(a.alloc_count(), baseline + 1);
}

/// Pins the reset transmute pair (`&Scope<'_> → &Scope<'static>` outer cast plus the
/// raw-region-ptr re-anchor) under tree borrows: after reset, a fresh alloc via
/// `region()` and a `bind_value` on `scope()` must coexist.
#[test]
fn call_arena_try_reset_for_tail_round_trip() {
    let outer_region = KoanRegion::new();
    let outer_scope = default_scope(&outer_region, Box::new(std::io::sink()));
    let mut frame: Rc<CallFrame> = CallFrame::new(outer_scope, None);
    let _pre = frame.region().alloc_object(KObject::Number(1.0));
    assert!(frame.region().alloc_count() >= 1);

    let did_reset = frame.try_reset_for_tail(outer_scope);
    assert!(did_reset, "Rc was unique, reset must succeed");

    // Fresh region: only the new child scope remains.
    assert_eq!(frame.region().alloc_count(), 1);

    let v = frame.region().alloc_object(KObject::Number(42.0));
    frame
        .scope()
        .bind_value("k".to_string(), v, BindingIndex::BUILTIN)
        .unwrap();
    assert!(matches!(frame.scope().lookup("k"), Some(KObject::Number(n)) if *n == 42.0));
    assert!(frame.scope().outer().is_some());
}

/// `try_reset_for_tail` refuses when another `Rc<CallFrame>` *shell* clone exists — a
/// transient holder still naming the frame, for which in-place reset would mutate the shell
/// under a live alias. (An escaped value pins `FrameStorage`, not the shell — see
/// [`call_arena_try_reset_for_tail_allows_reset_under_escaped_storage`].)
#[test]
fn call_arena_try_reset_for_tail_refuses_when_aliased() {
    let outer_region = KoanRegion::new();
    let outer_scope = default_scope(&outer_region, Box::new(std::io::sink()));
    let mut frame: Rc<CallFrame> = CallFrame::new(outer_scope, None);
    let pre_region_addr = frame.region() as *const KoanRegion as usize;

    // A second shell holder (not an escape): clone the `Rc<CallFrame>` so strong_count > 1.
    let _alias = Rc::clone(&frame);

    let did_reset = frame.try_reset_for_tail(outer_scope);
    assert!(!did_reset, "aliased frame must refuse reset");

    assert_eq!(
        frame.region() as *const KoanRegion as usize,
        pre_region_addr,
        "refused reset must leave region pointer unchanged",
    );
}

/// An escaped value pins the frame's `FrameStorage`, not its shell, so the shell stays uniquely
/// owned and `try_reset_for_tail` *succeeds*: the escapee's snapshot rides the `FrameStorage` it
/// still holds while the shell installs fresh storage. A gate keyed on the shell's `Rc` count
/// could not distinguish this from a live shell alias and would refuse it.
#[test]
fn call_arena_try_reset_for_tail_allows_reset_under_escaped_storage() {
    let outer_region = KoanRegion::new();
    let outer_scope = default_scope(&outer_region, Box::new(std::io::sink()));
    let mut frame: Rc<CallFrame> = CallFrame::new(outer_scope, None);
    let _escaped = frame.region().alloc_object(KObject::Number(7.0));
    let pre_alloc_count = frame.region().alloc_count();
    let pre_storage_addr = frame.region() as *const KoanRegion as usize;

    // Simulate a closure escape: hold the frame's storage Rc (what an anchored value carries).
    let escaped_storage = frame.storage_rc();

    let did_reset = frame.try_reset_for_tail(outer_scope);
    assert!(
        did_reset,
        "an escaped *storage* hold must not foreclose reuse"
    );

    // The shell reset to a fresh region, distinct from the snapshot the escapee still holds.
    assert_ne!(
        frame.region() as *const KoanRegion as usize,
        pre_storage_addr,
        "reuse installed fresh storage",
    );
    // The escaped snapshot is still alive (its retained storage Rc still owns the pre-reset
    // region, allocations intact) — the reset dropped only the shell's reference to it.
    assert!(std::ptr::eq(
        escaped_storage.region() as *const KoanRegion,
        pre_storage_addr as *const KoanRegion
    ));
    assert_eq!(escaped_storage.region().alloc_count(), pre_alloc_count);
}

/// Cycle gate: alloc'ing a value that anchors back at the receiving region via an
/// `Rc<CallFrame>` redirects to the escape region. Without the redirect the per-call
/// region's storage would hold an Rc to itself and never drop.
#[test]
fn alloc_object_redirects_self_anchored_value_to_escape_arena() {
    let outer = KoanRegion::new();
    let scope = default_scope(&outer, Box::new(std::io::sink()));
    let frame: Rc<CallFrame> = CallFrame::new(scope, None);
    // Build a List whose only element is a `KFunction` carrying an
    // `Rc<FrameStorage>` pointing at `frame.region()`. The cycle gate only inspects the
    // carried `Rc`, so the placeholder `KFunction` body is irrelevant.
    let dummy_fn_obj = outer.alloc_object(KObject::KFunction(
        outer.alloc_function(crate::machine::core::kfunction::KFunction::new(
            crate::machine::model::types::ExpressionSignature {
                return_type: crate::machine::model::types::ReturnType::Resolved(
                    crate::machine::model::types::KType::Null,
                ),
                elements: vec![crate::machine::model::types::SignatureElement::Keyword(
                    "DUMMY".into(),
                )],
            },
            crate::machine::core::kfunction::Body::Builtin(|ctx| {
                crate::machine::core::kfunction::action::Action::Done(Ok(
                    crate::machine::model::Carried::Object(
                        ctx.scope
                            .region
                            .alloc_object(crate::machine::model::KObject::Null),
                    ),
                ))
            }),
            scope,
        )),
        None,
    ));
    let f_ref = match dummy_fn_obj {
        KObject::KFunction(f, _) => *f,
        _ => unreachable!(),
    };
    let cyclic_kfn = KObject::KFunction(f_ref, Some(frame.storage_rc()));
    let list = KObject::list(vec![cyclic_kfn]);

    let stored = frame.region().alloc_object(list);
    let stored_ptr = stored as *const KObject<'_>;
    assert!(
        outer.owns_object(stored_ptr),
        "self-anchored alloc should redirect to the escape region (outer)",
    );
    assert!(
        !frame.region().owns_object(stored_ptr),
        "self-anchored value must not land in the per-call region",
    );
}
