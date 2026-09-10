//! The live builtin registration set, derived once, and the law that pins [`FORMS`] against it.
//!
//! Recognizing a builtin form by its full bucket key is sound only because a matched key can
//! resolve to nothing but that builtin's overloads. That soundness is a claim about the *live*
//! registration table, not about the form table, so it is checked by walking the seeded root and
//! comparing — never by reading the table against itself. This module holds the one walk; every
//! table⟺registration question in the test tree is answered from it.

use std::collections::BTreeMap;

use super::lazy::kind_of;
use crate::builtins::test_support::TestRun;
use crate::machine::model::{KType, SignatureElement, TypeNode};
use crate::memory::{program_storage, run_root_storage};
use crate::parse::UntypedKey;
use crate::parse::forms::binder::BinderFacts;
use crate::parse::forms::lazy::LazyKinds;
use crate::parse::forms::{FORMS, Form, key_matches, render_key};

/// One live builtin bucket: the key it registers under, and per argument index the slot type each
/// of its overloads declares there.
struct LiveBucket {
    key: UntypedKey,
    slot_types: BTreeMap<usize, Vec<KType>>,
}

/// The **one** derivation of the live registration set in the test tree: a single walk of the
/// seeded root's scope chain, giving every registered bucket key beside the slot types its
/// overloads declare. Everything the table is pinned against is read off this.
fn live_registrations(run: &TestRun<'_>) -> Vec<LiveBucket> {
    let mut live = Vec::new();
    for scope in run.scope.ancestors() {
        for (key, bucket) in scope.bindings().functions().iter() {
            let mut slot_types: BTreeMap<usize, Vec<KType>> = BTreeMap::new();
            for entry in bucket.iter() {
                let opened = entry.sealed.open_at();
                for (index, element) in opened.value().signature.elements().iter().enumerate() {
                    if let SignatureElement::Argument(argument) = element {
                        slot_types.entry(index).or_default().push(argument.ktype);
                    }
                }
            }
            live.push(LiveBucket {
                key: key.to_vec(),
                slot_types,
            });
        }
    }
    live
}

/// The lazy slots a live bucket actually declares: per slot index, the union over its overloads of
/// each raw-capture slot's kind. A bucket declaring none is absent from the map.
fn declared_lazy_slots(
    bucket: &LiveBucket,
    types: &crate::machine::model::TypeRegistry,
) -> BTreeMap<usize, LazyKinds> {
    let mut slots: BTreeMap<usize, LazyKinds> = BTreeMap::new();
    for (index, ktypes) in &bucket.slot_types {
        for ktype in ktypes {
            if let Some(kind) = kind_of(*ktype, types) {
                let slot = slots.entry(*index).or_default();
                *slot = slot.with(kind);
            }
        }
    }
    slots
}

/// Every form the table gives binder facts, with those facts beside it.
fn binder_forms() -> impl Iterator<Item = (&'static Form, BinderFacts)> {
    FORMS
        .iter()
        .filter_map(|form| form.binder.map(|binder| (form, binder)))
}

/// The form table says the same thing about the builtins as the builtins do.
///
/// Four readings of one walk, in both directions:
///
/// - a non-reserved key names a live bucket — a builtin renamed, re-shaped or dropped leaves the
///   entry recognizing a shape nothing reaches;
/// - a reserved key names none — a registration there would mean the shape has a success reading
///   after all, and its diagnosis would be describing a form that works;
/// - a live bucket with a raw-capture slot has an entry declaring exactly those slots and kinds,
///   so a builtin that grows, loses or re-indexes one fails here;
/// - a masked type slot is a slot the bucket's live overloads really read as a raw type expression
///   — some overload takes `:(…)` there and **none** types it `:KExpression`. The second half is
///   what matters: flipping a code slot's `(…)` to `SigiledTypeExpr` would silently retype a body.
///   One-directional on purpose — the mask is opt-in, not derived, so a slot may satisfy the
///   predicate and stay unmasked (`NEWTYPE <name> = <repr>` does).
#[test]
fn the_form_table_matches_the_live_registrations() {
    let program = program_storage();
    let storage = run_root_storage();
    let run = TestRun::silent(&program, &storage);
    let live = live_registrations(&run);
    let types = run.types();

    let matching = |form: &'static Form| {
        live.iter()
            .filter(move |bucket| key_matches(form.key, bucket.key.iter().copied()))
    };

    for form in FORMS {
        let registered = matching(form).count();
        if form.reserved {
            assert_eq!(
                registered,
                0,
                "reserved form key {:?} has a registered bucket",
                render_key(form.key)
            );
        } else {
            assert!(
                registered > 0,
                "form key {:?} has no registered bucket",
                render_key(form.key)
            );
        }
    }

    for bucket in &live {
        let expected = declared_lazy_slots(bucket, types);
        if expected.is_empty() {
            continue;
        }
        let form = crate::parse::forms::form_for(bucket.key.iter().copied()).unwrap_or_else(|| {
            panic!("live bucket with lazy slots {expected:?} has no FORMS entry")
        });
        let declared: BTreeMap<usize, LazyKinds> = form
            .lazy_slots
            .iter()
            .map(|(index, kinds)| (*index, *kinds))
            .collect();
        assert_eq!(
            declared,
            expected,
            "form key {:?} declares the wrong lazy slots",
            render_key(form.key)
        );
    }

    let admits_sigiled = |ktype: &KType| {
        ktype.union_has_member(KType::SIGILED_TYPE_EXPR, types)
            || matches!(types.node(*ktype), TypeNode::OfKind(_))
    };
    for (form, binder) in binder_forms() {
        for &index in binder.type_slots {
            let slot_types: Vec<KType> = matching(form)
                .filter_map(|bucket| bucket.slot_types.get(&index))
                .flatten()
                .copied()
                .collect();
            assert!(
                !slot_types.is_empty(),
                "form key {:?} masks slot {index}, which no live registration types",
                render_key(form.key)
            );
            assert!(
                slot_types.iter().any(admits_sigiled),
                "form key {:?} masks slot {index}, which no registration admits a `:(…)` at",
                render_key(form.key)
            );
            assert!(
                !slot_types
                    .iter()
                    .any(|ktype| ktype.union_has_member(KType::KEXPRESSION, types)),
                "form key {:?} masks slot {index}, which some registration reads as code",
                render_key(form.key)
            );
        }
    }
}
