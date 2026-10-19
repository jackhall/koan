//! A specialization's key: which body, under which binding.
//!
//! A [`SpecKey`] names a callee's body by its [`Segment`], so two bodies of one text whose captures
//! differ in type never share one, and its binding by the content digests of the arguments at the
//! names its key list holds, in slot order. [`lay_keys`] lays that list on every callable: the
//! parameters its invariant sites read, and each of its own entry-bound names that a fact whose
//! seam is the entry of it, or of a callable nested in it, waits on, where its caching record lets
//! that name key ([keys](README.md#keys-which-bindings-make-a-specialization)). A parameter's own
//! type fact keys nothing: whatever reads the parameter waits on its names too. A fact's seam is
//! found as [`list_seams`](super::list_seams) finds it. The asker computes the digests; the drain
//! looks the key up in the parent's table.

use crate::memory::{BumpAllocator, BumpVec, Writer, bump_table, collect};
use crate::scope::{Address, BodyShape, FactKey, Position, SeamAt, Segment, ShapeKind, Slot};

use super::seams::{Sites, seam_of};
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

/// Lay every callable body's specialization keys under `root`: the parameters its invariant sites
/// read, and each of its own entry-bound names a fact seamed at the entry of it, or of a callable
/// nested in it, waits on, where its record lets that name key. A fact seamed at a unit or at a
/// quote's code entry keys nothing, nor does a callable parameter's own type fact.
pub fn lay_keys<'graph>(
    root: &'graph BodyShape<'graph>,
    writer: Writer<'graph, '_>,
    scratch: BumpAllocator<'_>,
) {
    let mut chain = BumpVec::new_in(scratch);
    let mut named = BumpVec::new_in(scratch);
    let mut sites = bump_table(scratch);
    collect_names(root, &mut chain, &mut named, &mut sites);
    named.sort_unstable();
    named.dedup();
    fix(root, &named, writer, scratch);
}

/// Gather, under `shape` with `chain` the bodies enclosing it, each `(segment, slot)` that a fact
/// seamed at a callable's entry waits on and that the declaring callable's record lets key.
fn collect_names<'graph>(
    shape: &'graph BodyShape<'graph>,
    chain: &mut BumpVec<'_, &'graph BodyShape<'graph>>,
    named: &mut BumpVec<'_, (Segment, Slot)>,
    sites: &mut Sites<'_, 'graph>,
) {
    chain.push(shape);
    let (channel, statics) = (shape.channel_residual(), shape.statics_residual());
    for (key, frontier) in channel.entries().iter().chain(statics.entries()) {
        // A parameter's type waits on the names its declared type reads; a fact reading the
        // parameter waits on them too, so the type alone keys nothing.
        if let FactKey::Slot(slot) = key
            && shape.kind() == ShapeKind::Callable
            && shape.position_of(*slot) == Position::PARAMETER
        {
            continue;
        }
        let Some((body, SeamAt::Entry)) = seam_of(frontier.unknowns(), chain, sites) else {
            continue;
        };
        if body.kind() != ShapeKind::Callable {
            continue;
        }
        for unknown in frontier.unknowns() {
            let Address::Slot(slot) = unknown.at else {
                continue;
            };
            // Only an unknown a body enclosing the fact declares: one off the chain is bound in a
            // frame of its own.
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
        collect_names(nested, chain, named, sites);
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
