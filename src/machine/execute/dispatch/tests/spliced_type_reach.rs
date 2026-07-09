use std::rc::Rc;

use crate::builtins::default_scope;
use crate::builtins::test_support::run_root_bare;
use crate::machine::core::{run_root_storage, FrameStorageExt, Scope, StoredReach};
use crate::machine::model::ast::TypeIdentifier;
use crate::machine::model::types::{KType, TypeResolution};
use crate::machine::model::values::{Carried, CarriedFamily, Module};
use crate::machine::BindingIndex;
use crate::machine::CarrierWitness;
use crate::witnessed::{Delivered, Sealed};

/// A first-class type spliced inline via `part_walk`'s wrap-slot arm arrives with its binding's
/// stored reach, and adopting the cell pins the type's producer region: `T`'s registered `KType`
/// reaches into a foreign frame's `Module` (the only variants that hold a real `&'a` region
/// pointer, per `KType`'s doc comment). The consumer lives in its own independent frame and every
/// other direct handle (the binding scope's own frame, the type's original producer frame) drops
/// before the read — an empty-reach witness (the old `Witnessed::resident` fallback) would leave
/// this dangling.
#[test]
fn spliced_type_carrier_pins_the_producer_region_after_drop() {
    let storage = run_root_storage();
    let scope = default_scope(&storage, Box::new(std::io::sink()));

    // Build and seal a `Module`-carrying type entirely within a foreign frame's own scope — the
    // erasure at `resident_type_carrier` decouples the sealed cell from `foreign`'s borrow.
    let foreign = run_root_storage();
    let foreign_scope = run_root_bare(&foreign);
    let child = foreign
        .brand()
        .alloc_scope(Scope::child_under(foreign_scope));
    let module = foreign
        .brand()
        .alloc_module(Module::new("M".to_string(), child));
    foreign_scope.register_type(
        "T".to_string(),
        KType::Module { module },
        BindingIndex::BUILTIN,
        StoredReach::empty(),
    );
    let foreign_hit =
        match foreign_scope.resolve_type_identifier(&TypeIdentifier::leaf("T".to_string()), None) {
            TypeResolution::Done(hit) => hit,
            _ => panic!("expected TypeResolution::Done for a registered type"),
        };
    let produced: Sealed<CarriedFamily, CarrierWitness> =
        Sealed::seal(foreign_scope.resident_type_carrier(
            foreign_hit.kt,
            foreign_hit.reach,
            foreign_hit.borrows_into_home,
        ));

    // Adopt the sealed type into `scope` and register it there with the foreign reach — the
    // type-channel mirror of a `LET` binding a module value returned from elsewhere. The envelope
    // host is the foreign frame the type resides in, exactly what a delivered dep would carry.
    let cell = Delivered::hosted(produced.duplicate(), Rc::clone(&foreign));
    let stored = scope.host_reach_of(&cell);
    let kt = match scope.adopt_sealed(&cell) {
        Carried::Type(kt) => kt.clone(),
        _ => panic!("expected the adopted Type"),
    };
    // Drop the envelope now: its only job was the mint + adopt above, and holding it past this
    // point (its `Rc::clone(&foreign)`) would mask the later `drop(foreign)` check below —
    // `scope`'s minted arena set, not this envelope, must be what keeps `foreign` alive from here.
    drop(cell);
    scope.register_type("T".to_string(), kt, BindingIndex::BUILTIN, stored);

    // Drive the exact surface the fixed splice arm uses.
    let hit = match scope.resolve_type_identifier(&TypeIdentifier::leaf("T".to_string()), None) {
        TypeResolution::Done(hit) => hit,
        _ => panic!("expected TypeResolution::Done for a registered type"),
    };
    assert!(
        hit.reach.is_some(),
        "the stored reach should round-trip a non-empty foreign reach",
    );

    // Build the cell as the splice now does — the resident carrier as a delivery envelope pinned
    // by the home frame the value lives under.
    let cell = scope.seal_resident_delivered(scope.resident_type_carrier(
        hit.kt,
        hit.reach,
        hit.borrows_into_home,
    ));

    // The consumer lives in its own frame, independent of `scope`'s (`storage`) and the type's
    // original producer frame (`foreign`) — nothing but its own adopted fold may keep them alive.
    let consumer_storage = run_root_storage();
    let consumer = run_root_bare(&consumer_storage);
    let adopted: Carried = consumer.adopt_sealed(&cell);

    // Drop every other direct handle: `consumer`'s reach-set (folded by `adopt_sealed` above) is
    // now the sole pin on both `scope`'s frame and the type's producer frame.
    drop(cell);
    let _ = produced;
    drop(storage);
    drop(foreign);

    // Read through the adopted type — Miri confirms no use-after-free.
    match adopted {
        Carried::Type(KType::Module { module }) => assert_eq!(module.path, "M"),
        _ => panic!("expected the adopted Module type"),
    }
}
