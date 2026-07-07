//! Targeted Miri coverage for the unsafe sites in this file. Each test pins down a
//! specific aliasing/lifetime shape under tree borrows; logical assertions are minimal
//! — these tests fail when Miri reports UB, not on values.

use super::*;
use crate::builtins::default_scope;
use crate::machine::core::StoredReach;
use crate::machine::model::types::KType;
use crate::machine::model::values::{Carried, CarriedFamily, Held, KObject};
use crate::machine::model::Record;
use crate::machine::BindingIndex;
use crate::machine::CarrierWitness;
use crate::witnessed::{Erased, Sealed, SetWitness, Witnessed};
use std::marker::PhantomData;

/// Test-only destination-region operand: a bare [`RegionBrand`], the `HasRegionHandle` mint target
/// a `merge`/`transfer_into` composition needs. Mirrors `execute::run_loop::RegionRefFamily`, which
/// this file (under `core`, not `execute`) cannot reach.
struct BrandFamily;
crate::witnessed::reattachable!(BrandFamily => RegionBrand<'r>);

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
    let sole = merged.sole().expect("ancestor subsumed by descendant");
    assert!(std::ptr::eq(sole.region(), child.region()));

    // Order-independent: the antichain is the same either way.
    let merged_rev = FrameSet::union(&ancestor, &descendant);
    let sole_rev = merged_rev.sole().expect("ancestor subsumed by descendant");
    assert!(std::ptr::eq(sole_rev.region(), child.region()));
}

/// `FrameSet::union` over unrelated carts keeps both — neither `outer` chain pins the other.
#[test]
fn frameset_merge_keeps_unrelated() {
    let a = run_root_storage();
    let b = run_root_storage();
    let merged = FrameSet::union(&FrameSet::singleton(a), &FrameSet::singleton(b));
    assert!(merged.sole().is_none(), "unrelated regions both kept");
}

/// The single-owner `Rc<FrameStorage>` witness (the `yoke` seam) exposes exactly its own region. A
/// singleton `FrameSet` exposes its sole frame; the empty set exposes none.
#[test]
fn single_owner_exposes_region_and_frameset_sole() {
    let root = run_root_storage();
    // The `yoke` seam is `WitnessRegion for Rc<FrameStorage>`: a held owner pins exactly one region.
    assert!(std::ptr::eq(WitnessRegion::region(&root), root.region()));
    let set = FrameSet::singleton(Rc::clone(&root));
    assert!(set.sole().is_some());
    assert!(FrameSet::empty().sole().is_none());
    assert!(FrameSet::empty().is_empty());
}

/// `with_scope` opens the child scope at a `for<'b>` brand. A scalar copies out; a bind / lookup
/// consumed in place stays inside the brand (the value is allocated at the same `'b` via the opened
/// scope's own region), so nothing branded escapes.
#[test]
fn with_scope_opens_child_scope_at_brand() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let frame: Rc<CallFrame> = CallFrame::new_test(scope, None);
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
            StoredReach::empty(),
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
    let frame: Rc<CallFrame> = CallFrame::new_test(scope, None);
    frame.with_scope(|child| {
        // `alloc_object` erases the caller-`'a` input and re-homes it at the frame region, so no
        // pre-shortening is needed.
        let it_obj = child.brand().alloc_object(it_value);
        child
            .bind_value(
                "it".to_string(),
                it_obj,
                BindingIndex::BUILTIN,
                StoredReach::empty(),
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
    let frame = CallFrame::new_test(scope, None);
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
    let frame: Rc<CallFrame> = CallFrame::new_test(scope, None);
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
                StoredReach::empty(),
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
    let outer = CallFrame::new_test(run_scope, None);
    // The returned `Rc<CallFrame>` carries no brand lifetime, so it escapes the open.
    let inner =
        outer.with_scope(|outer_child| CallFrame::new_test(outer_child, Some(outer.storage_rc())));
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
        let frame = CallFrame::new_test(scope, None);
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

/// A value `yoke`d into a frame's region comes back bundled with that frame as its reach witness,
/// co-located by construction. Read back after the original frame handle drops — the bundled witness
/// is the sole owner of the region the carrier's reference points into. The region-pure / single-frame
/// shape the object and type families' common case takes.
#[test]
fn alloc_witnessed_yokes_a_co_located_value() {
    let frame = run_root_storage();
    let w: Witnessed<CarriedFamily, CarrierWitness> =
        KoanRegion::alloc_witnessed(Rc::clone(&frame), |region| {
            Carried::Object(region.alloc_object(KObject::Number(7.0)))
        });
    drop(frame); // the bundled witness now solely owns the region the value lives in.
    let got = w.with(|c| match *c {
        Carried::Object(KObject::Number(n)) => *n,
        _ => panic!("expected a Number object"),
    });
    assert_eq!(got, 7.0);
}

/// The cross-region `merge` folds a *foreign* region-resident element in (a list/dict element
/// borrowing into another frame's region). The foreign value is
/// `yoke`d in an unrelated frame; merging it into a carrier built here succeeds because `FrameSet` is
/// a *set* witness — it represents the union of two unrelated regions (where a single-region witness
/// returns `None`, cf. `merge_rejects_unrelated_carts` in `witnessed/tests.rs`). After both call
/// handles drop, the merged carrier's witness still pins the foreign backing the bound value points
/// into.
#[test]
fn alloc_witnessed_merge_folds_an_independent_foreign_value() {
    let here_frame = run_root_storage();
    let foreign_frame = run_root_storage(); // unrelated — a sibling producer's frame.
    let foreign: Witnessed<CarriedFamily, CarrierWitness> =
        KoanRegion::alloc_witnessed(Rc::clone(&foreign_frame), |r| {
            Carried::Object(r.alloc_object(KObject::Number(1.0)))
        });
    // `here_frame`'s own brand is the destination operand: the `HasRegionHandle` mint target the
    // merge composes against. `foreign`'s value is untouched (still living in `foreign_frame`'s own
    // arena) — only its witness re-homes onto `here_frame`, minting `foreign_frame` into its arena
    // as a reach member (the single-host carrier's take on "union of two unrelated regions").
    let here_dest: Witnessed<BrandFamily, CarrierWitness> =
        KoanRegion::yoke_branded::<BrandFamily, _>(Rc::clone(&here_frame), |b| b);
    let merged: Witnessed<CarriedFamily, CarrierWitness> = foreign
        .merge::<BrandFamily, CarriedFamily>(here_dest, |foreign, _brand, _b: PhantomData<&_>| {
            foreign
        });
    drop(here_frame);
    drop(foreign_frame); // `merged` holds its own clones of both frames (host + minted reach member).
    let got = merged.with(|c| match *c {
        Carried::Object(KObject::Number(n)) => *n,
        _ => panic!("expected a Number object"),
    });
    assert_eq!(got, 1.0); // the foreign element survived the merge and both handle drops.
}

/// AC bullet 3's walking half: duplicating a carrier for dep delivery is a pure pass-through — it
/// copies the erased value and clones the witness's `host` `Rc`, but the reach set itself is never
/// re-minted, so every duplicate's reach pointer is identical to the original's. Fails on UB/leaks
/// under Miri (a re-mint would show up as extra per-member `Rc` traffic on the foreign frame), not on
/// values.
#[test]
fn pass_through_duplicate_keeps_host_and_reach_pointers_and_mints_nothing() {
    let foreign_frame = run_root_storage();
    let here_frame = run_root_storage();
    let foreign: Witnessed<CarriedFamily, CarrierWitness> =
        KoanRegion::alloc_witnessed(Rc::clone(&foreign_frame), |r| {
            Carried::Object(r.alloc_object(KObject::Number(1.0)))
        });
    let here_dest: Witnessed<BrandFamily, CarrierWitness> =
        KoanRegion::yoke_branded::<BrandFamily, _>(Rc::clone(&here_frame), |b| b);
    let merged: Witnessed<CarriedFamily, CarrierWitness> = foreign
        .merge::<BrandFamily, CarriedFamily>(here_dest, |foreign, _brand, _b: PhantomData<&_>| {
            foreign
        });

    let reach_ptr = merged
        .witness()
        .with_reach(|r| r.map(|set| set as *const _));
    let here_count_before = Rc::strong_count(&here_frame);
    let foreign_count_before = Rc::strong_count(&foreign_frame);

    // The walking motion — dep delivery duplicates a producer slot's carrier for each consumer.
    let sealed: Sealed<CarriedFamily, CarrierWitness> = Sealed::seal(merged);
    let copy_a = sealed.duplicate();
    let copy_b = sealed.duplicate();

    for copy in [&copy_a, &copy_b] {
        let copy_ptr = copy.witness().with_reach(|r| r.map(|set| set as *const _));
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
         foreign frame duplicating minted into the reach"
    );
}

/// Workload-level accumulator carrier for the aggregate construction fold: the dest region the
/// finished aggregate node lands in, paired with the partial element cells built so far. The
/// production family the object-family construction inversion uses lives in the execute layer; this
/// is the spike stand-in that proves the carrier round-trips and the fold composition is sound.
struct AggBuildFamily;
crate::witnessed::reattachable!(AggBuildFamily => (RegionBrand<'r>, Vec<Held<'r>>));

/// The **aggregate** construction fold: a list / dict / record built from several dep producers —
/// the shape the object family folds with shipped verbs only (no new substrate primitive). The
/// accumulator is `yoke`d empty over the dest frame's region; each foreign dep's
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
    let frame_a = run_root_storage();
    let frame_b = run_root_storage();
    let dep_a: Sealed<CarriedFamily, CarrierWitness> =
        Sealed::seal(KoanRegion::alloc_witnessed(Rc::clone(&frame_a), |r| {
            Carried::Object(r.alloc_object(KObject::Number(1.0)))
        }));
    let dep_b: Sealed<CarriedFamily, CarrierWitness> =
        Sealed::seal(KoanRegion::alloc_witnessed(Rc::clone(&frame_b), |r| {
            Carried::Object(r.alloc_object(KObject::Number(2.0)))
        }));
    // The consumer's own frame: the region the finished list node lands in.
    let dest_frame = run_root_storage();
    // `yoke` the empty accumulator (the dest region + no cells yet) into the dest frame's region.
    let acc0: Witnessed<AggBuildFamily, CarrierWitness> =
        KoanRegion::yoke_branded::<AggBuildFamily, _>(Rc::clone(&dest_frame), |region| {
            (region, Vec::new())
        });
    // Fold each dep in: bind its re-anchored carrier into the cells (a list element borrows into the
    // foreign region exactly as a surviving closure rides its bare borrow); the witness accumulates
    // the union. `transfer_into` borrows the dep's seal (does not consume it — other consumers keep
    // reading the producer terminal).
    let acc1 = dep_a.transfer_into::<AggBuildFamily, AggBuildFamily>(
        acc0,
        |dep, (region, mut cells), _brand| {
            cells.push(Held::from_carried(dep));
            (region, cells)
        },
    );
    let acc2 = dep_b.transfer_into::<AggBuildFamily, AggBuildFamily>(
        acc1,
        |dep, (region, mut cells), _brand| {
            cells.push(Held::from_carried(dep));
            (region, cells)
        },
    );
    // Allocate the list node from the carried dest region; the cells ride borrows into both foreign
    // regions, all three now named on this one carrier's witness.
    let list: Witnessed<CarriedFamily, CarrierWitness> = acc2.map(|(region, cells), _brand| {
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
        set.sole().is_some_and(|f| Rc::ptr_eq(f, &foreign)),
        "a foreign frame must fold into the reach-set",
    );

    // Re-folding the same foreign frame is idempotent (subsumption dedups by region).
    set.fold_omitting(&FrameSet::singleton(Rc::clone(&foreign)), |region| {
        home.pins_region(region)
    });
    assert!(
        set.sole().is_some(),
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
    let carrier: Witnessed<CarriedFamily, CarrierWitness> =
        storage.brand().alloc_object_witnessed(KObject::Number(1.0));
    // A sibling alloc into the same region coexists — the membership-table write and the prior store
    // do not alias under tree borrows.
    let sibling = storage.brand().alloc_object(KObject::Number(2.0));
    // Read the escaped carrier back while `region` (its backing) is live.
    let got = carrier.with(|c| match *c {
        Carried::Object(KObject::Number(n)) => *n,
        _ => panic!("expected a Number object"),
    });
    assert_eq!(got, 1.0);
    assert!(matches!(sibling, KObject::Number(n) if *n == 2.0));
}

/// The empty-witness transient — the crux of the foreign-reach-only alloc. A region-pure carrier born
/// under [`FrameSet::empty`] (the brand-confined [`alloc_object_witnessed`](super::Region::alloc_object_witnessed))
/// pins **nothing**, sound only because the producer is folded into its witness **before** the carrier
/// is stored on a node. This pins that fold-before-store across the producer shell's drop: fold the
/// producer, seal, then drop the producer shell outright (a `FreshTail` tail hop never resets a shell
/// in place — it mints a fresh cart and drops the retiring one) — the folded producer-storage pin
/// keeps the region (where the value lives) alive, so opening the sealed carrier after the drop reads
/// a live pointee, not a freed one. Without the fold the empty witness would pin nothing and the drop
/// would free the region under the stored carrier.
#[test]
fn empty_witness_carrier_survives_producer_shell_drop_after_fold() {
    let outer_region = run_root_storage();
    let outer_scope = default_scope(&outer_region, Box::new(std::io::sink()));
    let frame: Rc<CallFrame> = CallFrame::new_test(outer_scope, None);

    // Born foreign-reach-only (empty): the active frame is excluded at the alloc site.
    let carrier: Witnessed<CarriedFamily, CarrierWitness> =
        frame.brand().alloc_object_witnessed(KObject::Number(7.0));
    assert!(
        carrier.witness().is_empty(),
        "a region-pure carrier is born under the empty set",
    );

    // The scope-reach seal at close: fold the producer in before storage (the `finalize` shape), then
    // seal for node storage. The fold is what gives the otherwise-unpinned carrier its pin. The old
    // witness (`Empty`) contributes nothing, so `rehost` — not a `ComposeWitness` union — is the
    // right replacement.
    let folded = carrier.rehost(CarrierWitness::singleton(frame.storage_rc()));
    assert!(
        !folded.witness().is_empty(),
        "folding the producer pins the carrier",
    );
    let sealed: Sealed<CarriedFamily, CarrierWitness> = Sealed::seal(folded);

    // Drop the producer shell outright — the sealed carrier holds the *storage* Rc, not the shell,
    // so the region stays alive under the drop.
    drop(frame);

    // The pointee is still live: the folded producer-storage pin held the region across the shell's
    // drop, so opening the stored carrier reads a valid value rather than a freed one.
    let got = sealed.open(|c| match c {
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
    use crate::machine::core::kfunction::action::Action;
    use crate::machine::model::{ExpressionSignature, ReturnType, SignatureElement};
    use crate::machine::{Body, KFunction};
    // Capture `home`'s child scope (read at the brand), alloc the closure into `home`'s own region —
    // where that scope lives — and wrap it as a `KObject::KFunction` in the same region, so the escaping
    // `&KObject` reaches exactly that region.
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
            false,
        );
        let kf_ref = home.brand().alloc_function(kf);
        home.brand().alloc_object(KObject::KFunction(kf_ref))
    })
}

/// A closure carrier born witnessed by its home frame — the born-witnessed `resident` / `reseal_under`
/// path (production's finalize seal / [`RegionBrand::seal_resident`](super::RegionBrand)), never an
/// asserted co-location bundle. A closure captures only its home frame's own scope, so it is region-pure
/// there: `resident` bundles it under the empty set and `reseal_under` folds in its producer frame (a
/// witness-only `merge`). A closure can't be `yoke`d — yoke's `for<'b>` build closure can't capture the
/// frame's existing scope, and minting a fresh one needs the frame's storage `Rc` a `for<'b>` forbids.
fn witnessed_closure(home: &Rc<CallFrame>) -> Witnessed<CarriedFamily, CarrierWitness> {
    Witnessed::from_erased(
        Erased::erase(Carried::Object(alloc_home_closure(home))),
        CarrierWitness::singleton(home.storage_rc()),
    )
}

/// Record-fold accumulator family: the dest region plus the named field cells built so far — the record
/// twin of [`AggBuildFamily`]. Each closure cell `transfer_into`s (a `merge`) its value and reach onto
/// the accumulator; the final `map` builds the record from the region.
struct RecordCellFamily;
crate::witnessed::reattachable!(RecordCellFamily => (RegionBrand<'r>, Vec<(String, Held<'r>)>));

/// **Multi-region shape 1 — a list of closures over distinct, independently-dying per-call regions.**
/// Each closure is `transfer_into`d into a list accumulator, relocating the value into the dest region
/// and *unioning its reach* onto the carrier; the source carrier drops at the end of its statement, so
/// after the fold only the aggregate's own witness set keeps the closure regions alive. Every producing
/// frame is then freed and each closure's captured scope read back — a use-after-free the instant the
/// witness under-counts (a single frame witnessing the whole list would free the others' regions).
/// Fails on UB, not values.
#[test]
fn multi_region_list_of_closures_survives_frame_free() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    // Three independent per-call frames — distinct regions, no shared ancestry, each dying on its own.
    let frame_a: Rc<CallFrame> = CallFrame::new_test(scope, None);
    let frame_b: Rc<CallFrame> = CallFrame::new_test(scope, None);
    let dest_frame: Rc<CallFrame> = CallFrame::new_test(scope, None); // the list node lands here.

    let acc0 = KoanRegion::yoke_branded::<AggBuildFamily, _>(dest_frame.storage_rc(), |region| {
        (region, Vec::new())
    });
    // Fold each closure terminal (born witnessed by its own frame) into the accumulator; the temporary
    // source carrier drops after each statement, leaving only the aggregate witness holding the region.
    let acc1 = Sealed::seal(witnessed_closure(&frame_a))
        .transfer_into::<AggBuildFamily, AggBuildFamily>(
            acc0,
            |dep, (region, mut cells), _brand| {
                cells.push(Held::from_carried(dep));
                (region, cells)
            },
        );
    let acc2 = Sealed::seal(witnessed_closure(&frame_b))
        .transfer_into::<AggBuildFamily, AggBuildFamily>(
            acc1,
            |dep, (region, mut cells), _brand| {
                cells.push(Held::from_carried(dep));
                (region, cells)
            },
        );
    let list: Witnessed<CarriedFamily, CarrierWitness> = acc2.map(|(region, cells), _brand| {
        Carried::Object(region.alloc_object(KObject::list_of_held(cells)))
    });

    // Free every producing frame: the list's own witness set (dest ∪ frame_a ∪ frame_b) is now the sole
    // owner of all three regions. Under-count any one and the read below touches freed memory.
    drop(frame_a);
    drop(frame_b);
    drop(dest_frame);

    // Read every closure's captured scope back — each deref rides a `&KFunction` in its (now
    // witness-only-pinned) region.
    let ids: Vec<_> = list.with(|c| match c.object() {
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
    let frame_outer: Rc<CallFrame> = CallFrame::new_test(scope, None);
    let frame_1: Rc<CallFrame> = CallFrame::new_test(scope, None);
    let frame_2: Rc<CallFrame> = CallFrame::new_test(scope, None);

    // Fold the two inner closures into a list carrier over frame_outer's region — its witness derives to
    // {frame_outer, frame_1, frame_2} through the fold, never a hand-assembled union.
    let acc0 = KoanRegion::yoke_branded::<AggBuildFamily, _>(frame_outer.storage_rc(), |region| {
        (region, Vec::new())
    });
    let acc1 = Sealed::seal(witnessed_closure(&frame_1))
        .transfer_into::<AggBuildFamily, AggBuildFamily>(
            acc0,
            |dep, (region, mut cells), _brand| {
                cells.push(Held::from_carried(dep));
                (region, cells)
            },
        );
    let acc2 = Sealed::seal(witnessed_closure(&frame_2))
        .transfer_into::<AggBuildFamily, AggBuildFamily>(
            acc1,
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
    let captured: Witnessed<CarriedFamily, CarrierWitness> = witnessed_closure(&frame_outer)
        .merge::<AggBuildFamily, CarriedFamily>(
        acc2,
        |outer_v, (region, cells), _brand| {
            if let KObject::KFunction(kf) = outer_v.object() {
                let list_obj = region.alloc_object(KObject::list_of_held(cells));
                kf.captured_scope()
                    .bind_value(
                        "inners".to_string(),
                        list_obj,
                        BindingIndex::BUILTIN,
                        StoredReach::empty(),
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
    // captured scope — touching all three regions after they would have died without the union pin.
    let ids: Vec<_> = captured.with(|c| match c.object() {
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
    let frame_a: Rc<CallFrame> = CallFrame::new_test(scope, None);
    let frame_b: Rc<CallFrame> = CallFrame::new_test(scope, None);
    let dest_frame: Rc<CallFrame> = CallFrame::new_test(scope, None);

    // Fold each field's closure into a named-cell accumulator over the dest region; the record's witness
    // derives to {dest ∪ frame_a ∪ frame_b} through the fold, never a hand-assembled union.
    let acc0 = KoanRegion::yoke_branded::<RecordCellFamily, _>(dest_frame.storage_rc(), |region| {
        (region, Vec::new())
    });
    let acc1 = Sealed::seal(witnessed_closure(&frame_a))
        .transfer_into::<RecordCellFamily, RecordCellFamily>(
            acc0,
            |dep, (region, mut cells), _brand| {
                cells.push(("a".to_string(), Held::from_carried(dep)));
                (region, cells)
            },
        );
    let acc2 = Sealed::seal(witnessed_closure(&frame_b))
        .transfer_into::<RecordCellFamily, RecordCellFamily>(
            acc1,
            |dep, (region, mut cells), _brand| {
                cells.push(("b".to_string(), Held::from_carried(dep)));
                (region, cells)
            },
        );
    let record: Witnessed<CarriedFamily, CarrierWitness> = acc2.map(|(region, cells), _brand| {
        Carried::Object(region.alloc_object(KObject::record_of_held(Record::from_pairs(cells))))
    });

    drop(frame_a);
    drop(frame_b);
    drop(dest_frame);

    // Read each field's closure back, dereferencing its captured scope — a use-after-free if either
    // field's region were dropped from the union.
    let ids: Vec<_> = record.with(|c| match c.object() {
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
            true,
        );
        let kf_ref: &KFunction = home.brand().alloc_function(kf);
        let kt = KType::KFunctor {
            params: Record::new(),
            ret: Box::new(KType::Null),
            body: Some(kf_ref),
        };
        home.brand().alloc_ktype(kt)
    })
}

/// **`alloc_type_with`'s reach fold, exercised through the actual finish-surface helper.** Mirrors
/// what a field-list finish now does: a dep terminal's `KType::KFunctor { body: Some(&f) }` — the
/// stand-in for `t.value`/`t.carrier` — is cloned into a fresh `Record` type built in a *different*
/// frame's region via `alloc_type_with`, the same clone-embedding shape `field_list.rs`'s re-walk
/// performs. The fold unions the producer's reach into the result's witness; every producer-frame
/// handle then drops, and reading the record's embedded functor body must not dangle. Fails on UB,
/// not values — the closing case for the reach hole `alloc_type` (no fold) leaves open.
#[test]
fn functor_field_reach_fold_survives_producer_frame_free() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));

    // Producer: a KFunctor type (wrapping a KFunction) resident in its own frame's region — the
    // stand-in for a dep terminal delivered to the finish.
    let producer_frame: Rc<CallFrame> = CallFrame::new_test(scope, None);
    let kt: &KType = alloc_home_functor_type(&producer_frame);
    let expected_id = match kt {
        KType::KFunctor { body: Some(f), .. } => f.captured_scope().id,
        other => panic!("expected a KFunctor with a body, got {}", other.name()),
    };
    let dep: Sealed<CarriedFamily, CarrierWitness> = Sealed::seal(Witnessed::from_erased(
        Erased::erase(Carried::Type(kt)),
        CarrierWitness::singleton(producer_frame.storage_rc()),
    ));

    // Consumer: a StepContext over a *different* frame — the finish surface's own region. The fold
    // clones `kt` in exactly as the field-list re-walk clones a sub-dispatch terminal's `t.value`.
    let consumer_frame: Rc<CallFrame> = CallFrame::new_test(scope, None);
    let ctx = StepContext::<FrameStorage>::new(consumer_frame.storage_rc());
    let record: Witnessed<CarriedFamily, CarrierWitness> = ctx.alloc_type_with(
        &[&dep],
        KType::Record(Box::new(Record::from_pairs(vec![(
            "f".to_string(),
            kt.clone(),
        )]))),
    );

    // Drop the dep seal and every producer-frame handle: only the fold (if it happened) keeps the
    // producer's region alive through the record's own witness.
    drop(dep);
    drop(producer_frame);
    drop(consumer_frame);

    // Read back through the record carrier into the embedded functor's captured scope.
    let read_id = record.with(|c| match c {
        Carried::Type(KType::Record(fields)) => match fields.get("f") {
            Some(KType::KFunctor { body: Some(f), .. }) => f.captured_scope().id,
            Some(other) => panic!(
                "expected a KFunctor field with a body, got {}",
                other.name()
            ),
            None => panic!("expected field \"f\" in the record"),
        },
        other => panic!("expected a Record type, got {}", other.summarize()),
    });
    assert_eq!(
        read_id, expected_id,
        "functor field's captured scope read back after producer frame freed"
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

    let sole = minted.sole().expect("ancestor subsumed by descendant");
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
