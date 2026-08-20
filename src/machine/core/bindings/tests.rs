//! Unit coverage for the `types` map write primitive `write_type`, the cross-kind
//! exclusion that makes the `data`/`types` partition structural (no name in both), and the claim
//! store a still-finalizing binder stamps beside the binding maps.

use std::rc::Rc;

use super::*;
use crate::machine::ProducerId;
use crate::machine::core::arena::RegionBrand;
use crate::machine::core::arena::{FrameStorageExt, run_root_storage};
use crate::machine::core::{FrameCoverage, FrameReach, FrameStorage};
use crate::machine::model::KObject;
use crate::machine::model::KType;
use crate::machine::model::Scalar;
use crate::machine::model::values::Carried;
use workgraph::witnessed::Sealed;

use crate::builtins::test_support::{mock_declaration_site, run_root_bare};
use crate::machine::core::GroupSeal;
use crate::machine::core::kfunction::{Body, KFunction};
use crate::machine::core::tests::{body_no_op, unit_signature};
use crate::machine::model::{
    ReductionMode, ReturnType, SignatureDraft, SignatureElement, TypeRegistry, probe_key,
};

/// Seal `obj` as resident in `region` under a description naming `foreign` — the shape a bind
/// door produces once its mint has run, assembled here without a `Scope`.
fn sealed_reaching<'a>(
    region: RegionBrand<'a>,
    obj: &'a KObject<'a>,
    foreign: &Rc<FrameStorage>,
) -> (SealedValue<'a>, &'a FrameReach) {
    // Mint a description naming `foreign` (foreign to `region`, so the self rule keeps it in the
    // owned bundle) to stand in for the reach a value borrows. The mint retains its own bundle in
    // `region`; these tests assert on the description the seal carries, and `foreign` outlives them
    // on the stack.
    let foreign_bundle = FrameCoverage::of(Rc::clone(foreign));
    let reach_set = region.handle().mint_retained(&[&foreign_bundle]);
    let sealed = Sealed::seal(
        region.seal_reaching(Carried::Object(obj), reach_set),
        region.handle(),
    );
    (sealed, reach_set)
}

/// The sole member of the description a bound entry's seal carries. The reach is readable only
/// under a pin, so the caller's own hold on the hosting frame opens the seal.
fn sole_reach_member(sealed: &SealedValue<'_>) -> Rc<FrameStorage> {
    sealed.open_at().with_reach_for_test(|reach| {
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
            Rc::ptr_eq(&sole_reach_member(&hit), &foreign),
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
        Some(NameLookup::Bound(hit)) => {
            hit.open_at().with_reach_for_test(|reach| reach as *const _)
        }
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
            ProducerId::for_test(7),
            BindingIndex::BUILTIN,
            BindKind::Type,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("placeholder install should succeed on fresh bindings");
    assert_eq!(
        bindings.pending_names(),
        vec![("Bar".to_string(), BindKind::Type, ProducerId::for_test(7))],
    );
    bindings
        .write_type(
            "Bar",
            kt,
            DeclarationSite::BUILTIN,
            TypeWritePolicy::Insert,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("type register should succeed and retire the claim");
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

/// Declaration identity is the installing statement: two `UpsertEqual write`s of one name whose
/// [`Installer`]s carry distinct [`StatementId`]s are two declarations, and a re-entry under the
/// identical id is one declaration finalizing twice. `StatementId`s are minted from a
/// never-recycled process-global counter, so this also settles the persistent-scope case for free:
/// a later run over the same scope submits its statements under fresh ids and cannot collide with
/// an entry an earlier run installed. A regression that let a recycled or restarted id stand in
/// would take the idempotent arm on the second install and this test would fail.
#[test]
fn distinct_statement_redeclare_rebinds() {
    let storage = run_root_storage();
    let bindings = Bindings::new(storage.brand());
    let first = StatementId::next();
    let second = StatementId::next();
    assert_ne!(
        first, second,
        "two submissions must mint distinct StatementIds"
    );
    // Both declarations land at the same binding index: a driver numbers submissions like
    // lines of a file, but a persistent scope's later run starts numbering from 1 again, so
    // two separate runs can install declarations under equal indices — only the statement
    // can tell them apart.
    let site = |statement| DeclarationSite {
        installer: Installer::Statement(statement),
        index: BindingIndex::value(0),
    };

    bindings
        .write_type(
            "Maybe",
            KType::NUMBER,
            site(first),
            TypeWritePolicy::UpsertEqual,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("the first declaration should install");

    // The same statement re-entering (a parallel finalize of it): idempotent overwrite.
    bindings
        .write_type(
            "Maybe",
            KType::NUMBER,
            site(first),
            TypeWritePolicy::UpsertEqual,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("a same-statement parallel finalize should overwrite idempotently");

    // A second statement declaring the same name — a redeclaration, whatever its content: Rebind.
    let error = match bindings.write_type(
        "Maybe",
        KType::STR,
        site(second),
        TypeWritePolicy::UpsertEqual,
        &mut crate::machine::WriteGate::for_test(),
    ) {
        Err(e) => e,
        Ok(_) => panic!("a second declaration of Maybe must Rebind, not overwrite"),
    };
    assert!(
        matches!(&error.kind, KErrorKind::Rebind { name } if name == "Maybe"),
        "expected Rebind naming Maybe, got {error}",
    );
    // The first statement's entry survives the rejected redeclaration.
    assert_eq!(
        bindings
            .types()
            .get("Maybe")
            .expect("Maybe should still be bound")
            .0,
        KType::NUMBER,
    );
}

// --- Cross-kind exclusion (AC1/AC4) -----------------------------------------
// `partition_guard` classifies a name by its token class alone (`parse::is_type_name`): a value
// token may not bind type-side, a type token may not bind value-side, so the same name never lands
// in both maps. `value_token_may_not_bind_type_side` / `type_token_may_not_bind_value_side` below
// drive the write primitives against a plain `Bindings::new()`. A bare `FN` binds neither `data`
// nor `types`, so it has nothing to partition.

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
        Sealed::seal(region.seal_resident(Carried::Object(val)), region.handle()),
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

// --- Claims live in the store, and a commit retires its own -------------------
// A still-finalizing binder claims the name it will resolve into; the binding maps hold committed
// bindings only, so a commit writes its map and removes its claim rather than overwriting an arm.

/// A value write commits into `data` and **retires its own claim** — it carries the name and the
/// index it is writing at, so nothing is searched. Afterwards the statement's live mask is zero,
/// which is the whole of the success path: retiring the statement finds nothing left to drop.
#[test]
fn value_write_commits_and_retires_its_own_claim() {
    let storage = run_root_storage();
    let region = storage.brand();
    let bindings = Bindings::new(region);
    bindings
        .install_placeholder(
            "x",
            ProducerId::for_test(11),
            BindingIndex::value(2),
            BindKind::Value,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("value claim should succeed on fresh bindings");
    assert_eq!(
        bindings.pending_value("x").map(|p| p.producer),
        Some(ProducerId::for_test(11)),
    );
    assert!(matches!(
        bindings.lookup_value("x", None),
        Some(NameLookup::Parked(id)) if id == ProducerId::for_test(11),
    ));

    let val: &KObject = region.alloc_scalar(Scalar::Number(5.0));
    bindings
        .write_value(
            "x",
            BindingIndex::value(2),
            Sealed::seal(region.seal_resident(Carried::Object(val)), region.handle()),
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("the finalize write should commit and retire the claim");
    assert!(bindings.pending_value("x").is_none());
    assert!(bindings.pending_names().is_empty());
    assert!(matches!(
        bindings.lookup_value("x", None),
        Some(NameLookup::Bound(_)),
    ));
    assert_eq!(bindings.data().len(), 1, "the key is stored once");

    // The zero-mask path: the slot's retirement has nothing left to do, and does nothing.
    bindings.retire_claims(
        BindingIndex::value(2),
        &mut crate::machine::WriteGate::for_test(),
    );
    assert!(matches!(
        bindings.lookup_value("x", None),
        Some(NameLookup::Bound(_)),
    ));
}

/// One bucket-keyed binder declaring **two** buckets seals only one of them — the shape a
/// `UNARY OP` takes when its bridge key never registers. The sealing write retires that key's claim
/// alone, and the statement's retirement drops the other, so no claim outlives the edge its owner
/// releases.
#[test]
fn a_two_bucket_binder_retires_the_key_it_did_not_seal() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let mut gate = crate::machine::WriteGate::for_test();
    let f = KFunction::alloc_captured(scope, unit_signature(), Body::Builtin(body_no_op), &types);
    let sealed_key = f.open(|f| f.signature.untyped_key());
    let bridge_key: UntypedKey = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![SignatureElement::Keyword("BRIDGE")],
    }
    .untyped_key();

    for key in [&sealed_key, &bridge_key] {
        scope
            .install_pending_overload(
                key.clone(),
                ProducerId::for_test(4),
                BindingIndex::value(1),
                &mut gate,
            )
            .expect("one statement claims both keys it declares");
    }
    let bindings = scope.bindings();
    assert_eq!(bindings.pending_overload_entries(&sealed_key).len(), 1);
    assert_eq!(bindings.pending_overload_entries(&bridge_key).len(), 1);

    scope
        .register_function_direct("FOO".to_string(), &f, BindingIndex::value(1), &mut gate)
        .expect("the seal lands and retires this key's claim");
    assert!(
        bindings.pending_overload_entries(&sealed_key).is_empty(),
        "the sealed key's claim is retired by its own commit",
    );
    assert_eq!(
        bindings.pending_overload_entries(&bridge_key).len(),
        1,
        "the unsealed key's claim is untouched by the other key's commit",
    );

    scope.retire_claims(BindingIndex::value(1), &mut gate);
    assert!(
        bindings.pending_overload_entries(&bridge_key).is_empty(),
        "the statement's retirement drops the claim its commit did not",
    );
    assert_eq!(
        bindings.lookup_function(&sealed_key, None).overloads.len(),
        1,
        "retirement touches no committed overload",
    );
}

/// `bulk_install_from` copies the source's committed bindings and **nothing else**. A live claim is
/// not filtered out of the copy — it was never in a map to be copied. That is what makes handing a
/// view a park on an edge of the source's own scheduler run unrepresentable rather than merely
/// unobserved.
#[test]
fn bulk_install_copies_committed_bindings_past_a_live_claim() {
    let storage = run_root_storage();
    let region = storage.brand();
    let source = Bindings::new(region);
    let target = Bindings::new(region);
    let mut gate = crate::machine::WriteGate::for_test();

    let val: &KObject = region.alloc_scalar(Scalar::Number(7.0));
    source
        .write_value(
            "settled",
            BindingIndex::value(1),
            Sealed::seal(region.seal_resident(Carried::Object(val)), region.handle()),
            &mut gate,
        )
        .expect("the committed binding lands");
    source
        .install_placeholder(
            "in_flight",
            ProducerId::for_test(3),
            BindingIndex::value(2),
            BindKind::Value,
            &mut gate,
        )
        .expect("the second statement is still running");

    target
        .bulk_install_from(&source, &mut gate)
        .expect("the view replays the source's committed surface");
    assert!(matches!(
        target.lookup_value("settled", None),
        Some(NameLookup::Bound(_)),
    ));
    assert!(
        target.lookup_value("in_flight", None).is_none(),
        "a claim never crosses into a view",
    );
    assert!(target.pending_names().is_empty());
    // The source keeps its own claim: the copy read past it, it did not consume it.
    assert_eq!(
        source.pending_value("in_flight").map(|c| c.producer),
        Some(ProducerId::for_test(3)),
    );
}

/// A `types` slot holds a bound identity and a pending producer at once: a parallel nominal
/// finalize pre-installs the name's external identity while its producer is still in flight.
/// `lookup_type` answers the bound identity (it probes `types` first), `type_placeholder_producer`
/// still surfaces the producer for the finalize gate to park on, and retiring the statement drops
/// only the claim — the bound identity survives, since the two live in different structures.
#[test]
fn a_bound_identity_and_a_live_claim_stand_on_one_name_at_once() {
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
            ProducerId::for_test(9),
            BindingIndex::BUILTIN,
            BindKind::Type,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("the in-flight binder claims the same name");

    assert!(matches!(
        bindings.lookup_type("Wrapper", None),
        Some(NameLookup::Bound(kt)) if kt == KType::NUMBER,
    ));
    assert_eq!(
        bindings.type_placeholder_producer("Wrapper"),
        Some(ProducerId::for_test(9)),
    );

    bindings.retire_claims(
        BindingIndex::BUILTIN,
        &mut crate::machine::WriteGate::for_test(),
    );
    assert!(bindings.pending_names().is_empty());
    assert_eq!(bindings.expect_type("Wrapper"), KType::NUMBER);
}

/// **All five tables past their resize thresholds, in one scope.** Binding a handful of names
/// never leaves hashbrown's initial capacity, so this test binds past it: a table that has actually
/// reallocated its bucket array into the bump, a purge that empties a bucket and strands its key,
/// and a powerset install against a table already holding entries.
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
                    mock_declaration_site(i),
                    TypeWritePolicy::Insert,
                    &mut gate,
                )
                .expect("a fresh type bind lands");
        }
        assert!(scope.bindings().lookup_type("Ty63", None).is_some());

        // A dispatch bucket claimed by two sibling binders, one of which finalizes and retires its
        // own claim while the sibling's stands.
        let f =
            KFunction::alloc_captured(scope, unit_signature(), Body::Builtin(body_no_op), &types);
        let sealed_key = f.open(|f| f.signature.untyped_key());
        for claim in [ProducerId::for_test(7), ProducerId::for_test(8)] {
            scope
                .install_pending_overload(
                    sealed_key.clone(),
                    claim,
                    BindingIndex::value(1),
                    &mut gate,
                )
                .expect("a sibling claim appends");
        }
        scope
            .register_function_direct("FOO".to_string(), &f, BindingIndex::value(1), &mut gate)
            .expect("the seal appends and retires this binder's own claim");

        // A second producer's claim on its own bucket, retired without a commit — the failed-binder
        // path, which strands the claim's bump bytes, exercised so the leak check sees it.
        let purged_key: UntypedKey = SignatureDraft {
            return_type: ReturnType::Resolved(KType::ANY),
            elements: vec![SignatureElement::Keyword("BAR")],
        }
        .untyped_key();
        scope
            .install_pending_overload(
                purged_key.clone(),
                ProducerId::for_test(9),
                BindingIndex::value(2),
                &mut gate,
            )
            .expect("the purged binder claims its bucket");
        scope
            .bindings()
            .retire_claims(BindingIndex::value(2), &mut gate);
        assert!(
            scope
                .bindings()
                .lookup_function(&purged_key, None)
                .overloads
                .is_empty()
        );
        assert_eq!(
            scope
                .bindings()
                .lookup_function(&sealed_key, None)
                .overloads
                .len(),
            1,
            "the finalized overload survives the sibling's retirement",
        );

        // A per-group powerset install: every subset key's text bumped, all pointing at one record.
        let record = scope.birth_operator_group(&["+", "-", "*"], ReductionMode::FoldLeft);
        for probe in powerset_probes(&["+", "-", "*"]) {
            scope
                .register_operator_group_direct(
                    probe,
                    GroupSeal::of_delivered(scope, &record),
                    BindingIndex::value(3),
                    &mut gate,
                )
                .expect("every subset of one declaration upserts to the same record");
        }
        assert!(
            scope
                .bindings()
                .lookup_operator_group(&probe_key(&["*", "+"]), None)
                .is_some()
        );

        // SIG slot records through a real scope: the fifth table, over the same bump.
        let sig = scope.alloc_child_under_sig("Shape");
        for i in 0..48 {
            sig.write_sig_slot(format!("slot_{i}"), KType::NUMBER)
                .expect("a fresh VAL slot records");
        }
        assert_eq!(sig.sig_value_slots().len(), 48);
    }
    drop(region);
}
