//! Targeted Miri coverage for the unsafe sites in this file. Each test pins down a
//! specific aliasing/lifetime shape under tree borrows; logical assertions are minimal
//! — these tests fail when Miri reports UB, not on values.

use super::*;
use crate::builtins::default_scope;
use crate::machine::model::types::KType;
use crate::machine::model::values::{Carried, CarriedFamily, Held, KObject};
use crate::machine::BindingIndex;
use crate::witnessed::{Sealed, Witnessed};
use std::marker::PhantomData;

/// A child `FrameStorage` whose `outer` chains `parent` — the ancestry shape `FrameSet`
/// subsumption walks. Region escape is irrelevant to the `outer`-chain test, so a plain region.
fn child_storage(parent: &Rc<FrameStorage>) -> Rc<FrameStorage> {
    Rc::new(FrameStorage {
        region: KoanRegion::new(),
        outer: Some(Rc::clone(parent)),
    })
}

/// `FrameStorage::pins_region` walks `self` + its `outer` chain: a descendant pins every ancestor's
/// region, never the reverse.
#[test]
fn pins_region_walks_outer_chain() {
    let root = FrameStorage::run_root();
    let child = child_storage(&root);
    assert!(
        child.pins_region(child.region()),
        "self pins its own region"
    );
    assert!(
        child.pins_region(root.region()),
        "descendant pins its ancestor"
    );
    assert!(
        !root.pins_region(child.region()),
        "ancestor does not pin descendant"
    );
}

/// `FrameSet::merge` over related carts collapses to the descendant singleton (the ancestor's region
/// is already pinned by the descendant's `outer` chain), regardless of operand order.
#[test]
fn frameset_merge_subsumes_ancestor() {
    let root = FrameStorage::run_root();
    let child = child_storage(&root);
    let descendant = FrameSet::singleton(Rc::clone(&child));
    let ancestor = FrameSet::singleton(Rc::clone(&root));

    let merged =
        FrameSet::merge(&descendant, &ancestor).expect("a set always represents the union");
    assert_eq!(merged.frames.len(), 1, "ancestor subsumed by descendant");
    assert!(std::ptr::eq(merged.frames[0].region(), child.region()));

    // Order-independent: the antichain is the same either way.
    let merged_rev = FrameSet::merge(&ancestor, &descendant).expect("union always represents");
    assert_eq!(merged_rev.frames.len(), 1);
    assert!(std::ptr::eq(merged_rev.frames[0].region(), child.region()));
}

/// `FrameSet::merge` over unrelated carts keeps both — neither `outer` chain pins the other.
#[test]
fn frameset_merge_keeps_unrelated() {
    let a = FrameStorage::run_root();
    let b = FrameStorage::run_root();
    let merged = FrameSet::merge(&FrameSet::singleton(a), &FrameSet::singleton(b))
        .expect("a set always represents the union");
    assert_eq!(merged.frames.len(), 2, "unrelated regions both kept");
}

/// A singleton set exposes its sole owner's region (the `yoke` seam) and its sole frame.
#[test]
fn frameset_singleton_exposes_region_and_sole() {
    let root = FrameStorage::run_root();
    let set = FrameSet::singleton(Rc::clone(&root));
    assert!(std::ptr::eq(set.region(), root.region()));
    assert!(set.sole().is_some());
    assert!(FrameSet::empty().sole().is_none());
    assert!(FrameSet::empty().is_empty());
}

/// `with_scope` opens the child scope at a `for<'b>` brand — the frame-side read folded onto `open`.
/// A scalar copies out; a bind / lookup consumed in place stays inside the brand (the value is
/// allocated at the same `'b` via the opened scope's own region), so nothing branded escapes.
#[test]
fn with_scope_opens_child_scope_at_brand() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let frame: Rc<CallFrame> = CallFrame::new_test(scope, None);
    // Scalar copy-out: matches `scope_id`.
    let id = frame.with_scope(|s| s.id);
    assert_eq!(id, frame.scope_id());
    // In-place bind + lookup, all at the brand `'b` (value allocated via the opened scope's region).
    frame.with_scope(|s| {
        let v = s.region.alloc_object(KObject::Number(7.0));
        s.bind_value("k".to_string(), v, BindingIndex::BUILTIN)
            .unwrap();
        assert!(matches!(s.lookup("k"), Some(KObject::Number(n)) if *n == 7.0));
    });
}

/// The seed-side re-anchor: a caller-lifetime value relocated into the frame brand region through the
/// substrate (`reattach_with` witnessed by the opened scope's own region — a shortening of the caller
/// lifetime), then bound. The MATCH / TRY `it`-bind and the user-fn param-bind take this shape; pins
/// the relocate-into-the-brand-and-bind aliasing under tree borrows.
#[test]
fn with_scope_relocates_seed_value_into_brand() {
    use crate::witnessed::reattach_with;
    // The caller value is a deep clone of a value resident in its own, longer-lived region —
    // mirroring the matched `it` / a bound arg.
    let caller_region = KoanRegion::new();
    let it_value: KObject<'_> = caller_region
        .alloc_object(KObject::Number(99.0))
        .deep_clone();
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let frame: Rc<CallFrame> = CallFrame::new_test(scope, None);
    frame.with_scope(|child| {
        let relocated = reattach_with::<KObject<'static>, _>(it_value, child.region);
        let it_obj = child.region.alloc_object(relocated);
        child
            .bind_value("it".to_string(), it_obj, BindingIndex::BUILTIN)
            .unwrap();
        assert!(matches!(child.lookup("it"), Some(KObject::Number(n)) if *n == 99.0));
    });
}

/// `SealedExtern::attach` — the now-callerless borrow-bounded scope re-anchor kept for the
/// single-open-verb follow-up. Pins the witness-bounded `reattach_ref_with` shape directly on the
/// frame's child-scope carrier: a borrow capped at the witness `Rc` with a free content lifetime,
/// distinct from `with_scope`'s `for<'b>` brand (borrow == content). Read within the witness borrow,
/// then alloc + bind into the re-anchored scope's region.
#[test]
fn sealed_extern_attach_bounded_reanchor() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let frame: Rc<CallFrame> = CallFrame::new_test(scope, None);
    let carrier = frame.scope_sealed();
    let storage = frame.storage_rc();
    let child: &Scope<'_> = carrier.attach(&storage);
    assert!(child.outer().is_some());
    let v = child.region.alloc_object(KObject::Number(5.0));
    child
        .bind_value("a".to_string(), v, BindingIndex::BUILTIN)
        .unwrap();
    assert!(matches!(child.lookup("a"), Some(KObject::Number(n)) if *n == 5.0));
}

/// The opened child scope's re-borrow stays valid when the region is mutated through a sibling
/// pointer afterward — `with_scope`'s `&Scope` and `region().alloc(...)` must coexist soundly under
/// tree borrows.
#[test]
fn call_frame_scope_survives_subsequent_alloc() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let frame = CallFrame::new_test(scope, None);
    frame.with_scope(|s| {
        let _new = s.region.alloc_object(KObject::Number(1.0));
        assert!(std::ptr::eq(s.region, frame.region()));
    });
}

/// Raw-pointer roundtrip inside the brand: lifetime-anchor an extracted `*const KoanRegion` and
/// `*const Scope<'_>` from the opened child scope, then mutate via one ref while the other stays live.
#[test]
fn call_frame_scope_survives_subsequent_alloc_via_raw_ptr_roundtrip() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let frame: Rc<CallFrame> = CallFrame::new_test(scope, None);
    frame.with_scope(|child| {
        let region_ptr: *const KoanRegion = child.region;
        let scope_ptr: *const Scope<'_> = child;
        let inner_region: &KoanRegion = unsafe { &*(region_ptr as *const _) };
        let child_ref: &Scope<'_> = unsafe { &*(scope_ptr as *const _) };
        let it_obj: &KObject<'_> = inner_region.alloc_object(KObject::Number(42.0));
        child_ref
            .bind_value("it".to_string(), it_obj, BindingIndex::BUILTIN)
            .unwrap();
        assert!(matches!(child_ref.lookup("it"), Some(KObject::Number(n)) if *n == 42.0));
    });
}

/// Two-deep chain: dropping the local `outer` handle leaves only `inner`'s `FrameStorage.outer`
/// keeping the outer region alive while we read through `inner`'s child scope's `outer`.
#[test]
fn call_frame_chained_outer_frame_walkable() {
    let region = FrameStorage::run_root();
    let run_scope = default_scope(&region, Box::new(std::io::sink()));
    let outer = CallFrame::new_test(run_scope, None);
    // Build the inner frame parented on the outer frame's child scope, read at the brand. The
    // returned `Rc<CallFrame>` carries no brand lifetime, so it escapes the open.
    let inner =
        outer.with_scope(|outer_child| CallFrame::new_test(outer_child, Some(outer.storage_rc())));
    drop(outer);
    inner.with_scope(|inner_child| {
        let outer_scope = inner_child
            .outer()
            .expect("inner's child scope must have an outer");
        assert!(std::ptr::eq(
            outer_scope.region,
            inner_child.outer().unwrap().region
        ));
        assert!(outer_scope.outer().is_some());
    });
}

/// Allocating records the stored address into the `membership` side-table via
/// `RefCell::borrow_mut` while a prior `&KObject` from the same region is shared-borrowed.
/// Pins that tree-borrows shape.
#[test]
fn region_alloc_while_prior_ref_live() {
    let a = KoanRegion::new();
    let r1 = a.alloc_object(KObject::Number(1.0));
    let r2 = a.alloc_object(KObject::Number(2.0));
    assert!(matches!(r1, KObject::Number(n) if *n == 1.0));
    assert!(matches!(r2, KObject::Number(n) if *n == 2.0));
}

/// `alloc_ktype` returns a region-lifetime `&KType` and bumps `alloc_count` by one.
#[test]
fn alloc_ktype_returns_region_lifetime_ref_and_counts() {
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
fn call_frame_try_reset_for_tail_round_trip() {
    let outer_region = FrameStorage::run_root();
    let outer_scope = default_scope(&outer_region, Box::new(std::io::sink()));
    let mut frame: Rc<CallFrame> = CallFrame::new_test(outer_scope, None);
    let _pre = frame.region().alloc_object(KObject::Number(1.0));
    assert!(frame.region().alloc_count() >= 1);

    let did_reset = frame.try_reset_for_tail_test(outer_scope);
    assert!(did_reset, "Rc was unique, reset must succeed");

    // Fresh region: only the new child scope remains.
    assert_eq!(frame.region().alloc_count(), 1);

    // After reset, a fresh alloc via the opened scope's region and a bind on that scope coexist.
    frame.with_scope(|child| {
        let v = child.region.alloc_object(KObject::Number(42.0));
        child
            .bind_value("k".to_string(), v, BindingIndex::BUILTIN)
            .unwrap();
        assert!(matches!(child.lookup("k"), Some(KObject::Number(n)) if *n == 42.0));
        assert!(child.outer().is_some());
    });
}

/// `try_reset_for_tail` refuses when another `Rc<CallFrame>` *shell* clone exists — a
/// transient holder still naming the frame, for which in-place reset would mutate the shell
/// under a live alias. (An escaped value pins `FrameStorage`, not the shell — see
/// [`call_frame_try_reset_for_tail_allows_reset_under_escaped_storage`].)
#[test]
fn call_frame_try_reset_for_tail_refuses_when_aliased() {
    let outer_region = FrameStorage::run_root();
    let outer_scope = default_scope(&outer_region, Box::new(std::io::sink()));
    let mut frame: Rc<CallFrame> = CallFrame::new_test(outer_scope, None);
    let pre_region_addr = frame.region() as *const KoanRegion as usize;

    // A second shell holder (not an escape): clone the `Rc<CallFrame>` so strong_count > 1.
    let _alias = Rc::clone(&frame);

    let did_reset = frame.try_reset_for_tail_test(outer_scope);
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
fn call_frame_try_reset_for_tail_allows_reset_under_escaped_storage() {
    let outer_region = FrameStorage::run_root();
    let outer_scope = default_scope(&outer_region, Box::new(std::io::sink()));
    let mut frame: Rc<CallFrame> = CallFrame::new_test(outer_scope, None);
    let _escaped = frame.region().alloc_object(KObject::Number(7.0));
    let pre_alloc_count = frame.region().alloc_count();
    let pre_storage_addr = frame.region() as *const KoanRegion as usize;

    // Simulate a closure escape: hold the frame's storage Rc (what an anchored value carries).
    let escaped_storage = frame.storage_rc();

    let did_reset = frame.try_reset_for_tail_test(outer_scope);
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

/// A per-call frame whose parent is the run root holds **no** strong ref back to the run-root
/// `FrameStorage`: a dispatched frame's `outer` is `None`, so no child→run-root back-edge exists. An
/// escaped value (here, the frame's storage `Rc`) therefore cannot keep the run root alive past its
/// own strong refs, so the run root drops once its own ref is released — which is also what lets a
/// consumer frame retain an escapee's region without forming a cycle.
#[test]
fn per_call_frame_storage_holds_no_strong_ref_to_run_root() {
    let run_root = FrameStorage::run_root();
    let run_root_weak = Rc::downgrade(&run_root);
    // Build a per-call frame under the run root, then keep only its storage `Rc` — the shape an
    // escaped closure pins. The frame shell and the borrowing scope drop at the block boundary.
    let escapee = {
        let scope = default_scope(&run_root, Box::new(std::io::sink()));
        let frame = CallFrame::new_test(scope, None);
        frame.storage_rc()
    };
    assert_eq!(
        Rc::strong_count(&run_root),
        1,
        "the per-call frame's storage must not strong-own its run-root escape target",
    );
    drop(run_root);
    // `escapee` is still held here, yet the run root is gone — the old escape cycle would have kept
    // it alive (the leak); without the stored back-edge it drops.
    assert!(
        run_root_weak.upgrade().is_none(),
        "run root drops once its own strong ref is released — the escaped storage holds no cycle",
    );
    drop(escapee);
}

/// Spike for the [`alloc_witnessed`](super::Region::alloc_witnessed) construction inversion: a value
/// `yoke`d into a frame's region comes back bundled with that frame as its reach witness, co-located
/// by construction. Read back after the original frame handle drops — the bundled witness is the sole
/// owner of the region the carrier's reference points into. This is the region-pure / single-frame
/// shape the object and type families' common case takes.
#[test]
fn alloc_witnessed_yokes_a_co_located_value() {
    let frame = FrameStorage::run_root();
    let w: Witnessed<CarriedFamily, FrameSet> =
        KoanRegion::alloc_witnessed(FrameSet::singleton(Rc::clone(&frame)), |region| {
            Carried::Object(region.alloc_object(KObject::Number(7.0)))
        });
    drop(frame); // the bundled witness now solely owns the region the value lives in.
    let got = w.with(|c| match *c {
        Carried::Object(KObject::Number(n)) => *n,
        _ => panic!("expected a Number object"),
    });
    assert_eq!(got, 7.0);
}

/// Spike for the cross-region `merge` the construction inversion folds a *foreign* region-resident
/// element with (a list/dict element borrowing into another frame's region). The foreign value is
/// `yoke`d in an unrelated frame; merging it into a carrier built here succeeds because `FrameSet` is
/// a *set* witness — it represents the union of two unrelated regions (where a single-region witness
/// returns `None`, cf. `merge_rejects_unrelated_carts` in `witnessed/tests.rs`). After both call
/// handles drop, the merged carrier's witness still pins the foreign backing the bound value points
/// into.
#[test]
fn alloc_witnessed_merge_folds_an_independent_foreign_value() {
    let here_frame = FrameStorage::run_root();
    let foreign_frame = FrameStorage::run_root(); // unrelated — a sibling producer's frame.
    let foreign: Witnessed<CarriedFamily, FrameSet> =
        KoanRegion::alloc_witnessed(FrameSet::singleton(Rc::clone(&foreign_frame)), |r| {
            Carried::Object(r.alloc_object(KObject::Number(1.0)))
        });
    let here: Witnessed<CarriedFamily, FrameSet> =
        KoanRegion::alloc_witnessed(FrameSet::singleton(Rc::clone(&here_frame)), |r| {
            Carried::Object(r.alloc_object(KObject::Number(2.0)))
        });
    // Fold the foreign element in at the shared brand; re-seal under the union of both regions.
    let merged: Witnessed<CarriedFamily, FrameSet> = here
        .merge::<CarriedFamily, CarriedFamily>(
            foreign,
            |_here, foreign, _brand: PhantomData<&_>| foreign,
        )
        .expect("a FrameSet set witness always represents the union of unrelated regions");
    drop(here_frame);
    drop(foreign_frame); // `merged` holds its own clones of both frames.
    let got = merged.with(|c| match *c {
        Carried::Object(KObject::Number(n)) => *n,
        _ => panic!("expected a Number object"),
    });
    assert_eq!(got, 1.0); // the foreign element survived the merge and both handle drops.
}

/// Workload-level accumulator carrier for the aggregate construction fold: the dest region the
/// finished aggregate node lands in, paired with the partial element cells built so far. The
/// production family the object-family construction inversion uses lives in the execute layer; this
/// is the spike stand-in that proves the carrier round-trips and the fold composition is sound.
struct AggBuildFamily;
crate::witnessed::reattachable!(AggBuildFamily => (&'r KoanRegion, Vec<Held<'r>>));

/// Spike for the **aggregate** construction inversion: a list / dict / record built from several dep
/// producers — the shape the object family folds with shipped verbs only (no new substrate
/// primitive). The accumulator is `yoke`d empty over the dest frame's region; each foreign dep's
/// `Sealed` carrier is folded in with [`Sealed::transfer_into`](crate::witnessed::Sealed::transfer_into),
/// which re-anchors it at the shared brand, binds it into the cells, and re-seals under the union of
/// every reached region (a `FrameSet` set witness — the multi-foreign case a single-region witness
/// cannot represent); a final [`map`](Witnessed::map) allocates the list node into the carried region.
/// After every producer handle drops, the folded witness is the sole owner of all three regions the
/// list reaches, so reading the cells back is sound — the proof the construction site names its reach
/// on the one carrier rather than reconstructing it from the value. Mirrors the production fold; fails
/// on UB / leaks, not values.
#[test]
fn alloc_witnessed_fold_builds_a_list_over_independent_foreign_deps() {
    // Two unrelated producer frames, each holding one element — sibling producers whose terminals
    // this consumer aggregates.
    let frame_a = FrameStorage::run_root();
    let frame_b = FrameStorage::run_root();
    let dep_a: Sealed<CarriedFamily, FrameSet> = Sealed::seal(KoanRegion::alloc_witnessed(
        FrameSet::singleton(Rc::clone(&frame_a)),
        |r| Carried::Object(r.alloc_object(KObject::Number(1.0))),
    ));
    let dep_b: Sealed<CarriedFamily, FrameSet> = Sealed::seal(KoanRegion::alloc_witnessed(
        FrameSet::singleton(Rc::clone(&frame_b)),
        |r| Carried::Object(r.alloc_object(KObject::Number(2.0))),
    ));
    // The consumer's own frame: the region the finished list node lands in.
    let dest_frame = FrameStorage::run_root();
    // `yoke` the empty accumulator (the dest region + no cells yet) into the dest frame's region.
    let acc0: Witnessed<AggBuildFamily, FrameSet> = Witnessed::<AggBuildFamily, FrameSet>::yoke(
        FrameSet::singleton(Rc::clone(&dest_frame)),
        |region| (region, Vec::new()),
    );
    // Fold each dep in: bind its re-anchored carrier into the cells (a list element borrows into the
    // foreign region exactly as a surviving closure rides its bare borrow); the witness accumulates
    // the union. `transfer_into` borrows the dep's seal (does not consume it — other consumers keep
    // reading the producer terminal).
    let acc1 = dep_a
        .transfer_into::<AggBuildFamily, AggBuildFamily>(
            acc0,
            |dep, (region, mut cells), _brand| {
                cells.push(Held::from_carried(dep));
                (region, cells)
            },
        )
        .expect("a FrameSet set witness always represents the union");
    let acc2 = dep_b
        .transfer_into::<AggBuildFamily, AggBuildFamily>(
            acc1,
            |dep, (region, mut cells), _brand| {
                cells.push(Held::from_carried(dep));
                (region, cells)
            },
        )
        .expect("a FrameSet set witness always represents the union");
    // Allocate the list node from the carried dest region; the cells ride borrows into both foreign
    // regions, all three now named on this one carrier's witness.
    let list: Witnessed<CarriedFamily, FrameSet> = acc2.map(|(region, cells), _brand| {
        Carried::Object(region.alloc_object(KObject::list_of_held(cells)))
    });
    // Drop every producer handle: the folded witness solely owns all three regions the list reaches.
    drop(frame_a);
    drop(frame_b);
    drop(dest_frame);
    let got = list.with(|c| match c.object() {
        KObject::List(items, _) => items
            .iter()
            .map(|h| match h.object() {
                KObject::Number(n) => *n,
                _ => panic!("expected a Number element"),
            })
            .collect::<Vec<_>>(),
        _ => panic!("expected a List object"),
    });
    assert_eq!(got, vec![1.0, 2.0]); // both foreign elements survived the fold and every handle drop.
}

/// [`FrameSet::fold_foreign`] is the per-scope reach-set's fold: it merges a bound value's carrier
/// witness into the builder but **omits** any frame the scope's home frame already pins, so a resident
/// value never witnesses its own home frame — the `region → scope → set → frame` cycle the reach-set
/// forbids (and the source of the `let rec` self-bind no-op). A same-region (home) singleton folds to
/// nothing; a foreign frame is kept; `None` (a frameless scope with no home to omit) keeps everything.
#[test]
fn fold_foreign_omits_the_home_frame_and_keeps_foreign_reach() {
    let home = FrameStorage::run_root();
    let foreign = FrameStorage::run_root();

    // A same-region value's witness names the home frame itself — folding it contributes no foreign
    // reach (the self-bind / home-frame omission).
    let mut set = FrameSet::empty();
    set.fold_foreign(&FrameSet::singleton(Rc::clone(&home)), Some(&home));
    assert!(
        set.is_empty(),
        "the home frame must be omitted from the reach-set"
    );

    // A foreign frame is kept — the region a bound closure / module borrows into.
    set.fold_foreign(&FrameSet::singleton(Rc::clone(&foreign)), Some(&home));
    assert!(
        set.sole().is_some_and(|f| Rc::ptr_eq(f, &foreign)),
        "a foreign frame must fold into the reach-set",
    );

    // Re-folding the same foreign frame is idempotent (subsumption dedups by region).
    set.fold_foreign(&FrameSet::singleton(Rc::clone(&foreign)), Some(&home));
    assert!(
        set.sole().is_some(),
        "a duplicate fold stays a singleton, not a double entry",
    );

    // With no home frame (a frameless scope owning no escapable region), nothing is omitted.
    let mut frameless = FrameSet::empty();
    frameless.fold_foreign(&FrameSet::singleton(Rc::clone(&home)), None);
    assert!(
        !frameless.is_empty(),
        "with no home frame to omit, the full witness folds in",
    );
}
