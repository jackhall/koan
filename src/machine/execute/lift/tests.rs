//! Tests for [`copy_carried`] — the witnessed-transfer copy hook. It structurally copies a
//! [`Carried`] into a destination region: the top node is re-allocated there, while the composite
//! spine shares its `Rc` payloads and a `KFunction` / first-class `Module` rides a bare
//! borrow preserved verbatim. No region anchor is embedded in the value — the regions a copied
//! value reaches are pinned by the carrier's witness set at the `transfer_into` layer, not here.

use super::*;
use crate::builtins::test_support::TestRun;
use crate::machine::core::{
    force_substrate_borrows_host, run_root_storage, FoldingBrand, KoanRegion, KoanRegionExt,
    KoanStorageProfile,
};
use crate::machine::model::Held;
use crate::machine::model::KType;
use crate::machine::model::Record;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{Carried, KObject};
use crate::machine::CallFrame;
use crate::machine::CarrierWitness;
use crate::witnessed::{reattachable, Delivered, Erased, FoldedPlacement, RegionHandle, Witnessed};
use std::rc::Rc;

/// A `KFunction` allocated into `home`'s region (its captured scope lives there), for the
/// borrow-preservation tests. The body is never run.
fn alloc_local_kf<'run>(home: &'run Rc<CallFrame>) -> &'run crate::machine::KFunction<'run> {
    use crate::machine::model::{ExpressionSignature, ReturnType, SignatureElement};
    use crate::machine::{Body, KFunction};
    // Capture the home frame's child scope (read at the brand), build the function there, and alloc it
    // into `home`'s region — where the captured scope genuinely lives — inside the open, so the re-homed
    // `&KFunction` escapes at `home`'s lifetime without a fixed-lifetime reattach. Mirrors a closure
    // capturing its defining scope in its own region.
    let types = crate::machine::model::TypeRegistry::new();
    home.with_scope(|child| {
        let kf = KFunction::new(
            ExpressionSignature {
                return_type: ReturnType::Resolved(KType::NULL),
                elements: vec![SignatureElement::Keyword("__INNER__".into())],
            },
            Body::Builtin(|ctx| {
                crate::machine::core::Action::done_resident(Carried::Object(
                    ctx.scope.brand().alloc_object(KObject::Null),
                ))
            }),
            child,
            false,
            &types,
        );
        home.brand().alloc_function(kf)
    })
}

/// The top node of a relocated `Carried::Object` is a fresh allocation owned by `dest`, not the
/// source — that relocation (so the copy outlives the producer's dying frame) is the whole point.
#[test]
fn object_top_node_relocates_into_dest() {
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);

    let obj: &KObject = source.brand().alloc_object(KObject::Number(2.5));
    let relocated = copy_carried(
        Carried::Object(obj),
        RegionEscape::Copy { released: false },
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle())),
    );
    match relocated {
        Carried::Object(r) => {
            assert!(dest.region().owns_object(r), "relocated node lives in dest");
            assert!(
                !std::ptr::eq(r, obj),
                "top node is a fresh allocation, not the source"
            );
            assert!(
                matches!(r, KObject::Number(n) if *n == 2.5),
                "value preserved"
            );
        }
        Carried::Type(_) | Carried::UnresolvedType(_) => panic!("expected an Object carrier"),
    }
}

/// A `List` relocated under a `Copy` verb is totally rebuilt at the destination brand: the rebuilt
/// element substrate lives in `dest`'s region, not the source's — a list is a region-resident
/// substrate, not a shared `Rc` spine.
#[test]
fn list_relocation_rebuilds_substrate_into_dest() {
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);
    let types = test_run.types.clone();

    let source_door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(source.brand().handle()));
    let list: &KObject = source_door.alloc_object_folded(KObject::list_of_held(
        source_door,
        vec![
            Held::Object(KObject::Number(1.0)),
            Held::Object(KObject::Number(2.0)),
        ],
        &types,
    ));

    let relocated = copy_carried(
        Carried::Object(list),
        RegionEscape::Copy { released: false },
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle())),
    );
    match relocated {
        Carried::Object(r @ KObject::List(out, _)) => {
            assert!(
                dest.region().owns_object(r),
                "relocated list node lives in dest"
            );
            assert!(
                dest.region().owns_substrate(*out),
                "the rebuilt element substrate lives in dest"
            );
            assert!(
                !source.region().owns_substrate(*out),
                "the source no longer owns the rebuilt substrate"
            );
        }
        Carried::Object(other) => panic!("expected a List, got {:?}", other.ktype()),
        Carried::Type(_) | Carried::UnresolvedType(_) => panic!("expected an Object carrier"),
    }
}

/// A `Dict` relocated under a `Copy` verb is totally rebuilt at the destination brand: the rebuilt
/// entry substrate lives in `dest`'s region, not the source's — a dict is a region-resident
/// substrate, not a shared `Rc` spine.
#[test]
fn dict_relocation_rebuilds_substrate_into_dest() {
    use crate::machine::model::KKey;
    use std::collections::HashMap;
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);
    let types = test_run.types.clone();

    let source_door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(source.brand().handle()));
    let mut map: HashMap<KKey, Held> = HashMap::new();
    map.insert(KKey::String("a".into()), Held::Object(KObject::Number(1.0)));
    let dict: &KObject =
        source_door.alloc_object_folded(KObject::dict_of_held(source_door, map, &types));

    let relocated = copy_carried(
        Carried::Object(dict),
        RegionEscape::Copy { released: false },
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle())),
    );
    match relocated {
        Carried::Object(r @ KObject::Dict(out, _)) => {
            assert!(
                dest.region().owns_object(r),
                "relocated dict node lives in dest"
            );
            assert!(
                dest.region().owns_substrate(*out),
                "the rebuilt entry substrate lives in dest"
            );
            assert!(
                !source.region().owns_substrate(*out),
                "the source no longer owns the rebuilt substrate"
            );
        }
        Carried::Object(other) => panic!("expected a Dict, got {:?}", other.ktype()),
        Carried::Type(_) | Carried::UnresolvedType(_) => panic!("expected an Object carrier"),
    }
}

/// A `Tagged` shares its `value` `Rc` through relocation, and its tag and interned `identity` type
/// handle ride along unchanged.
#[test]
fn tagged_relocation_shares_value_and_identity() {
    use crate::machine::core::ScopeId;
    use crate::machine::model::TypeNode;
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);
    let types = test_run.types.clone();

    let inner = Rc::new(KObject::Number(42.0));
    // The value's own type handle: a `Maybe` constructor applied to `Number` — the shape a tagged
    // union member's `identity` interns to.
    let ctor = types.intern(TypeNode::AbstractType {
        source: ScopeId::from_raw(0, 0x11),
        name: "Maybe".into(),
        param_names: vec!["T".into()],
        nonce: None,
    });
    let identity =
        types.constructor_apply(ctor, Record::from_pairs([("T".to_string(), KType::NUMBER)]));
    let tagged: &KObject = source
        .brand()
        .alloc_object_checked(
            KObject::Tagged {
                tag: "Just".into(),
                value: Rc::clone(&inner),
                identity,
            },
            &types,
        )
        .expect("a fresh owned Tagged is always resident-in-self");

    let relocated = copy_carried(
        Carried::Object(tagged),
        RegionEscape::Copy { released: false },
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle())),
    );
    match relocated {
        Carried::Object(
            r @ KObject::Tagged {
                tag,
                value,
                identity: out_identity,
            },
        ) => {
            assert!(
                dest.region().owns_object(r),
                "relocated tagged node lives in dest"
            );
            assert_eq!(tag, "Just");
            assert!(Rc::ptr_eq(value, &inner), "the wrapped value is shared");
            assert_eq!(
                *out_identity, identity,
                "the identity handle rides along unchanged"
            );
        }
        Carried::Object(other) => panic!("expected a Tagged, got {:?}", other.ktype()),
        Carried::Type(_) | Carried::UnresolvedType(_) => panic!("expected an Object carrier"),
    }
}

/// A `KFunction` rides a *bare* borrow preserved verbatim — relocation copies the reference, never
/// the closure (which may reference anything reachable from its captured scope). The borrow points
/// back into the source region; the carrier's witness set keeps that region alive.
#[test]
fn kfunction_borrow_preserved_verbatim() {
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);
    let types = test_run.types.clone();

    let kf_ref = alloc_local_kf(&source);
    let obj: &KObject = source
        .brand()
        .alloc_object_checked(KObject::KFunction(kf_ref), &types)
        .expect("f was just allocated into region\'s own region");

    let relocated = copy_carried(
        Carried::Object(obj),
        RegionEscape::Copy { released: false },
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle())),
    );
    match relocated {
        Carried::Object(r @ KObject::KFunction(f)) => {
            assert!(
                dest.region().owns_object(r),
                "the KFunction node relocated into dest"
            );
            assert!(
                std::ptr::eq(*f, kf_ref),
                "the function borrow is preserved verbatim"
            );
        }
        Carried::Object(other) => panic!("expected a KFunction, got {:?}", other.ktype()),
        Carried::Type(_) | Carried::UnresolvedType(_) => panic!("expected an Object carrier"),
    }
}

/// A recursive newtype's sealed member *type* handle relocates by copying its digest, and stays
/// navigable afterward: reading the relocated handle back through the registry still finds the
/// member's `children` field self-referencing the sealed `Tree` member. Guards against a relocated
/// type value losing its recursive self-edge.
#[test]
fn type_recursive_member_relocates_and_navigates() {
    use crate::machine::model::{NodeSchema, RecursiveGroupWindow, RelativeSchema, TypeNode};
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let dest = CallFrame::new(scope);
    let types = crate::machine::model::TypeRegistry::new();

    // A self-recursive `Tree` whose `children` field is `List(Tree)` — the shape a
    // `NEWTYPE Tree = :{children :(LIST OF Tree)}` seals into. The self-edge starts as `Sibling(0)`
    // and seals to the member's own absolute handle.
    let tree = RecursiveGroupWindow::seal_singleton(
        "Tree".into(),
        RelativeSchema::NewType(types.record(Record::from_pairs([(
            "children".to_string(),
            types.list(types.intern(TypeNode::Sibling(0))),
        )]))),
        None,
        &types,
    );
    let type_value = tree;

    let relocated = copy_carried(
        Carried::Type(type_value),
        RegionEscape::Copy { released: false },
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle())),
    );
    match relocated {
        Carried::Type(out) => {
            assert_eq!(
                out, tree,
                "relocation copies the member's digest handle unchanged"
            );
            // Navigable: reading the relocated handle back finds the member's `children` field
            // self-referencing the sealed `Tree` member.
            match types.node(out) {
                TypeNode::SetMember {
                    schema: NodeSchema::NewType(repr),
                    ..
                } => match types.node(repr) {
                    TypeNode::Record { fields } => assert_eq!(
                        fields.get("children"),
                        Some(&types.list(tree)),
                        "the relocated Tree's children field self-references the sealed Tree member",
                    ),
                    _ => panic!("expected a record repr, got {}", repr.name(&types)),
                },
                _ => panic!("expected a navigable NewType member, got {}", out.name(&types)),
            }
        }
        Carried::UnresolvedType(ti) => {
            panic!(
                "expected a member type, got the unlowered name {}",
                ti.render()
            )
        }
        Carried::Object(_) => panic!("expected a Type carrier"),
    }
}

/// Build-time accumulator for the aggregate-fold mirrors below: the destination region plus the
/// partial cell vector — a local twin of `dispatch::literal::AggBuildFamily` (private to that
/// module), reattached here so the tests can drive `fold_cells`'s own mechanism
/// (`copied_seam_mode` + `transfer_into_placing` + `copy_held_from_carried`) directly.
struct RecordAggFamily;
reattachable!(RecordAggFamily => (RegionHandle<'r, KoanStorageProfile>, Vec<Held<'r>>));

/// Accumulator twin for the value-level-seam pin mirror below: the destination region plus the
/// relocated `Carried` cells [`copy_carried`] produces (the value-level relocate that honors the
/// [`RegionEscape`], unlike the container-cell [`copy_held_from_carried`] which always rebuilds). Used
/// only by the pin mirror, which is gated out of the `seam-force-copy` build.
#[cfg(not(feature = "seam-force-copy"))]
struct PinAggFamily;
#[cfg(not(feature = "seam-force-copy"))]
reattachable!(PinAggFamily => (RegionHandle<'r, KoanStorageProfile>, Vec<Carried<'r>>));

/// A `KFunction` allocated into `home`'s region wrapped in a `Record` field, both born through
/// `home`'s own brand (not a transient `with_scope` sub-brand) so the reference escapes at `home`'s
/// own lifetime — the shape a list-literal cell born from `({f = (FN …)})` takes.
fn alloc_home_closure_record<'run>(
    home: &'run Rc<CallFrame>,
    types: &TypeRegistry,
) -> &'run KObject<'run> {
    let kf = alloc_local_kf(home);
    let door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(home.brand().handle()));
    let fields = Record::from_pairs(vec![(
        "f".to_string(),
        Held::Object(KObject::KFunction(kf)),
    )]);
    door.alloc_object_folded(KObject::record_of_held(door, fields, types))
}

/// Escape with **copy**: `fold_cells`'s exact aggregate loop (`copied_seam_mode` +
/// `transfer_into_placing` + `copy_held_from_carried`), mirrored here for `DEPTH` independent
/// producers each contributing a plain-data record — no field borrows anything, so
/// `still_borrows_host` answers false and every cell selects `Residence::Released`: the
/// record is totally rebuilt into the aggregate's own region and every producer frame is dropped
/// *before* the read, proving the seam genuinely releases rather than conservatively pinning.
#[test]
fn plain_record_cells_select_released_and_survive_every_producer_free() {
    const DEPTH: usize = 5;
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let dest_frame: Rc<CallFrame> = CallFrame::new(scope);
    let types = TypeRegistry::new();
    let dest_storage = dest_frame.storage_rc();

    let mut producers: Vec<Rc<CallFrame>> = Vec::with_capacity(DEPTH);
    let acc0 = KoanRegion::yoke_branded::<RecordAggFamily, _>(Rc::clone(&dest_storage), |region| {
        (region.handle(), Vec::with_capacity(DEPTH))
    });
    let acc_final = (0..DEPTH).fold(acc0, |acc, i| {
        let producer: Rc<CallFrame> = CallFrame::new(scope);
        let door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(
            producer.brand().handle(),
        ));
        let fields = Record::from_pairs(vec![(
            "acc".to_string(),
            Held::Object(KObject::Number(i as f64)),
        )]);
        let obj: &KObject<'_> =
            door.alloc_object_folded(KObject::record_of_held(door, fields, &types));
        // The seal chokepoint (Ruling 5, design/value-substrates.md): every record's carrier
        // conservatively forces `borrows_host = true` at construction, regardless of its own
        // contents — `copied_seam_mode`'s exact `still_borrows_host` answer is what
        // actually decides Released vs. Copied below; the seal bit only matters if `Copied` wins.
        let sealed = force_substrate_borrows_host(
            Witnessed::from_erased(
                Erased::erase(Carried::Object(obj)),
                CarrierWitness::default(),
            ),
            &producer.storage_rc(),
        );
        let dep: DeliveredCarried = Delivered::seal(sealed, producer.storage_rc());
        let mode = copied_seam_mode(&dep);
        assert!(
            matches!(mode, Residence::Released),
            "a plain-data record cell must select Released"
        );
        producers.push(producer);
        dep.transfer_into_placing::<RecordAggFamily, RecordAggFamily, _>(
            acc,
            mode,
            |value, (region, mut cells), placement| {
                cells.push(copy_held_from_carried(
                    value,
                    FoldingBrand::in_fold_closure(placement),
                ));
                (region, cells)
            },
        )
    });

    for producer in producers.drain(..) {
        drop(producer);
    }

    let values: Vec<f64> = acc_final.with_pinned(&dest_storage, |(_region, cells)| {
        cells
            .iter()
            .map(|h| match h.object() {
                KObject::Record(substrate, _) => {
                    match substrate.fields().get("acc").map(|h| h.object()) {
                        Some(KObject::Number(n)) => *n,
                        _ => panic!("expected field acc: Number"),
                    }
                }
                other => panic!("expected a Record cell, got {}", other.ktype().name(&types)),
            })
            .collect()
    });
    assert_eq!(
        values,
        (0..DEPTH).map(|i| i as f64).collect::<Vec<_>>(),
        "every record cell reads back unchanged after its producer frame freed"
    );
}

/// Escape with **pin**: the same `fold_cells` mechanism, but each of the `DEPTH` producers
/// contributes a record whose field is a genuine borrow leaf into its own producer (a closure
/// captured in that same frame) — `still_borrows_host` answers true (the leaf's home IS the
/// delivered cell's own host), so every cell selects `Residence::Copied` and its producer
/// materializes into the aggregate's reach. Dropping every producer shell and reading each
/// closure's captured scope back is the no-use-after-free check under tree borrows; a regression
/// that instead released these producers (mistaking the record for plain data) would dangle here.
#[test]
fn closure_embedding_record_cells_select_copied_and_pin_every_producer() {
    const DEPTH: usize = 5;
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let dest_frame: Rc<CallFrame> = CallFrame::new(scope);
    let types = TypeRegistry::new();
    let dest_storage = dest_frame.storage_rc();

    let mut producers: Vec<Rc<CallFrame>> = Vec::with_capacity(DEPTH);
    let mut expected_ids = Vec::with_capacity(DEPTH);
    let acc0 = KoanRegion::yoke_branded::<RecordAggFamily, _>(Rc::clone(&dest_storage), |region| {
        (region.handle(), Vec::with_capacity(DEPTH))
    });
    let acc_final = (0..DEPTH).fold(acc0, |acc, _| {
        let producer: Rc<CallFrame> = CallFrame::new(scope);
        let obj = alloc_home_closure_record(&producer, &types);
        expected_ids.push(match obj {
            KObject::Record(substrate, _) => {
                match substrate.fields().get("f").map(|h| h.object()) {
                    Some(KObject::KFunction(f)) => f.captured_scope().id,
                    _ => panic!("expected field f: KFunction"),
                }
            }
            other => panic!("expected a Record, got {}", other.ktype().name(&types)),
        });
        // The seal chokepoint (Ruling 5): every record's carrier conservatively forces
        // `borrows_host = true` at construction — without it, `Residence::Copied`'s
        // `materialize_hosts` arm (`iff borrows_host`) would skip materializing the producer even
        // though `copied_seam_mode` below correctly selects `Copied`, dangling the read at the end.
        let sealed = force_substrate_borrows_host(
            Witnessed::from_erased(
                Erased::erase(Carried::Object(obj)),
                CarrierWitness::default(),
            ),
            &producer.storage_rc(),
        );
        let dep: DeliveredCarried = Delivered::seal(sealed, producer.storage_rc());
        let mode = copied_seam_mode(&dep);
        assert!(
            matches!(mode, Residence::Copied),
            "a closure-embedding record cell must select Copied"
        );
        producers.push(producer);
        dep.transfer_into_placing::<RecordAggFamily, RecordAggFamily, _>(
            acc,
            mode,
            |value, (region, mut cells), placement| {
                cells.push(copy_held_from_carried(
                    value,
                    FoldingBrand::in_fold_closure(placement),
                ));
                (region, cells)
            },
        )
    });

    for producer in producers.drain(..) {
        drop(producer);
    }

    let read_ids: Vec<_> = acc_final.with_pinned(&dest_storage, |(_region, cells)| {
        cells
            .iter()
            .map(|h| match h.object() {
                KObject::Record(substrate, _) => {
                    match substrate.fields().get("f").map(|h| h.object()) {
                        Some(KObject::KFunction(f)) => f.captured_scope().id,
                        _ => panic!("expected field f: KFunction"),
                    }
                }
                other => panic!("expected a Record cell, got {}", other.ktype().name(&types)),
            })
            .collect()
    });
    assert_eq!(
        read_ids, expected_ids,
        "every closure's captured scope reads back unchanged after its producer frame freed"
    );
}

/// Escape with the **cost-chooser-selected pin** verb at the value-level seam (`seam_verb` →
/// [`RegionEscape::Pin`] → `Residence::Kept` + [`copy_carried`]), the shape `relocate_terminal` /
/// `single_poll` / `finalize` take for a top-level record — distinct from the two container-cell
/// cases above, which route `copied_seam_mode` (never a pin). Each of the `DEPTH` producers
/// contributes a record whose only field is a closure captured in that same frame: priceable (the
/// closure leaf costs zero) with `borrows_home` set, so the chooser returns `Pin`. Under the verb's
/// `Kept` residence the producer host is minted into the destination reach unconditionally, and
/// `copy_carried` pointer-copies the record — the region-resident substrate borrow **rides shared**,
/// never rebuilt. Dropping every producer shell and reading each closure's captured scope back
/// through the shared substrate is the no-use-after-free check under tree borrows; a regression that
/// failed to mint the Kept host (or rebuilt instead of sharing) would dangle here.
#[cfg(not(feature = "seam-force-copy"))]
#[test]
fn record_seam_pin_verb_shares_substrate_and_survives_producer_free() {
    const DEPTH: usize = 5;
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let dest_frame: Rc<CallFrame> = CallFrame::new(scope);
    let types = TypeRegistry::new();
    let dest_storage = dest_frame.storage_rc();

    let mut producers: Vec<Rc<CallFrame>> = Vec::with_capacity(DEPTH);
    let mut expected_ids = Vec::with_capacity(DEPTH);
    let acc0 = KoanRegion::yoke_branded::<PinAggFamily, _>(Rc::clone(&dest_storage), |region| {
        (region.handle(), Vec::with_capacity(DEPTH))
    });
    let acc_final = (0..DEPTH).fold(acc0, |acc, _| {
        let producer: Rc<CallFrame> = CallFrame::new(scope);
        let obj = alloc_home_closure_record(&producer, &types);
        expected_ids.push(match obj {
            KObject::Record(substrate, _) => {
                match substrate.fields().get("f").map(|h| h.object()) {
                    Some(KObject::KFunction(f)) => f.captured_scope().id,
                    _ => panic!("expected field f: KFunction"),
                }
            }
            other => panic!("expected a Record, got {}", other.ktype().name(&types)),
        });
        let sealed = Witnessed::from_erased(
            Erased::erase(Carried::Object(obj)),
            CarrierWitness::default(),
        );
        let dep: DeliveredCarried = Delivered::seal(sealed, producer.storage_rc());
        let verb = seam_verb(&dep);
        assert!(
            matches!(verb, RegionEscape::Pin),
            "a priceable home-borrowing record must select the Pin verb at the value-level seam"
        );
        producers.push(producer);
        dep.transfer_into_placing::<PinAggFamily, PinAggFamily, _>(
            acc,
            verb.residence(),
            |value, (region, mut cells), placement| {
                cells.push(copy_carried(
                    value,
                    verb,
                    FoldingBrand::in_fold_closure(placement),
                ));
                (region, cells)
            },
        )
    });

    for producer in producers.drain(..) {
        drop(producer);
    }

    let read_ids: Vec<_> = acc_final.with_pinned(&dest_storage, |(_region, cells)| {
        cells
            .iter()
            .map(|carried| match carried.object() {
                KObject::Record(substrate, _) => {
                    match substrate.fields().get("f").map(|h| h.object()) {
                        Some(KObject::KFunction(f)) => f.captured_scope().id,
                        _ => panic!("expected field f: KFunction"),
                    }
                }
                other => panic!("expected a Record cell, got {}", other.ktype().name(&types)),
            })
            .collect()
    });
    assert_eq!(
        read_ids, expected_ids,
        "every pinned record's shared substrate reads its captured scope back after producer free"
    );
}

// Phase-1 substrate cost memos ([`RecordSubstrate::copy_cost`] / [`RecordSubstrate::borrows_home`]):
// each test drives `record_of_held` through a fold door and reads the memos the same construction
// pass computed, per the per-cell table in the substrate's doc.

/// One flat `Held` cell's byte width — the unit a type cell or a scalar contributes to a record's
/// copy cost. `Held` is invariant in its lifetime, so the width is lifetime-independent.
fn held_flat() -> u64 {
    std::mem::size_of::<Held<'static>>() as u64
}

/// Build a record homed in `home`'s region from `fields` and return its
/// `(copy_cost, borrows_home)` memos.
fn record_memos<'run>(
    home: &'run Rc<CallFrame>,
    fields: Record<Held<'run>>,
    types: &TypeRegistry,
) -> (u64, bool) {
    let door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(home.brand().handle()));
    match door.alloc_object_folded(KObject::record_of_held(door, fields, types)) {
        KObject::Record(substrate, _) => (substrate.copy_cost(), substrate.borrows_home()),
        other => panic!("expected a Record, got {}", other.ktype().name(types)),
    }
}

/// A scalar-only record is priceable at one flat `Held` per cell and borrows nothing home.
#[test]
fn substrate_memo_scalar_record_is_priceable_and_home_free() {
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let home = CallFrame::new(scope);
    let types = TypeRegistry::new();

    let fields = Record::from_pairs(vec![
        ("a".to_string(), Held::Object(KObject::Number(1.0))),
        ("b".to_string(), Held::Object(KObject::Bool(true))),
    ]);
    let (cost, borrows_home) = record_memos(&home, fields, &types);
    assert_eq!(cost, 2 * held_flat(), "two scalar cells cost two flat Held");
    assert!(!borrows_home, "no borrow leaf leaves borrows_home clear");
}

/// A `KString` cell adds its byte length to the flat `Held` width.
#[test]
fn substrate_memo_string_cell_adds_its_length() {
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let home = CallFrame::new(scope);
    let types = TypeRegistry::new();

    let fields = Record::from_pairs(vec![(
        "s".to_string(),
        Held::Object(KObject::KString("hello".into())),
    )]);
    let (cost, borrows_home) = record_memos(&home, fields, &types);
    assert_eq!(
        cost,
        held_flat() + 5,
        "a five-byte string adds five to the flat Held width"
    );
    assert!(!borrows_home);
}

/// A home-captured closure is a 0-weight borrow leaf: it adds no rebuild bytes yet sets
/// `borrows_home`. A foreign-captured closure is equally weightless but leaves the bit clear.
#[test]
fn substrate_memo_home_vs_foreign_closure_leaf() {
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let home = CallFrame::new(scope);
    let foreign = CallFrame::new(scope);
    let types = TypeRegistry::new();

    let base = Record::from_pairs(vec![("n".to_string(), Held::Object(KObject::Number(0.0)))]);
    let (base_cost, base_home) = record_memos(&home, base, &types);
    assert_eq!(base_cost, held_flat());
    assert!(!base_home);

    let home_kf = alloc_local_kf(&home);
    let with_home = Record::from_pairs(vec![
        ("n".to_string(), Held::Object(KObject::Number(0.0))),
        ("f".to_string(), Held::Object(KObject::KFunction(home_kf))),
    ]);
    let (home_cost, home_bit) = record_memos(&home, with_home, &types);
    assert_eq!(
        home_cost, base_cost,
        "the 0-weight closure leaf adds no rebuild bytes"
    );
    assert!(home_bit, "a home-captured closure sets borrows_home");

    let foreign_kf = alloc_local_kf(&foreign);
    let with_foreign = Record::from_pairs(vec![
        ("n".to_string(), Held::Object(KObject::Number(0.0))),
        (
            "f".to_string(),
            Held::Object(KObject::KFunction(foreign_kf)),
        ),
    ]);
    let (foreign_cost, foreign_bit) = record_memos(&home, with_foreign, &types);
    assert_eq!(
        foreign_cost, base_cost,
        "a foreign closure leaf is equally weightless"
    );
    assert!(
        !foreign_bit,
        "a foreign-captured closure leaves borrows_home clear"
    );
}

/// A nested record cell contributes exactly its own memoized `copy_cost` and `borrows_home` —
/// composed from the memo, never re-walked. The inner record here holds a string and a home-captured
/// closure, so both bits are non-trivial and must ride up to the outer substrate.
#[test]
fn substrate_memo_nested_record_composes_by_memo() {
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let home = CallFrame::new(scope);
    let types = TypeRegistry::new();

    let inner_kf = alloc_local_kf(&home);
    let inner_fields = Record::from_pairs(vec![
        ("x".to_string(), Held::Object(KObject::KString("ab".into()))),
        ("f".to_string(), Held::Object(KObject::KFunction(inner_kf))),
    ]);
    let door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(home.brand().handle()));
    let inner = door.alloc_object_folded(KObject::record_of_held(door, inner_fields, &types));
    let (inner_cost, inner_home) = match inner {
        KObject::Record(substrate, _) => (substrate.copy_cost(), substrate.borrows_home()),
        other => panic!("expected a Record, got {}", other.ktype().name(&types)),
    };
    assert_eq!(
        inner_cost,
        held_flat() + 2,
        "string cell plus 0-weight closure"
    );
    assert!(inner_home, "inner holds a home closure");

    let outer_fields = Record::from_pairs(vec![(
        "inner".to_string(),
        Held::Object(inner.deep_clone()),
    )]);
    let (cost, borrows_home) = record_memos(&home, outer_fields, &types);
    assert_eq!(
        cost, inner_cost,
        "the nested record contributes its own memoized copy_cost"
    );
    assert_eq!(
        borrows_home, inner_home,
        "the nested record contributes its own memoized borrows_home"
    );
}

/// A plain-data `List` cell is a substrate now, so it is **priceable**: it contributes its own
/// element substrate's cost, and the enclosing record stays priceable and borrows nothing home.
#[test]
fn substrate_memo_list_cell_is_priceable_and_home_free() {
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let home = CallFrame::new(scope);
    let types = TypeRegistry::new();

    // The list cell is itself born through a door homed in `home`; its one scalar element costs one
    // flat `Held`, which the enclosing record's memo pass reads back through the list's own memo.
    let list_door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(home.brand().handle()));
    let list = KObject::list_of_held(list_door, vec![Held::Object(KObject::Number(1.0))], &types);
    let fields = Record::from_pairs(vec![("l".to_string(), Held::Object(list))]);
    let (cost, borrows_home) = record_memos(&home, fields, &types);
    assert_eq!(
        cost,
        held_flat(),
        "the list cell contributes its own element substrate's cost (one scalar)"
    );
    assert!(!borrows_home, "a plain-data list borrows nothing home");
}

// Phase-3 escape-seam chooser ([`copy_or_pin`]): each test builds a record homed in `home`'s
// region, then reads the verb the CostDriven table selects for it at a home or foreign crossing.
// Gated to the default build (`SEAM_POLICY == CostDriven`): the two forced policies override the
// table, so these table assertions apply only to the cost-driven build; the forced-policy
// equivalence battery is phase 5.
#[cfg(not(any(feature = "seam-force-copy", feature = "seam-force-pin")))]
mod seam_verb_table {
    use super::*;

    /// Build a record homed in `home`'s region from `fields`, returning the whole `&KObject::Record` (its
    /// substrate address lives in `home`, so `home.region().owns_substrate` reports a home crossing).
    fn build_record<'run>(
        home: &'run Rc<CallFrame>,
        fields: Record<Held<'run>>,
        types: &TypeRegistry,
    ) -> &'run KObject<'run> {
        let door =
            FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(home.brand().handle()));
        door.alloc_object_folded(KObject::record_of_held(door, fields, types))
    }

    /// The chooser's substrate borrow, extracted from a `&KObject::Record`.
    fn substrate_of<'a>(value: &KObject<'a>) -> &'a crate::machine::model::RecordSubstrate<'a> {
        match value {
            KObject::Record(substrate, _) => substrate,
            other => panic!("expected a Record, got {:?}", other.ktype()),
        }
    }

    /// An **unpriceable** record (holds a splice-free `KExpression` cell — unpriceable, but plain
    /// data with no borrow leaf) copies, and its `released` bit tracks the exact probe: no borrow
    /// leaf survives, so the copy frees the host.
    #[test]
    fn seam_verb_unpriceable_plain_data_copies_released() {
        use crate::machine::model::ast::KExpression;
        let root = run_root_storage();
        let test_run = TestRun::silent(&root);
        let scope = test_run.scope;
        let home = CallFrame::new(scope);
        let types = TypeRegistry::new();

        let expr = KObject::KExpression(KExpression::new(Vec::new()));
        let fields = Record::from_pairs(vec![("e".to_string(), Held::Object(expr))]);
        let value = build_record(&home, fields, &types);

        assert_eq!(
            copy_or_pin(substrate_of(value), value, home.region()),
            RegionEscape::Copy { released: true },
            "an unpriceable plain-data record copies and the probe frees the host"
        );
    }

    /// A **priceable, home-crossing** record whose `borrows_home` bit is **set** (holds a closure
    /// captured in the home region) pins outright — a copy would pay the rebuild and still keep the pin.
    #[test]
    fn seam_verb_priceable_borrows_home_pins() {
        let root = run_root_storage();
        let test_run = TestRun::silent(&root);
        let scope = test_run.scope;
        let home = CallFrame::new(scope);
        let types = TypeRegistry::new();

        // `alloc_home_closure_record` builds `{f = <home closure>}` through `home`'s door: priceable
        // (the closure leaf is 0-weight) with `borrows_home` set.
        let value = alloc_home_closure_record(&home, &types);
        assert!(substrate_of(value).borrows_home(), "precondition: bit set");

        assert_eq!(
            copy_or_pin(substrate_of(value), value, home.region()),
            RegionEscape::Pin,
            "a set borrows-home bit forces a pin exactly"
        );
    }

    /// A **priceable, home-crossing** record with a **clear** `borrows_home` bit whose exact rebuild cost
    /// is a small fraction of the fat host's allocated total copies (released) — the payoff clears the
    /// ratio.
    #[test]
    fn seam_verb_priceable_small_cost_vs_fat_host_copies_released() {
        let root = run_root_storage();
        let test_run = TestRun::silent(&root);
        let scope = test_run.scope;
        let home = CallFrame::new(scope);
        let types = TypeRegistry::new();

        // Inflate the host's allocated total so a one-scalar record is far under 1/ALPHA_DIVISOR of it.
        for n in 0..300 {
            home.brand().alloc_object(KObject::Number(n as f64));
        }

        let fields =
            Record::from_pairs(vec![("a".to_string(), Held::Object(KObject::Number(1.0)))]);
        let value = build_record(&home, fields, &types);
        assert!(
            !substrate_of(value).borrows_home(),
            "precondition: bit clear"
        );

        assert_eq!(
            copy_or_pin(substrate_of(value), value, home.region()),
            RegionEscape::Copy { released: true },
            "a small priceable record against a fat host copies and releases"
        );
    }

    /// A **priceable, home-crossing** record with a **clear** `borrows_home` bit whose cost is *not* under
    /// the ratio (a long string against a tiny host) pins — the rebuild is not worth paying.
    #[test]
    fn seam_verb_priceable_cost_over_ratio_pins() {
        let root = run_root_storage();
        let test_run = TestRun::silent(&root);
        let scope = test_run.scope;
        let home = CallFrame::new(scope);
        let types = TypeRegistry::new();

        // A long string dominates the record's rebuild cost while the host stays tiny (String bytes are
        // heap, not arena, so the host's allocated total does not grow with the string).
        let big = "x".repeat(8192);
        let fields =
            Record::from_pairs(vec![("s".to_string(), Held::Object(KObject::KString(big)))]);
        let value = build_record(&home, fields, &types);
        assert!(
            !substrate_of(value).borrows_home(),
            "precondition: bit clear"
        );

        assert_eq!(
            copy_or_pin(substrate_of(value), value, home.region()),
            RegionEscape::Pin,
            "a costly record against a tiny host pins"
        );
    }

    /// A **foreign crossing** (the host passed to the chooser does not own the substrate) pins,
    /// regardless of the record's own memos — pricing a copy-out at an intermediate host is region
    /// evacuation's job.
    #[test]
    fn seam_verb_foreign_crossing_pins() {
        let root = run_root_storage();
        let test_run = TestRun::silent(&root);
        let scope = test_run.scope;
        let home = CallFrame::new(scope);
        let foreign = CallFrame::new(scope);
        let types = TypeRegistry::new();

        let fields =
            Record::from_pairs(vec![("a".to_string(), Held::Object(KObject::Number(1.0)))]);
        let value = build_record(&home, fields, &types);
        assert!(
            !foreign.region().owns_substrate(substrate_of(value)),
            "precondition: foreign host does not own the substrate"
        );

        assert_eq!(
            copy_or_pin(substrate_of(value), value, foreign.region()),
            RegionEscape::Pin,
            "a foreign crossing pins"
        );
    }
}
