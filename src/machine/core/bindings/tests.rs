//! Unit coverage for the `types` map write primitive `write_type`, plus the cross-kind
//! exclusion that makes the `data`/`types` partition structural (no name in both).

use super::*;
use crate::machine::core::arena::RegionBrand;
use crate::machine::core::arena::{run_root_storage, FrameStorageExt};
use crate::machine::core::{FrameCoverage, FrameReach, FrameStorage};
use crate::machine::model::values::Carried;
use crate::machine::model::KObject;
use crate::machine::model::KType;
use workgraph::scheduler::Sealed;

/// Seal `obj` as resident in `region` under a description naming `foreign` — the shape a bind
/// door produces once its mint has run, assembled here without a `Scope`.
fn sealed_reaching<'a>(
    region: RegionBrand<'a>,
    obj: &'a KObject<'a>,
    foreign: &Rc<FrameStorage>,
) -> (SealedValue, &'a FrameReach) {
    // Mint a description naming `foreign` (foreign to `region`, so the self rule keeps it in the
    // owned bundle) to stand in for the reach a value borrows. The mint retains its own bundle in
    // `region`; these tests assert on the description the seal carries, and `foreign` outlives them
    // on the stack.
    let foreign_bundle = FrameCoverage::of(Rc::clone(foreign));
    let reach_set = region.handle().mint_retained(&[&foreign_bundle]);
    let sealed = Sealed::seal(region.seal_reaching(Carried::Object(obj), reach_set));
    (sealed, reach_set)
}

/// The sole member of the description a bound entry's seal carries. The reach is readable only
/// under a pin, so the caller's own hold on the hosting frame opens the seal.
fn sole_reach_member(sealed: &SealedValue, pin: &Rc<FrameStorage>) -> Rc<FrameStorage> {
    sealed.open_at(pin).with_reach(|reach| {
        let members = reach.members();
        match members.as_slice() {
            [only] => Rc::clone(only),
            _ => panic!("expected a single-member reach"),
        }
    })
}

/// A value binding round-trips the exact reach it was sealed with: the entry stores the seal
/// verbatim, so a read hands back a carrier naming the value's reach without reconstructing it
/// from the value.
#[test]
fn data_binding_round_trips_sealed_reach() {
    let storage = run_root_storage();
    let region = storage.brand();
    let bindings: Bindings = Bindings::new();
    let obj: &KObject = region.alloc_object(KObject::Number(1.0));
    // A synthetic foreign frame the value "reaches" — carried on the seal as its reach.
    let foreign = run_root_storage();
    let (sealed, _) = sealed_reaching(region, obj, &foreign);
    bindings
        .write_value(
            "x",
            BindingIndex::BUILTIN,
            sealed,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("value bind should succeed");
    match bindings.lookup_value("x", None) {
        Some(NameLookup::Bound(hit)) => assert!(
            Rc::ptr_eq(&sole_reach_member(&hit, &storage), &foreign),
            "the sealed reach should round-trip the foreign frame",
        ),
        _ => panic!("expected a bound value hit"),
    }
}

/// A read duplicates the entry's seal, which copies the hosted `&FrameReach` reference — no per-hit
/// clone. Two independent reads of the same binding name the *same* description, proving the read
/// path reuses the arena-hosted set rather than cloning a fresh one on every hit.
#[test]
fn value_binding_read_copies_the_reach_pointer_not_a_clone() {
    let storage = run_root_storage();
    let region = storage.brand();
    let bindings: Bindings = Bindings::new();
    let obj: &KObject = region.alloc_object(KObject::Number(1.0));
    let foreign = run_root_storage();
    let (sealed, reach_set) = sealed_reaching(region, obj, &foreign);
    bindings
        .write_value(
            "x",
            BindingIndex::BUILTIN,
            sealed,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("value bind should succeed");

    let read = |label: &str| match bindings.lookup_value("x", None) {
        Some(NameLookup::Bound(hit)) => hit.open_at(&storage).with_reach(|reach| reach as *const _),
        _ => panic!("expected a bound value hit for {label}"),
    };
    let (first, second) = (read("first"), read("second"));
    assert!(
        std::ptr::eq(first, second),
        "two reads of the same binding must name the same &FrameReach — a clone would allocate a \
         fresh Vec at a distinct address on every hit",
    );
    assert!(
        std::ptr::eq(first, reach_set),
        "the carried reach is the exact reference sealed in, not a copy of it",
    );
}

#[test]
fn write_type_inserts_into_types_map() {
    let bindings: Bindings = Bindings::new();
    let kt: KType = KType::NUMBER;
    bindings
        .write_type(
            "Foo",
            kt,
            DeclarationSite::BUILTIN,
            TypeWritePolicy::Insert,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("write_type should succeed on fresh bindings");
    let stored = bindings
        .types()
        .get("Foo")
        .expect("Foo should be in types map")
        .0;
    assert_eq!(stored, kt);
    assert!(bindings.data().get("Foo").is_none());
}

#[test]
fn write_type_rejects_collision_with_rebind() {
    let bindings: Bindings = Bindings::new();
    let kt1: KType = KType::NUMBER;
    let kt2: KType = KType::STR;
    bindings
        .write_type(
            "Foo",
            kt1,
            DeclarationSite::BUILTIN,
            TypeWritePolicy::Insert,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("first register should succeed");
    let err = match bindings.write_type(
        "Foo",
        kt2,
        DeclarationSite::BUILTIN,
        TypeWritePolicy::Insert,
        &mut crate::machine::WriteGate::for_test(),
    ) {
        Err(e) => e,
        Ok(_) => panic!("second register on same name should error, not succeed"),
    };
    assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "Foo"));
    let stored = bindings
        .types()
        .get("Foo")
        .expect("Foo should still be present")
        .0;
    assert_eq!(stored, kt1);
}

#[test]
fn write_type_clears_matching_placeholder() {
    let bindings: Bindings = Bindings::new();
    let kt: KType = KType::NUMBER;
    bindings
        .install_placeholder(
            "Bar".to_string(),
            NodeId(7),
            BindingIndex::BUILTIN,
            BindKind::Type,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("placeholder install should succeed on fresh bindings");
    assert!(bindings.placeholders().contains_key("Bar"));
    bindings
        .write_type(
            "Bar",
            kt,
            DeclarationSite::BUILTIN,
            TypeWritePolicy::Insert,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("type register should succeed and clear placeholder");
    assert!(!bindings.placeholders().contains_key("Bar"));
}

#[test]
fn write_type_does_not_touch_data_or_functions() {
    let bindings: Bindings = Bindings::new();
    let kt: KType = KType::NUMBER;
    bindings
        .write_type(
            "Foo",
            kt,
            DeclarationSite::BUILTIN,
            TypeWritePolicy::Insert,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("register should succeed");
    assert!(bindings.data().is_empty());
    assert!(bindings.functions().is_empty());
}

/// Declaration identity is run-qualified: two `UpsertEqual write`s of one name whose
/// [`NodeHandle`]s share a `NodeId` but carry distinct [`RunId`]s are two declarations, because
/// `NodeId`s are scheduler-local and restart per run — only the pair identifies a declaration
/// statement across the lifetime of a persistent scope. The same-run re-entry (identical handle) is
/// an idempotent parallel finalize; the cross-run re-entry (same node, later run) is a `Rebind`.
/// This pins the accepted persistent-scope consequence directly at the decision door: a regression
/// that compared only the `NodeId` — dropping the `RunId` from handle equality — would take the
/// idempotent arm on the cross-run install and this test would fail.
#[test]
fn cross_run_redeclare_rebinds_on_run_qualified_handle() {
    let bindings: Bindings = Bindings::new();
    let first_run = RunId::next();
    let second_run = RunId::next();
    assert_ne!(first_run, second_run, "two runs must mint distinct RunIds");
    // One scheduler-local NodeId, reused across both runs — as a per-run scheduler would restart it.
    let node = NodeId(5);
    let site = |run| DeclarationSite {
        node: NodeHandle { run, node },
        index: BindingIndex::value(0),
    };

    bindings
        .write_type(
            "Maybe",
            KType::NUMBER,
            site(first_run),
            TypeWritePolicy::UpsertEqual,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("the first declaration should install");

    // Same handle re-entering (a parallel finalize of the first declaration): idempotent overwrite.
    bindings
        .write_type(
            "Maybe",
            KType::NUMBER,
            site(first_run),
            TypeWritePolicy::UpsertEqual,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("a same-handle parallel finalize should overwrite idempotently");

    // A later run over the persistent scope reuses the NodeId but carries a fresh RunId, so its
    // handle differs from the stored entry's and the install is a second declaration: Rebind.
    let error = match bindings.write_type(
        "Maybe",
        KType::STR,
        site(second_run),
        TypeWritePolicy::UpsertEqual,
        &mut crate::machine::WriteGate::for_test(),
    ) {
        Err(e) => e,
        Ok(_) => panic!("a cross-run redeclaration of Maybe must Rebind, not overwrite"),
    };
    assert!(
        matches!(&error.kind, KErrorKind::Rebind { name } if name == "Maybe"),
        "expected Rebind naming Maybe across runs, got {error}",
    );
    // The first run's entry survives the rejected cross-run install.
    assert_eq!(
        bindings
            .types()
            .get("Maybe")
            .expect("Maybe should still be present")
            .0,
        KType::NUMBER,
    );
}

// --- Cross-kind exclusion (AC1/AC4) -----------------------------------------
// Each declarator routes to one of these write primitives (LET-value →
// `write_value`; LET-type-alias / VAL / NEWTYPE-sigil → `write_type`;
// MODULE / SIG / UNION / NEWTYPE-record / RECURSIVE-finalize →
// `UpsertEqual write`; module/USING replay → `try_bulk_install_from`).
// `partition_guard` is the single enforcement point every one of these primitives calls, so
// `value_token_may_not_bind_type_side` / `type_token_may_not_bind_value_side` below — exercised
// against a plain `Bindings::new()` — prove the exclusion for every bind site: a name's token
// class fixes which map it may ever enter, so the same name can never land in both. The reverse —
// a bare `FN`, which binds neither `data` nor `types` — is exempt; that is covered Scope-side in
// `core::tests::register`.

/// The token-class partition: `types` and `data` are different universes, and a name's token class
/// decides which one it belongs to. A value token may not name a type…
#[test]
fn value_token_may_not_bind_type_side() {
    let bindings: Bindings = Bindings::new();
    let kt: KType = KType::NUMBER;
    let error = match bindings.write_type(
        "int_ord",
        kt,
        DeclarationSite::BUILTIN,
        TypeWritePolicy::Insert,
        &mut crate::machine::WriteGate::for_test(),
    ) {
        Err(e) => e,
        Ok(_) => panic!("a value token names a value, not a type"),
    };
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg) if msg.contains("is a value token")),
        "expected the token-class partition error, got {error}",
    );
    assert!(bindings.types().get("int_ord").is_none());
}

/// …and a Type token may not name a value. Together these commit every name to exactly one
/// universe: the partition admits no exception, so a cross-kind collision — the same name
/// landing in both maps — is unconstructible.
#[test]
fn type_token_may_not_bind_value_side() {
    let storage = run_root_storage();
    let region = storage.brand();
    let bindings: Bindings = Bindings::new();
    let val: &KObject = region.alloc_object(KObject::Number(7.0));
    let error = match bindings.write_value(
        "IntOrd",
        BindingIndex::BUILTIN,
        Sealed::seal(region.seal_resident(Carried::Object(val))),
        &mut crate::machine::WriteGate::for_test(),
    ) {
        Err(e) => e,
        Ok(_) => panic!("a Type token names a type, not a value"),
    };
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg) if msg.contains("is a Type token")),
        "expected the token-class partition error, got {error}",
    );
    assert!(bindings.data().get("IntOrd").is_none());
}
