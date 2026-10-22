//! A specialization's key: which body, under which binding.
//!
//! A [`SpecKey`] names a callee's body by its [`Segment`], so two bodies of one text whose captures
//! differ in type never share one, and its binding by the content digests of the arguments at the
//! names its key list holds, in slot order. The keys fold ([`folds`](super::folds)) lays that list
//! ([keys](README.md#keys-which-bindings-make-a-specialization)). The asker computes the digests;
//! the drain looks the key up in the parent's table.

use crate::scope::Segment;
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
