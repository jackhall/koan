//! Tests for [`copy_carried`] — the witnessed-transfer copy hook. It structurally copies a
//! [`Carried`] into a destination region: a substrate carrier is totally rebuilt so its
//! region-resident substrate lands at `dest`, while a scalar re-allocates its top node and a
//! `KFunction` / first-class `Module` rides a bare borrow preserved verbatim. No region anchor is
//! embedded in the value — the regions a copied value reaches are pinned by the carrier's witness
//! set at the `transfer_into` layer, not here.

use super::*;
use crate::builtins::test_support::{run_root_bare, TestRun};
use crate::machine::core::{
    program_storage, run_root_storage, FoldingBrand, FrameCoverage, KoanRegion, KoanRegionExt,
    KoanStorageProfile,
};
use crate::machine::execute::run_loop::{dest_brand, DestHandleFamily};
use crate::machine::model::CarriedFamily;
use crate::machine::model::Held;
use crate::machine::model::KType;
use crate::machine::model::Record;
use crate::machine::model::Scalar;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{Carried, KObject};
use crate::machine::CallFrame;
use crate::witnessed::{reattachable, Delivered, FoldedPlacement, RegionHandle};
use std::rc::Rc;

/// A `KFunction` allocated into `home`'s region (its captured scope lives there), for the
/// borrow-preservation tests. The body is never run.
fn alloc_local_kf<'run>(home: &'run Rc<CallFrame>) -> &'run crate::machine::KFunction<'run> {
    use crate::machine::model::{ReturnType, SignatureDraft, SignatureElement};
    use crate::machine::Body;
    // The captured scope and the function both land in `home`'s region, so the `&KFunction` comes back
    // at `home`'s own lifetime with nothing retyped. Mirrors a closure capturing its defining scope.
    let types = crate::machine::model::TypeRegistry::new();
    CallFrame::alloc_capturing_scope(
        home,
        SignatureDraft {
            return_type: ReturnType::Resolved(KType::NULL),
            elements: vec![SignatureElement::Keyword("__INNER__")],
        },
        Body::Builtin(|ctx| {
            crate::machine::core::Action::done_resident(
                ctx.scope,
                Carried::Object(ctx.scope.brand().alloc_scalar(Scalar::Null)),
            )
        }),
        &types,
    )
}

/// The top node of a relocated `Carried::Object` is a fresh allocation owned by `dest`, not the
/// source — that relocation (so the copy outlives the producer's dying frame) is the whole point.
#[test]
fn object_top_node_relocates_into_dest() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);

    let obj: &KObject = source.brand().alloc_scalar(Scalar::Number(2.5));
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let relocated = copy_carried(
        Carried::Object(obj),
        RegionEscape::Copy,
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle()))
            .with_holder(&owned_cells),
    );
    match relocated {
        Carried::Object(r) => {
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
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);
    let types = test_run.types.clone();

    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let source_door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(source.brand().handle()))
            .with_holder(&owned_cells);
    let list: &KObject = source_door.alloc_object_folded(KObject::list_of_held(
        source_door,
        &[
            Held::Object(KObject::Number(1.0)),
            Held::Object(KObject::Number(2.0)),
        ],
        &types,
    ));

    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let relocated = copy_carried(
        Carried::Object(list),
        RegionEscape::Copy,
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle()))
            .with_holder(&owned_cells),
    );
    match relocated {
        Carried::Object(r @ KObject::List(out, _)) => {
            assert!(
                !std::ptr::eq(r, list),
                "the top list node is a fresh allocation, not the source"
            );
            assert!(
                out.homed_in(dest.region()),
                "the rebuilt element substrate lives in dest"
            );
            assert!(
                !out.homed_in(source.region()),
                "the source is not the rebuilt substrate's home"
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
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);
    let types = test_run.types.clone();

    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let source_door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(source.brand().handle()))
            .with_holder(&owned_cells);
    let mut map: HashMap<KKey, Held> = HashMap::new();
    map.insert(KKey::String("a"), Held::Object(KObject::Number(1.0)));
    let dict: &KObject =
        source_door.alloc_object_folded(KObject::dict_of_held(source_door, map, &types));

    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let relocated = copy_carried(
        Carried::Object(dict),
        RegionEscape::Copy,
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle()))
            .with_holder(&owned_cells),
    );
    match relocated {
        Carried::Object(r @ KObject::Dict(out, _)) => {
            assert!(
                !std::ptr::eq(r, dict),
                "the top dict node is a fresh allocation, not the source"
            );
            assert!(
                out.homed_in(dest.region()),
                "the rebuilt entry substrate lives in dest"
            );
            assert!(
                !out.homed_in(source.region()),
                "the source is not the rebuilt substrate's home"
            );
        }
        Carried::Object(other) => panic!("expected a Dict, got {:?}", other.ktype()),
        Carried::Type(_) | Carried::UnresolvedType(_) => panic!("expected an Object carrier"),
    }
}

/// A `Tagged` relocated under a `Copy` verb is totally rebuilt at the destination brand: the rebuilt
/// payload substrate lives in `dest`'s region, not the source's — a tagged value is a region-resident
/// substrate, not a shared `Rc` spine. Its tag and interned `identity` type handle ride along
/// unchanged.
#[test]
fn tagged_relocation_rebuilds_payload_into_dest() {
    use crate::machine::core::ScopeId;
    use crate::machine::model::TypeNode;
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);
    let types = test_run.types.clone();

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
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let source_door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(source.brand().handle()))
            .with_holder(&owned_cells);
    let tagged: &KObject = source_door.alloc_object_folded(KObject::tagged(
        source_door,
        "Just",
        &KObject::Number(42.0),
        identity,
    ));

    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let relocated = copy_carried(
        Carried::Object(tagged),
        RegionEscape::Copy,
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle()))
            .with_holder(&owned_cells),
    );
    match relocated {
        Carried::Object(
            r @ KObject::Tagged {
                tag,
                value: out,
                identity: out_identity,
            },
        ) => {
            assert!(
                !std::ptr::eq(r, tagged),
                "the top tagged node is a fresh allocation, not the source"
            );
            assert_eq!(*tag, "Just");
            assert!(
                out.homed_in(dest.region()),
                "the rebuilt payload substrate lives in dest"
            );
            assert!(
                !out.homed_in(source.region()),
                "the source is not the rebuilt substrate's home"
            );
            assert!(matches!(out.payload(), KObject::Number(n) if *n == 42.0));
            assert_eq!(
                *out_identity, identity,
                "the identity handle rides along unchanged"
            );
        }
        Carried::Object(other) => panic!("expected a Tagged, got {:?}", other.ktype()),
        Carried::Type(_) | Carried::UnresolvedType(_) => panic!("expected an Object carrier"),
    }
}

/// A `Wrapped` relocated under a `Copy` verb is totally rebuilt at the destination brand: the rebuilt
/// payload substrate lives in `dest`'s region, not the source's, and the `type_id` rides unchanged.
#[test]
fn wrapped_relocation_rebuilds_payload_into_dest() {
    use crate::machine::core::ScopeId;
    use crate::machine::model::TypeNode;
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);
    let types = test_run.types.clone();

    let type_id = types.intern(TypeNode::AbstractType {
        source: ScopeId::from_raw(0, 0x12),
        name: "Distance".into(),
        param_names: vec![],
        nonce: None,
    });
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let source_door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(source.brand().handle()))
            .with_holder(&owned_cells);
    let wrapped: &KObject = source_door.alloc_object_folded(KObject::wrapped_hold(
        source_door,
        &KObject::Number(7.0),
        type_id,
    ));

    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let relocated = copy_carried(
        Carried::Object(wrapped),
        RegionEscape::Copy,
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle()))
            .with_holder(&owned_cells),
    );
    match relocated {
        Carried::Object(
            r @ KObject::Wrapped {
                inner: out,
                type_id: out_type_id,
            },
        ) => {
            assert!(
                !std::ptr::eq(r, wrapped),
                "the top wrapped node is a fresh allocation, not the source"
            );
            assert!(
                out.homed_in(dest.region()),
                "the rebuilt payload substrate lives in dest"
            );
            assert!(
                !out.homed_in(source.region()),
                "the source is not the rebuilt substrate's home"
            );
            assert!(matches!(out.payload(), KObject::Number(n) if *n == 7.0));
            assert_eq!(
                *out_type_id, type_id,
                "the type_id handle rides along unchanged"
            );
        }
        Carried::Object(other) => panic!("expected a Wrapped, got {:?}", other.ktype()),
        Carried::Type(_) | Carried::UnresolvedType(_) => panic!("expected an Object carrier"),
    }
}

/// A `KFunction` rides a *bare* borrow preserved verbatim — relocation copies the reference, never
/// the closure (which may reference anything reachable from its captured scope). The borrow points
/// back into the source region; the carrier's witness set keeps that region alive.
#[test]
fn kfunction_borrow_preserved_verbatim() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);

    let kf_ref = alloc_local_kf(&source);
    let source_scope = run_root_bare(source.storage());
    let obj: &KObject = source_scope.brand().alloc_value(KObject::KFunction(kf_ref));

    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let relocated = copy_carried(
        Carried::Object(obj),
        RegionEscape::Copy,
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle()))
            .with_holder(&owned_cells),
    );
    match relocated {
        Carried::Object(r @ KObject::KFunction(f)) => {
            assert!(
                !std::ptr::eq(r, obj),
                "the top node is a fresh allocation, not the source"
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
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
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

    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let relocated = copy_carried(
        Carried::Type(type_value),
        RegionEscape::Copy,
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle()))
            .with_holder(&owned_cells),
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
/// cells folded in so far — a local twin of `dispatch::literal::AggBuildFamily` (private to that
/// module), reattached here so the tests can drive `fold_cells`'s own mechanism
/// (`cell_still_borrows` + `transfer_into_placing` + `copy_held_from_carried`) directly, including
/// its region-bumped cell slice.
struct RecordAggFamily;
reattachable!(RecordAggFamily => (RegionHandle<'r, KoanStorageProfile>, &'r [Held<'r>]));

/// The birth mint at a fold door: a record literal assembled by `merge_into_placing` into the
/// destination brand — `schedule_record_literal`'s terminal step verbatim — references a
/// description whose members name the very region it was built in. The substrate is region-resident,
/// so the fresh value genuinely borrows into its birth region, and the mint at the door records that
/// as ordinary membership. The question is asked of the opened carrier and answered off the
/// description; there is no bit to consult, and nothing rebuilds the witness after the fold.
#[test]
fn substrate_born_at_a_fold_door_reaches_its_birth_region() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let dest_frame: Rc<CallFrame> = CallFrame::new(scope);
    let types = TypeRegistry::new();
    let dest_storage = dest_frame.storage_rc();

    // `fold_cells`'s seed: a bare handle on the destination region plus an empty cell slice, homed
    // in the destination frame. A handle and an empty slice reach nothing, so the seed's own
    // coverage is empty — every member the product ends up with comes from the birth mint.
    let acc = Delivered::seal(
        KoanRegion::yoke_branded::<RecordAggFamily, _>(Rc::clone(&dest_storage), |region| {
            (region.handle(), &[][..])
        }),
        Rc::clone(&dest_storage),
        FrameCoverage::empty(),
    );

    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let born: DeliveredCarried = acc
        .merge_into_placing::<DestHandleFamily, CarriedFamily, KoanStorageProfile>(
            dest_brand(Rc::clone(&dest_storage)),
            move |(_region, _cells), _dest_handle, placement| {
                let door = FoldingBrand::in_fold_closure(placement).with_holder(&owned_cells);
                let fields =
                    Record::from_pairs(vec![("a".to_string(), Held::Object(KObject::Number(1.0)))]);
                Carried::Object(
                    door.alloc_object_folded(KObject::record_of_held(door, fields, &types)),
                )
            },
        );

    let opened = born.open_at();
    assert!(
        opened.reach_covers(dest_frame.region()),
        "the substrate was built in this region, so the birth mint names it as an ordinary member"
    );
    assert!(
        opened.borrows_home(),
        "and that region is the value's own residence — membership and host agree, from one record"
    );
}

/// A `KFunction` allocated into `home`'s region wrapped in a `Record` field, both born through
/// `home`'s own brand (not a transient `with_scope` sub-brand) so the reference escapes at `home`'s
/// own lifetime — the shape a list-literal cell born from `({f = (FN …)})` takes.
fn alloc_home_closure_record<'run>(
    home: &'run Rc<CallFrame>,
    types: &TypeRegistry,
) -> &'run KObject<'run> {
    let kf = alloc_local_kf(home);
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(home.brand().handle()))
            .with_holder(&owned_cells);
    let fields = Record::from_pairs(vec![(
        "f".to_string(),
        Held::Object(KObject::KFunction(kf)),
    )]);
    door.alloc_object_folded(KObject::record_of_held(door, fields, types))
}

/// Escape with **copy**: `fold_cells`'s exact aggregate loop (`cell_still_borrows` +
/// `transfer_into_placing` + `copy_held_from_carried`), mirrored here for `DEPTH` independent
/// producers each contributing a plain-data record — no field borrows anything, so the retention
/// predicate answers false over the rebuilt cell and every producer is released: the
/// record is totally rebuilt into the aggregate's own region and every producer frame is dropped
/// *before* the read, proving the seam genuinely releases rather than conservatively pinning.
#[test]
fn plain_record_cells_select_released_and_survive_every_producer_free() {
    const DEPTH: usize = 5;
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let dest_frame: Rc<CallFrame> = CallFrame::new(scope);
    let types = TypeRegistry::new();
    let dest_storage = dest_frame.storage_rc();

    let mut producers: Vec<Rc<CallFrame>> = Vec::with_capacity(DEPTH);
    let acc0 = Delivered::seal(
        KoanRegion::yoke_branded::<RecordAggFamily, _>(Rc::clone(&dest_storage), |region| {
            (region.handle(), &[][..])
        }),
        Rc::clone(&dest_storage),
        FrameCoverage::empty(),
    );
    let acc_final = (0..DEPTH).fold(acc0, |acc, i| {
        let producer: Rc<CallFrame> = CallFrame::new(scope);
        let owned_cells = crate::machine::core::FrameCoverage::empty();
        let door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(
            producer.brand().handle(),
        ))
        .with_holder(&owned_cells);
        let fields = Record::from_pairs(vec![(
            "acc".to_string(),
            Held::Object(KObject::Number(i as f64)),
        )]);
        let obj: &KObject<'_> =
            door.alloc_object_folded(KObject::record_of_held(door, fields, &types));
        // The seal chokepoint (Ruling 5, design/value-substrates.md): every record's carrier
        // conservatively claims its own home as a member at construction, regardless of its own
        // contents — the retention predicate's walk over the rebuilt cell is what actually
        // decides release vs. retain below; the claim only matters if the source is retained.
        let sealed = producer.with_scope(|child| {
            child
                .seal_reaching(Carried::Object(obj), child.mint_born_here(true))
                .unseal()
        });
        let owned_cells = crate::machine::core::FrameCoverage::empty();
        let dep: DeliveredCarried =
            Delivered::seal(sealed, producer.storage_rc(), FrameCoverage::empty());
        producers.push(producer);
        dep.transfer_into_placing::<RecordAggFamily, RecordAggFamily, _>(
            acc,
            cell_still_borrows(&dep),
            |value, (region, cells), placement| {
                let cell = copy_held_from_carried(
                    value,
                    FoldingBrand::in_fold_closure(placement).with_holder(&owned_cells),
                );
                let mut grown = Vec::with_capacity(cells.len() + 1);
                grown.extend_from_slice(cells);
                grown.push(cell);
                (region, placement.bump().slice(&grown))
            },
        )
    });

    assert!(
        acc_final.coverage_releasing_home().is_empty(),
        "every plain-data record cell releases its producer: beyond its own destination home, the \
         accumulated envelope covers nothing"
    );

    for producer in producers.drain(..) {
        drop(producer);
    }

    // The accumulator is not a `Copy` family, so the read is by reference; the envelope's own pins
    // keep everything the fold claimed alive across it.
    let values: Vec<f64> = acc_final.open_ref(|(_region, cells)| {
        cells
            .iter()
            .map(|h| match h.object() {
                KObject::Record(substrate, _) => match substrate.field("acc").map(|h| h.object()) {
                    Some(KObject::Number(n)) => *n,
                    _ => panic!("expected field acc: Number"),
                },
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
/// captured in that same frame) — the rebuilt cell's run still names that producer (the leaf's home
/// IS the delivered cell's own home), so every cell claims its envelope's pins and its producer
/// materializes into the aggregate's reach. Dropping every producer shell and reading each
/// closure's captured scope back is the no-use-after-free check under tree borrows; a regression
/// that instead released these producers (mistaking the record for plain data) would dangle here.
#[test]
fn closure_embedding_record_cells_select_copied_and_pin_every_producer() {
    const DEPTH: usize = 5;
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let dest_frame: Rc<CallFrame> = CallFrame::new(scope);
    let types = TypeRegistry::new();
    let dest_storage = dest_frame.storage_rc();

    let mut producers: Vec<Rc<CallFrame>> = Vec::with_capacity(DEPTH);
    let mut expected_ids = Vec::with_capacity(DEPTH);
    let acc0 = Delivered::seal(
        KoanRegion::yoke_branded::<RecordAggFamily, _>(Rc::clone(&dest_storage), |region| {
            (region.handle(), &[][..])
        }),
        Rc::clone(&dest_storage),
        FrameCoverage::empty(),
    );
    let acc_final = (0..DEPTH).fold(acc0, |acc, _| {
        let producer: Rc<CallFrame> = CallFrame::new(scope);
        let obj = alloc_home_closure_record(&producer, &types);
        expected_ids.push(match obj {
            KObject::Record(substrate, _) => match substrate.field("f").map(|h| h.object()) {
                Some(KObject::KFunction(f)) => f.captured_scope().id,
                _ => panic!("expected field f: KFunction"),
            },
            other => panic!("expected a Record, got {}", other.ktype().name(&types)),
        });
        // The seal chokepoint (Ruling 5): every record's carrier conservatively claims its own home
        // as a member at construction; the retention predicate below independently walks the
        // rebuilt cell and finds the closure leaf, so the producer is retained either way.
        let sealed = producer.with_scope(|child| {
            child
                .seal_reaching(Carried::Object(obj), child.mint_born_here(true))
                .unseal()
        });
        let owned_cells = crate::machine::core::FrameCoverage::empty();
        let dep: DeliveredCarried =
            Delivered::seal(sealed, producer.storage_rc(), FrameCoverage::empty());
        producers.push(producer);
        dep.transfer_into_placing::<RecordAggFamily, RecordAggFamily, _>(
            acc,
            cell_still_borrows(&dep),
            |value, (region, cells), placement| {
                let cell = copy_held_from_carried(
                    value,
                    FoldingBrand::in_fold_closure(placement).with_holder(&owned_cells),
                );
                let mut grown = Vec::with_capacity(cells.len() + 1);
                grown.extend_from_slice(cells);
                grown.push(cell);
                (region, placement.bump().slice(&grown))
            },
        )
    });

    assert!(
        !acc_final.coverage_releasing_home().is_empty(),
        "every closure-embedding record cell keeps its producer: the accumulated envelope covers it \
         beyond its own destination home"
    );

    for producer in producers.drain(..) {
        drop(producer);
    }

    // The accumulator is not a `Copy` family, so the read is by reference; the envelope's own pins
    // keep everything the fold claimed alive across it.
    let read_ids: Vec<_> = acc_final.open_ref(|(_region, cells)| {
        cells
            .iter()
            .map(|h| match h.object() {
                KObject::Record(substrate, _) => match substrate.field("f").map(|h| h.object()) {
                    Some(KObject::KFunction(f)) => f.captured_scope().id,
                    _ => panic!("expected field f: KFunction"),
                },
                other => panic!("expected a Record cell, got {}", other.ktype().name(&types)),
            })
            .collect()
    });
    assert_eq!(
        read_ids, expected_ids,
        "every closure's captured scope reads back unchanged after its producer frame freed"
    );
}

/// Escape with the **cost-chooser-selected pin** verb at the value-level seam, routed through the
/// production [`relocate_seam`] itself — the fused verb choice, product-derived retention claim,
/// and relocate hook that `relocate_terminal` and the literal park finish take — distinct from the
/// two container-cell cases above, which route `cell_still_borrows` (never a pin). Each of the
/// `DEPTH` producers contributes a record whose only field is a closure captured in that same
/// frame: priceable (the closure leaf costs zero) with `borrows_home` set, so the chooser returns
/// `Pin`, the producer host is minted into the relocated envelope's reach, and the record is
/// pointer-copied — the region-resident substrate borrow **rides shared**, never rebuilt. Dropping
/// every producer shell and reading each closure's captured scope back through the shared substrate
/// is the no-use-after-free check under tree borrows; a regression that failed to mint the kept
/// host (or rebuilt instead of sharing) would dangle here.
#[cfg(not(feature = "seam-force-copy"))]
#[test]
fn record_seam_pin_verb_shares_substrate_and_survives_producer_free() {
    const DEPTH: usize = 5;
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let dest_frame: Rc<CallFrame> = CallFrame::new(scope);
    let types = TypeRegistry::new();
    let dest_storage = dest_frame.storage_rc();

    let mut producers: Vec<Rc<CallFrame>> = Vec::with_capacity(DEPTH);
    let mut expected_ids = Vec::with_capacity(DEPTH);
    let relocated: Vec<DeliveredCarried> = (0..DEPTH)
        .map(|_| {
            let producer: Rc<CallFrame> = CallFrame::new(scope);
            let obj = alloc_home_closure_record(&producer, &types);
            expected_ids.push(match obj {
                KObject::Record(substrate, _) => match substrate.field("f").map(|h| h.object()) {
                    Some(KObject::KFunction(f)) => f.captured_scope().id,
                    _ => panic!("expected field f: KFunction"),
                },
                other => panic!("expected a Record, got {}", other.ktype().name(&types)),
            });
            // Born in the producer's own region with a home-borrowing closure leaf, so home is an
            // ordinary member of the description the birth mint stamps.
            let sealed = producer.with_scope(|child| {
                child
                    .seal_reaching(Carried::Object(obj), child.mint_born_here(true))
                    .unseal()
            });
            let dep: DeliveredCarried =
                Delivered::seal(sealed, producer.storage_rc(), FrameCoverage::empty());
            assert!(
                matches!(seam_verb(&dep), RegionEscape::Pin),
                "a priceable home-borrowing record must select the Pin verb at the value-level seam"
            );
            producers.push(producer);
            relocate_seam(&dep, dest_brand(Rc::clone(&dest_storage)))
        })
        .collect();

    for producer in producers.drain(..) {
        drop(producer);
    }

    let read_ids: Vec<_> = relocated
        .into_iter()
        .map(|envelope| {
            // The envelope's own pins cover the read, keeping everything the seam's fold claimed —
            // each pinned producer among it — alive across it.
            envelope.open_ref(|carried| match carried.object() {
                KObject::Record(substrate, _) => match substrate.field("f").map(|h| h.object()) {
                    Some(KObject::KFunction(f)) => f.captured_scope().id,
                    _ => panic!("expected field f: KFunction"),
                },
                other => panic!("expected a Record cell, got {}", other.ktype().name(&types)),
            })
        })
        .collect();
    assert_eq!(
        read_ids, expected_ids,
        "every pinned record's shared substrate reads its captured scope back after producer free"
    );
}

/// Both bump-hosted **index** shapes, rebuilt at the destination by one relocation and read back
/// after the producer frame frees.
///
/// A record's index is the sorted name slice `alloc_record` bumps (the slice and every name's
/// bytes); the fields go in unsorted, so the slice the substrate binary-searches is genuinely
/// reordered against the literal. A dict's is the `hashbrown` table `alloc_dict` freezes over
/// re-bumped keys — and the keys were already re-bumped once into the producer at construction, so
/// the relocation must re-bump them *again* rather than share. Nesting the dict in a record field
/// puts both under a single `transfer_into_placing`: the outer index is read on the way to the inner
/// one, so an index still pointing into the producer's freed bump reads dead bytes here — which only
/// tree borrows observes, since a normal build compares them back intact and the lookup succeeds.
#[test]
fn substrate_indexes_rehome_and_read_back_after_producer_free() {
    use crate::machine::model::KKey;
    use std::collections::HashMap;
    const NAMES: [&str; 4] = ["zeta", "alpha", "middle", "beta"];
    const KEYS: [&str; 4] = ["yankee", "bravo", "lima", "delta"];
    const TABLE: &str = "table";
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let dest_frame: Rc<CallFrame> = CallFrame::new(scope);
    let types = TypeRegistry::new();
    let dest_storage = dest_frame.storage_rc();

    let acc = Delivered::seal(
        KoanRegion::yoke_branded::<RecordAggFamily, _>(Rc::clone(&dest_storage), |region| {
            (region.handle(), &[][..])
        }),
        Rc::clone(&dest_storage),
        FrameCoverage::empty(),
    );

    let producer: Rc<CallFrame> = CallFrame::new(scope);
    let born_cells = crate::machine::core::FrameCoverage::empty();
    let door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(producer.brand().handle()))
            .with_holder(&born_cells);
    let mut map: HashMap<KKey, Held> = HashMap::new();
    for (i, key) in KEYS.iter().enumerate() {
        map.insert(KKey::String(key), Held::Object(KObject::Number(i as f64)));
    }
    let table = KObject::dict_of_held(door, map, &types);
    let fields = Record::from_pairs(
        NAMES
            .iter()
            .enumerate()
            .map(|(i, name)| ((*name).to_string(), Held::Object(KObject::Number(i as f64))))
            .chain(std::iter::once((TABLE.to_string(), Held::Object(table))))
            .collect::<Vec<_>>(),
    );
    let obj: &KObject<'_> = door.alloc_object_folded(KObject::record_of_held(door, fields, &types));
    let sealed = producer.with_scope(|child| {
        child
            .seal_reaching(Carried::Object(obj), child.mint_born_here(true))
            .unseal()
    });
    let dep: DeliveredCarried =
        Delivered::seal(sealed, producer.storage_rc(), FrameCoverage::empty());
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let acc_final = dep.transfer_into_placing::<RecordAggFamily, RecordAggFamily, _>(
        acc,
        cell_still_borrows(&dep),
        |value, (region, cells), placement| {
            let cell = copy_held_from_carried(
                value,
                FoldingBrand::in_fold_closure(placement).with_holder(&owned_cells),
            );
            let mut grown = Vec::with_capacity(cells.len() + 1);
            grown.extend_from_slice(cells);
            grown.push(cell);
            (region, placement.bump().slice(&grown))
        },
    );

    drop(producer);

    let read: Vec<(String, f64)> = acc_final.open_ref(|(_region, cells)| match cells[0].object() {
        KObject::Record(substrate, _) => {
            // Read through the name index twice over: the name-order walk, and a by-name
            // lookup per field — the binary search over the sorted name slice.
            let walked: Vec<String> = substrate.fields().map(|(n, _)| n.to_string()).collect();
            let mut sorted = NAMES
                .iter()
                .chain(std::iter::once(&TABLE))
                .map(|n| n.to_string())
                .collect::<Vec<_>>();
            sorted.sort();
            assert_eq!(
                walked, sorted,
                "the relocated record walks its fields in name order — the index is the \
                         sorted slice, not the order the literal was written"
            );
            // Then the nested dict's own index, reached *through* the outer one: walk the
            // table, then look every key back up — the hash and the byte-wise compare both
            // read key slices the relocation re-bumped.
            match substrate.field(TABLE).map(|h| h.object()) {
                Some(KObject::Dict(table, _)) => {
                    assert_eq!(
                        table.entries().count(),
                        KEYS.len(),
                        "the relocated dict keeps every entry"
                    );
                    for (i, key) in KEYS.iter().enumerate() {
                        match table.entry(&KKey::String(key)).map(|h| h.object()) {
                            Some(KObject::Number(n)) => assert_eq!(
                                *n, i as f64,
                                "entry {key} looks up after the producer frame freed"
                            ),
                            _ => panic!("expected entry {key}: Number"),
                        }
                    }
                }
                _ => panic!("expected field {TABLE}: Dict"),
            }
            NAMES
                .iter()
                .map(|name| match substrate.field(name).map(|h| h.object()) {
                    Some(KObject::Number(n)) => ((*name).to_string(), *n),
                    _ => panic!("expected field {name}: Number"),
                })
                .collect()
        }
        other => panic!("expected a Record, got {}", other.ktype().name(&types)),
    });
    assert_eq!(
        read,
        NAMES
            .iter()
            .enumerate()
            .map(|(i, name)| ((*name).to_string(), i as f64))
            .collect::<Vec<_>>(),
        "every field reads back by name after the producer frame freed"
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
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(home.brand().handle()))
            .with_holder(&owned_cells);
    match door.alloc_object_folded(KObject::record_of_held(door, fields, types)) {
        KObject::Record(substrate, _) => (substrate.copy_cost(), substrate.borrows_home()),
        other => panic!("expected a Record, got {}", other.ktype().name(types)),
    }
}

/// A scalar-only record is priceable at one flat `Held` per cell and borrows nothing home.
#[test]
fn substrate_memo_scalar_record_is_priceable_and_home_free() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
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
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let home = CallFrame::new(scope);
    let types = TypeRegistry::new();

    let fields = Record::from_pairs(vec![(
        "s".to_string(),
        Held::Object(KObject::KString("hello")),
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
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
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
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let home = CallFrame::new(scope);
    let types = TypeRegistry::new();

    let inner_kf = alloc_local_kf(&home);
    let inner_fields = Record::from_pairs(vec![
        ("x".to_string(), Held::Object(KObject::KString("ab"))),
        ("f".to_string(), Held::Object(KObject::KFunction(inner_kf))),
    ]);
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(home.brand().handle()))
            .with_holder(&owned_cells);
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
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let home = CallFrame::new(scope);
    let types = TypeRegistry::new();

    // The list cell is itself born through a door homed in `home`; its one scalar element costs one
    // flat `Held`, which the enclosing record's memo pass reads back through the list's own memo.
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let list_door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(home.brand().handle()))
            .with_holder(&owned_cells);
    let list = KObject::list_of_held(list_door, &[Held::Object(KObject::Number(1.0))], &types);
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

    /// Build a record homed in `home`'s region from `fields`, returning the whole `&KObject::Record`
    /// (its substrate's stored reach names `home`, so the chooser reads a home crossing).
    fn build_record<'run>(
        home: &'run Rc<CallFrame>,
        fields: Record<Held<'run>>,
        types: &TypeRegistry,
    ) -> &'run KObject<'run> {
        let owned_cells = crate::machine::core::FrameCoverage::empty();
        let door =
            FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(home.brand().handle()))
                .with_holder(&owned_cells);
        door.alloc_object_folded(KObject::record_of_held(door, fields, types))
    }

    /// The chooser's substrate borrow, extracted from a `&KObject::Record`.
    fn substrate_of<'a>(
        value: &KObject<'a>,
    ) -> &'a crate::machine::model::values::RecordSubstrate<'a> {
        match value {
            KObject::Record(substrate, _) => substrate,
            other => panic!("expected a Record, got {:?}", other.ktype()),
        }
    }

    /// A record holding a `KExpression` cell prices like any other plain-data record: the expression
    /// is a borrow leaf costing nothing, since its parts run lives in the program storage that parsed
    /// it. So the record copies — and the rebuilt product's stored reach names no run into the host,
    /// so the relocation's retention claim frees it.
    #[test]
    fn seam_verb_expression_cell_prices_as_a_borrow_leaf_and_copies() {
        let program = program_storage();
        let root = run_root_storage();
        let test_run = TestRun::silent(&program, &root);
        let scope = test_run.scope;
        let home = CallFrame::new(scope);
        let types = TypeRegistry::new();

        // Program storage is where a raw AST node lives; an expression cell points into it.
        let expr = KObject::KExpression(program.brand().new_expression(Vec::new()));
        let fields = Record::from_pairs(vec![("e".to_string(), Held::Object(expr))]);
        let value = build_record(&home, fields, &types);

        assert_eq!(
            copy_or_pin(substrate_of(value), home.region()),
            RegionEscape::Copy,
            "a record holding an expression cell copies"
        );
    }

    /// A **priceable, home-crossing** record whose `borrows_home` bit is **set** (holds a closure
    /// captured in the home region) pins outright — a copy would pay the rebuild and still keep the pin.
    #[test]
    fn seam_verb_priceable_borrows_home_pins() {
        let program = program_storage();
        let root = run_root_storage();
        let test_run = TestRun::silent(&program, &root);
        let scope = test_run.scope;
        let home = CallFrame::new(scope);
        let types = TypeRegistry::new();

        // `alloc_home_closure_record` builds `{f = <home closure>}` through `home`'s door: priceable
        // (the closure leaf is 0-weight) with `borrows_home` set.
        let value = alloc_home_closure_record(&home, &types);
        assert!(substrate_of(value).borrows_home(), "precondition: bit set");

        assert_eq!(
            copy_or_pin(substrate_of(value), home.region()),
            RegionEscape::Pin,
            "a set borrows-home bit forces a pin exactly"
        );
    }

    /// A **priceable, home-crossing** record with a **clear** `borrows_home` bit whose exact rebuild cost
    /// is a small fraction of the fat host's allocated total copies — the payoff clears the ratio.
    #[test]
    fn seam_verb_priceable_small_cost_vs_fat_host_copies() {
        let program = program_storage();
        let root = run_root_storage();
        let test_run = TestRun::silent(&program, &root);
        let scope = test_run.scope;
        let home = CallFrame::new(scope);
        let types = TypeRegistry::new();

        // Inflate the host's allocated total so a one-scalar record is far under 1/ALPHA_DIVISOR of it.
        for n in 0..300 {
            home.brand().alloc_scalar(Scalar::Number(n as f64));
        }

        let fields =
            Record::from_pairs(vec![("a".to_string(), Held::Object(KObject::Number(1.0)))]);
        let value = build_record(&home, fields, &types);
        assert!(
            !substrate_of(value).borrows_home(),
            "precondition: bit clear"
        );

        assert_eq!(
            copy_or_pin(substrate_of(value), home.region()),
            RegionEscape::Copy,
            "a small priceable record against a fat host copies"
        );
    }

    /// A **priceable, home-crossing** record with a **clear** `borrows_home` bit whose cost is *not* under
    /// the ratio (a long string against a tiny host) pins — the rebuild is not worth paying.
    #[test]
    fn seam_verb_priceable_cost_over_ratio_pins() {
        let big = "x".repeat(8192);
        let program = program_storage();
        let root = run_root_storage();
        let test_run = TestRun::silent(&program, &root);
        let scope = test_run.scope;
        let home = CallFrame::new(scope);
        let types = TypeRegistry::new();

        // A long string dominates the record's rebuild cost. The record door re-bumps the bytes into
        // the host region, so they price on both sides of the ratio — the rebuild cost is still the
        // dominant term, so the verb pins.
        let fields = Record::from_pairs(vec![(
            "s".to_string(),
            Held::Object(KObject::KString(&big)),
        )]);
        let value = build_record(&home, fields, &types);
        assert!(
            !substrate_of(value).borrows_home(),
            "precondition: bit clear"
        );

        assert_eq!(
            copy_or_pin(substrate_of(value), home.region()),
            RegionEscape::Pin,
            "a costly record against a tiny host pins"
        );
    }

    /// A **foreign crossing** (the host passed to the chooser does not own the substrate) pins,
    /// regardless of the record's own memos — pricing a copy-out at an intermediate host is region
    /// evacuation's job.
    #[test]
    fn seam_verb_foreign_crossing_pins() {
        let program = program_storage();
        let root = run_root_storage();
        let test_run = TestRun::silent(&program, &root);
        let scope = test_run.scope;
        let home = CallFrame::new(scope);
        let foreign = CallFrame::new(scope);
        let types = TypeRegistry::new();

        let fields =
            Record::from_pairs(vec![("a".to_string(), Held::Object(KObject::Number(1.0)))]);
        let value = build_record(&home, fields, &types);
        assert!(
            !substrate_of(value).homed_in(foreign.region()),
            "precondition: the foreign host is not the substrate's home"
        );

        assert_eq!(
            copy_or_pin(substrate_of(value), foreign.region()),
            RegionEscape::Pin,
            "a foreign crossing pins"
        );
    }
}
