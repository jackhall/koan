//! Unit coverage for [`Scope::adopt_for_binding`]'s pin branch and for projection's run-exactness. A bound record whose cost
//! chooser selects [`RegionEscape::Pin`] — a home-borrowing record crossing out of its producer — rides
//! the producer region by hold: the projection is pointer-copied at the fold brand (sharing the
//! producer-resident substrate) and the fold's composition keeps every member the envelope named,
//! the producer's region among them, which is what covers the foreign substrate.

use std::rc::Rc;

use super::*;
use crate::builtins::test_support::TestRun;
use crate::machine::model::values::RecordSubstrate;
use crate::machine::model::Scalar;
use crate::machine::model::{
    Held, Record, ReturnType, SignatureDraft, SignatureElement, TypeRegistry,
};
use crate::machine::{program_storage, run_root_storage};
use crate::machine::{Body, CallFrame, KFunction};
use crate::witnessed::FoldedPlacement;

/// A `KFunction` whose captured scope lives in `home`'s region, allocated into `home`'s region — a
/// borrow leaf pointing at `home`, the shape a closure capturing its own defining frame takes.
fn alloc_home_closure<'run>(home: &'run Rc<CallFrame>) -> &'run KFunction<'run> {
    let types = TypeRegistry::new();
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

/// A record `{v = <value>, f = <home closure>}` built through `home`'s own door: its substrate is
/// `home`-resident (a home crossing) and its `f` leaf borrows `home` (`borrows_home` set), so it is
/// priceable — the exact shape the cost chooser pins.
fn alloc_home_borrowing_record<'run>(
    home: &'run Rc<CallFrame>,
    value: f64,
    types: &TypeRegistry,
) -> &'run KObject<'run> {
    let closure = alloc_home_closure(home);
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(home.brand().handle()))
            .with_holder(&owned_cells);
    let fields = Record::from_pairs(vec![
        ("v".to_string(), Held::Object(KObject::Number(value))),
        ("f".to_string(), Held::Object(KObject::KFunction(closure))),
    ]);
    door.alloc_object_folded(KObject::record_of_held(door, fields, types))
}

/// The substrate address of a `&KObject::Record`, as a lifetime-free integer for identity checks.
fn substrate_address(value: &KObject<'_>) -> usize {
    match value {
        KObject::Record(substrate, _) => *substrate as *const RecordSubstrate<'_> as usize,
        other => panic!("expected a Record, got {:?}", other.ktype()),
    }
}

/// A home-borrowing record delivered out of a producer frame binds by **pin**:
/// [`Scope::adopt_for_binding`] pointer-copies the projection at the fold brand (sharing the
/// producer's substrate) under the `Pin` verb's keep-everything retention claim. The bound value
/// reads its field back correctly after the producer frame shell drops — the consumer region's
/// union bundle, which the composition folded the producer's pins into, holds the producer region.
/// (The enclosing module is gated out of the `seam-force-copy` build, which rebuilds instead of
/// pinning — see the `mod tests` declaration in the parent.)
#[test]
fn adopt_for_binding_pins_a_home_borrowing_record() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let consumer = test_run.scope;
    let producer: Rc<CallFrame> = CallFrame::new(consumer);
    let types = TypeRegistry::new();

    let record = alloc_home_borrowing_record(&producer, 7.0, &types);
    let producer_substrate = substrate_address(record);

    // Precondition: the cost chooser selects Pin for this home-crossing, borrows-home record.
    let substrate = match record {
        KObject::Record(substrate, _) => substrate,
        _ => unreachable!(),
    };
    assert!(
        substrate.borrows_home(),
        "precondition: the record's borrows-home bit is set"
    );
    assert_eq!(
        copy_or_pin(substrate, producer.region()),
        RegionEscape::Pin,
        "a home-crossing, borrows-home record must select Pin"
    );

    // The record lives in the producer's region and borrows into it, so its description is hosted
    // there with home as an ordinary member — what the producer's own birth mint would have stamped.
    let sealed = producer.with_scope(|child| {
        child
            .seal_reaching(Carried::Object(record), child.mint_born_here(true))
            .unseal()
    });
    let dep: DeliveredCarried =
        Delivered::seal(sealed, producer.storage_rc(), FrameCoverage::empty());

    let bound_seal = consumer
        .adopt_for_binding(&dep, |carried| Ok(carried.object()))
        .expect("a whole-value projection is infallible");
    let opened = bound_seal.open_at(&root);
    let bound = opened.value().object();

    // The pin shares the producer-resident substrate rather than rebuilding it.
    assert_eq!(
        substrate_address(bound),
        producer_substrate,
        "the pinned record shares the producer's substrate (no rebuild)"
    );

    // Drop the producer frame shell: the consumer region's union bundle holds the producer region
    // alive, so the pinned substrate reads its field values back correctly.
    drop(dep);
    drop(producer);
    match bound {
        KObject::Record(bound_substrate, _) => {
            match bound_substrate.field("v").map(|h| h.object()) {
                Some(KObject::Number(n)) => {
                    assert_eq!(*n, 7.0, "field v reads back after producer drop")
                }
                other => panic!(
                    "expected field v: Number, got {:?}",
                    other.map(|o| o.ktype())
                ),
            }
        }
        _ => unreachable!(),
    }
}

/// A record `{v = 1, here = <closure in `home`>, there = <closure in `foreign`>}` built through
/// `home`'s own door — three cells whose reach differs cell by cell, so the container's union names
/// both regions while no single run does.
fn alloc_split_reach_record<'run>(
    home: &'run Rc<CallFrame>,
    foreign: &'run Rc<CallFrame>,
    types: &TypeRegistry,
) -> &'run KObject<'run> {
    let here = alloc_home_closure(home);
    let there = alloc_home_closure(foreign);
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(home.brand().handle()))
            .with_holder(&owned_cells);
    let fields = Record::from_pairs(vec![
        ("v".to_string(), Held::Object(KObject::Number(1.0))),
        ("here".to_string(), Held::Object(KObject::KFunction(here))),
        ("there".to_string(), Held::Object(KObject::KFunction(there))),
    ]);
    door.alloc_object_folded(KObject::record_of_held(door, fields, types))
}

/// Parting a cell hands it out under **its own run's** stored reach, not the container's union: the
/// owned scalar's run is empty, and each closure cell's run names only the region that closure
/// captures. This is what makes a projection release-exact — the container reaches both regions, so
/// a subset walk over the whole value would over-state every cell.
#[test]
fn a_projected_cell_carries_its_own_run_not_the_containers_union() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let consumer = test_run.scope;
    // Two sibling frames: neither one's owner chain keeps the other's region alive, so a run naming
    // one provably does not cover the other.
    let home: Rc<CallFrame> = CallFrame::new(consumer);
    let foreign: Rc<CallFrame> = CallFrame::new(consumer);
    let types = TypeRegistry::new();

    let record = alloc_split_reach_record(&home, &foreign, &types);
    let substrate = match record {
        KObject::Record(substrate, _) => substrate,
        other => panic!("expected a Record, got {:?}", other.ktype()),
    };

    // The value-level union — what a whole-value carrier stores — names both regions.
    assert!(substrate.reach().pins_region(home.region()));
    assert!(substrate.reach().pins_region(foreign.region()));

    let cell = |name: &str| {
        substrate
            .project(
                substrate
                    .field_index(name)
                    .expect("the record declares this field"),
            )
            .expect("the index came from the substrate's own layout")
    };

    let v = cell("v");
    assert!(
        !v.has_reach_members(),
        "an owned scalar cell lands in an empty-reach run"
    );

    let here = cell("here");
    assert!(here.reach_covers(home.region()));
    assert!(
        !here.reach_covers(foreign.region()),
        "the home closure's run must not name the region only its sibling cell reaches"
    );

    let there = cell("there");
    assert!(there.reach_covers(foreign.region()));
    assert!(
        !there.reach_covers(home.region()),
        "the foreign closure's run must not name the container's own home"
    );

    // The relocation seam re-owns exactly that run: the lifted envelope keeps reporting the cell's
    // own reach, never widening to the container's union.
    let lifted = there.lift_out();
    assert!(lifted.open_at().reach_covers(foreign.region()));
    assert!(!lifted.open_at().reach_covers(home.region()));
}
