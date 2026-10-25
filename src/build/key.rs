//! A settlement's key: which body, under which binding.
//!
//! A [`SettlementKey`] names a callee's body by the base whose shape table holds it — the load's
//! root, or a composition's — and its [`Segment`] there, so two bodies of one text whose captures
//! differ in type never share one, nor do two bases' bodies at one segment; and its binding by the content digests of the arguments at the
//! names its key list holds, in slot order. The keys fold ([`folds`](super::folds)) lays that list
//! ([keys](README.md#keys-which-bindings-make-a-specialization)). The asker computes the digests;
//! the drain looks the key up in the parent's table.

use crate::scope::Segment;
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{ContentDigest, SettlementKey};

/// The key of the settlement of `segment`, of the base whose digest is `base`, under the argument
/// `digests`, one per name its key list holds, in slot order.
pub fn key_of(
    base: ContentDigest,
    segment: Segment,
    digests: impl Iterator<Item = ContentDigest>,
) -> SettlementKey {
    let mut hasher = DigestHasher::new(Tag::Settlement);
    hasher.digest(base);
    hasher.count(segment.index() as usize);
    for digest in digests {
        hasher.digest(digest);
    }
    SettlementKey(hasher.finished())
}
