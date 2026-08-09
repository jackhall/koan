//! Unit coverage for the `types` map write primitive `write_type`, the cross-kind
//! exclusion that makes the `data`/`types` partition structural (no name in both), and the
//! pending arms a still-finalizing binder occupies in its destination table.

use std::rc::Rc;

use super::*;
use crate::machine::core::arena::RegionBrand;
use crate::machine::core::arena::{run_root_storage, FrameStorageExt};
use crate::machine::core::{FrameCoverage, FrameReach, FrameStorage};
use crate::machine::model::values::Carried;
use crate::machine::model::KObject;
use crate::machine::model::KType;
use crate::machine::model::Scalar;
use workgraph::scheduler::Sealed;

use crate::builtins::test_support::{mock_declaration_site, run_root_bare};
use crate::machine::core::kfunction::{Body, KFunction};
use crate::machine::core::tests::{body_no_op, unit_signature};
use crate::machine::core::GroupSeal;
use crate::machine::model::{
    probe_key, OperatorGroup, ReductionMode, ReturnType, SignatureDraft, SignatureElement,
    TypeRegistry,
};

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
    let bindings = Bindings::new(region);
    let obj: &KObject = region.alloc_scalar(Scalar::Number(1.0));
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
    let bindings = Bindings::new(region);
    let obj: &KObject = region.alloc_scalar(Scalar::Number(1.0));
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
    let storage = run_root_storage();
    let bindings = Bindings::new(storage.brand());
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
        .and_then(|slot| slot.bound())
        .expect("Foo should be bound in the types map")
        .0;
    assert_eq!(stored, kt);
    assert!(bindings.data().get("Foo").is_none());
}

#[test]
fn write_type_rejects_collision_with_rebind() {
    let storage = run_root_storage();
    let bindings = Bindings::new(storage.brand());
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
        .and_then(|slot| slot.bound())
        .expect("Foo should still be bound")
        .0;
    assert_eq!(stored, kt1);
}

#[test]
fn write_type_finalizes_pending_arm_in_place() {
    let storage = run_root_storage();
    let bindings = Bindings::new(storage.brand());
    let kt: KType = KType::NUMBER;
    bindings
        .install_placeholder(
            "Bar",
            NodeId(7),
            BindingIndex::BUILTIN,
            BindKind::Type,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("placeholder install should succeed on fresh bindings");
    assert_eq!(
        bindings.pending_names(),
        vec![("Bar".to_string(), BindKind::Type, NodeId(7))],
    );
    bindings
        .write_type(
            "Bar",
            kt,
            DeclarationSite::BUILTIN,
            TypeWritePolicy::Insert,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("type register should succeed and drop the pending arm");
    assert!(bindings.pending_names().is_empty());
    assert_eq!(bindings.expect_type("Bar"), kt);
}

#[test]
fn write_type_does_not_touch_data_or_functions() {
    let storage = run_root_storage();
    let bindings = Bindings::new(storage.brand());
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
    let storage = run_root_storage();
    let bindings = Bindings::new(storage.brand());
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
            .and_then(|slot| slot.bound())
            .expect("Maybe should still be bound")
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
    let storage = run_root_storage();
    let bindings = Bindings::new(storage.brand());
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
    let bindings = Bindings::new(region);
    let val: &KObject = region.alloc_scalar(Scalar::Number(7.0));
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

// --- Pending arms live in the destination table ------------------------------
// A still-finalizing binder claims the slot it will resolve into, so finalizing
// overwrites that slot rather than moving an entry between containers.

/// A value write finalizes the name's pending arm **in place**: the slot flips to `Bound` and the
/// claim is gone, with the key stored once. The overwrite keys on the name alone — a write whose
/// producer differs from the one that claimed the slot still finalizes it.
#[test]
fn value_write_finalizes_the_pending_arm_in_place() {
    let storage = run_root_storage();
    let region = storage.brand();
    let bindings = Bindings::new(region);
    bindings
        .install_placeholder(
            "x",
            NodeId(11),
            BindingIndex::value(2),
            BindKind::Value,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("value claim should succeed on fresh bindings");
    assert_eq!(
        bindings.pending_value("x").map(|p| p.producer),
        Some(NodeId(11)),
    );
    assert!(matches!(
        bindings.lookup_value("x", None),
        Some(NameLookup::Parked(NodeId(11))),
    ));

    let val: &KObject = region.alloc_scalar(Scalar::Number(5.0));
    bindings
        .write_value(
            "x",
            BindingIndex::value(2),
            Sealed::seal(region.seal_resident(Carried::Object(val))),
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("the finalize write should overwrite the pending arm");
    assert!(bindings.pending_value("x").is_none());
    assert!(bindings.pending_names().is_empty());
    assert!(matches!(
        bindings.lookup_value("x", None),
        Some(NameLookup::Bound(_)),
    ));
    assert_eq!(bindings.data().len(), 1, "the key is stored once");
}

/// A `types` slot holds a bound identity and a pending producer at once: a parallel nominal
/// finalize pre-installs the name's external identity while its producer is still in flight.
/// `lookup_type` answers the bound identity (it prefers the bound arm), `type_placeholder_producer`
/// still surfaces the producer for the finalize gate to park on, and the producer-failure sweep
/// drops only the pending arm — the bound identity survives.
#[test]
fn type_slot_carries_a_bound_identity_and_a_pending_producer_at_once() {
    let storage = run_root_storage();
    let bindings = Bindings::new(storage.brand());
    bindings
        .write_type(
            "Wrapper",
            KType::NUMBER,
            DeclarationSite::BUILTIN,
            TypeWritePolicy::Insert,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("the seal pre-installs the external identity");
    bindings
        .install_placeholder(
            "Wrapper",
            NodeId(9),
            BindingIndex::BUILTIN,
            BindKind::Type,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("the in-flight producer claims the same slot");

    assert!(matches!(
        bindings.lookup_type("Wrapper", None),
        Some(NameLookup::Bound(kt)) if kt == KType::NUMBER,
    ));
    assert_eq!(
        bindings.type_placeholder_producer("Wrapper"),
        Some(NodeId(9)),
    );

    bindings.clear_placeholders_for_producer(NodeId(9), &mut crate::machine::WriteGate::for_test());
    assert!(bindings.pending_names().is_empty());
    assert_eq!(bindings.expect_type("Wrapper"), KType::NUMBER);
}

/// **All five tables past their resize thresholds, in one scope.** Every other test here binds a
/// handful of names, which never leaves hashbrown's initial capacity — so nothing else exercises a
/// table that has actually reallocated its bucket array into the bump, nor a purge that empties a
/// bucket and strands its key, nor a powerset install against a table already holding entries.
///
/// Behavioural, not a memory audit: the allocator seam itself is pinned under tree borrows by
/// workgraph's own slate (`a_bump_backed_table_survives_growth_overwrite_and_removal`), and what
/// would otherwise be a leak claim here — an entry smuggling drop glue back in — is a compile-time
/// assert at [`bump_table`] and on [`Bindings`] rather than something a run could observe.
#[test]
fn bump_backed_tables_full_churn() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    {
        let scope = run_root_bare(&region);
        let mut gate = crate::machine::WriteGate::for_test();

        // Enough value binds to force several geometric resizes off hashbrown's initial capacity,
        // with each key's text bumped between the reallocations.
        for i in 0..96 {
            let value = scope.brand().alloc_scalar(Scalar::Number(i as f64));
            scope
                .bind_resident_for_test(
                    format!("value_{i}"),
                    value,
                    BindingIndex::value(i as usize),
                    &mut gate,
                )
                .expect("a fresh value bind lands");
        }
        assert!(scope.bindings().lookup_value("value_95", None).is_some());

        // Type binds resize the second table against the same bump.
        for i in 0..64 {
            scope
                .bindings()
                .write_type(
                    &format!("Ty{i}"),
                    KType::NUMBER,
                    mock_declaration_site(i, i),
                    TypeWritePolicy::Insert,
                    &mut gate,
                )
                .expect("a fresh type bind lands");
        }
        assert!(scope.bindings().lookup_type("Ty63", None).is_some());

        // A dispatch bucket claimed by two sibling binders, one of which finalizes into its own
        // pending slot — the in-place overwrite that keeps peak occupancy at the binding count.
        let f = KFunction::alloc_captured(
            scope,
            unit_signature(),
            Body::Builtin(body_no_op),
            false,
            &types,
        );
        let sealed_key = f.signature.untyped_key();
        for producer in [NodeId(7), NodeId(8)] {
            scope
                .install_pending_overload(
                    sealed_key.clone(),
                    producer,
                    BindingIndex::value(1),
                    &mut gate,
                )
                .expect("a sibling claim appends");
        }
        scope
            .register_function_direct("FOO".to_string(), f, BindingIndex::value(1), &mut gate)
            .expect("the seal lands in the claim it finalizes");

        // A second producer's claim on its own bucket, purged so the bucket empties and its key is
        // removed — the one path that strands bump bytes, exercised so the leak check sees it.
        let purged_key: UntypedKey = SignatureDraft {
            return_type: ReturnType::Resolved(KType::ANY),
            elements: vec![SignatureElement::Keyword("BAR")],
        }
        .untyped_key();
        scope
            .install_pending_overload(
                purged_key.clone(),
                NodeId(9),
                BindingIndex::value(2),
                &mut gate,
            )
            .expect("the purged binder claims its bucket");
        scope
            .bindings()
            .clear_placeholders_for_producer(NodeId(9), &mut gate);
        assert!(scope
            .bindings()
            .lookup_function(&purged_key, None)
            .overloads
            .is_empty());
        assert_eq!(
            scope
                .bindings()
                .lookup_function(&sealed_key, None)
                .overloads
                .len(),
            1,
            "the finalized overload survives the sibling purge",
        );

        // A per-group powerset install: every subset key's text bumped, all pointing at one record.
        let record = OperatorGroup::alloc(scope.brand(), &["+", "-", "*"], ReductionMode::FoldLeft);
        for probe in powerset_probes(&["+", "-", "*"]) {
            scope
                .register_operator_group_direct(
                    probe,
                    GroupSeal::of_resident(scope, record),
                    BindingIndex::value(3),
                    &mut gate,
                )
                .expect("every subset of one declaration upserts to the same record");
        }
        assert!(scope
            .bindings()
            .lookup_operator_group(&probe_key(&["*", "+"]), None)
            .is_some());

        // SIG slot records through a real scope: the fifth table, over the same bump.
        let sig = scope.alloc_child_under_sig("Shape".to_string());
        for i in 0..48 {
            sig.write_sig_slot(format!("slot_{i}"), KType::NUMBER)
                .expect("a fresh VAL slot records");
        }
        assert_eq!(sig.sig_value_slots().len(), 48);
    }
    drop(region);
}
