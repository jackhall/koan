//! The reach-tightness slate: what [`FoldAudit`] flags at the fold chokepoint and, just as
//! importantly, what it does not. Each fold is driven through the real
//! [`StepAllocator::alloc_carried_with`] door, so what is measured is the production instrumentation
//! rather than a stand-in.
//!
//! Operand values are **strings**, not numbers: a string's bytes are region-hosted, so the operand's
//! address survives into whatever the product embeds, which is exactly the contribution signal the
//! audit reads. Each test resets the thread-local log first; test threads see it in isolation.

use std::rc::Rc;

use super::*;
use crate::builtins::test_support::per_call_storage;
use crate::machine::core::{FrameStorageExt, StepAllocator};
use crate::machine::model::{CarriedFamily, Scalar};
use crate::machine::run_root_storage;

/// The fold door under audit, named once so a flag's `site` assertion cannot drift from it.
const SITE: &str = "StepAllocator::alloc_carried_with";

/// A per-call producer frame holding one region-hosted string, delivered as a fold operand. Per-call
/// rather than run-root: an eternal owner is never flagged, so a frame at the eternal tier could not
/// witness an over-fold at all.
fn producer(text: &str) -> (Rc<FrameStorage>, DeliveredCarried) {
    let storage = per_call_storage();
    let object = storage.brand().alloc_string(text);
    let delivered = storage
        .brand()
        .deliver_resident::<CarriedFamily>(Carried::Object(object));
    (storage, delivered)
}

/// The acceptance case: a fold declaring two operands whose product embeds only the first pins the
/// second's region for the product's whole life, and nothing else in the engine notices.
#[test]
fn over_fold_is_flagged() {
    reset_tightness_flags();
    let (_used, dep_used) = producer("embedded");
    let (unused, dep_unused) = producer("ignored");
    let consumer = per_call_storage();

    let allocator = StepAllocator::over_frame(Rc::clone(&consumer));
    let _product =
        allocator.alloc_carried_with(&[&dep_used, &dep_unused], |_brand, views| views[0]);

    let flags = tightness_flags();
    assert_eq!(
        flags.len(),
        1,
        "one unjustified member: the unused operand's home"
    );
    assert_eq!(flags[0].site, SITE);
    assert_eq!(flags[0].member, Rc::as_ptr(&unused) as usize);
    assert_eq!(flags[0].non_contributing, vec![1]);
}

/// The tight fold: one operand, whose value the product embeds verbatim. Every member of the
/// product's description is justified by that operand's coverage, so nothing is flagged.
#[test]
fn tight_fold_is_clean() {
    reset_tightness_flags();
    let (_producer, dep) = producer("embedded");
    let consumer = per_call_storage();

    let allocator = StepAllocator::over_frame(Rc::clone(&consumer));
    let _product = allocator.alloc_carried_with(&[&dep], |_brand, views| views[0]);

    assert!(tightness_flags().is_empty());
}

/// Justification is per **member**, not per dep: two operands homed in the same producer frame fold
/// one member between them, so embedding either one justifies it. The second operand is recorded as
/// non-contributing but raises no flag, because the region it would have over-pinned is pinned on
/// the first operand's authority anyway.
#[test]
fn co_homed_non_contributor_is_justified() {
    reset_tightness_flags();
    let storage = per_call_storage();
    let embedded = storage.brand().alloc_string("embedded");
    let ignored = storage.brand().alloc_string("ignored");
    let dep_used = storage
        .brand()
        .deliver_resident::<CarriedFamily>(Carried::Object(embedded));
    let dep_unused = storage
        .brand()
        .deliver_resident::<CarriedFamily>(Carried::Object(ignored));
    let consumer = per_call_storage();

    let allocator = StepAllocator::over_frame(Rc::clone(&consumer));
    let _product =
        allocator.alloc_carried_with(&[&dep_used, &dep_unused], |_brand, views| views[0]);

    assert!(tightness_flags().is_empty());
}

/// No false positive on the scalar-door pattern: a fold whose product is a rebuilt owned scalar
/// embeds nothing of its operand at all, but an operand homed in eternal storage introduces no
/// member a pin could over-hold — the retention's eternal rule would drop it regardless.
#[test]
fn scalar_rebuild_is_clean() {
    reset_tightness_flags();
    let eternal = run_root_storage();
    let object = eternal.brand().alloc_string("source");
    let dep = eternal
        .brand()
        .deliver_resident::<CarriedFamily>(Carried::Object(object));
    let consumer = per_call_storage();

    let allocator = StepAllocator::over_frame(Rc::clone(&consumer));
    let _product = allocator.alloc_carried_with(&[&dep], |brand, _views| {
        Carried::Object(brand.alloc_scalar(Scalar::Number(1.0)))
    });

    assert!(tightness_flags().is_empty());
}
