//! A specialization's key: which body, under which binding.
//!
//! A [`SpecKey`] names a callee's body by its [`Segment`], so two bodies of one text whose captures
//! differ in type never share one, and its binding by the content digests of the arguments at the
//! names its key list holds, in slot order. [`lay_keys`] lays that list on every callable once the
//! seams are listed: the parameters its invariant sites read, and each of its own entry-bound names
//! an entry seam of it, or of a callable nested in it, waits on, where its caching record lets that
//! name key ([keys](README.md#keys-which-bindings-make-a-specialization)). The asker computes the
//! digests; the drain looks the key up in the parent's table.

use crate::memory::{BumpAllocator, BumpVec, Writer, collect};
use crate::scope::{Address, BodyShape, SeamAt, Segment, ShapeKind, Slot};
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{ContentDigest, SpecKey};

/// The key of `segment`'s specialization under the argument `digests`, one per name its key list
/// holds, in slot order.
pub fn key_of(segment: Segment, digests: impl Iterator<Item = ContentDigest>) -> SpecKey {
    let mut hasher = DigestHasher::new(Tag::Specialization);
    hasher.count(segment.index() as usize);
    for digest in digests {
        hasher.digest(digest);
    }
    SpecKey(hasher.finished())
}

/// Lay every callable body's specialization keys under `root`, once its seams are listed: the
/// parameters its invariant sites read, and each of its own entry-bound names an entry seam of it,
/// or of a callable nested in it, waits on, where its record lets that name key. A seam at a unit
/// or at a quote's code entry keys nothing.
pub fn lay_keys<'graph>(
    root: &'graph BodyShape<'graph>,
    writer: Writer<'graph, '_>,
    scratch: BumpAllocator<'_>,
) {
    let mut chain = BumpVec::new_in(scratch);
    let mut named = BumpVec::new_in(scratch);
    collect_names(root, &mut chain, &mut named);
    named.sort_unstable();
    named.dedup();
    fix(root, &named, writer, scratch);
}

/// Gather, under `shape` with `chain` the bodies enclosing it, each `(segment, slot)` a callable's
/// entry seam waits on that the declaring callable's record lets key.
fn collect_names<'graph>(
    shape: &'graph BodyShape<'graph>,
    chain: &mut BumpVec<'_, &'graph BodyShape<'graph>>,
    named: &mut BumpVec<'_, (Segment, Slot)>,
) {
    chain.push(shape);
    let entry = (shape.seams().iter()).filter(|seam| seam.at == SeamAt::Entry);
    if shape.kind() == ShapeKind::Callable {
        for unknown in entry.flat_map(|seam| seam.waits_on.unknowns()) {
            let Address::Slot(slot) = unknown.at else {
                continue;
            };
            let declaring = chain.iter().find(|body| body.segment() == unknown.segment);
            let keys = declaring.is_some_and(|body| {
                body.kind() == ShapeKind::Callable
                    && (body.caching())
                        .is_some_and(|caching| caching.keyable.binary_search(&slot).is_ok())
            });
            if keys {
                named.push((unknown.segment, slot));
            }
        }
    }
    for (_, nested) in shape.nested_shapes() {
        collect_names(nested, chain, named);
    }
    chain.pop();
}

/// Fix the key list of `shape` and every body nested in it from `named`, sorted.
fn fix<'graph>(
    shape: &'graph BodyShape<'graph>,
    named: &[(Segment, Slot)],
    writer: Writer<'graph, '_>,
    scratch: BumpAllocator<'_>,
) {
    if shape.kind() == ShapeKind::Callable {
        let segment = shape.segment();
        let from = named.partition_point(|(each, _)| *each < segment);
        let to = named.partition_point(|(each, _)| *each <= segment);
        let invariant = shape.caching().and_then(|caching| caching.invariant);
        if invariant.is_some() || from < to {
            let mut keys = BumpVec::new_in(scratch);
            keys.extend(named[from..to].iter().map(|(_, slot)| *slot));
            keys.extend_from_slice(invariant.unwrap_or(&[]));
            keys.sort_unstable();
            keys.dedup();
            shape.fix_specialization_keys(Some(collect(writer, keys.iter().copied())));
        }
    }
    for (_, nested) in shape.nested_shapes() {
        fix(nested, named, writer, scratch);
    }
}
