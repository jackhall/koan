//! Targeted Miri coverage for the region doors this module fronts. Each test pins down a
//! specific aliasing/lifetime shape under tree borrows; logical assertions are minimal
//! — these tests fail when Miri reports UB, not on values.

use super::*;
use crate::builtins::test_support::{TestRun, per_call_storage, run_root_bare};
use crate::machine::BindingIndex;
use crate::machine::CarrierWitness;
use crate::machine::DeliveredCarried;
use crate::machine::core::Bindings;
use crate::machine::core::{Action, Body, KFunction};
use crate::machine::model::KType;
use crate::machine::model::Record;
use crate::machine::model::Scalar;
use crate::machine::model::TypeRegistry;
use crate::machine::model::values::RecordSubstrate;
use crate::machine::model::{Argument, ReturnType, SignatureDraft, SignatureElement};
use crate::machine::model::{Carried, CarriedFamily, Held, KObject};
use crate::machine::model::{Module, ModuleDraft, SigSchema};
use crate::witnessed::{Delivered, FoldedPlacement, RegionHost, Sealed, WitnessRegion, Witnessed};

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

/// `CallFrame::pins_scope_region` reads the **storage** chain, not the lexical scope graph: a frame
/// pins its own child scope's region, answers `true` for an eternal-tier scope (which needs no pin
/// at all), and `false` for a scope in an unrelated per-call region.
#[test]
fn pins_scope_region_reads_the_storage_chain() {
    let root = run_root_storage();
    let run_scope = run_root_bare(&root);
    let frame = CallFrame::new(run_scope);

    assert!(
        frame.with_scope(|child| frame.pins_scope_region(child)),
        "a frame pins the region its own child scope lives in"
    );
    assert!(
        frame.pins_scope_region(run_scope),
        "an eternal-tier scope needs no pin, so every frame answers for it"
    );

    // A per-call region no frame in this chain owns: the chain walk finds nothing.
    let unrelated_storage = per_call_storage();
    let unrelated = run_root_bare(&unrelated_storage);
    assert!(
        !frame.pins_scope_region(unrelated),
        "a frame does not pin an unrelated per-call region"
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
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let frame: Rc<CallFrame> = CallFrame::new(scope);
    // Scalar copy-out: matches `scope_id`.
    let id = frame.with_scope(|s| s.id);
    assert_eq!(id, frame.scope_id());
    // In-place bind + lookup, all at the brand `'b` (value allocated via the opened scope's region).
    frame.with_scope(|s| {
        let v = s.brand().alloc_scalar(Scalar::Number(7.0));
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

/// The seed-side re-anchor: a caller-lifetime value crossing into the frame brand region as a
/// delivery envelope, whose bind relocates it there. The MATCH / TRY `it`-bind and the user-fn
/// param-bind take this shape — a bare caller-`'a` reference cannot cross `with_scope`'s `for<'b>`
/// signature at all, so the envelope is the whole route. Pins the relocate-into-the-brand-and-bind
/// aliasing under tree borrows.
#[test]
fn with_scope_relocates_seed_value_into_brand() {
    // The caller value is placed in its own, longer-lived region and enveloped there — mirroring the
    // matched `it` / a bound arg.
    let caller_storage = run_root_storage();
    let caller_scope = run_root_bare(&caller_storage);
    let it_carrier = caller_scope
        .deliver_pure_value(&KObject::Number(99.0))
        .expect("a Number is region-pure");
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let frame: Rc<CallFrame> = CallFrame::new(scope);
    frame.with_scope(|child| {
        child
            .bind_delivered_direct(
                "it".to_string(),
                &it_carrier,
                BindingIndex::BUILTIN,
                |carried| Ok(carried.object()),
                &mut crate::machine::WriteGate::for_test(),
            )
            .unwrap();
        assert!(matches!(child.lookup("it"), Some(KObject::Number(n)) if *n == 99.0));
    });
}

/// The born door's own erase-store / re-anchor round trip, nested inside an open: a grandchild
/// scope allocated through [`Scope::alloc_child_under`] at the frame brand comes back co-located,
/// and stays readable while its own brand mutates the same region underneath it. The store the door
/// routes is the substrate's, so the shape is exercised end to end with no hand-written pointer
/// arithmetic anywhere — the re-anchor is the library's single audited retype. The opened child's
/// own re-borrow rides along: it stays valid — and still names the frame's region — while a sibling
/// pointer allocates into that same region, so `with_scope`'s `&Scope` and `brand().alloc(…)`
/// coexisting soundly is pinned by the same run.
#[test]
fn born_child_scope_survives_subsequent_alloc_in_its_own_region() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let frame: Rc<CallFrame> = CallFrame::new(scope);
    frame.with_scope(|child| {
        let _sibling = child.brand().alloc_scalar(Scalar::Number(1.0));
        assert!(std::ptr::eq(child.region(), frame.region()));
        let grandchild = child.alloc_child_under();
        assert!(std::ptr::eq(grandchild.region(), child.region()));
        // Alloc into the region through the freshly stored scope's own brand while the parent
        // reference stays live — one region under two co-located references.
        let it_obj: &KObject<'_> = grandchild.brand().alloc_scalar(Scalar::Number(42.0));
        grandchild
            .bind_resident_for_test(
                "it".to_string(),
                it_obj,
                BindingIndex::BUILTIN,
                &mut crate::machine::WriteGate::for_test(),
            )
            .unwrap();
        assert!(matches!(grandchild.lookup("it"), Some(KObject::Number(n)) if *n == 42.0));
        // The parent re-reads correctly after its child's store appended to the same family cell.
        assert!(std::ptr::eq(
            grandchild.outer().expect("the born child names its parent"),
            child
        ));
    });
}

/// Two-deep chain: dropping the local `outer` handle leaves only `inner`'s `FrameStorage.outer`
/// keeping the outer region alive while we read through `inner`'s child scope's `outer`. The
/// UB-shaped twin — a crossing store pinned by the destination's own host, read back after every
/// direct handle on the operand drops — is
/// `the_born_with_door_accepts_the_childs_own_host_as_the_pin` in the workgraph slate; this one
/// pins the same chain over koan's `CallFrame` and runs under plain `cargo test`.
#[test]
fn call_frame_chained_outer_frame_walkable() {
    let program = program_storage();
    let region = run_root_storage();
    let run_test_run = TestRun::silent(&program, &region);
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
    let program = program_storage();
    let region = run_root_storage();
    let run_test_run = TestRun::silent(&program, &region);
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
    let program = program_storage();
    let region = run_root_storage();
    let run_test_run = TestRun::silent(&program, &region);
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
    let program = program_storage();
    let region = run_root_storage();
    let run_test_run = TestRun::silent(&program, &region);
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

/// `KType` is a `Copy` content-digest handle — constructing one is not a region allocation.
#[test]
fn ktype_construction_is_not_a_region_allocation() {
    let storage = run_root_storage();
    let a = storage.brand();
    let baseline = a.region().allocated_total();
    let t: KType = KType::NUMBER;
    assert!(t == KType::NUMBER);
    assert_eq!(
        a.region().allocated_total(),
        baseline,
        "a handle names registry-owned content: neither a sub-arena nor the bump grows"
    );
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

/// Workload-level accumulator carrier for the aggregate construction fold: the dest region the
/// finished aggregate node lands in, paired with the element cells built so far — re-bumped into
/// that region each step, so the accumulator rests on the Copy tier between folds. The
/// production family the object-family construction inversion uses lives in the execute layer; this
/// is the spike stand-in that proves the carrier round-trips and the fold composition is sound.
struct AggBuildFamily;
crate::witnessed::reattachable!(AggBuildFamily => (RegionHandle<'r, KoanStorageProfile>, &'r [Held<'r>]));

/// The **aggregate** construction fold: a list / dict / record built from several dep producers —
/// the shape the object family folds with shipped verbs only (no new substrate primitive). The
/// accumulator is `yoke`d empty over the dest frame's region; each foreign dep's
/// `Delivered` envelope is folded in with
/// [`Delivered::transfer_into`](crate::witnessed::Delivered::transfer_into), which re-anchors it at
/// the shared brand, binds it into the cells, and re-seals under the union of
/// every reached region (a `FrameReach` set witness — the multi-foreign case a single-region witness
/// cannot represent); a final [`project`](crate::witnessed::Delivered::project) allocates the list
/// node into the carried region under the envelope's own pins.
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
            (region.handle(), &[][..])
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
        |dep, (region, cells), placement| {
            let mut grown = Vec::with_capacity(cells.len() + 1);
            grown.extend_from_slice(cells);
            grown.push(Held::from_carried(dep));
            (region, placement.allocator().slice(&grown))
        },
    );
    let acc2 = dep_b.transfer_into::<AggBuildFamily, AggBuildFamily, _>(
        acc1,
        |_product, _region| true,
        |dep, (region, cells), placement| {
            let mut grown = Vec::with_capacity(cells.len() + 1);
            grown.extend_from_slice(cells);
            grown.push(Held::from_carried(dep));
            (region, placement.allocator().slice(&grown))
        },
    );
    // Allocate the list node from the carried dest region; the cells ride borrows into both foreign
    // regions, both now minted as members into the dest arena. A production `project` closure only
    // *selects* a part of the value — a placement is mintable only by a fold engine — so the door
    // here comes from the test-only forge over the operand's own head handle. That keeps the
    // envelope's coverage correct for the same reason selection does: the product is built into the
    // envelope's home region, which its pins already name.
    let list = acc2.project::<CarriedFamily>(|(region, cells), _token| {
        let owned_cells = crate::machine::core::FrameCoverage::empty();
        let region = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(region))
            .with_holder(&owned_cells);
        Carried::Object(region.alloc_object_folded(KObject::list_of_held(region, cells, &types)))
    });
    // Drop the producer handles: the dest arena's minted set solely owns both foreign regions; the
    // dest region itself rides the envelope's own pins, which cover the read.
    drop(frame_a);
    drop(frame_b);
    let got = list.open_ref(|c| match c.object() {
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
        let obj = scope.brand().alloc_scalar(Scalar::Number(1.0));
        // The bind-door mint: derive the exact reach into `dest`'s arena and fold the owning bundle
        // into `dest`'s region union. `foreign` is not the dest, so the self rule keeps it.
        let reach = scope.mint_retained(&[&FrameCoverage::of(Rc::clone(&foreign))]);
        let bindings = Bindings::new(scope.brand());
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

        // A table has no destructor to run at all — its storage is the region's bump — so letting
        // the binding go out of scope releases nothing, which is the claim.
        assert!(!std::mem::needs_drop::<Bindings<'_>>());
        let _ = &bindings;
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

/// FROM's own construction shape — [`record_projection::body`](crate::builtins::record_projection)
/// narrows a record's carried type by sharing its substrate borrow whole, built at the fold brand
/// from the delivered `record` operand's view (`alloc_carried_with`). The combinator's
/// by-construction dep fold is pinned library-side (`alloc_with_folds_dep_reach_before_result_read`
/// in the workgraph slate); this exercises it over the `Record` substrate specifically: the
/// substrate stays in the *producer's* region (never copied — `record_with_type` swaps only the
/// type handle), and the fold's reach union is what keeps that region alive once every producer
/// handle drops. A regression that copied the substrate instead of sharing it would still pass
/// (a copy is also readable); the pointer-identity assertion is what actually pins "shares, never
/// copies," while Miri is what catches a dangling read if the reach fold is skipped.
#[test]
fn record_retype_shares_substrate_across_producer_frame_free() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
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
    // `RecordSubstrate` is invariant in its lifetime, so the comparison casts through `usize`
    // rather than keeping a lifetime-parameterized raw pointer type alive across the fold below.
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

    let read_addr = sealed.inspect_at(Rc::clone(&consumer_storage), |c| match c.object() {
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
/// `Disposition::Restamp` motion. The pointer-identity assertion is what pins "shares, never
/// copies" (`deep_clone` pointer-copies a `Record` substrate; `stamp_type` swaps only the handle);
/// the verb's self-rule leak shape and its re-anchored read are pinned library-side in the
/// workgraph slate (`restamp_in_place_keeps_home_in_the_description_but_pins_nothing_on_itself`).
#[test]
fn restamp_in_place_shares_substrate_and_self_rule_strips_the_owned_self_pin() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
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

    // The producer storage is the sole pin: the re-stamped value lives in its own region, so resting
    // it there retains nothing (the self rule strips the one member) and the seal read below names
    // that storage.
    let restamped: Sealed<CarriedFamily, CarrierWitness> =
        restamped.rest_into(RegionHandle::from_owner(&*producer_frame.storage_rc()));
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

// `FrameReach::mint` — the witness-set hosting substrate (workgraph/design/reach.md § Composition).
// Each test below pins one rule of the mint's composition (exact membership, the self rule,
// outer-chain subsumption, precise reads, teardown release). The mint returns the hosted
// (`Weak`-membered) description alone and retains the owned bundle that pins its members into the
// destination's own region, so each test reads `members()` under that region's retention.

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
    assert!(
        minted
            .members()
            .iter()
            .any(|m| std::ptr::eq(m.region(), a.region()))
    );
    assert!(
        minted
            .members()
            .iter()
            .any(|m| std::ptr::eq(m.region(), b.region()))
    );
}

/// The self rule: a source naming the destination's own region stays an **exact member** of the
/// stored description (membership is exact — home is an ordinary member) but is stripped from the
/// **owned bundle the mint retains there**, since a region holding a pin on its own owner is a
/// cycle.
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

/// A mint lands in the destination's reach **side table**, never its value storage — so the
/// region's bump, which is where every value now lives, is untouched. Reach descriptions are
/// `Drop`-bearing heap data hosted beside the bump, not in it (the storage-move invariant).
#[test]
fn mint_leaves_value_storage_untouched() {
    let a = run_root_storage();
    let c = run_root_storage();

    let before = c.region().bump_capacity();
    let _minted = c
        .brand()
        .handle()
        .mint_retained(&[&FrameCoverage::of(Rc::clone(&a))]);
    assert_eq!(
        c.region().bump_capacity(),
        before,
        "a minted reach set lives in the side table, not the region's value storage"
    );
}

/// Teardown releases a retained bundle's members: the side-table description names its members with
/// `Weak`, so the owned bundle the mint retains carries the strong pins. Minting into `C`'s region
/// ([`RegionHandle::mint_retained`]) makes `C` the members' liveness home; dropping `C`'s storage
/// drops the retained bundle, decrementing each member's refcount. No self-cycle
/// (the self rule forbids `C` from holding its own `Rc`), so the extra refs fall away at `C`'s death.
/// The refcount assertions fail loud under plain `cargo test`; the Miri leak sign-off for the
/// split-membership shape lives in the workgraph slate
/// (`mint_keeps_home_in_the_description_but_not_the_bundle`). (AC: teardown releasing members at
/// region death.)
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

/// The expression door bumps a `KObject::KExpression` into the brand's own region and seals it
/// resident: an AST node names no producer region, so a cell pointing at program text pins nothing
/// the empty (own-region-only) witness this door seals under would have to name. The quote-capture
/// lane (`dispatch::single_poll::literal_pass_through`) stores every quoted body through this door.
#[test]
fn raw_expression_seals_through_the_expression_door() {
    use crate::machine::model::ExpressionPart;
    use crate::source::Spanned;

    let program = program_storage();
    let brand = program.brand().region();
    let storage = run_root_storage();
    let scope = run_root_bare(&storage);

    let expression =
        program
            .brand()
            .new_expression(vec![Spanned::bare(ExpressionPart::Identifier(
                brand.allocator().text("x"),
            ))]);
    let carried = scope.brand().alloc_expression_witnessed(expression);
    let parts = carried.inspect_at(Rc::clone(&storage), |c| match c.object() {
        KObject::KExpression(e) => e.parts.len(),
        _ => panic!("the expression door stores an expression cell"),
    });
    assert_eq!(parts, 1, "the stored cell reads back as the quoted body");
}

/// `KObject::record_of_held` — the record door's read half — stores a fresh `RecordSubstrate`
/// through `FoldingBrand::alloc_substrate_folded` into its own brand's region. The stored
/// description names that region as the substrate's home, which is the residence claim the door's
/// brand makes and every later home-crossing read answers from.
#[test]
fn alloc_substrate_folded_homes_a_record_substrate_in_its_own_brand() {
    let frame = run_root_storage();
    let types = TypeRegistry::new();
    let acc0: Witnessed<AggBuildFamily, CarrierWitness> =
        KoanRegion::yoke_branded::<AggBuildFamily, _>(Rc::clone(&frame), |region| {
            (region.handle(), &[][..])
        });
    // The door is forged over the operand's own head handle (test-only: a production `project`
    // closure gets a bare `FoldToken` and so can only select), which is why the store lands in the
    // envelope's home region — exactly what the assertion below reads back.
    let stored = Delivered::seal(acc0, Rc::clone(&frame), FrameCoverage::empty())
        .project::<CarriedFamily>(|(region, _cells), _token| {
            let owned_cells = crate::machine::core::FrameCoverage::empty();
            let door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(region))
                .with_holder(&owned_cells);
            let fields =
                Record::from_pairs(vec![("x".to_string(), Held::Object(KObject::Number(1.0)))]);
            Carried::Object(door.alloc_object_folded(KObject::record_of_held(door, fields, &types)))
        });
    let homed = stored.open_ref(|c| match c.object() {
        KObject::Record(substrate, _) => substrate.homed_in(frame.region()),
        other => panic!("expected a Record, got {}", other.ktype().name(&types)),
    });
    assert!(
        homed,
        "alloc_substrate_folded stores into its own brand's region"
    );
}

/// [`KoanRegionExt::allocated_total`] weights each family by the flat size of its stored form, so a
/// batch of scalar allocations puts at least that many `KObject` widths into the total. A lower
/// bound rather than an exact delta because the total's other term is the bump's reserved capacity,
/// which grows in chunks of the allocator's own choosing — an exact figure would be asserting
/// bumpalo's private sizing policy.
#[test]
fn allocated_total_covers_the_families_it_weighs() {
    const BATCH: usize = 1000;
    let storage = run_root_storage();
    let before = storage.region().allocated_total();

    for n in 0..BATCH {
        storage.brand().alloc_scalar(Scalar::Number(n as f64));
    }

    let after = storage.region().allocated_total();
    assert!(
        after - before >= BATCH as u64 * std::mem::size_of::<KObject<'static>>() as u64,
        "{BATCH} KObject allocations add at least {BATCH} KObject widths"
    );
}

/// A bound **bare string** must not keep borrowing the producer region's bump bytes. A copying
/// adoption claims that region's release — `retains_home` answers `false` for a `KString`, so the
/// composition drops the producer from what it retains — and the bump keeps no address table, so
/// nothing downstream could catch a pointer copy that kept pointing there. The copy therefore has to
/// re-bump at the destination ([`KObject::needs_destination_door`] is the gate
/// [`relocate_object_into`](crate::machine::model::relocate_object_into) reads).
///
/// This binds a producer-resident string into a consumer scope, drops every handle on the producer,
/// and reads the bytes back: tree borrows reports a use-after-free if the copy pointer-copied the
/// producer's bump instead of rebuilding at the destination.
#[test]
fn a_bound_bare_string_rebumps_at_its_destination() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let consumer = test_run.scope;

    let producer: Rc<CallFrame> = CallFrame::new(consumer);
    let producer_scope = run_root_bare(producer.storage());
    // The bytes land in `producer`'s bump, so the value genuinely borrows into the region the copy
    // below claims to release.
    let text = producer_scope
        .fold_resident_object(|brand| KObject::KString(brand.allocator().text("koan")));
    let sealed = producer_scope
        .seal_reaching(Carried::Object(text), producer_scope.mint_born_here(false))
        .unseal();
    let dep: DeliveredCarried =
        Delivered::seal(sealed, producer.storage_rc(), FrameCoverage::empty());

    let bound = consumer
        .adopt_for_binding(&dep, |carried| Ok(carried.object()))
        .expect("a whole-value projection is infallible");

    // Drop every handle on the producer: the copy released it, so its region frees here.
    drop(dep);
    drop(producer);

    match bound.open_at(&root).value().object() {
        KObject::KString(s) => assert_eq!(
            *s, "koan",
            "the bound string reads back after the producer frees"
        ),
        other => panic!("expected a KString, got {:?}", other.ktype()),
    }
}

/// Region death for the `Drop`-free families is deallocation only. A frame region is filled with
/// every substrate shape — list, dict, record, and both payload carriers (`Tagged` and `Wrapped`) —
/// each carrying a string leaf so the bump holds re-homed bytes as well as cells and index metadata,
/// and with a run of **callables**, whose signatures put a bumped element run and re-homed keyword /
/// parameter-name bytes in the same region, with **modules**, whose paths, member-map keys and
/// member-table bucket arrays land there too, and with a chain of **scopes**, the one family that
/// keeps *mutating in place* after it is stored: its five binding tables and its SIG slot collector
/// grow past their resize thresholds against the same bump while the scope sits resident, so what
/// dies unfreed if a table's suppressed destructor was load-bearing is a bucket array written long
/// after the store. That is the leak claim a `Copy` bound cannot state — a scope is not `Copy`, and
/// only the `!needs_drop` asserts stand between it and a silently-stranded spine. The region is then
/// dropped while nothing outside it holds a borrow. No slot in any of those shapes has a destructor to run, so the whole teardown is the
/// bump's chunk free; Miri's process-exit leak check is the assertion. A family that quietly
/// reintroduced an owning slot — a `Vec` spine, a `String` name, an `Rc` — leaks its buffer here,
/// because a bump frees its chunks without visiting them. The signature names are synthesized rather
/// than literal so the bytes genuinely belong to this region: a `&'static` spelling would pass the
/// leak check without exercising the re-home it exists to pin — the same reason the module paths and
/// member names below are built rather than spelled.
#[test]
fn region_death_frees_every_drop_free_family() {
    use crate::machine::model::KKey;
    use std::collections::HashMap;
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let types = test_run.types.clone();

    let frame: Rc<CallFrame> = CallFrame::new(test_run.scope);
    let scope = run_root_bare(frame.storage());
    let owned_cells = FrameCoverage::empty();

    let shapes: Vec<&KObject<'_>> = vec![
        scope.fold_resident_object(|brand| {
            let door = brand.with_holder(&owned_cells);
            KObject::list_of_held(
                door,
                &[
                    Held::Object(KObject::KString(door.allocator().text("first"))),
                    Held::Object(KObject::Number(2.0)),
                ],
                &types,
            )
        }),
        scope.fold_resident_object(|brand| {
            let door = brand.with_holder(&owned_cells);
            let mut map: HashMap<KKey, Held> = HashMap::new();
            map.insert(
                KKey::String("key"),
                Held::Object(KObject::KString(door.allocator().text("value"))),
            );
            KObject::dict_of_held(door, map, &types)
        }),
        scope.fold_resident_object(|brand| {
            let door = brand.with_holder(&owned_cells);
            let fields = Record::from_pairs(vec![(
                "field".to_string(),
                Held::Object(KObject::KString(door.allocator().text("payload"))),
            )]);
            KObject::record_of_held(door, fields, &types)
        }),
        scope.fold_resident_object(|brand| {
            let door = brand.with_holder(&owned_cells);
            let inner = KObject::KString(door.allocator().text("tagged"));
            KObject::tagged(door, "Tag", &inner, KType::NULL)
        }),
        scope.fold_resident_object(|brand| {
            let door = brand.with_holder(&owned_cells);
            let inner = KObject::KString(door.allocator().text("wrapped"));
            KObject::wrapped_hold(door, &inner, KType::NULL)
        }),
    ];
    assert_eq!(
        shapes.len(),
        5,
        "every drop-free shape is live in the region"
    );
    assert!(
        frame.region().allocated_total() > 0,
        "the shapes genuinely occupy the region under test"
    );

    let brand = scope.brand();
    let callables: Vec<&KFunction<'_>> = (0..4)
        .map(|i| {
            let draft = SignatureDraft {
                return_type: ReturnType::Resolved(KType::NULL),
                elements: vec![
                    SignatureElement::Keyword(brand.allocator().text(&format!("TAKE_{i}"))),
                    SignatureElement::Argument(Argument {
                        name: brand.allocator().text(&format!("operand_{i}")),
                        ktype: KType::NUMBER,
                    }),
                ],
            };
            KFunction::alloc_captured(
                scope,
                draft,
                Body::Builtin(|ctx| {
                    Action::done_resident(
                        ctx.scope,
                        Carried::Object(ctx.scope.brand().alloc_scalar(Scalar::Null)),
                    )
                }),
                false,
                &types,
            )
        })
        .collect();
    assert_eq!(callables.len(), 4, "every callable is live in the region");

    // A module at a child scope of this region, born through its own door: the path bytes, every
    // member-map key's bytes and both member tables' bucket arrays land in the same bump. Synthesized
    // names again, for the same reason the callables' are.
    let modules: Vec<&Module<'_>> = (0..2)
        .map(|i| {
            let child = scope.alloc_child_under_module(&format!("member_module_{i}"), None);
            let mut draft = ModuleDraft::empty();
            draft
                .type_members
                .insert(format!("Member_{i}"), KType::NUMBER);
            draft
                .slot_type_tags
                .insert(format!("slot_{i}"), KType::NUMBER);
            let self_sig = types.signature(SigSchema::raw_self_sig(child, &draft));
            Module::alloc_at_child_scope(&format!("module_{i}"), child, draft, self_sig)
        })
        .collect();
    assert_eq!(modules.len(), 2, "every module is live in the region");
    assert!(
        modules[0].type_members.get(&"Member_0").is_some(),
        "the member map reads back by content through its re-homed keys",
    );

    // A chain of scopes in the same region, each mutating **after** it was bumped: the block child's
    // value and type tables and the group child's dispatch / operator tables are all pushed past
    // hashbrown's initial capacity, and the SIG child's slot collector is filled the same way. Every
    // one of those bucket arrays is a bump allocation made long after the scope itself landed, which
    // is the shape `Copy` cannot express and only the suppressed-destructor asserts hold in line.
    let mut gate = crate::machine::WriteGate::for_test();
    let block = scope.alloc_child_under();
    for i in 0..96 {
        let value = block.brand().alloc_scalar(Scalar::Number(i as f64));
        block
            .bind_resident_for_test(
                format!("value_{i}"),
                value,
                BindingIndex::value(i),
                &mut gate,
            )
            .expect("a fresh value bind lands");
    }
    assert!(block.bindings().lookup_value("value_95", None).is_some());

    let sig_child = block.alloc_child_under_sig("Shape");
    for i in 0..64 {
        sig_child
            .write_sig_slot(format!("slot_{i}"), KType::NUMBER)
            .expect("a fresh VAL slot records");
    }
    assert_eq!(sig_child.sig_value_slots().len(), 64);

    // A grandchild whose parent link, root link and region brand are all reads of bumped scopes —
    // so the walk itself dereferences the chain after every table above has reallocated.
    let leaf = sig_child.alloc_child_under();
    assert_eq!(leaf.ancestors().count(), 4);
    assert!(leaf.bindings().lookup_value("value_95", None).is_none());
    assert!(block.bindings().lookup_value("value_95", None).is_some());

    // Nothing outside the region borrows into it, so this is the whole of region death: every family
    // here lives in the bump, which frees its chunks without a destructor pass.
    drop(shapes);
    drop(callables);
    drop(modules);
    drop(frame);
}
