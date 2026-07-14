//! Targeted Miri coverage for the unsafe sites in this file. Each test pins down a
//! specific aliasing/lifetime shape under tree borrows; logical assertions are minimal
//! — these tests fail when Miri reports UB, not on values.

use super::*;
use crate::builtins::default_scope;
use crate::builtins::test_support::{delivered_with_host, run_root_bare};
use crate::machine::core::StoredReach;
use crate::machine::model::types::{KType, SigSource};
use crate::machine::model::values::{Carried, CarriedFamily, Held, KObject};
use crate::machine::model::Record;
use crate::machine::BindingIndex;
use crate::machine::CarrierWitness;
use crate::machine::DeliveredCarried;
use crate::machine::KFunction;
use crate::witnessed::{Delivered, Erased, FoldToken, FoldedPlacement, Residence, Witnessed};

/// Test-only destination-region operand: the library's [`RegionHandleFamily`], the
/// `HasRegionHandle` mint target a `merge`/`transfer_into` composition needs — the same family
/// production's `execute::run_loop::DestHandleFamily` aliases.
type BrandFamily = RegionHandleFamily<KoanStorageProfile>;

/// A child `FrameStorage` whose `outer` chains `parent` — the ancestry shape `FrameSet`
/// subsumption walks. Region escape is irrelevant to the `outer`-chain test, so a plain region.
fn child_storage(parent: &Rc<FrameStorage>) -> Rc<FrameStorage> {
    RegionHost::fresh(Some(Rc::clone(parent)))
}

/// `FrameStorage::pins_region` walks `self` + its `outer` chain: a descendant pins every ancestor's
/// region, never the reverse.
#[test]
fn pins_region_walks_outer_chain() {
    let root = run_root_storage();
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

/// `FrameSet::union` over related carts collapses to the descendant singleton (the ancestor's region
/// is already pinned by the descendant's `outer` chain), regardless of operand order.
#[test]
fn frameset_merge_subsumes_ancestor() {
    let root = run_root_storage();
    let child = child_storage(&root);
    let descendant = FrameSet::singleton(Rc::clone(&child));
    let ancestor = FrameSet::singleton(Rc::clone(&root));

    let merged = FrameSet::union(&descendant, &ancestor);
    let [sole] = merged.members() else {
        panic!("ancestor subsumed by descendant");
    };
    assert!(std::ptr::eq(sole.region(), child.region()));

    // Order-independent: the antichain is the same either way.
    let merged_rev = FrameSet::union(&ancestor, &descendant);
    let [sole_rev] = merged_rev.members() else {
        panic!("ancestor subsumed by descendant");
    };
    assert!(std::ptr::eq(sole_rev.region(), child.region()));
}

/// `FrameSet::union` over unrelated carts keeps both — neither `outer` chain pins the other.
#[test]
fn frameset_merge_keeps_unrelated() {
    let a = run_root_storage();
    let b = run_root_storage();
    let merged = FrameSet::union(&FrameSet::singleton(a), &FrameSet::singleton(b));
    assert_eq!(merged.members().len(), 2, "unrelated regions both kept");
}

/// The single-owner `Rc<FrameStorage>` witness (the `yoke` seam) exposes exactly its own region. A
/// singleton `FrameSet` exposes its sole frame; the empty set exposes none.
#[test]
fn single_owner_exposes_region_and_frameset_members() {
    let root = run_root_storage();
    // The `yoke` seam is `WitnessRegion for Rc<FrameStorage>`: a held owner pins exactly one region.
    assert!(std::ptr::eq(WitnessRegion::region(&root), root.region()));
    let set = FrameSet::singleton(Rc::clone(&root));
    assert_eq!(set.members().len(), 1);
    assert!(FrameSet::empty().members().is_empty());
    assert!(FrameSet::empty().is_empty());
}

/// `with_scope` opens the child scope at a `for<'b>` brand. A scalar copies out; a bind / lookup
/// consumed in place stays inside the brand (the value is allocated at the same `'b` via the opened
/// scope's own region), so nothing branded escapes.
#[test]
fn with_scope_opens_child_scope_at_brand() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let frame: Rc<CallFrame> = CallFrame::new(scope);
    // Scalar copy-out: matches `scope_id`.
    let id = frame.with_scope(|s| s.id);
    assert_eq!(id, frame.scope_id());
    // In-place bind + lookup, all at the brand `'b` (value allocated via the opened scope's region).
    frame.with_scope(|s| {
        let v = s.brand().alloc_object(KObject::Number(7.0));
        s.bind_value(
            "k".to_string(),
            v,
            BindingIndex::BUILTIN,
            StoredReach::for_test(None, false),
        )
        .unwrap();
        assert!(matches!(s.lookup("k"), Some(KObject::Number(n)) if *n == 7.0));
    });
}

/// The seed-side re-anchor: a caller-lifetime value relocated into the frame brand region through the
/// substrate (the erasing `alloc_object`, which forgets the caller lifetime and re-homes the value at
/// the opened scope's own region), then bound. The MATCH / TRY `it`-bind and the user-fn param-bind
/// take this shape; pins the relocate-into-the-brand-and-bind aliasing under tree borrows.
#[test]
fn with_scope_relocates_seed_value_into_brand() {
    // The caller value is a deep clone of a value resident in its own, longer-lived region —
    // mirroring the matched `it` / a bound arg.
    let caller_storage = run_root_storage();
    let caller_region = caller_storage.brand();
    let it_value: KObject<'_> = caller_region
        .alloc_object(KObject::Number(99.0))
        .deep_clone();
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let frame: Rc<CallFrame> = CallFrame::new(scope);
    frame.with_scope(|child| {
        // `alloc_object_checked` erases the caller-`'a` input and re-homes it at the frame region,
        // so no pre-shortening is needed; a deep-cloned `Number` is always resident-in-self.
        let it_obj = child
            .brand()
            .alloc_object_checked(it_value)
            .expect("a deep-cloned Number is always resident-in-self");
        child
            .bind_value(
                "it".to_string(),
                it_obj,
                BindingIndex::BUILTIN,
                StoredReach::for_test(None, false),
            )
            .unwrap();
        assert!(matches!(child.lookup("it"), Some(KObject::Number(n)) if *n == 99.0));
    });
}

/// The opened child scope's re-borrow stays valid when the region is mutated through a sibling
/// pointer afterward — `with_scope`'s `&Scope` and `region().alloc(...)` must coexist soundly under
/// tree borrows.
#[test]
fn call_frame_scope_survives_subsequent_alloc() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let frame = CallFrame::new(scope);
    frame.with_scope(|s| {
        let _new = s.brand().alloc_object(KObject::Number(1.0));
        assert!(std::ptr::eq(s.region(), frame.region()));
    });
}

/// Raw-pointer roundtrip inside the brand: lifetime-anchor an extracted `*const KoanRegion` and
/// `*const Scope<'_>` from the opened child scope, then mutate via the scope's brand while the
/// reconstructed region reference stays live.
#[test]
fn call_frame_scope_survives_subsequent_alloc_via_raw_ptr_roundtrip() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let frame: Rc<CallFrame> = CallFrame::new(scope);
    frame.with_scope(|child| {
        let region_ptr: *const KoanRegion = child.region();
        let scope_ptr: *const Scope<'_> = child;
        let inner_region: &KoanRegion = unsafe { &*(region_ptr as *const _) };
        let child_ref: &Scope<'_> = unsafe { &*(scope_ptr as *const _) };
        // Alloc through the reconstructed scope's brand while `inner_region` (the raw-region roundtrip)
        // stays live — the same region under two reconstructed references.
        let it_obj: &KObject<'_> = child_ref.brand().alloc_object(KObject::Number(42.0));
        assert!(std::ptr::eq(inner_region, child_ref.region()));
        child_ref
            .bind_value(
                "it".to_string(),
                it_obj,
                BindingIndex::BUILTIN,
                StoredReach::for_test(None, false),
            )
            .unwrap();
        assert!(matches!(child_ref.lookup("it"), Some(KObject::Number(n)) if *n == 42.0));
    });
}

/// Two-deep chain: dropping the local `outer` handle leaves only `inner`'s `FrameStorage.outer`
/// keeping the outer region alive while we read through `inner`'s child scope's `outer`.
#[test]
fn call_frame_chained_outer_frame_walkable() {
    let region = run_root_storage();
    let run_scope = default_scope(&region, Box::new(std::io::sink()));
    let outer = CallFrame::new(run_scope);
    // The returned `Rc<CallFrame>` carries no brand lifetime, so it escapes the open.
    let inner = outer.with_scope(CallFrame::new);
    drop(outer);
    inner.with_scope(|inner_child| {
        let outer_scope = inner_child
            .outer()
            .expect("inner's child scope must have an outer");
        assert!(std::ptr::eq(
            outer_scope.region(),
            inner_child.outer().unwrap().region()
        ));
        assert!(outer_scope.outer().is_some());
    });
}

/// Derivation, top-level case: a per-call frame built directly under a **root-region** scope chains
/// no ancestor pin. `parent_frame_pin` returns `None` for a root-region scope, so the frame's
/// storage has no `outer` — matching the former hand-passed `outer_frame == None` at top level.
#[test]
fn builtin_frame_at_top_level_chains_nothing() {
    let region = run_root_storage();
    let run_scope = default_scope(&region, Box::new(std::io::sink()));
    assert!(run_scope.parent_frame_pin().is_none());
    let frame = CallFrame::new(run_scope);
    assert!(frame.storage_rc().outer().is_none());
}

/// Derivation, nested case: a per-call frame whose parent scope lives in an ancestor **per-call**
/// region chains that region's owning storage — the pin `parent_frame_pin` reads off the parent
/// scope's own `region_owner`, so a caller cannot mis-wire it.
#[test]
fn builtin_frame_under_per_call_parent_chains_region_owner() {
    let region = run_root_storage();
    let run_scope = default_scope(&region, Box::new(std::io::sink()));
    let outer = CallFrame::new(run_scope);
    let inner = outer.with_scope(|outer_child| {
        // `outer_child` lives in `outer`'s per-call region, so it derives `Some(outer.storage)`.
        assert!(Rc::ptr_eq(
            &outer_child
                .parent_frame_pin()
                .expect("a per-call parent scope pins its region owner"),
            &outer.storage_rc(),
        ));
        CallFrame::new(outer_child)
    });
    assert!(Rc::ptr_eq(
        inner
            .storage_rc()
            .outer()
            .expect("a frame under a per-call parent chains that parent's storage"),
        &outer.storage_rc(),
    ));
}

/// The reserved tail door chains nothing **even** when its parent scope is per-call: a fresh-tail
/// cart strong-owns no ancestor, so tail recursion stays constant-space and no back-edge forms —
/// the one deliberate no-chain shape, distinct from the derived `CallFrame::new`.
#[test]
fn new_tail_chains_nothing_under_per_call_parent() {
    let region = run_root_storage();
    let run_scope = default_scope(&region, Box::new(std::io::sink()));
    let outer = CallFrame::new(run_scope);
    let tail = outer.with_scope(CallFrame::new_tail);
    assert!(tail.storage_rc().outer().is_none());
}

/// Allocating records the stored address into the `membership` side-table via
/// `RefCell::borrow_mut` while a prior `&KObject` from the same region is shared-borrowed.
/// Pins that tree-borrows shape.
#[test]
fn region_alloc_while_prior_ref_live() {
    let storage = run_root_storage();
    let a = storage.brand();
    let r1 = a.alloc_object(KObject::Number(1.0));
    let r2 = a.alloc_object(KObject::Number(2.0));
    assert!(matches!(r1, KObject::Number(n) if *n == 1.0));
    assert!(matches!(r2, KObject::Number(n) if *n == 2.0));
}

/// `alloc_ktype` returns a region-lifetime `&KType` and bumps `alloc_count` by one.
#[test]
fn alloc_ktype_returns_region_lifetime_ref_and_counts() {
    let storage = run_root_storage();
    let a = storage.brand();
    let baseline = a.region().alloc_count();
    let t: &KType = a.alloc_ktype(KType::Number);
    assert!(matches!(t, KType::Number));
    assert_eq!(a.region().alloc_count(), baseline + 1);
}

/// A per-call frame whose parent is the run root holds **no** strong ref back to the run-root
/// `FrameStorage`: a dispatched frame's `outer` is `None`, so no child→run-root back-edge exists. An
/// escaped value (here, the frame's storage `Rc`) therefore cannot keep the run root alive past its
/// own strong refs, so the run root drops once its own ref is released — which is also what lets a
/// consumer frame retain an escapee's region without forming a cycle.
#[test]
fn per_call_frame_storage_holds_no_strong_ref_to_run_root() {
    let run_root = run_root_storage();
    let run_root_weak = Rc::downgrade(&run_root);
    // Build a per-call frame under the run root, then keep only its storage `Rc` — the shape an
    // escaped closure pins. The frame shell and the borrowing scope drop at the block boundary.
    let escapee = {
        let scope = default_scope(&run_root, Box::new(std::io::sink()));
        let frame = CallFrame::new(scope);
        frame.storage_rc()
    };
    assert_eq!(
        Rc::strong_count(&run_root),
        1,
        "the per-call frame's storage must not strong-own its run-root escape target",
    );
    drop(run_root);
    // `escapee` is still held here, yet the run root is gone — a stored child→run-root back-edge would
    // keep it alive (a leak); without one it drops.
    assert!(
        run_root_weak.upgrade().is_none(),
        "run root drops once its own strong ref is released — the escaped storage holds no cycle",
    );
    drop(escapee);
}

/// A value `yoke`d into a frame's region comes back reference-only: the yoke brand proves it is
/// region-derived, and the carrier pins nothing — liveness is the caller's held frame owner (the
/// scheduler's retention hold in production), which the pinned read names. The region-pure /
/// single-frame shape the object and type families' common case takes.
#[test]
fn alloc_witnessed_yokes_a_reference_only_value() {
    let frame = run_root_storage();
    let w: Witnessed<CarriedFamily, CarrierWitness> =
        KoanRegion::alloc_witnessed(Rc::clone(&frame), |region| {
            Carried::Object(region.alloc_object(KObject::Number(7.0)))
        });
    assert!(w.witness().is_empty(), "born reference-only: empty reach");
    // The held `frame` (the retention stand-in) is the pin the read names.
    let got = w.with_pinned(&frame, |c| match *c {
        Carried::Object(KObject::Number(n)) => *n,
        _ => panic!("expected a Number object"),
    });
    assert_eq!(got, 7.0);
}

/// The cross-region envelope transfer folds a *foreign* region-resident element in (a list/dict
/// element borrowing into another frame's region). The foreign value travels as its delivery
/// envelope (host = its producer frame); the `Residence::Kept` transfer mints that producer into
/// the destination's own arena as a reach member. After the producer handle drops, that minted
/// member is the sole owner of the foreign backing the value points into; the destination itself
/// stays pinned by the held `here_frame` (the retention stand-in), which the read names.
#[test]
fn envelope_transfer_folds_an_independent_foreign_value() {
    let here_frame = run_root_storage();
    let foreign_frame = run_root_storage(); // unrelated — a sibling producer's frame.
    let foreign: Witnessed<CarriedFamily, CarrierWitness> =
        KoanRegion::alloc_witnessed(Rc::clone(&foreign_frame), |r| {
            Carried::Object(r.alloc_object(KObject::Number(1.0)))
        });
    // `here_frame`'s own brand is the destination operand: the `HasRegionHandle` mint target the
    // transfer composes against. `foreign`'s value is untouched (still living in `foreign_frame`'s
    // own arena) — only its carrier re-homes: the envelope's host mints into `here_frame`'s arena
    // as a reach member (Kept: the value keeps living there).
    let delivered: DeliveredCarried = Delivered::seal(foreign, Rc::clone(&foreign_frame));
    let here_dest: Witnessed<BrandFamily, CarrierWitness> =
        KoanRegion::yoke_branded::<BrandFamily, _>(Rc::clone(&here_frame), |b| b.handle());
    let merged: Witnessed<CarriedFamily, CarrierWitness> = delivered
        .transfer_into::<BrandFamily, CarriedFamily, _>(
            here_dest,
            Residence::Kept,
            |foreign, _brand, _b: FoldToken<'_>| foreign,
        );
    drop(delivered);
    drop(foreign_frame); // the minted member in `here_frame`'s arena is now the sole foreign owner.
    let got = merged.with_pinned(&here_frame, |c| match *c {
        Carried::Object(KObject::Number(n)) => *n,
        _ => panic!("expected a Number object"),
    });
    assert_eq!(got, 1.0); // the foreign element survived the transfer and the producer handle drop.
}

/// AC bullet 3's walking half: duplicating an envelope for dep delivery is a pure pass-through —
/// it copies the erased value, bit-copies the reference-only carrier, and clones exactly one `Rc`
/// (the envelope's retained host); the reach set itself is never re-minted, so every duplicate's
/// reach pointer is identical to the original's. Fails on UB/leaks under Miri (a re-mint would show
/// up as extra per-member `Rc` traffic on the foreign frame), not on values.
#[test]
fn pass_through_duplicate_keeps_reach_pointer_and_mints_nothing() {
    let foreign_frame = run_root_storage();
    let here_frame = run_root_storage();
    let foreign: Witnessed<CarriedFamily, CarrierWitness> =
        KoanRegion::alloc_witnessed(Rc::clone(&foreign_frame), |r| {
            Carried::Object(r.alloc_object(KObject::Number(1.0)))
        });
    let here_dest: Witnessed<BrandFamily, CarrierWitness> =
        KoanRegion::yoke_branded::<BrandFamily, _>(Rc::clone(&here_frame), |b| b.handle());
    let merged: Witnessed<CarriedFamily, CarrierWitness> =
        Delivered::seal(foreign, Rc::clone(&foreign_frame))
            .transfer_into::<BrandFamily, CarriedFamily, _>(
                here_dest,
                Residence::Kept,
                |foreign, _brand, _b: FoldToken<'_>| foreign,
            );

    let reach_ptr = merged
        .witness()
        .with_reach(Some(&here_frame), |r| r.map(|set| set as *const _));
    let here_count_before = Rc::strong_count(&here_frame);
    let foreign_count_before = Rc::strong_count(&foreign_frame);

    // The walking motion — dep delivery duplicates a producer slot's envelope for each consumer.
    let envelope: DeliveredCarried = Delivered::seal(merged, Rc::clone(&here_frame));
    let here_count_before = here_count_before + 1; // the envelope itself holds one host clone.
    let copy_a = envelope.duplicate();
    let copy_b = envelope.duplicate();

    for copy in [&copy_a, &copy_b] {
        let copy_ptr = copy
            .witness()
            .with_reach(Some(copy.host()), |r| r.map(|set| set as *const _));
        assert_eq!(
            copy_ptr, reach_ptr,
            "duplicating rides the same reach set by reference -- no re-mint"
        );
    }
    assert_eq!(
        Rc::strong_count(&here_frame),
        here_count_before + 2,
        "one host Rc clone per duplicate, nothing more"
    );
    assert_eq!(
        Rc::strong_count(&foreign_frame),
        foreign_count_before,
        "the reach set itself is a reference copy -- no per-member refcount traffic on the \
         foreign frame"
    );
}

/// Workload-level accumulator carrier for the aggregate construction fold: the dest region the
/// finished aggregate node lands in, paired with the partial element cells built so far. The
/// production family the object-family construction inversion uses lives in the execute layer; this
/// is the spike stand-in that proves the carrier round-trips and the fold composition is sound.
struct AggBuildFamily;
crate::witnessed::reattachable!(AggBuildFamily => (RegionHandle<'r, KoanStorageProfile>, Vec<Held<'r>>));

/// The **aggregate** construction fold: a list / dict / record built from several dep producers —
/// the shape the object family folds with shipped verbs only (no new substrate primitive). The
/// accumulator is `yoke`d empty over the dest frame's region; each foreign dep's
/// `Delivered` envelope is folded in with
/// [`Delivered::transfer_into`](crate::witnessed::Delivered::transfer_into), which re-anchors it at
/// the shared brand, binds it into the cells, and re-seals under the union of
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
    let frame_a = run_root_storage();
    let frame_b = run_root_storage();
    let dep_a: DeliveredCarried = Delivered::seal(
        KoanRegion::alloc_witnessed(Rc::clone(&frame_a), |r| {
            Carried::Object(r.alloc_object(KObject::Number(1.0)))
        }),
        Rc::clone(&frame_a),
    );
    let dep_b: DeliveredCarried = Delivered::seal(
        KoanRegion::alloc_witnessed(Rc::clone(&frame_b), |r| {
            Carried::Object(r.alloc_object(KObject::Number(2.0)))
        }),
        Rc::clone(&frame_b),
    );
    // The consumer's own frame: the region the finished list node lands in.
    let dest_frame = run_root_storage();
    // `yoke` the empty accumulator (the dest region + no cells yet) into the dest frame's region.
    let acc0: Witnessed<AggBuildFamily, CarrierWitness> =
        KoanRegion::yoke_branded::<AggBuildFamily, _>(Rc::clone(&dest_frame), |region| {
            (region.handle(), Vec::new())
        });
    // Fold each dep in: bind its re-anchored carrier into the cells (a list element borrows into the
    // foreign region exactly as a surviving closure rides its bare borrow); the witness accumulates
    // the union. `transfer_into` borrows the dep's seal (does not consume it — other consumers keep
    // reading the producer terminal).
    let acc1 = dep_a.transfer_into::<AggBuildFamily, AggBuildFamily, _>(
        acc0,
        Residence::Kept,
        |dep, (region, mut cells), _brand| {
            cells.push(Held::from_carried(dep));
            (region, cells)
        },
    );
    let acc2 = dep_b.transfer_into::<AggBuildFamily, AggBuildFamily, _>(
        acc1,
        Residence::Kept,
        |dep, (region, mut cells), _brand| {
            cells.push(Held::from_carried(dep));
            (region, cells)
        },
    );
    // Allocate the list node from the carried dest region; the cells ride borrows into both foreign
    // regions, both now minted as members into the dest arena.
    let list: Witnessed<CarriedFamily, CarrierWitness> =
        acc2.map_pinned(&dest_frame, |(region, cells), _token| {
            let region = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(region));
            Carried::Object(region.alloc_object_folded(KObject::list_of_held(cells)))
        });
    // Drop the producer handles: the dest arena's minted set solely owns both foreign regions; the
    // dest region itself rides the held `dest_frame` (the retention stand-in), which the read names.
    drop(frame_a);
    drop(frame_b);
    let got = list.with_pinned(&dest_frame, |c| match c.object() {
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

/// [`FrameSet::fold_omitting`] is the per-scope reach-set's fold: it merges a bound value's carrier
/// witness into the builder but **omits** any frame the scope's home frame already pins, so a resident
/// value never witnesses its own home frame — the `region → scope → set → frame` cycle the reach-set
/// forbids (and the source of the `let rec` self-bind no-op). A same-region (home) singleton folds to
/// nothing; a foreign frame is kept; an always-false predicate (a frameless scope with no home to omit)
/// keeps everything.
#[test]
fn fold_omitting_skips_the_home_frame_and_keeps_foreign_reach() {
    let home = run_root_storage();
    let foreign = run_root_storage();

    // A same-region value's witness names the home frame itself — folding it contributes no foreign
    // reach (the self-bind / home-frame omission).
    let mut set = FrameSet::empty();
    set.fold_omitting(&FrameSet::singleton(Rc::clone(&home)), |region| {
        home.pins_region(region)
    });
    assert!(
        set.is_empty(),
        "the home frame must be omitted from the reach-set"
    );

    // A foreign frame is kept — the region a bound closure / module borrows into.
    set.fold_omitting(&FrameSet::singleton(Rc::clone(&foreign)), |region| {
        home.pins_region(region)
    });
    assert!(
        matches!(set.members(), [only] if Rc::ptr_eq(only, &foreign)),
        "a foreign frame must fold into the reach-set",
    );

    // Re-folding the same foreign frame is idempotent (subsumption dedups by region).
    set.fold_omitting(&FrameSet::singleton(Rc::clone(&foreign)), |region| {
        home.pins_region(region)
    });
    assert_eq!(
        set.members().len(),
        1,
        "a duplicate fold stays a singleton, not a double entry",
    );

    // With no home frame to omit (a frameless scope owning no escapable region), nothing is omitted.
    let mut frameless = FrameSet::empty();
    frameless.fold_omitting(&FrameSet::singleton(Rc::clone(&home)), |_region| false);
    assert!(
        !frameless.is_empty(),
        "with no home frame to omit, the full witness folds in",
    );
}

/// The brand-confined [`Region::alloc`] engine hands the freshly-stored value to its closure at a
/// `for<'b>` brand and lets only the erased carrier escape (an empty-witnessed [`Witnessed`], no
/// `'b`); a sibling alloc into the same region after the store coexists under tree borrows — the
/// closure-surface twin of [`region_alloc_while_prior_ref_live`]. The escaped carrier reads back while
/// its region backing is live.
#[test]
fn alloc_engine_brand_coexists_with_sibling_alloc() {
    let storage = run_root_storage();
    // `alloc_object_witnessed` routes the engine's brand-confined `alloc`, storing `value` and
    // letting only the erased carrier escape — `Witnessed::resident` (the empty-witness constructor)
    // names no `'b`.
    let carrier: StepCarried = storage.brand().alloc_object_witnessed(KObject::Number(1.0));
    // A sibling alloc into the same region coexists — the membership-table write and the prior store
    // do not alias under tree borrows.
    let sibling = storage.brand().alloc_object(KObject::Number(2.0));
    // Read the escaped carrier back while `storage` (its backing) is live — the pin the read names.
    let got = carrier.inspect_pinned(&storage, |c| match *c {
        Carried::Object(KObject::Number(n)) => *n,
        _ => panic!("expected a Number object"),
    });
    assert_eq!(got, 1.0);
    assert!(matches!(sibling, KObject::Number(n) if *n == 2.0));
}

/// The reference-only carrier at the Done boundary: a region-pure carrier pins **nothing**, sound
/// because the scheduler seeds a retention hold on the producer's *storage* at finalize and every
/// read opens under it. This pins that shape across the producer shell's drop: seal the carrier
/// as-is into its delivery envelope (host = the storage `Rc`, the retention hold's stand-in), then
/// drop the producer shell outright (a `FreshTail` tail hop never resets a shell in place — it
/// mints a fresh cart and drops the retiring one) — the retained storage keeps the region (where
/// the value lives) alive, so opening the envelope after the drop reads a live pointee, not a
/// freed one. Without the hold the empty carrier would pin nothing and the drop would free the
/// region under the stored carrier.
#[test]
fn reference_only_carrier_survives_producer_shell_drop_under_retention_hold() {
    let outer_region = run_root_storage();
    let outer_scope = default_scope(&outer_region, Box::new(std::io::sink()));
    let frame: Rc<CallFrame> = CallFrame::new(outer_scope);

    // Born reference-only: the active frame is excluded at the alloc site.
    let carrier: StepCarried = frame.brand().alloc_object_witnessed(KObject::Number(7.0));
    assert!(
        carrier.reach_is_empty(),
        "a region-pure carrier is born under the empty reach",
    );

    // The finalize shape: seal as-is; the retention hold (the producer's storage Rc) rides the
    // delivery envelope, never the carrier.
    let envelope: DeliveredCarried = carrier.seal_for_test(frame.storage_rc());

    // Drop the producer shell outright — the envelope holds the *storage* Rc, not the shell,
    // so the region stays alive under the drop.
    drop(frame);

    // The pointee is still live: the retained storage held the region across the shell's drop, so
    // opening the envelope reads a valid value rather than a freed one.
    let got = envelope.open(|c| match c {
        Carried::Object(KObject::Number(n)) => *n,
        _ => panic!("expected a Number object"),
    });
    assert_eq!(got, 7.0);
}

/// A `KObject::KFunction` whose captured scope lives in `home`'s own region — a closure value genuinely
/// reaching that per-call region, so dereferencing the returned `&KObject` (its inner `&KFunction`, or
/// that function's captured scope) touches the region's memory. Both the function and its wrapping
/// object land in `home`'s region; the body is never run. Mirrors `alloc_local_kf` in the lift slate.
fn alloc_home_closure<'run>(home: &'run Rc<CallFrame>) -> &'run KObject<'run> {
    // Capture `home`'s child scope (read at the brand), alloc the closure into `home`'s own region —
    // where that scope lives — and wrap it as a `KObject::KFunction` in the same region, so the escaping
    // `&KObject` reaches exactly that region.
    home.with_scope(|child| {
        let kf_ref = home.brand().alloc_function(no_op_closure(child));
        home.brand()
            .alloc_object_checked(KObject::KFunction(kf_ref))
            .expect("f was just allocated into region\'s own region")
    })
}

/// A no-op `KFunction` capturing `scope` — the closure value the multi-region shapes fold; the body
/// is never run.
fn no_op_closure<'x>(captured: &'x Scope<'x>) -> KFunction<'x> {
    use crate::machine::core::kfunction::action::Action;
    use crate::machine::model::{ExpressionSignature, ReturnType, SignatureElement};
    use crate::machine::Body;
    KFunction::new(
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Null),
            elements: vec![SignatureElement::Keyword("__INNER__".into())],
        },
        Body::Builtin(|ctx| {
            Action::done_resident(Carried::Object(
                ctx.scope.brand().alloc_object(KObject::Null),
            ))
        }),
        captured,
        None,
        None,
    )
}

/// A closure carrier in its delivery envelope — the value reference-only (region-pure in its home
/// frame, so its carrier is empty) and the envelope's host the home frame's storage, the retention
/// hold's stand-in. A closure can't be `yoke`d — yoke's `for<'b>` build closure can't capture the
/// frame's existing scope, and minting a fresh one needs the frame's storage `Rc` a `for<'b>`
/// forbids — so the erased pairing here mirrors production's resident seal.
fn delivered_closure(home: &Rc<CallFrame>) -> DeliveredCarried {
    Delivered::seal(
        Witnessed::from_erased(
            Erased::erase(Carried::Object(alloc_home_closure(home))),
            CarrierWitness::default(),
        ),
        home.storage_rc(),
    )
}

/// A closure element as the LET-bind → entry-re-read pipeline delivers it: the closure lives whole in
/// `home` (its captured scope co-located, `alloc_function`'s invariant), and a *reader* scope in a
/// different region binds it — `host_reach_of` mints `home` into the reader's arena as the entry's
/// stored reach, the re-read seal (`resident_value_carrier`) rides that reach, and the element's
/// envelope host is the reader's frame. The closure's captured scope is thus foreign to both the
/// element's host and any destination the element folds into: its region rides the element's *reach*,
/// never its residence host — the pin `host_reach_of` documents for a closure's captured scope (a
/// per-call frame carries no storage `outer` under TCO).
fn delivered_reread_closure<'run>(
    home: &'run Rc<FrameStorage>,
    reader: &'run Rc<FrameStorage>,
    reader_scope: &'run Scope<'run>,
) -> DeliveredCarried {
    let home_scope = run_root_bare(home);
    let kf_ref = home.brand().alloc_function(no_op_closure(home_scope));
    let obj = home
        .brand()
        .alloc_object_checked(KObject::KFunction(kf_ref))
        .expect("closure co-located with its captured scope");
    // The bind-time mint: `home` materializes into the reader's arena as the entry's stored reach.
    let bind_cell = delivered_with_host(Carried::Object(obj), Rc::clone(home));
    let stored = reader_scope.host_reach_of(&bind_cell);
    drop(bind_cell);
    Delivered::seal(
        reader_scope.resident_value_carrier(obj, stored),
        Rc::clone(reader),
    )
}

/// Record-fold accumulator family: the dest region plus the named field cells built so far — the record
/// twin of [`AggBuildFamily`]. Each closure cell `transfer_into`s (a `merge`) its value and reach onto
/// the accumulator; the final `map` builds the record from the region.
struct RecordCellFamily;
crate::witnessed::reattachable!(RecordCellFamily => (RegionHandle<'r, KoanStorageProfile>, Vec<(String, Held<'r>)>));

/// **Multi-region shape 1 — a list of closures whose captured scopes are foreign to every element
/// host.** Each element rides the LET-bind → entry-re-read pipeline ([`delivered_reread_closure`]):
/// the closure lives in its own home region, a reader frame's arena holds the minted entry reach
/// naming that home, and the element's envelope host is the reader — so the closure regions ride the
/// elements' *reach sets*, never their residence hosts. Each `transfer_into` must union that reach
/// onto the accumulator (host materialization alone covers only the readers). Every home and reader
/// frame is then freed and each closure's captured scope read back — a use-after-free the instant the
/// fold drops a reach member (residence-only folding would free both closure regions). Fails on UB,
/// not values.
#[test]
fn multi_region_list_of_closures_survives_frame_free() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    // Two closure homes and two reader frames — four distinct regions, no shared ancestry, each
    // dying on its own — plus the dest the list node lands in.
    let home_a = run_root_storage();
    let home_b = run_root_storage();
    let reader_a = run_root_storage();
    let reader_a_scope = run_root_bare(&reader_a);
    let reader_b = run_root_storage();
    let reader_b_scope = run_root_bare(&reader_b);
    let dest_frame: Rc<CallFrame> = CallFrame::new(scope); // the list node lands here.

    let acc0 = KoanRegion::yoke_branded::<AggBuildFamily, _>(dest_frame.storage_rc(), |region| {
        (region.handle(), Vec::new())
    });
    // Fold each re-read element into the accumulator; the temporary source carrier drops after each
    // statement, leaving only the aggregate witness (reach union + materialized reader hosts)
    // holding the four regions.
    let acc1 = delivered_reread_closure(&home_a, &reader_a, reader_a_scope)
        .transfer_into::<AggBuildFamily, AggBuildFamily, _>(
            acc0,
            Residence::Kept,
            |dep, (region, mut cells), _brand| {
                cells.push(Held::from_carried(dep));
                (region, cells)
            },
        );
    let acc2 = delivered_reread_closure(&home_b, &reader_b, reader_b_scope)
        .transfer_into::<AggBuildFamily, AggBuildFamily, _>(
            acc1,
            Residence::Kept,
            |dep, (region, mut cells), _brand| {
                cells.push(Held::from_carried(dep));
                (region, cells)
            },
        );
    // The retention stand-in: the dest frame's storage, held past the shell drops below — the hold
    // the scheduler seeds at finalize.
    let dest_storage = dest_frame.storage_rc();
    let list: Witnessed<CarriedFamily, CarrierWitness> =
        acc2.map_pinned(&dest_storage, |(region, cells), _token| {
            let region = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(region));
            Carried::Object(region.alloc_object_folded(KObject::list_of_held(cells)))
        });

    // Free every home and reader shell: the dest arena's minted set (the unioned closure homes plus
    // the materialized readers) and the retained dest storage are now the sole owners of all five
    // regions. Drop any one member and the read below touches freed memory.
    drop(home_a);
    drop(home_b);
    drop(reader_a);
    drop(reader_b);
    drop(dest_frame);

    // Read every closure's captured scope back — each deref rides a `&KFunction` in its (now
    // mint-only-pinned) region.
    let ids: Vec<_> = list.with_pinned(&dest_storage, |c| match c.object() {
        KObject::List(items, _) => items
            .iter()
            .map(|h| match h.object() {
                KObject::KFunction(f) => f.captured_scope().id,
                other => panic!("expected a KFunction cell, got {}", other.ktype().name()),
            })
            .collect(),
        other => panic!("expected a List, got {}", other.ktype().name()),
    });
    assert_eq!(
        ids.len(),
        2,
        "both closures read back after their frames freed"
    );
}

/// **Multi-region shape 2 — a closure capturing closures across several regions (the reach tree).** The
/// outer closure captures a scope binding two inner closures, each home to its own region; its reach
/// branches into three independent lineages, flattened into the witness union. Every frame is freed and
/// the outer closure followed through its bindings to each inner closure's captured scope — a
/// use-after-free the moment an inner region is dropped from the union. Fails on UB, not values.
#[test]
fn multi_region_closure_capturing_closures_survives_frame_free() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    // A capturing frame and two capture-target frames — three distinct regions forming a reach tree.
    let frame_outer: Rc<CallFrame> = CallFrame::new(scope);
    let frame_1: Rc<CallFrame> = CallFrame::new(scope);
    let frame_2: Rc<CallFrame> = CallFrame::new(scope);

    // Fold the two inner closures into a list carrier over frame_outer's region — its witness derives to
    // {frame_outer, frame_1, frame_2} through the fold, never a hand-assembled union.
    let acc0 = KoanRegion::yoke_branded::<AggBuildFamily, _>(frame_outer.storage_rc(), |region| {
        (region.handle(), Vec::new())
    });
    let acc1 = delivered_closure(&frame_1).transfer_into::<AggBuildFamily, AggBuildFamily, _>(
        acc0,
        Residence::Kept,
        |dep, (region, mut cells), _brand| {
            cells.push(Held::from_carried(dep));
            (region, cells)
        },
    );
    let acc2 = delivered_closure(&frame_2).transfer_into::<AggBuildFamily, AggBuildFamily, _>(
        acc1,
        Residence::Kept,
        |dep, (region, mut cells), _brand| {
            cells.push(Held::from_carried(dep));
            (region, cells)
        },
    );
    // The outer closure (born region-pure in frame_outer) `merge`s the still-`AggBuildFamily`-typed
    // accumulator directly — so the destination region (needed to allocate the list) and the
    // accumulated reach (frame_1 ∪ frame_2, needed for the composed witness) arrive together, rather
    // than collapsing to a bare `CarriedFamily` value first (which would carry no `HasRegionHandle`
    // mint target). The merged witness re-homes onto the outer frame with the list's reach folded
    // in, so the outer closure now reaches frame_1 / frame_2 through the bound list (the reach tree).
    let outer_storage = frame_outer.storage_rc();
    let captured: Witnessed<CarriedFamily, CarrierWitness> = delivered_closure(&frame_outer)
        .transfer_into_placing::<AggBuildFamily, CarriedFamily, _>(
        acc2,
        Residence::Kept,
        |outer_v, (_region, cells), placement| {
            let region = FoldingBrand::in_fold_closure(placement);
            if let KObject::KFunction(kf) = outer_v.object() {
                let list_obj = region.alloc_object_folded(KObject::list_of_held(cells));
                kf.captured_scope()
                    .bind_value(
                        "inners".to_string(),
                        list_obj,
                        BindingIndex::BUILTIN,
                        StoredReach::for_test(None, false),
                    )
                    .expect("bind the inners list into the outer closure's scope");
            }
            outer_v
        },
    );

    drop(frame_outer);
    drop(frame_1);
    drop(frame_2);

    // Follow the outer closure's captured scope to the bound list and deref each inner closure's
    // captured scope — touching all three regions after they would have died without the minted
    // members plus the retained outer storage (the retention stand-in the read names).
    let ids: Vec<_> = captured.with_pinned(&outer_storage, |c| match c.object() {
        KObject::KFunction(outer) => match outer.captured_scope().lookup("inners") {
            Some(KObject::List(items, _)) => items
                .iter()
                .map(|h| match h.object() {
                    KObject::KFunction(f) => f.captured_scope().id,
                    other => panic!("expected a KFunction cell, got {}", other.ktype().name()),
                })
                .collect(),
            _ => panic!("`inners` must be bound to a list of closures"),
        },
        other => panic!("expected a KFunction, got {}", other.ktype().name()),
    });
    assert_eq!(
        ids.len(),
        2,
        "both inner closures reached through the captured scope after frames freed",
    );
}

/// **Multi-region shape 3 — a record whose field values reach distinct regions.** An owned record
/// `{a, b}` whose two field cells ride bare `&KFunction` borrows into separate per-call regions; its
/// witness is the union of both. Both frames are freed and each field's closure read back — a
/// use-after-free if either field's region is dropped from the union. Fails on UB, not values.
#[test]
fn multi_region_record_of_closures_survives_frame_free() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    // Two independent frames whose closures the record's fields reach, plus the dest it lands in.
    let frame_a: Rc<CallFrame> = CallFrame::new(scope);
    let frame_b: Rc<CallFrame> = CallFrame::new(scope);
    let dest_frame: Rc<CallFrame> = CallFrame::new(scope);

    // Fold each field's closure into a named-cell accumulator over the dest region; the record's witness
    // derives to {dest ∪ frame_a ∪ frame_b} through the fold, never a hand-assembled union.
    let acc0 = KoanRegion::yoke_branded::<RecordCellFamily, _>(dest_frame.storage_rc(), |region| {
        (region.handle(), Vec::new())
    });
    let acc1 = delivered_closure(&frame_a).transfer_into::<RecordCellFamily, RecordCellFamily, _>(
        acc0,
        Residence::Kept,
        |dep, (region, mut cells), _brand| {
            cells.push(("a".to_string(), Held::from_carried(dep)));
            (region, cells)
        },
    );
    let acc2 = delivered_closure(&frame_b).transfer_into::<RecordCellFamily, RecordCellFamily, _>(
        acc1,
        Residence::Kept,
        |dep, (region, mut cells), _brand| {
            cells.push(("b".to_string(), Held::from_carried(dep)));
            (region, cells)
        },
    );
    let dest_storage = dest_frame.storage_rc();
    let record: Witnessed<CarriedFamily, CarrierWitness> =
        acc2.map_pinned(&dest_storage, |(region, cells), _token| {
            let region = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(region));
            Carried::Object(
                region.alloc_object_folded(KObject::record_of_held(Record::from_pairs(cells))),
            )
        });

    drop(frame_a);
    drop(frame_b);
    drop(dest_frame);

    // Read each field's closure back, dereferencing its captured scope — a use-after-free if either
    // field's region were dropped from the minted set (the retained dest storage pins the rest).
    let ids: Vec<_> = record.with_pinned(&dest_storage, |c| match c.object() {
        KObject::Record(fields, _) => fields
            .values()
            .map(|h| match h.object() {
                KObject::KFunction(f) => f.captured_scope().id,
                other => panic!("expected a KFunction field, got {}", other.ktype().name()),
            })
            .collect(),
        other => panic!("expected a Record, got {}", other.ktype().name()),
    });
    assert_eq!(
        ids.len(),
        2,
        "both record fields read back after their frames freed"
    );
}

/// A `KFunction` plus a `KType::KFunctor { body: Some(&f), .. }` wrapping it, both resident in
/// `home`'s own region — the stand-in for a dep terminal's `t.value`/`t.carrier` pair (a bound
/// functor whose `body` names the callable). Mirrors [`alloc_home_closure`]'s construction, but
/// returns the *type*, since it is the functor type's `body` borrow the fold closes a hole around.
fn alloc_home_functor_type<'run>(home: &'run Rc<CallFrame>) -> &'run KType<'run> {
    use crate::machine::core::kfunction::action::Action;
    use crate::machine::model::{ExpressionSignature, ReturnType, SignatureElement};
    use crate::machine::{Body, KFunction};
    home.with_scope(|child| {
        let kf = KFunction::new(
            ExpressionSignature {
                return_type: ReturnType::Resolved(KType::Null),
                elements: vec![SignatureElement::Keyword("__INNER__".into())],
            },
            Body::Builtin(|ctx| {
                Action::done_resident(Carried::Object(
                    ctx.scope.brand().alloc_object(KObject::Null),
                ))
            }),
            child,
            None,
            None,
        );
        let kf_ref: &KFunction = home.brand().alloc_function(kf);
        let kt = KType::functor_type(Record::new(), Box::new(KType::Null), Some(kf_ref));
        home.brand()
            .alloc_ktype_checked(kt)
            .expect("kf_ref was just allocated into home's own region")
    })
}

/// **`alloc_type_of`'s reach fold, exercised through the actual finish-surface helper.** A dep
/// terminal's `KType::KFunctor { body: Some(&f) }` — the stand-in for `t.value`/`t.carrier` — is
/// sealed as the step's own carrier via `alloc_type_of`, rebuilt at the fold brand from the dep's
/// view in a *different* frame's region. The fold unions the producer's reach into the result's
/// witness; every producer-frame handle then drops, and reading the sealed functor body must not
/// dangle. Fails on UB, not values — the closing case for the reach hole `alloc_type` (no fold)
/// leaves open.
#[test]
fn functor_field_reach_fold_survives_producer_frame_free() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));

    // Producer: a KFunctor type (wrapping a KFunction) resident in its own frame's region — the
    // stand-in for a dep terminal delivered to the finish.
    let producer_frame: Rc<CallFrame> = CallFrame::new(scope);
    let kt: &KType = alloc_home_functor_type(&producer_frame);
    let expected_id = match kt {
        KType::KFunctor { body: Some(f), .. } => f.captured_scope().id,
        other => panic!("expected a KFunctor with a body, got {}", other.name()),
    };
    let dep: DeliveredCarried = Delivered::seal(
        Witnessed::from_erased(Erased::erase(Carried::Type(kt)), CarrierWitness::default()),
        producer_frame.storage_rc(),
    );

    // Consumer: a StepContext over a *different* frame — the finish surface's own region.
    // `alloc_type_of` rebuilds `kt` at the brand from the dep's view and folds the producer's reach.
    let consumer_frame: Rc<CallFrame> = CallFrame::new(scope);
    let ctx = StepAllocator::over_frame(consumer_frame.storage_rc());
    let sealed: StepCarried = ctx.alloc_type_of(&dep);

    // Drop the dep envelope and every frame shell: only the fold (if it happened) keeps the
    // producer's region alive, through the set minted into the consumer arena — itself pinned by
    // the retained consumer storage (the retention stand-in the read names).
    let consumer_storage = consumer_frame.storage_rc();
    drop(dep);
    drop(producer_frame);
    drop(consumer_frame);

    // Read back through the sealed carrier into the functor's captured scope.
    let read_id = sealed.inspect_pinned(&consumer_storage, |c| match c {
        Carried::Type(KType::KFunctor { body: Some(f), .. }) => f.captured_scope().id,
        Carried::Type(other) => panic!("expected a KFunctor with a body, got {}", other.name()),
        other => panic!("expected a KFunctor type, got {}", other.summarize()),
    });
    assert_eq!(
        read_id, expected_id,
        "functor type read back after producer frame freed"
    );
}

/// **`alloc_type_of`'s scalar gate.** A region-free scalar type (`Number`) delivered as a dep
/// terminal seals with an **empty** reach — it references no region, so the fold is skipped and the
/// carrier pins nothing, even though the dep envelope names a producer frame. The complement of the
/// fold arm above: reach = own region ∪ dep reach for a region-reaching type, empty for a scalar.
#[test]
fn alloc_type_of_scalar_gate_seals_empty_reach() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));

    // A region-free scalar type sealed as a dep terminal, its envelope naming a producer frame.
    let producer_frame: Rc<CallFrame> = CallFrame::new(scope);
    let kt: &KType = producer_frame.brand().alloc_ktype(KType::Number);
    let dep: DeliveredCarried = Delivered::seal(
        Witnessed::from_erased(Erased::erase(Carried::Type(kt)), CarrierWitness::default()),
        producer_frame.storage_rc(),
    );

    let consumer_frame: Rc<CallFrame> = CallFrame::new(scope);
    let ctx = StepAllocator::over_frame(consumer_frame.storage_rc());
    let sealed: StepCarried = ctx.alloc_type_of(&dep);

    assert!(
        sealed.reach_is_empty(),
        "a region-free scalar type folds no dep: empty reach"
    );
    // The scalar value rebuilds owned at the brand, so the sealed carrier is the same `Number`.
    let is_number = sealed.inspect_pinned(&consumer_frame.storage_rc(), |c| {
        matches!(c, Carried::Type(KType::Number))
    });
    assert!(is_number, "alloc_type_of seals the scalar's own value");
}

/// **`alloc_type_composed`'s correlation, exercised through the actual door.** A mixed operand
/// list — a region-free `Pure` value at position 0, a region-reaching `Reaching` carrier at
/// position 1 — composes into a `Dict` whose `k`/`v` land at the same positions as the operands,
/// and the `v` side (the dep's functor type) survives dropping the dep envelope and the producer
/// frame: the fold, not ambient capture, is what keeps it alive. This is the pin the builtin-level
/// tests can't provide — an ambient capture would reproduce the same read-back surface without the
/// reach-fold, so only this door-level test distinguishes the two.
#[test]
fn alloc_type_composed_correlates_mixed_operands() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));

    let producer_frame: Rc<CallFrame> = CallFrame::new(scope);
    let kt: &KType = alloc_home_functor_type(&producer_frame);
    let expected_id = match kt {
        KType::KFunctor { body: Some(f), .. } => f.captured_scope().id,
        other => panic!("expected a KFunctor with a body, got {}", other.name()),
    };
    let dep: DeliveredCarried = Delivered::seal(
        Witnessed::from_erased(Erased::erase(Carried::Type(kt)), CarrierWitness::default()),
        producer_frame.storage_rc(),
    );

    let consumer_frame: Rc<CallFrame> = CallFrame::new(scope);
    let ctx = StepAllocator::over_frame(consumer_frame.storage_rc());
    let operands = vec![
        TypeOperand::Pure(KType::Number),
        TypeOperand::Reaching(&dep),
    ];
    let composed: StepCarried = ctx.alloc_type_composed(operands, |_brand, parts| {
        KType::dict(Box::new(parts[0].clone()), Box::new(parts[1].clone()))
    });

    let consumer_storage = consumer_frame.storage_rc();
    drop(dep);
    drop(producer_frame);
    drop(consumer_frame);

    let (k_is_number, read_id) = composed.inspect_pinned(&consumer_storage, |c| match c {
        Carried::Type(KType::Dict {
            key: k, value: v, ..
        }) => {
            let k_is_number = matches!(k.as_ref(), KType::Number);
            let id = match v.as_ref() {
                KType::KFunctor { body: Some(f), .. } => f.captured_scope().id,
                other => panic!(
                    "expected v to be a KFunctor with a body, got {}",
                    other.name()
                ),
            };
            (k_is_number, id)
        }
        other => panic!("expected a Dict type, got {}", other.summarize()),
    });
    assert!(k_is_number, "Pure operand at position 0 lands in k");
    assert_eq!(
        read_id, expected_id,
        "Reaching operand at position 1 lands in v and survives producer frame free"
    );
}

/// Swapping the two operands' positions from
/// [`alloc_type_composed_correlates_mixed_operands`] swaps which side of the composed `Dict` they
/// land in — correlation is purely positional, not by operand kind.
#[test]
fn alloc_type_composed_operand_order_is_positional() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));

    let producer_frame: Rc<CallFrame> = CallFrame::new(scope);
    let kt: &KType = alloc_home_functor_type(&producer_frame);
    let expected_id = match kt {
        KType::KFunctor { body: Some(f), .. } => f.captured_scope().id,
        other => panic!("expected a KFunctor with a body, got {}", other.name()),
    };
    let dep: DeliveredCarried = Delivered::seal(
        Witnessed::from_erased(Erased::erase(Carried::Type(kt)), CarrierWitness::default()),
        producer_frame.storage_rc(),
    );

    let consumer_frame: Rc<CallFrame> = CallFrame::new(scope);
    let ctx = StepAllocator::over_frame(consumer_frame.storage_rc());
    let operands = vec![
        TypeOperand::Reaching(&dep),
        TypeOperand::Pure(KType::Number),
    ];
    let composed: StepCarried = ctx.alloc_type_composed(operands, |_brand, parts| {
        KType::dict(Box::new(parts[0].clone()), Box::new(parts[1].clone()))
    });

    let consumer_storage = consumer_frame.storage_rc();
    drop(dep);
    drop(producer_frame);
    drop(consumer_frame);

    let (read_id, v_is_number) = composed.inspect_pinned(&consumer_storage, |c| match c {
        Carried::Type(KType::Dict {
            key: k, value: v, ..
        }) => {
            let id = match k.as_ref() {
                KType::KFunctor { body: Some(f), .. } => f.captured_scope().id,
                other => panic!(
                    "expected k to be a KFunctor with a body, got {}",
                    other.name()
                ),
            };
            let v_is_number = matches!(v.as_ref(), KType::Number);
            (id, v_is_number)
        }
        other => panic!("expected a Dict type, got {}", other.summarize()),
    });
    assert_eq!(
        read_id, expected_id,
        "Reaching operand now at position 0 lands in k and survives producer frame free"
    );
    assert!(v_is_number, "Pure operand now at position 1 lands in v");
}

/// An all-`Pure` operand list folds no dep — reach = own region only, the same exact-reach result
/// [`alloc_type_of`](StepAllocator::alloc_type_of)'s scalar gate produces elsewhere, here
/// without needing a gate at all: `Pure` operands simply add nothing to `deps`.
#[test]
fn alloc_type_composed_all_pure_seals_empty_reach() {
    let root = run_root_storage();
    let ctx = StepAllocator::over_frame(Rc::clone(&root));
    let operands = vec![
        TypeOperand::Pure(KType::Str),
        TypeOperand::Pure(KType::Number),
    ];
    let composed: StepCarried = ctx.alloc_type_composed(operands, |_brand, parts| {
        KType::dict(Box::new(parts[0].clone()), Box::new(parts[1].clone()))
    });
    assert!(
        composed.reach_is_empty(),
        "all-Pure operand list folds no dep: empty reach"
    );
}

// `RegionSet::mint` — the witness-set hosting substrate (design/witness-hosting.md § Composition).
// Each test below pins one rule of the mint's composition (home-omission, borrows-host
// materialization, outer-chain subsumption, precise reads, teardown release).

/// The mint reads its sources' **exact** member lists — two unrelated frames named through
/// disjoint singleton sources both survive, with no coarsening. (AC: precise members.)
#[test]
fn mint_composes_exact_members() {
    let a = run_root_storage();
    let b = run_root_storage();
    let c = run_root_storage();

    let source_a = FrameSet::singleton(Rc::clone(&a));
    let source_b = FrameSet::singleton(Rc::clone(&b));
    let minted = FrameSet::mint(c.brand().0, &[&source_a, &source_b], &[], |_| false).unwrap();

    assert_eq!(minted.members().len(), 2, "exact members — no coarsening");
    assert!(minted
        .members()
        .iter()
        .any(|m| std::ptr::eq(m.region(), a.region())));
    assert!(minted
        .members()
        .iter()
        .any(|m| std::ptr::eq(m.region(), b.region())));
}

/// Home-omission (rule 1, the self-cycle rule): a source naming the destination's own region never
/// lands as a member of the set minted into it. (AC: home-omission.)
#[test]
fn mint_home_omits_dest_region() {
    let c = run_root_storage();
    let source_c = FrameSet::singleton(Rc::clone(&c));

    let minted = FrameSet::mint(c.brand().0, &[&source_c], &[], |_| false);

    assert!(
        minted.is_none(),
        "dest's own region is never a member of its own minted set"
    );
}

/// Borrows-host materialization (rule 2): a `materialize_hosts` entry becomes a member iff its
/// region is foreign to `dest` — materializing into its own home is home-omitted instead. (AC:
/// rule 2.)
#[test]
fn mint_materializes_foreign_host() {
    let a = run_root_storage();
    let c = run_root_storage();

    let minted_into_c = FrameSet::mint(c.brand().0, &[], &[Rc::clone(&a)], |_| false).unwrap();
    assert_eq!(minted_into_c.members().len(), 1, "A is foreign to C");
    assert!(std::ptr::eq(
        minted_into_c.members()[0].region(),
        a.region()
    ));

    let minted_into_a = FrameSet::mint(a.brand().0, &[], &[Rc::clone(&a)], |_| false);
    assert!(
        minted_into_a.is_none(),
        "materializing A's own host into A is home-omitted"
    );
}

/// Outer-chain subsumption (rule 3): composing a descendant and its ancestor collapses to the
/// descendant alone — the ancestor's region is already pinned by the descendant's `outer` chain.
/// (AC: rule 3.)
#[test]
fn mint_subsumes_ancestor() {
    let a = run_root_storage();
    let b = child_storage(&a);
    let c = run_root_storage();

    let source_a = FrameSet::singleton(Rc::clone(&a));
    let source_b = FrameSet::singleton(Rc::clone(&b));
    let minted = FrameSet::mint(c.brand().0, &[&source_a, &source_b], &[], |_| false).unwrap();

    let [sole] = minted.members() else {
        panic!("ancestor subsumed by descendant");
    };
    assert!(std::ptr::eq(sole.region(), b.region()));
}

/// A minted set's members are a pinned read: held through `c`'s own storage, iterating
/// `members()` reads back the exact regions minted in. (AC: frozen read.)
#[test]
fn mint_reads_back_under_pin() {
    let a = run_root_storage();
    let c = run_root_storage();
    let source_a = FrameSet::singleton(Rc::clone(&a));

    let minted = FrameSet::mint(c.brand().0, &[&source_a], &[], |_| false).unwrap();

    let regions: Vec<*const KoanRegion> = minted
        .members()
        .iter()
        .map(|m| m.region() as *const _)
        .collect();
    assert_eq!(regions, vec![a.region() as *const _]);
}

/// A mint lands in the destination's `FrameSet` sub-arena — exactly one allocation regardless of
/// how many sources/hosts compose into it.
#[test]
fn mint_bumps_alloc_count() {
    let a = run_root_storage();
    let c = run_root_storage();
    let source_a = FrameSet::singleton(Rc::clone(&a));

    let before = c.region().alloc_count();
    let _minted = FrameSet::mint(c.brand().0, &[&source_a], &[], |_| false);
    assert_eq!(
        c.region().alloc_count(),
        before + 1,
        "mint stores exactly one set in dest's arena"
    );
}

/// Teardown releases a minted set's members: dropping `C`'s storage drops the stored `FrameSet`,
/// decrementing each member's refcount. No self-cycle (home-omission forbids `C` from holding its
/// own `Rc`), so the extra refs mint added fall away at `C`'s death — the shape the Miri leak audit
/// exercises. (AC: teardown releasing members at region death.)
#[test]
fn mint_teardown_releases_members() {
    let a = run_root_storage();
    let b = run_root_storage();
    let c = run_root_storage();

    let count_before_a = Rc::strong_count(&a);
    let count_before_b = Rc::strong_count(&b);

    {
        let source_a = FrameSet::singleton(Rc::clone(&a));
        let source_b = FrameSet::singleton(Rc::clone(&b));
        let minted = FrameSet::mint(c.brand().0, &[&source_a, &source_b], &[], |_| false).unwrap();
        assert_eq!(minted.members().len(), 2);
    }
    assert_eq!(
        Rc::strong_count(&a),
        count_before_a + 1,
        "C's arena holds the sole remaining extra ref to A"
    );
    assert_eq!(
        Rc::strong_count(&b),
        count_before_b + 1,
        "C's arena holds the sole remaining extra ref to B"
    );

    drop(c);
    assert_eq!(Rc::strong_count(&a), count_before_a, "C's death releases A");
    assert_eq!(Rc::strong_count(&b), count_before_b, "C's death releases B");
}

/// The checked-seal rejection this item's audits exist to catch: a `ModuleSignature` allocated
/// into region A, wrapped as `KType::Signature`, sealed into region B's `alloc_ktype_checked`
/// (no evidence naming A) — a structured `ShapeError`, and nothing stored.
#[test]
fn alloc_ktype_checked_rejects_foreign_signature_with_no_store() {
    use crate::machine::model::values::ModuleSignature;

    let region_a = run_root_storage();
    let scope_a = default_scope(&region_a, Box::new(std::io::sink()));
    let sig = region_a
        .brand()
        .alloc_signature(ModuleSignature::new("Sig".into(), scope_a));
    let kt = KType::signature(SigSource::Declared(sig), Vec::new());

    let region_b = run_root_storage();
    let before = region_b.region().alloc_count();
    let result = region_b.brand().alloc_ktype_checked(kt);

    let err = result.expect_err("a foreign-region Signature must be rejected, not stored");
    assert!(
        matches!(&err.kind, crate::machine::core::KErrorKind::ShapeError(_)),
        "expected ShapeError, got {err:?}"
    );
    assert_eq!(
        region_b.region().alloc_count(),
        before,
        "a rejected checked seal must store nothing"
    );
}

/// The reaching tier is evidence-gated, not a rubber stamp: the same foreign-region `Signature`,
/// sealed into region B's `alloc_ktype_reaching` under an **empty** `StoredReach`, is refused just
/// like the dest-only tier refuses it — B's own region and its ambient coverage name nothing in A.
/// This is the tier a deferred `-> er` return homes through, so a return type borrowing a region
/// the parameter's binding does not pin still errors loudly.
#[test]
fn alloc_ktype_reaching_rejects_foreign_signature_with_no_evidence() {
    use crate::machine::model::values::ModuleSignature;

    let region_a = run_root_storage();
    let scope_a = default_scope(&region_a, Box::new(std::io::sink()));
    let sig = region_a
        .brand()
        .alloc_signature(ModuleSignature::new("Sig".into(), scope_a));
    let kt = KType::signature(SigSource::Declared(sig), Vec::new());

    let region_b = run_root_storage();
    let scope_b = default_scope(&region_b, Box::new(std::io::sink()));
    let before = region_b.region().alloc_count();
    let result: Result<&KType<'_>, _> = scope_b.alloc_ktype_reaching(kt, &StoredReach::empty());

    let err = result.expect_err("no evidence names region A, so the seal must be refused");
    assert!(
        matches!(&err.kind, crate::machine::core::KErrorKind::ShapeError(_)),
        "expected ShapeError, got {err:?}"
    );
    assert_eq!(
        region_b.region().alloc_count(),
        before,
        "a rejected reaching seal must store nothing"
    );
}

/// `alloc_carried_with_scope` crosses two operands to the fold brand at once: a delivered dep view
/// and the consumer's own scope, re-anchored as `&'b Scope<'b>`. The build closure composes a
/// `KType::Record` whose `flag` field is a type read out of the dep view and whose `count` field is
/// a type read out of the crossed scope — proving the scope arrives as a declared operand usable at
/// the brand, not an ambient capture. The composed carrier round-trips through its own witness.
#[test]
fn alloc_carried_with_scope_folds_dep_view_and_scope_read() {
    let run_storage = run_root_storage();
    // The consumer scope, built inside `run_storage` so its region owner is held — the pin
    // `seal_scope_ref_delivered`'s `expect` requires. Builtins register `Number` as a readable type.
    let scope = default_scope(&run_storage, Box::new(std::io::sink()));
    let step_ctx = StepAllocator::over_frame(Rc::clone(&run_storage));

    // A dep view carrying a `Bool` type, delivered from an unrelated producer frame.
    let producer = run_root_storage();
    let bool_ty: Carried = Carried::Type(producer.brand().alloc_ktype(KType::Bool));
    let dep: DeliveredCarried = delivered_with_host(bool_ty, Rc::clone(&producer));

    let sealed: StepCarried =
        step_ctx.alloc_carried_with_scope(&[&dep], scope, |brand, views, scope| {
            let flag = match views[0] {
                Carried::Type(kt) => kt.clone(),
                Carried::Object(_) => panic!("dep view is a type"),
            };
            let count = scope
                .resolve_type("Number")
                .expect("Number resolves in the default scope")
                .clone();
            let record =
                Record::from_pairs([("flag".to_string(), flag), ("count".to_string(), count)]);
            Carried::Type(brand.alloc_ktype_folded(KType::record(Box::new(record))))
        });

    // The record lives in `run_storage`'s region; `producer` still pins the `Bool` leaf it embeds.
    sealed.inspect_pinned(&run_storage, |c| match c {
        Carried::Type(KType::Record { fields: r, .. }) => {
            assert_eq!(
                r.get("flag"),
                Some(&KType::Bool),
                "flag folded from the dep view"
            );
            assert_eq!(
                r.get("count"),
                Some(&KType::Number),
                "count read from the crossed scope"
            );
        }
        _ => panic!("expected a Record type"),
    });
}
