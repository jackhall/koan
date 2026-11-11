//! A settlement's key: which body, under which binding.
//!
//! A [`LayerKey`] names a callee's body by the base whose shape table holds it — the load's root,
//! or a composition's — and its [`Segment`] there, so two bodies of one text whose captures differ
//! in type never share one, nor do two bases' bodies at one segment; and its binding by what the
//! callee's frame binds at each name its key list holds, in slot order ([`key_of_ask`]): a type as
//! its type value, a module as its member, retyped where the frame binds it at another type. So
//! the key is a function of the [`Ask`] alone, and a request laid at load and the values a call
//! reads reach one key. The keys fold ([`folds`](super::folds)) lays that list
//! ([keys](README.md#keys-which-bindings-make-a-settlement)); the drain looks the key up in the
//! parent's table.

use crate::scope::{Ask, Segment};
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{Chain, ContentDigest, LayerKey};

/// The key of the settlement of `segment`, of the base whose digest is `base`, under the argument
/// `digests`, one per name its key list holds, in slot order.
pub fn key_of(
    base: ContentDigest,
    segment: Segment,
    digests: impl Iterator<Item = ContentDigest>,
) -> LayerKey {
    let mut hasher = DigestHasher::new(Tag::Settlement);
    hasher.digest(base);
    hasher.count(segment.index() as usize);
    for digest in digests {
        hasher.digest(digest);
    }
    LayerKey(hasher.finished())
}

/// The key of `ask`, its body of the base whose digest is `base`: what the callee's frame binds at
/// each keyed name, so a function of the ask alone.
pub fn key_of_ask(base: ContentDigest, ask: &Ask, types: &Chain<'_, '_, '_, '_>) -> LayerKey {
    key_of(
        base,
        ask.body,
        ask.bound.iter().map(|asked| asked.digest(types)),
    )
}
