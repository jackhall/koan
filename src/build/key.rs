//! A specialization's key: which body, under which binding.
//!
//! A [`SpecKey`] names a callee's body by its [`Segment`], so two bodies of one text whose captures
//! differ in type never share one, and its binding by the content digests of the arguments at the
//! parameters its invariant sites read, in slot order. The asker computes the digests; the drain
//! looks the key up in the parent's table.

use crate::scope::Segment;
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{ContentDigest, SpecKey};

/// The key of `segment`'s specialization under the argument `digests`, one per parameter its
/// invariant sites read, in slot order.
pub fn key_of(segment: Segment, digests: impl Iterator<Item = ContentDigest>) -> SpecKey {
    let mut hasher = DigestHasher::new(Tag::Specialization);
    hasher.count(segment.index() as usize);
    for digest in digests {
        hasher.digest(digest);
    }
    SpecKey(hasher.finished())
}
