//! A link: one slot of a run held inside a knot node — a data node's cell, a tagged node's payload,
//! a function's closure binding. It is a value word, or an [`Edge`] naming a sibling node of the
//! knot the holder sits in: a sibling has no address until the knot is tied, so a reference to one
//! is an index. A link is read only through the member holding it, which resolves an edge.

use crate::memory::Edge;

use super::digest::{ContentDigest, DigestHasher, Tag};
use super::{Knotted, Nothing, Value, Weight};

/// A value word, or an edge into the holder's own knot.
#[derive(Clone, Copy, Debug)]
pub enum Link<'cell, X = Nothing> {
    Value(Value<'cell, X>),
    Edge(Edge),
}

const _: () = assert!(size_of::<Link<'static>>() == 24);

impl<'cell, X: Knotted> Link<'cell, X> {
    /// The value this link denotes, read through `holder`, the member whose run holds it: an edge
    /// is the sibling it names.
    pub fn resolve(&self, holder: X) -> Value<'cell, X> {
        match *self {
            Link::Value(value) => value,
            Link::Edge(edge) => Value::Knotted(holder.sibling(edge)),
        }
    }

    /// This link rebuilt at another region lifetime: a value through `copy`, an edge verbatim — an
    /// edge names a node by index, so it means the same in a copy of its knot.
    pub fn copied<'to, Y>(
        &self,
        copy: &mut impl FnMut(&Value<'cell, X>) -> Value<'to, Y>,
    ) -> Link<'to, Y> {
        match self {
            Link::Value(value) => Link::Value(copy(value)),
            Link::Edge(edge) => Link::Edge(*edge),
        }
    }

    /// What a holder laying this link down inline writes: the link itself and what a value points
    /// at. An edge points at nothing.
    pub fn weight(&self) -> Weight {
        let referent = match self {
            Link::Value(value) => value.referent_weight(),
            Link::Edge(_) => Weight::ZERO,
        };
        Weight::flat::<Self>().plus(referent)
    }

    /// The link's digest inside its knot's: a value's own, or an edge's index, since the edge names
    /// a sibling the knot's digest covers.
    pub fn digest(&self) -> ContentDigest {
        match self {
            Link::Value(value) => value.digest(),
            Link::Edge(edge) => DigestHasher::new(Tag::Edge)
                .count(edge.index() as usize)
                .finished(),
        }
    }

    /// The part of [`weight`](Self::weight) past the link word.
    pub fn referent_weight(&self) -> Weight {
        match self {
            Link::Value(value) => value.referent_weight(),
            Link::Edge(_) => Weight::ZERO,
        }
    }
}
