//! Unit coverage for [`Scope::copy_delivered_substrate`]'s pin branch. A bound record whose cost
//! chooser selects [`RegionEscape::Pin`] — a home-borrowing record crossing out of its producer — rides
//! the producer region by hold: the projection is pointer-copied (sharing the producer-resident
//! substrate) and moved in under the binding's `Kept`-minted stored reach, whose named producer
//! region carries the foreign substrate past the residence audit on its `any_member_region` path.

use std::rc::Rc;

use super::*;
use crate::builtins::test_support::TestRun;
use crate::machine::model::{
    ExpressionSignature, Held, Record, RecordSubstrate, ReturnType, SignatureElement,
};
use crate::machine::run_root_storage;
use crate::machine::{Body, CallFrame, KFunction};
use crate::witnessed::{Erased, FoldedPlacement};

/// A `KFunction` whose captured scope lives in `home`'s region, allocated into `home`'s region — a
/// borrow leaf pointing at `home`, the shape a closure capturing its own defining frame takes.
fn alloc_home_closure<'run>(home: &'run Rc<CallFrame>) -> &'run KFunction<'run> {
    let types = TypeRegistry::new();
    home.with_scope(|child| {
        let inner = KFunction::new(
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
        home.brand().alloc_function(inner)
    })
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
    let door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(home.brand().handle()));
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
/// [`Scope::copy_delivered_substrate`] pointer-copies the projection (sharing the producer's substrate)
/// and moves it in under the `Kept`-minted stored reach. The bound value reads its field back
/// correctly after the producer frame shell drops — the stored reach holds the producer region.
/// (The enclosing module is gated out of the `seam-force-copy` build, which rebuilds instead of
/// pinning — see the `mod tests` declaration in the parent.)
#[test]
fn copy_delivered_substrate_pins_a_home_borrowing_record() {
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
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
        copy_or_pin(substrate, record, producer.region()),
        RegionEscape::Pin,
        "a home-crossing, borrows-home record must select Pin"
    );

    let sealed = Witnessed::from_erased(
        Erased::erase(Carried::Object(record)),
        CarrierWitness::default(),
    );
    let dep: DeliveredCarried = Delivered::seal(sealed, producer.storage_rc());

    let (bound, _stored) = consumer
        .copy_delivered_substrate(&dep, |carried| Ok(carried.object()), &types)
        .expect("a home-borrowing record pins into the binding under Kept-minted evidence");

    // The pin shares the producer-resident substrate rather than rebuilding it.
    assert_eq!(
        substrate_address(bound),
        producer_substrate,
        "the pinned record shares the producer's substrate (no rebuild)"
    );

    // Drop the producer frame shell: the binding's stored reach holds the producer region alive, so
    // the pinned substrate reads its field values back correctly.
    drop(dep);
    drop(producer);
    match bound {
        KObject::Record(bound_substrate, _) => {
            match bound_substrate.fields().get("v").map(|h| h.object()) {
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
