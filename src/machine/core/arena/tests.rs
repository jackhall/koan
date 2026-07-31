//! Targeted Miri coverage for the unsafe sites in this file. Each test pins down a
//! specific aliasing/lifetime shape under tree borrows; logical assertions are minimal
//! — these tests fail when Miri reports UB, not on values.

use super::*;
use crate::builtins::test_support::{per_call_storage, run_root_bare, TestRun};
use crate::machine::core::Bindings;
use crate::machine::model::KType;
use crate::machine::model::Record;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{Carried, CarriedFamily, Held, KObject};
use crate::machine::BindingIndex;
use crate::machine::CarrierWitness;
use crate::machine::DeliveredCarried;
use crate::machine::KFunction;
use crate::witnessed::{
    Delivered, FoldToken, FoldedPlacement, RegionHandleFamily, RegionHost, Sealed, WitnessRegion,
    Witnessed,
};

/// Test-only destination-region operand: the library's [`RegionHandleFamily`], the
/// `HasRegionHandle` mint target a `merge`/`transfer_into` composition needs — the same family
/// production's `execute::run_loop::DestHandleFamily` aliases.
type BrandFamily = RegionHandleFamily<KoanStorageProfile>;

/// The destination operand for a composition: `frame`'s own handle `yoke`d into its region and
/// sealed into the envelope the composition verbs take, homed there. A bare handle reaches nothing
/// beyond its own region, so the operand's coverage is empty — production's
/// `execute::run_loop::dest_brand` in test shape.
fn dest_operand(frame: &Rc<FrameStorage>) -> Delivered<BrandFamily, CarrierWitness, FrameStorage> {
    Delivered::seal(
        KoanRegion::yoke_branded::<BrandFamily, _>(Rc::clone(frame), |b| b.handle()),
        Rc::clone(frame),
        FrameCoverage::empty(),
    )
}

/// A child `FrameStorage` whose `outer` chains `parent` — the ancestry shape `FrameReach`
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

/// The single-owner `Rc<FrameStorage>` witness (the `yoke` seam) exposes exactly its own region.
/// The composition over those owners — union, subsumption, the antichain shape — is library
/// mechanism koan holds no vocabulary for; it is read here off the descriptions the mint freezes
/// ([`mint_subsumes_ancestor`], [`mint_composes_exact_members`]).
#[test]
fn single_owner_exposes_its_own_region() {
    let root = run_root_storage();
    // The `yoke` seam is `WitnessRegion for Rc<FrameStorage>`: a held owner pins exactly one region.
    assert!(std::ptr::eq(WitnessRegion::region(&root), root.region()));
    assert!(FrameCoverage::empty().is_empty());
}

/// `with_scope` opens the child scope at a `for<'b>` brand. A scalar copies out; a bind / lookup
/// consumed in place stays inside the brand (the value is allocated at the same `'b` via the opened
/// scope's own region), so nothing branded escapes.
#[test]
fn with_scope_opens_child_scope_at_brand() {
    let region = run_root_storage();
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let frame: Rc<CallFrame> = CallFrame::new(scope);
    // Scalar copy-out: matches `scope_id`.
    let id = frame.with_scope(|s| s.id);
    assert_eq!(id, frame.scope_id());
    // In-place bind + lookup, all at the brand `'b` (value allocated via the opened scope's region).
    frame.with_scope(|s| {
        let v = s.brand().alloc_object(KObject::Number(7.0));
        s.bind_resident_for_test(
            "k".to_string(),
            v,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
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
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let frame: Rc<CallFrame> = CallFrame::new(scope);
    let types = test_run.types.clone();
    frame.with_scope(|child| {
        // `alloc_object_checked` erases the caller-`'a` input and re-homes it at the frame region,
        // so no pre-shortening is needed; a deep-cloned `Number` is always resident-in-self.
        let it_obj = child
            .brand()
            .alloc_object_checked(it_value, &types)
            .expect("a deep-cloned Number is always resident-in-self");
        child
            .bind_resident_for_test(
                "it".to_string(),
                it_obj,
                BindingIndex::BUILTIN,
                &mut crate::machine::WriteGate::for_test(),
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
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
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
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
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
            .bind_resident_for_test(
                "it".to_string(),
                it_obj,
                BindingIndex::BUILTIN,
                &mut crate::machine::WriteGate::for_test(),
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
    let run_test_run = TestRun::silent(&region);
    let run_scope = run_test_run.scope;
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
    let run_test_run = TestRun::silent(&region);
    let run_scope = run_test_run.scope;
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
    let run_test_run = TestRun::silent(&region);
    let run_scope = run_test_run.scope;
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

/// A fresh-tail hop over a **per-call** captured scope chains that scope's region owner, so a
/// closure capturing a per-call frame survives the hop that retires the caller — the same derivation
/// as [`builtin_frame_under_per_call_parent_chains_region_owner`], reached through the fresh-tail
/// path (`resolve_frame_placement`'s `FreshTail` mints via `CallFrame::new`). A top-level-defined
/// recursive fn instead captures the run-root scope and chains nothing (see
/// [`builtin_frame_at_top_level_chains_nothing`]), keeping the common tail loop constant-space.
#[test]
fn fresh_tail_hop_over_per_call_captured_scope_pins_it() {
    let region = run_root_storage();
    let run_test_run = TestRun::silent(&region);
    let run_scope = run_test_run.scope;
    let outer = CallFrame::new(run_scope);
    // The fresh-tail hop's `outer` is the callee's captured scope; here that scope is per-call.
    let tail = outer.with_scope(CallFrame::new);
    assert!(Rc::ptr_eq(
        tail.storage_rc()
            .outer()
            .expect("a fresh-tail hop over a per-call captured scope chains its region owner"),
        &outer.storage_rc(),
    ));
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

/// `KType` is a `Copy` content-digest handle — constructing one is not a region allocation.
#[test]
fn ktype_construction_is_not_a_region_allocation() {
    let storage = run_root_storage();
    let a = storage.brand();
    let baseline = a.region().alloc_count();
    let t: KType = KType::NUMBER;
    assert!(t == KType::NUMBER);
    assert_eq!(a.region().alloc_count(), baseline);
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
        let scope = run_root_bare(&run_root);
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
fn fold_witnessed_yokes_a_reference_only_value() {
    let frame = run_root_storage();
    let w: Witnessed<CarriedFamily, CarrierWitness> =
        KoanRegion::fold_witnessed(Rc::clone(&frame), |region| {
            Carried::Object(region.alloc_object_folded(KObject::Number(7.0)))
        });
    // The held `frame` (the retention stand-in) is the pin every read below names — the reach
    // query included, since re-anchoring the description reference needs the same coverage.
    let sealed = Sealed::seal(w);
    assert!(
        !sealed.open_at(&frame).has_reach_members(),
        "born reference-only: empty reach",
    );
    let got = sealed.open_with(&frame, |c| match c {
        Carried::Object(KObject::Number(n)) => *n,
        _ => panic!("expected a Number object"),
    });
    assert_eq!(got, 7.0);
}

/// The cross-region envelope transfer folds a *foreign* region-resident element in (a list/dict
/// element borrowing into another frame's region). The foreign value travels as its delivery
/// envelope (its producer frame riding as an ordinary member); the transfer mints that producer into
/// the destination's own arena as a reach member. After the producer handle drops, that minted
/// member is the sole owner of the foreign backing the value points into; the destination itself
/// stays pinned by the held `here_frame` (the retention stand-in), which the read names.
#[test]
fn envelope_transfer_folds_an_independent_foreign_value() {
    let here_frame = run_root_storage();
    let foreign_frame = per_call_storage(); // unrelated — a sibling producer's frame.
    let foreign: Witnessed<CarriedFamily, CarrierWitness> =
        KoanRegion::fold_witnessed(Rc::clone(&foreign_frame), |r| {
            Carried::Object(r.alloc_object_folded(KObject::Number(1.0)))
        });
    // `here_frame`'s own brand is the destination operand: the `HasRegionHandle` mint target the
    // transfer composes against. `foreign`'s value is untouched (still living in `foreign_frame`'s
    // own arena) — only its carrier re-homes: the envelope's host mints into `here_frame`'s arena
    // as a reach member (the value keeps living there).
    let delivered: DeliveredCarried =
        Delivered::seal(foreign, Rc::clone(&foreign_frame), FrameCoverage::empty());
    let merged = delivered
        .transfer_into::<BrandFamily, CarriedFamily, _>(
            dest_operand(&here_frame),
            |_product, _region| true,
            |foreign, _brand, _b: FoldToken<'_>| foreign,
        )
        .into_cell()
        .unseal();
    drop(delivered);
    drop(foreign_frame); // the minted member in `here_frame`'s arena is now the sole foreign owner.
    let got = merged.with_pinned(&here_frame, |c| match *c {
        Carried::Object(KObject::Number(n)) => *n,
        _ => panic!("expected a Number object"),
    });
    assert_eq!(got, 1.0); // the foreign element survived the transfer and the producer handle drop.
}

/// AC bullet 3's walking half: duplicating an envelope for dep delivery keeps the reach
/// **description** by reference — it copies the erased value, bit-copies the reference-only carrier,
/// and never re-mints the description, so every duplicate's reach pointer is identical to the
/// original's. What it clones is the envelope's owned liveness: one retained-host `Rc` and its owned
/// foreign [`FrameCoverage`](crate::machine::FrameCoverage) bundle per duplicate, so each consumer owns its
/// own pins for the parked period. A re-mint of the description (the regression this gates) would
/// change the reach pointer; Miri backs the no-leak half.
#[test]
fn pass_through_duplicate_keeps_reach_pointer_and_mints_nothing() {
    let foreign_frame = run_root_storage();
    let here_frame = run_root_storage();
    let foreign: Witnessed<CarriedFamily, CarrierWitness> =
        KoanRegion::fold_witnessed(Rc::clone(&foreign_frame), |r| {
            Carried::Object(r.alloc_object_folded(KObject::Number(1.0)))
        });
    let source: DeliveredCarried =
        Delivered::seal(foreign, Rc::clone(&foreign_frame), FrameCoverage::empty());
    // The transfer's product **is** the envelope: homed in `here_frame`, covering everything the
    // relocated value reaches.
    let envelope: DeliveredCarried = source.transfer_into::<BrandFamily, CarriedFamily, _>(
        dest_operand(&here_frame),
        |_product, _region| true,
        |foreign, _brand, _b: FoldToken<'_>| foreign,
    );

    // The reach query lives on the **in-use** carrier state, opened under the envelope's own
    // coverage — the pins the reach re-anchor needs.
    let reach_ptr = envelope.open_at().with_reach(|r| r as *const _);
    // Baselines taken with the envelope already built, so they include its own host clone.
    let here_count_before = Rc::strong_count(&here_frame);
    let foreign_count_before = Rc::strong_count(&foreign_frame);

    // The walking motion — dep delivery duplicates a producer slot's envelope for each consumer.
    let copy_a = envelope.duplicate();
    let copy_b = envelope.duplicate();

    for copy in [&copy_a, &copy_b] {
        let copy_ptr = copy.open_at().with_reach(|r| r as *const _);
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
        foreign_count_before + 2,
        "the reach description rides by reference (no re-mint), but the envelope OWNS its foreign \
         pins: each of the two duplicates clones the owned coverage, one foreign-frame clone each"
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
/// every reached region (a `FrameReach` set witness — the multi-foreign case a single-region witness
/// cannot represent); a final [`map`](Witnessed::map) allocates the list node into the carried region.
/// After every producer handle drops, the folded witness is the sole owner of all three regions the
/// list reaches, so reading the cells back is sound — the proof the construction site names its reach
/// on the one carrier rather than reconstructing it from the value. Mirrors the production fold; fails
/// on UB / leaks, not values.
#[test]
fn fold_witnessed_builds_a_list_over_independent_foreign_deps() {
    // Two unrelated producer frames, each holding one element — sibling producers whose terminals
    // this consumer aggregates.
    let frame_a = run_root_storage();
    let frame_b = run_root_storage();
    let dep_a: DeliveredCarried = Delivered::seal(
        KoanRegion::fold_witnessed(Rc::clone(&frame_a), |r| {
            Carried::Object(r.alloc_object_folded(KObject::Number(1.0)))
        }),
        Rc::clone(&frame_a),
        FrameCoverage::empty(),
    );
    let dep_b: DeliveredCarried = Delivered::seal(
        KoanRegion::fold_witnessed(Rc::clone(&frame_b), |r| {
            Carried::Object(r.alloc_object_folded(KObject::Number(2.0)))
        }),
        Rc::clone(&frame_b),
        FrameCoverage::empty(),
    );
    // The consumer's own frame: the region the finished list node lands in.
    let dest_frame = run_root_storage();
    let types = TypeRegistry::new();
    // `yoke` the empty accumulator (the dest region + no cells yet) into the dest frame's region.
    let acc0: Delivered<AggBuildFamily, CarrierWitness, FrameStorage> = Delivered::seal(
        KoanRegion::yoke_branded::<AggBuildFamily, _>(Rc::clone(&dest_frame), |region| {
            (region.handle(), Vec::new())
        }),
        Rc::clone(&dest_frame),
        FrameCoverage::empty(),
    );
    // Fold each dep in: bind its re-anchored carrier into the cells (a list element borrows into the
    // foreign region exactly as a surviving closure rides its bare borrow); the accumulated envelope
    // covers the union. `transfer_into` borrows the dep's seal (does not consume it — other
    // consumers keep reading the producer terminal).
    let acc1 = dep_a.transfer_into::<AggBuildFamily, AggBuildFamily, _>(
        acc0,
        |_product, _region| true,
        |dep, (region, mut cells), _brand| {
            cells.push(Held::from_carried(dep));
            (region, cells)
        },
    );
    let acc2 = dep_b.transfer_into::<AggBuildFamily, AggBuildFamily, _>(
        acc1,
        |_product, _region| true,
        |dep, (region, mut cells), _brand| {
            cells.push(Held::from_carried(dep));
            (region, cells)
        },
    );
    // Allocate the list node from the carried dest region; the cells ride borrows into both foreign
    // regions, both now minted as members into the dest arena.
    let list: Witnessed<CarriedFamily, CarrierWitness> =
        acc2.into_cell()
            .unseal()
            .map_pinned(&dest_frame, |(region, cells), _token| {
                let owned_cells = crate::machine::core::FrameCoverage::empty();
                let region = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(region))
                    .with_holder(&owned_cells);
                Carried::Object(
                    region.alloc_object_folded(KObject::list_of_held(region, cells, &types)),
                )
            });
    // Drop the producer handles: the dest arena's minted set solely owns both foreign regions; the
    // dest region itself rides the held `dest_frame` (the retention stand-in), which the read names.
    drop(frame_a);
    drop(frame_b);
    let got = list.with_pinned(&dest_frame, |c| match c.object() {
        KObject::List(items, _) => items
            .elements()
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

/// A mint applies no destination-relative policy: every source member lands in the description, and
/// two sources naming the same region collapse to one member (subsumption dedups by region). Minted
/// into a neutral `dest` region so neither the self rule nor ancestor subsumption is in play.
#[test]
fn mint_keeps_every_source_member_and_dedups_by_region() {
    let foreign = run_root_storage();
    let dest = run_root_storage();

    // A source frame lands as a member — the region a bound closure / module borrows into. The
    // mint retains its own bundle in `dest`'s region, which pins the member across the `members()`
    // read.
    let kept = dest
        .brand()
        .handle()
        .mint_retained(&[&FrameCoverage::of(Rc::clone(&foreign))]);
    assert!(
        matches!(kept.members().as_slice(), [only] if Rc::ptr_eq(only, &foreign)),
        "a source frame must land in the minted set",
    );

    // Two sources naming the same foreign region collapse to one member.
    let deduped = dest.brand().handle().mint_retained(&[
        &FrameCoverage::of(Rc::clone(&foreign)),
        &FrameCoverage::of(Rc::clone(&foreign)),
    ]);
    assert_eq!(
        deduped.members().len(),
        1,
        "a duplicate member stays a singleton, not a double entry",
    );
}

/// Retention-timeline acceptance (claim: *reach pins release at region death*). A binding entry
/// owns nothing: the mint that derives a bound value's reach folds the owning [`FrameCoverage`] bundle
/// into the destination **region's** deduped union, which pins every region anything resident there
/// reaches and drops with the region itself. Mint a reach whose owned bundle is the sole strong
/// owner of a foreign region, then confirm the foreign region outlives both the entry and the
/// binding table, and is released only when the destination region dies.
#[test]
fn region_union_foreign_pins_release_at_region_death() {
    let foreign = per_call_storage();
    let weak = Rc::downgrade(&foreign);
    let dest = run_root_storage();
    {
        let scope = run_root_bare(&dest);
        let obj = scope.brand().alloc_object(KObject::Number(1.0));
        // The bind-door mint: derive the exact reach into `dest`'s arena and fold the owning bundle
        // into `dest`'s region union. `foreign` is not the dest, so the self rule keeps it.
        let reach = scope.mint_retained(&[&FrameCoverage::of(Rc::clone(&foreign))]);
        let bindings: Bindings = Bindings::new();
        bindings
            .write_value(
                "x",
                BindingIndex::BUILTIN,
                scope.seal_reaching(Carried::Object(obj), reach),
                &mut crate::machine::WriteGate::for_test(),
            )
            .expect("a fresh value bind lands");

        // The region's union is the sole strong owner of `foreign`.
        drop(foreign);
        assert!(
            weak.upgrade().is_some(),
            "the region's union keeps the reached region alive",
        );

        drop(bindings);
        assert!(
            weak.upgrade().is_some(),
            "an entry owns nothing, so entry death releases no pin",
        );
    }

    drop(dest);
    assert!(
        weak.upgrade().is_none(),
        "the union's foreign pins release with the region that owns them",
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
    let outer_test_run = TestRun::silent(&outer_region);
    let outer_scope = outer_test_run.scope;
    let frame: Rc<CallFrame> = CallFrame::new(outer_scope);

    // Born reference-only: the active frame is excluded at the alloc site.
    let carrier: StepCarried = frame.brand().alloc_object_witnessed(KObject::Number(7.0));

    // The finalize shape: seal as-is; the retention hold (the producer's storage Rc) rides the
    // delivery envelope, never the carrier.
    let envelope: DeliveredCarried = carrier.seal_for_test(frame.storage_rc());
    assert!(
        !envelope.open_at().has_reach_members(),
        "a region-pure carrier is born under the empty reach",
    );

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
    let types = TypeRegistry::new();
    home.with_scope(|child| {
        let kf_ref = home.brand().alloc_function(no_op_closure(child));
        home.brand()
            .alloc_object_checked(KObject::KFunction(kf_ref), &types)
            .expect("f was just allocated into region\'s own region")
    })
}

/// A no-op `KFunction` capturing `scope` — the closure value the multi-region shapes fold; the body
/// is never run.
fn no_op_closure<'x>(captured: &'x Scope<'x>) -> KFunction<'x> {
    use crate::machine::core::kfunction::action::Action;
    use crate::machine::model::{ExpressionSignature, ReturnType, SignatureElement};
    use crate::machine::Body;
    let types = TypeRegistry::new();
    KFunction::new(
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::NULL),
            elements: vec![SignatureElement::Keyword("__INNER__".into())],
        },
        Body::Builtin(|ctx| {
            Action::done_resident(
                ctx.scope,
                Carried::Object(ctx.scope.brand().alloc_object(KObject::Null)),
            )
        }),
        captured,
        false,
        &types,
    )
}

/// A closure carrier in its delivery envelope — the value reference-only (region-pure in its home
/// frame, so its description names that frame as host and no member at all) and the envelope's host
/// the home frame's storage, the retention hold's stand-in. A closure can't be `yoke`d — yoke's
/// `for<'b>` build closure can't capture the frame's existing scope, and minting a fresh one needs
/// the frame's storage `Rc` a `for<'b>` forbids — so this takes production's resident-seal door.
fn delivered_closure(home: &Rc<CallFrame>) -> DeliveredCarried {
    Delivered::seal(
        home.brand()
            .seal_resident(Carried::Object(alloc_home_closure(home))),
        home.storage_rc(),
        FrameCoverage::empty(),
    )
}

/// A closure element as the LET-bind → entry-re-read pipeline delivers it: the closure lives whole in
/// `home` (its captured scope co-located, `alloc_function`'s invariant), and a *reader* scope in a
/// different region binds it — `mint_retained` mints `home` into the reader's arena as the entry's
/// reach, the entry's seal rides that reach, and the read lifts it into an envelope hosted by the
/// reader's frame. The closure's captured scope is thus foreign to both the element's host and any
/// destination the element folds into: its region rides the element's *reach*, never its residence
/// host (a per-call frame carries no storage `outer` under TCO).
fn delivered_reread_closure<'run>(
    home: &'run Rc<FrameStorage>,
    reader_scope: &'run Scope<'run>,
) -> DeliveredCarried {
    let types = TypeRegistry::new();
    let home_scope = run_root_bare(home);
    let kf_ref = home.brand().alloc_function(no_op_closure(home_scope));
    let obj = home
        .brand()
        .alloc_object_checked(KObject::KFunction(kf_ref), &types)
        .expect("closure co-located with its captured scope");
    // The bind-time mint: `home` materializes into the reader's arena as the entry's reach, with
    // the owning bundle folded into the reader region's union. The read then lifts that entry —
    // upgrading the description's members `Weak → Rc` — into an envelope hosted by the reader.
    let reach = reader_scope.mint_retained(&[&FrameCoverage::of(Rc::clone(home))]);
    let sealed = reader_scope.seal_reaching(Carried::Object(obj), reach);
    reader_scope.lift_resident(sealed)
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
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    // Two closure homes and two reader frames — four distinct regions, no shared ancestry, each
    // dying on its own — plus the dest the list node lands in.
    let home_a = run_root_storage();
    let home_b = run_root_storage();
    let reader_a = run_root_storage();
    let reader_a_scope = run_root_bare(&reader_a);
    let reader_b = run_root_storage();
    let reader_b_scope = run_root_bare(&reader_b);
    let dest_frame: Rc<CallFrame> = CallFrame::new(scope); // the list node lands here.
    let types = test_run.types.clone();

    let acc0 = Delivered::seal(
        KoanRegion::yoke_branded::<AggBuildFamily, _>(dest_frame.storage_rc(), |region| {
            (region.handle(), Vec::new())
        }),
        dest_frame.storage_rc(),
        FrameCoverage::empty(),
    );
    // Fold each re-read element into the accumulator; the temporary source carrier drops after each
    // statement, leaving only the aggregate witness (reach union + materialized reader hosts)
    // holding the four regions.
    let source_a = delivered_reread_closure(&home_a, reader_a_scope);
    let acc1 = source_a.transfer_into::<AggBuildFamily, AggBuildFamily, _>(
        acc0,
        |_product, _region| true,
        |dep, (region, mut cells), _brand| {
            cells.push(Held::from_carried(dep));
            (region, cells)
        },
    );
    let source_b = delivered_reread_closure(&home_b, reader_b_scope);
    let acc2 = source_b.transfer_into::<AggBuildFamily, AggBuildFamily, _>(
        acc1,
        |_product, _region| true,
        |dep, (region, mut cells), _brand| {
            cells.push(Held::from_carried(dep));
            (region, cells)
        },
    );
    // The retention stand-in: the dest frame's storage, held past the shell drops below — the hold
    // the scheduler seeds at finalize.
    let dest_storage = dest_frame.storage_rc();
    let list: Witnessed<CarriedFamily, CarrierWitness> =
        acc2.into_cell()
            .unseal()
            .map_pinned(&dest_storage, |(region, cells), _token| {
                let owned_cells = crate::machine::core::FrameCoverage::empty();
                let region = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(region))
                    .with_holder(&owned_cells);
                Carried::Object(
                    region.alloc_object_folded(KObject::list_of_held(region, cells, &types)),
                )
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
            .elements()
            .iter()
            .map(|h| match h.object() {
                KObject::KFunction(f) => f.captured_scope().id,
                other => panic!(
                    "expected a KFunction cell, got {}",
                    other.ktype().name(&types)
                ),
            })
            .collect(),
        other => panic!("expected a List, got {}", other.ktype().name(&types)),
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
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    // A capturing frame and two capture-target frames — three distinct regions forming a reach tree.
    let frame_outer: Rc<CallFrame> = CallFrame::new(scope);
    let frame_1: Rc<CallFrame> = CallFrame::new(scope);
    let frame_2: Rc<CallFrame> = CallFrame::new(scope);
    let types = test_run.types.clone();

    // Fold the two inner closures into a list carrier over frame_outer's region — its witness derives to
    // {frame_outer, frame_1, frame_2} through the fold, never a hand-assembled union.
    let acc0 = Delivered::seal(
        KoanRegion::yoke_branded::<AggBuildFamily, _>(frame_outer.storage_rc(), |region| {
            (region.handle(), Vec::new())
        }),
        frame_outer.storage_rc(),
        FrameCoverage::empty(),
    );
    let source_1 = delivered_closure(&frame_1);
    let acc1 = source_1.transfer_into::<AggBuildFamily, AggBuildFamily, _>(
        acc0,
        |_product, _region| true,
        |dep, (region, mut cells), _brand| {
            cells.push(Held::from_carried(dep));
            (region, cells)
        },
    );
    let source_2 = delivered_closure(&frame_2);
    let acc2 = source_2.transfer_into::<AggBuildFamily, AggBuildFamily, _>(
        acc1,
        |_product, _region| true,
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
    let source_outer = delivered_closure(&frame_outer);
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let captured = source_outer
        .transfer_into_placing::<AggBuildFamily, CarriedFamily, _>(
            acc2,
            |_product, _region| true,
            |outer_v, (_region, cells), placement| {
                let region = FoldingBrand::in_fold_closure(placement).with_holder(&owned_cells);
                if let KObject::KFunction(kf) = outer_v.object() {
                    let list_obj =
                        region.alloc_object_folded(KObject::list_of_held(region, cells, &types));
                    kf.captured_scope()
                        .bind_resident_for_test(
                            "inners".to_string(),
                            list_obj,
                            BindingIndex::BUILTIN,
                            &mut crate::machine::WriteGate::for_test(),
                        )
                        .expect("bind the inners list into the outer closure's scope");
                }
                outer_v
            },
        )
        .into_cell()
        .unseal();

    drop(frame_outer);
    drop(frame_1);
    drop(frame_2);

    // Follow the outer closure's captured scope to the bound list and deref each inner closure's
    // captured scope — touching all three regions after they would have died without the minted
    // members plus the retained outer storage (the retention stand-in the read names).
    let ids: Vec<_> = captured.with_pinned(&outer_storage, |c| match c.object() {
        KObject::KFunction(outer) => match outer.captured_scope().lookup("inners") {
            Some(KObject::List(items, _)) => items
                .elements()
                .iter()
                .map(|h| match h.object() {
                    KObject::KFunction(f) => f.captured_scope().id,
                    other => panic!(
                        "expected a KFunction cell, got {}",
                        other.ktype().name(&types)
                    ),
                })
                .collect(),
            _ => panic!("`inners` must be bound to a list of closures"),
        },
        other => panic!("expected a KFunction, got {}", other.ktype().name(&types)),
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
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    // Two independent frames whose closures the record's fields reach, plus the dest it lands in.
    let frame_a: Rc<CallFrame> = CallFrame::new(scope);
    let frame_b: Rc<CallFrame> = CallFrame::new(scope);
    let dest_frame: Rc<CallFrame> = CallFrame::new(scope);
    let types = test_run.types.clone();

    // Fold each field's closure into a named-cell accumulator over the dest region; the record's witness
    // derives to {dest ∪ frame_a ∪ frame_b} through the fold, never a hand-assembled union.
    let acc0 = Delivered::seal(
        KoanRegion::yoke_branded::<RecordCellFamily, _>(dest_frame.storage_rc(), |region| {
            (region.handle(), Vec::new())
        }),
        dest_frame.storage_rc(),
        FrameCoverage::empty(),
    );
    let source_a = delivered_closure(&frame_a);
    let acc1 = source_a.transfer_into::<RecordCellFamily, RecordCellFamily, _>(
        acc0,
        |_product, _region| true,
        |dep, (region, mut cells), _brand| {
            cells.push(("a".to_string(), Held::from_carried(dep)));
            (region, cells)
        },
    );
    let source_b = delivered_closure(&frame_b);
    let acc2 = source_b.transfer_into::<RecordCellFamily, RecordCellFamily, _>(
        acc1,
        |_product, _region| true,
        |dep, (region, mut cells), _brand| {
            cells.push(("b".to_string(), Held::from_carried(dep)));
            (region, cells)
        },
    );
    let dest_storage = dest_frame.storage_rc();
    let record: Witnessed<CarriedFamily, CarrierWitness> =
        acc2.into_cell()
            .unseal()
            .map_pinned(&dest_storage, |(region, cells), _token| {
                let owned_cells = crate::machine::core::FrameCoverage::empty();
                let region = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(region))
                    .with_holder(&owned_cells);
                Carried::Object(region.alloc_object_folded(KObject::record_of_held(
                    region,
                    Record::from_pairs(cells),
                    &types,
                )))
            });

    drop(frame_a);
    drop(frame_b);
    drop(dest_frame);

    // Read each field's closure back, dereferencing its captured scope — a use-after-free if either
    // field's region were dropped from the minted set (the retained dest storage pins the rest).
    let ids: Vec<_> = record.with_pinned(&dest_storage, |c| match c.object() {
        KObject::Record(substrate, _) => substrate
            .cells()
            .iter()
            .map(|h| match h.object() {
                KObject::KFunction(f) => f.captured_scope().id,
                other => panic!(
                    "expected a KFunction field, got {}",
                    other.ktype().name(&types)
                ),
            })
            .collect(),
        other => panic!("expected a Record, got {}", other.ktype().name(&types)),
    });
    assert_eq!(
        ids.len(),
        2,
        "both record fields read back after their frames freed"
    );
}

/// **`alloc_carried_with`'s single-dep reach fold, exercised through the actual finish-surface
/// combinator.** A dep terminal's object — the stand-in for `t.value`/`t.carrier` — is sealed as
/// the step's own carrier by rebuilding it at the fold brand from the dep's view in a *different*
/// frame's region. The fold unions the producer's reach into the result's witness; every
/// producer-frame handle then drops, and reading the sealed object back through the arena
/// reference must not dangle. Fails on UB, not values — the closing case for the reach hole an
/// unfolded allocation leaves open.
#[test]
fn object_field_reach_fold_survives_producer_frame_free() {
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let types = test_run.types.clone();

    // Producer: a closure resident in its own frame's region. A `KFunction` borrows its captured
    // scope, so the pointee is a genuine region borrow — the dangle the fold has to prevent.
    let producer_frame: Rc<CallFrame> = CallFrame::new(scope);
    let obj: &KObject<'_> = alloc_home_closure(&producer_frame);
    let expected_id = match obj {
        KObject::KFunction(f) => f.captured_scope().id,
        other => panic!("expected a KFunction, got {}", other.ktype().name(&types)),
    };
    let dep: DeliveredCarried = Delivered::seal(
        producer_frame.brand().seal_resident(Carried::Object(obj)),
        producer_frame.storage_rc(),
        FrameCoverage::empty(),
    );

    // Consumer: a StepAllocator over a *different* frame — the finish surface's own region.
    // `alloc_carried_with` rebuilds the object at the fold brand from the dep's view and folds the
    // producer's reach into the sealed carrier's witness.
    let consumer_frame: Rc<CallFrame> = CallFrame::new(scope);
    let ctx = StepAllocator::over_frame(consumer_frame.storage_rc());
    // The dep's object rides into the result as a `Held` cell — a borrow into the producer's
    // region, which is exactly what the fold has to keep alive.
    let sealed: StepCarried = ctx.alloc_carried_with(&[&dep], |b, views| {
        let owned_cells = crate::machine::core::FrameCoverage::empty();
        let cells = vec![Held::from_carried(views[0])];
        Carried::Object(b.alloc_object_folded(KObject::list_of_held(
            b.with_holder(&owned_cells),
            cells,
            &types,
        )))
    });

    // Drop the dep envelope and every frame shell: only the fold (if it happened) keeps the
    // producer's region alive, through the set minted into the consumer arena — itself pinned by
    // the retained consumer storage (the retention stand-in the read names).
    let consumer_storage = consumer_frame.storage_rc();
    drop(dep);
    drop(producer_frame);
    drop(consumer_frame);

    // Read back through the sealed carrier's arena reference — the captured-scope read is what
    // dangles if the producer region was freed.
    let read = sealed.inspect_pinned(&consumer_storage, |c| match c.object() {
        KObject::List(items, _) => match items.elements()[0].object() {
            KObject::KFunction(f) => f.captured_scope().id,
            other => panic!(
                "expected a KFunction element, got {}",
                other.ktype().name(&types)
            ),
        },
        other => panic!("expected a List, got {}", other.ktype().name(&types)),
    });
    assert_eq!(
        read, expected_id,
        "captured scope read back after producer frame freed"
    );
}

/// FROM's own construction shape — [`record_projection::body`](crate::builtins::record_projection)
/// narrows a record's carried type by sharing its substrate borrow whole, built at the fold brand
/// from the delivered `record` operand's view (`alloc_carried_with`). This mirrors
/// `object_field_reach_fold_survives_producer_frame_free`'s `KFunction` shape one level up: the
/// substrate stays in the *producer's* region (never copied — `record_with_type` swaps only the
/// type handle), and the fold's reach union is what keeps that region alive once every producer
/// handle drops. A regression that copied the substrate instead of sharing it would still pass
/// (a copy is also readable); the pointer-identity assertion is what actually pins "shares, never
/// copies," while Miri is what catches a dangling read if the reach fold is skipped.
#[test]
fn record_retype_shares_substrate_across_producer_frame_free() {
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let types = test_run.types.clone();

    // Producer: a plain-data record resident in its own frame's region, born through the fold
    // door — the exact shape FROM's `record` operand arrives as. Allocated through the frame's own
    // brand (not a transient `with_scope` sub-brand), so the reference escapes at the frame's own
    // lifetime.
    let producer_frame: Rc<CallFrame> = CallFrame::new(scope);
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(
        producer_frame.brand().handle(),
    ))
    .with_holder(&owned_cells);
    let fields = Record::from_pairs(vec![
        ("x".to_string(), Held::Object(KObject::Number(1.0))),
        ("y".to_string(), Held::Object(KObject::Number(2.0))),
    ]);
    let obj: &KObject<'_> = door.alloc_object_folded(KObject::record_of_held(door, fields, &types));
    // `RecordSubstrate` is invariant in its lifetime, so the comparison casts through `usize` (see
    // `Residence::owns_substrate`'s identical cast) rather than keeping a lifetime-parameterized raw
    // pointer type alive across the fold below.
    let expected_addr = match obj {
        KObject::Record(substrate, _) => *substrate as *const RecordSubstrate<'_> as usize,
        other => panic!("expected a Record, got {}", other.ktype().name(&types)),
    };
    let dep: DeliveredCarried = Delivered::seal(
        producer_frame.brand().seal_resident(Carried::Object(obj)),
        producer_frame.storage_rc(),
        FrameCoverage::empty(),
    );

    // Consumer: a different frame — FROM's own step surface, narrowing to just `{x}`.
    let consumer_frame: Rc<CallFrame> = CallFrame::new(scope);
    let ctx = StepAllocator::over_frame(consumer_frame.storage_rc());
    let narrowed_type = types.record(Record::from_pairs([("x".to_string(), KType::NUMBER)]));
    let sealed: StepCarried = ctx.alloc_carried_with(&[&dep], move |b, views| {
        let substrate = match views[0] {
            Carried::Object(KObject::Record(substrate, _)) => substrate,
            _ => panic!("expected a Record dep view"),
        };
        Carried::Object(b.alloc_object_folded(KObject::record_with_type(substrate, narrowed_type)))
    });

    // Drop the dep envelope and every frame shell: only the fold's minted reach (through the
    // retained consumer storage) keeps the producer's region alive.
    let consumer_storage = consumer_frame.storage_rc();
    drop(dep);
    drop(producer_frame);
    drop(consumer_frame);

    let read_addr = sealed.inspect_pinned(&consumer_storage, |c| match c.object() {
        KObject::Record(substrate, record_type) => {
            assert_eq!(
                *record_type, narrowed_type,
                "narrowed to the FROM-selected type"
            );
            *substrate as *const RecordSubstrate<'_> as usize
        }
        other => panic!("expected a Record, got {}", other.ktype().name(&types)),
    });
    assert_eq!(
        read_addr, expected_addr,
        "the narrowed record shares the exact same substrate borrow — never copies — read back \
         after the producer frame freed"
    );
}

/// The **single escape seam** re-stamp: [`Delivered::restamp_in_place`](crate::witnessed::Delivered::restamp_in_place)
/// re-tags a declared substrate return's top node to its declared type and re-anchors it into the
/// *producer's own region*, sharing the substrate borrow verbatim — the exact `finalize_terminal`
/// `Disposition::Restamp` motion. Because the destination is the value's own home region, the self
/// rule strips that region from the **owned** bundle the composition retains there: the description
/// hosted inside the producer region names it only weakly, so nothing strong pins the producer
/// region from a set hosted *inside* it. That is the leak shape this pins — a regression that
/// retained a strong self-pin would self-cycle the producer region (it never drops → a Miri leak).
/// The value must also read back soundly, sharing the same substrate pointer, in its own producer
/// region after every intermediate handle drops.
#[test]
fn restamp_in_place_shares_substrate_and_self_rule_strips_the_owned_self_pin() {
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let types = test_run.types.clone();

    // Producer: a plain-data record resident in its own frame's region, born through the fold door —
    // the shape a declared substrate return arrives as at the Done boundary.
    let producer_frame: Rc<CallFrame> = CallFrame::new(scope);
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(
        producer_frame.brand().handle(),
    ))
    .with_holder(&owned_cells);
    let fields = Record::from_pairs(vec![("a".to_string(), Held::Object(KObject::Number(3.0)))]);
    let obj: &KObject<'_> = door.alloc_object_folded(KObject::record_of_held(door, fields, &types));
    let expected_addr = match obj {
        KObject::Record(substrate, _) => *substrate as *const RecordSubstrate<'_> as usize,
        other => panic!("expected a Record, got {}", other.ktype().name(&types)),
    };
    let envelope: DeliveredCarried = Delivered::seal(
        producer_frame.brand().seal_resident(Carried::Object(obj)),
        producer_frame.storage_rc(),
        FrameCoverage::empty(),
    );

    // The declared type the return re-stamps to — a distinct handle for the same record shape.
    let declared = types.record(Record::from_pairs([("a".to_string(), KType::NUMBER)]));

    // Re-stamp in place: re-tag the top node to `declared`, re-anchored into the producer's own
    // region through the folded placement — the substrate rides shared (`deep_clone` pointer-copies
    // it, `stamp_type` swaps only the handle).
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let restamped: DeliveredCarried = envelope
        .restamp_in_place::<CarriedFamily, KoanStorageProfile>(
            &producer_frame.storage_rc(),
            |value, _handle, placement| {
                let region = FoldingBrand::in_fold_closure(placement).with_holder(&owned_cells);
                Carried::Object(
                    region.alloc_object_folded(
                        value.object().deep_clone().stamp_type(declared, &types),
                    ),
                )
            },
        );
    // The composed carrier references a description hosted in the producer's own region naming
    // that region — membership stays exact — but every member is `Weak`, and the self rule left the
    // retained owned bundle empty, so the region holds no strong pin on its own owner. The drop
    // below (and Miri's leak check) is what proves it.
    assert!(
        restamped.open_at().has_reach_members(),
        "membership is exact: the restamped value's own home is an ordinary member"
    );

    // The producer storage is the sole pin: the re-stamped value lives in its own region, so the
    // product envelope's own pins are dropped here and the seal read below names that storage.
    let restamped: Sealed<CarriedFamily, CarrierWitness> = restamped.into_cell();
    let producer_storage = producer_frame.storage_rc();
    drop(envelope);
    drop(producer_frame);

    let read_addr = restamped.open_with(&producer_storage, |c| match c.object() {
        KObject::Record(substrate, record_type) => {
            assert_eq!(*record_type, declared, "re-stamped to the declared type");
            *substrate as *const RecordSubstrate<'_> as usize
        }
        other => panic!("expected a Record, got {}", other.ktype().name(&types)),
    });
    assert_eq!(
        read_addr, expected_addr,
        "the re-stamped record shares the exact same substrate borrow — never copies — read back in \
         its own producer region"
    );
    // Dropping `producer_storage` here frees the region; Miri confirms 0 leaks (no self-cycle).
}

// `FrameReach::mint` — the witness-set hosting substrate (design/witness-hosting.md § Composition).
// Each test below pins one rule of the mint's composition (exact membership, the self rule,
// outer-chain subsumption, precise reads, teardown release). The mint returns the
// hosted (`Weak`-membered) description alongside the owned `FrameCoverage` bundle that pins its members;
// each test keeps that bundle alive across its `members()` read.

/// The mint composes its inputs' **exact** member set — two unrelated frames materialized as
/// disjoint hosts both survive, with no coarsening. (AC: precise members.)
#[test]
fn mint_composes_exact_members() {
    let a = run_root_storage();
    let b = run_root_storage();
    let c = run_root_storage();

    let minted = c.brand().handle().mint_retained(&[
        &FrameCoverage::of(Rc::clone(&a)),
        &FrameCoverage::of(Rc::clone(&b)),
    ]);

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

/// The self rule: a source naming the destination's own region stays an **exact member** of the
/// stored description (membership is exact — home is an ordinary member) but is stripped from the
/// **owned bundle** that rides out, since a region holding a pin on its own owner is a cycle.
#[test]
fn mint_self_rule_strips_dest_from_the_bundle_only() {
    let c = run_root_storage();
    let count_before = Rc::strong_count(&c);

    let minted = c
        .brand()
        .handle()
        .mint_retained(&[&FrameCoverage::of(Rc::clone(&c))]);

    assert_eq!(
        minted.members().len(),
        1,
        "dest's own region stays an exact member — home is an ordinary member"
    );
    // The owned bundle is retained into `c`'s own region, so a surviving self member would show up
    // as a strong ref `c` holds on itself — the cycle the self rule forbids.
    assert_eq!(
        Rc::strong_count(&c),
        count_before,
        "the owned bundle never pins the destination region into itself"
    );
}

/// A source member foreign to `dest` lands in both the stored description and the owned bundle;
/// one naming `dest`'s own region lands in the description but is stripped from the bundle by the
/// self rule.
#[test]
fn mint_materializes_foreign_host() {
    let a = run_root_storage();
    let c = run_root_storage();

    let minted_into_c = c
        .brand()
        .handle()
        .mint_retained(&[&FrameCoverage::of(Rc::clone(&a))]);
    assert_eq!(minted_into_c.members().len(), 1, "A is foreign to C");
    assert!(std::ptr::eq(
        minted_into_c.members()[0].region(),
        a.region()
    ));

    let a_count_before = Rc::strong_count(&a);
    let minted_into_a = a
        .brand()
        .handle()
        .mint_retained(&[&FrameCoverage::of(Rc::clone(&a))]);
    assert_eq!(
        minted_into_a.members().len(),
        1,
        "A stays an exact member of the description minted into A"
    );
    assert_eq!(
        Rc::strong_count(&a),
        a_count_before,
        "the self rule strips A from the bundle retained into A"
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

    let minted = c.brand().handle().mint_retained(&[
        &FrameCoverage::of(Rc::clone(&a)),
        &FrameCoverage::of(Rc::clone(&b)),
    ]);

    let members = minted.members();
    let [sole] = members.as_slice() else {
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

    let minted = c
        .brand()
        .handle()
        .mint_retained(&[&FrameCoverage::of(Rc::clone(&a))]);

    let regions: Vec<*const KoanRegion> = minted
        .members()
        .iter()
        .map(|m| m.region() as *const _)
        .collect();
    assert_eq!(regions, vec![a.region() as *const _]);
}

/// A mint lands in the destination's reach **side table**, never its family arena pages — so the
/// counted `alloc_count()` (the value families) is untouched. Reach descriptions are `Drop`-bearing
/// heap data hosted beside the arena, not in it (the storage-move invariant).
#[test]
fn mint_leaves_arena_pages_untouched() {
    let a = run_root_storage();
    let c = run_root_storage();

    let before = c.region().alloc_count();
    let _minted = c
        .brand()
        .handle()
        .mint_retained(&[&FrameCoverage::of(Rc::clone(&a))]);
    assert_eq!(
        c.region().alloc_count(),
        before,
        "a minted reach set lives in the side table, not a counted value-family arena"
    );
}

/// Teardown releases a retained bundle's members: the side-table description names its members with
/// `Weak`, so the owned `FrameCoverage` bundle carries the strong pins. Retaining that bundle in `C`'s
/// region ([`RegionHandle::retain_reach`]) makes `C` the members' liveness home; dropping `C`'s
/// storage drops the retained bundle, decrementing each member's refcount. No self-cycle
/// (the self rule forbids `C` from holding its own `Rc`), so the extra refs fall away at `C`'s death
/// — the shape the Miri leak audit exercises. (AC: teardown releasing members at region death.)
#[test]
fn mint_teardown_releases_members() {
    let a = per_call_storage();
    let b = per_call_storage();
    let c = run_root_storage();

    let count_before_a = Rc::strong_count(&a);
    let count_before_b = Rc::strong_count(&b);

    {
        // The mint retains its own bundle in `c`'s region for that region's whole life — the
        // liveness home a resident value's reach rides while the side-table description only names
        // the members.
        let minted = c.brand().handle().mint_retained(&[
            &FrameCoverage::of(Rc::clone(&a)),
            &FrameCoverage::of(Rc::clone(&b)),
        ]);
        assert_eq!(minted.members().len(), 2);
    }
    assert_eq!(
        Rc::strong_count(&a),
        count_before_a + 1,
        "C's retained bundle holds the sole remaining extra ref to A"
    );
    assert_eq!(
        Rc::strong_count(&b),
        count_before_b + 1,
        "C's retained bundle holds the sole remaining extra ref to B"
    );

    drop(c);
    assert_eq!(Rc::strong_count(&a), count_before_a, "C's death releases A");
    assert_eq!(Rc::strong_count(&b), count_before_b, "C's death releases B");
}

/// The checked seal's family audit admits a `KObject::KExpression`: an AST node names no producer
/// region, so a cell pointing at program text pins nothing the empty (own-region-only) witness this
/// door seals under would have to name. The quote-capture lane
/// (`dispatch::single_poll::literal_pass_through`) stores every quoted body through this door.
#[test]
fn raw_expression_passes_the_checked_object_seal() {
    use crate::machine::model::{ExpressionPart, KExpression};
    use crate::source::Spanned;

    let program = program_storage();
    let brand = program.brand().region();
    let storage = run_root_storage();
    let scope = run_root_bare(&storage);
    let types = TypeRegistry::new();

    let expression = KExpression::new(
        brand,
        vec![Spanned::bare(ExpressionPart::Identifier(
            brand.alloc_text("x"),
        ))],
    );
    let result = scope
        .brand()
        .alloc_object_witnessed_checked(KObject::KExpression(expression), &types);
    assert!(
        result.is_ok(),
        "raw AST reaches no producer region, so the checked seal admits it"
    );
}

/// `KObject::record_of_held` — the record door's read half — stores a fresh `RecordSubstrate`
/// through `FoldingBrand::alloc_substrate_folded` into its own brand's region. The stored address is
/// a hit for both the bare `KoanRegionExt::owns_substrate` query and `Residence::owns_substrate`'s
/// dest-only case, the read halves the door's store makes true.
#[test]
fn alloc_substrate_folded_stores_and_owns_a_record_substrate() {
    let frame = run_root_storage();
    let types = TypeRegistry::new();
    let acc0: Witnessed<AggBuildFamily, CarrierWitness> =
        KoanRegion::yoke_branded::<AggBuildFamily, _>(Rc::clone(&frame), |region| {
            (region.handle(), Vec::new())
        });
    let stored: Witnessed<CarriedFamily, CarrierWitness> =
        acc0.map_pinned(&frame, |(region, _cells), _token| {
            let owned_cells = crate::machine::core::FrameCoverage::empty();
            let door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(region))
                .with_holder(&owned_cells);
            let fields =
                Record::from_pairs(vec![("x".to_string(), Held::Object(KObject::Number(1.0)))]);
            Carried::Object(door.alloc_object_folded(KObject::record_of_held(door, fields, &types)))
        });
    let (owns_bare, owns_via_residence) = stored.with_pinned(&frame, |c| match c.object() {
        KObject::Record(substrate, _) => {
            let region = frame.region();
            let ptr = *substrate as *const RecordSubstrate<'_>;
            (
                region.owns_substrate(ptr),
                super::Residence::with_reach(region, &[]).owns_substrate(substrate),
            )
        }
        other => panic!("expected a Record, got {}", other.ktype().name(&types)),
    });
    assert!(
        owns_bare,
        "alloc_substrate_folded stores into its own brand's region"
    );
    assert!(
        owns_via_residence,
        "Residence::owns_substrate's dest-only case hits the same store"
    );
}

/// `resident_in_visiting`'s `Record` arm — `residence.owns_substrate(substrate)` — is reached only
/// when a record rides inside another substrate carrier (`List`/`Dict`/`Tagged`/`Wrapped`) crossing
/// the checked tier: a bare top-level record never routes this walk (born resident by
/// construction through the fold door). This drives a `List` embedding a `Record` through
/// `Scope::store_value_reaching_for_test` twice — once with evidence naming the record's home region
/// (must pass, reading the address table, never the record's fields) and once without (must
/// reject) — proving the arm is a genuine O(1) membership check, not an always-true stand-in.
#[test]
fn record_nested_in_list_crosses_checked_tier_via_owns_substrate_membership() {
    let producer = run_root_storage();
    let types = TypeRegistry::new();

    let list_obj: KObject = {
        let owned_cells = crate::machine::core::FrameCoverage::empty();
        let door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(
            producer.brand().handle(),
        ))
        .with_holder(&owned_cells);
        let fields =
            Record::from_pairs(vec![("x".to_string(), Held::Object(KObject::Number(1.0)))]);
        let record = KObject::record_of_held(door, fields, &types);
        KObject::list_of_held(door, vec![Held::Object(record)], &types)
    };

    let consumer_storage = run_root_storage();
    let consumer_scope = run_root_bare(&consumer_storage);

    // Covered: evidence names `producer`'s region — the nested record's home. Minting `producer`
    // (foreign to the consumer) into the consumer region yields a hosted description naming it;
    // `_covering_pins` keeps the member pinned across the `store_value_reaching_for_test` read below.
    let covering = consumer_storage
        .brand()
        .handle()
        .mint_retained(&[&FrameCoverage::of(Rc::clone(&producer))]);
    let moved = consumer_scope
        .store_value_reaching_for_test(list_obj.deep_clone(), covering, &types)
        .expect("evidence naming the record's home region covers it via owns_substrate membership");
    match moved {
        KObject::List(items, _) => match items.elements()[0].object() {
            KObject::Record(substrate, _) => match substrate.field("x").map(|h| h.object()) {
                Some(KObject::Number(n)) => {
                    assert_eq!(*n, 1.0, "the nested record reads back unchanged")
                }
                _ => panic!("expected field x: Number"),
            },
            other => panic!(
                "expected a Record element, got {}",
                other.ktype().name(&types)
            ),
        },
        other => panic!("expected a List, got {}", other.ktype().name(&types)),
    }

    // Uncovered: no evidence names the record's home region, and it is foreign to `consumer`'s
    // own region too — the audit must reject rather than silently accept. A description minted with
    // no sources is the region-pure evidence: hosted in the consumer's region, naming nothing.
    let no_evidence = consumer_storage.brand().handle().mint_retained(&[]);
    let rejected =
        consumer_scope.store_value_reaching_for_test(list_obj.deep_clone(), no_evidence, &types);
    assert!(
        rejected.is_err(),
        "a nested record foreign to dest and evidence must be rejected, not silently accepted"
    );
}

/// [`KoanRegionExt::allocated_total`] weights each family by the flat size of its stored form:
/// three fresh `KObject` allocations raise the total by exactly three `KObject` widths.
#[test]
fn allocated_total_weights_families_by_size() {
    let storage = run_root_storage();
    let before = storage.region().allocated_total();

    for n in 0..3 {
        storage.brand().alloc_object(KObject::Number(n as f64));
    }

    let after = storage.region().allocated_total();
    assert_eq!(
        after - before,
        3 * std::mem::size_of::<KObject<'static>>() as u64,
        "three KObject allocations add three KObject widths"
    );
}
