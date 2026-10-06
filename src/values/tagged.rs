//! The one nominal wrap: a payload tagged with a type identity — a newtype construction, a union
//! variant, an abstract-type re-tag, a lowered error.

use std::marker::PhantomData;

use crate::memory::{BumpAllocator, Writer, resident};
use crate::type_lattice::{KType, TypeRegistry};

use super::digest::{ContentDigest, Tag, composite};
use super::{
    ConstructionRefused, Knotted, Link, Nothing, SealRefused, TypeValue, Value, Weight,
    construction, sealing,
};

/// A tagged value. The identity is its type, so no tag symbol rides beside the payload. Its payload
/// is a value word, or a [`Link`] when the tagged value is a knot's data node.
#[derive(Clone, Copy, Debug)]
pub struct Tagged<'cell, X = Nothing, C = Value<'cell, X>> {
    payload: C,
    ktype: KType,
    weight: Weight,
    /// Its payload's digest ([`NONE`](ContentDigest::NONE) for a knot's data node).
    contents: ContentDigest,
    /// The knot member a cell's word may hold, which a link cell names only through `C`.
    member: PhantomData<Value<'cell, X>>,
}

impl<'cell, X: Knotted> Tagged<'cell, X> {
    /// A construction `(head payload)`: [`construction`] checks the payload against what the head
    /// names — a newtype or a family — and the payload is held under the identity it answers.
    pub fn construct(
        writer: Writer<'cell>,
        head: &TypeValue,
        payload: Value<'cell, X>,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> Result<&'cell Tagged<'cell, X>, ConstructionRefused> {
        let identity = construction(types, scratch, head.handle(), payload.concrete_ktype())?;
        Ok(Self::hold(writer, payload, identity))
    }

    /// The raw wrap — a union variant, a lowered error, a retype: the payload rides verbatim,
    /// nested tagged layers included, so a newtype over another keeps every layer.
    pub fn hold(
        writer: Writer<'cell>,
        payload: Value<'cell, X>,
        identity: KType,
    ) -> &'cell Tagged<'cell, X> {
        let weight = Weight::flat::<Self>().plus(payload.referent_weight());
        Self::from_payload(writer, payload, identity, weight, payload.digest())
    }

    /// The tagged value's content digest: its identity and its payload's.
    pub fn digest(&self) -> ContentDigest {
        composite(Tag::Tagged, self.ktype, self.contents)
    }

    /// A member sealed behind an opaque view's barrier: [`sealing`] checks the identity is a mint
    /// of that view and the payload satisfies what the source binds the member to, and the payload
    /// takes the mint as its one tagged layer.
    pub fn seal(
        writer: Writer<'cell>,
        value: Value<'cell, X>,
        mint: KType,
        witness: KType,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> Result<&'cell Tagged<'cell, X>, SealRefused> {
        let identity = sealing(types, scratch, mint, witness, value.ktype())?;
        Ok(Self::peel(writer, value, identity))
    }

    /// A re-tag: a tagged value's own layer is replaced rather than wrapped, so the payload is never
    /// itself tagged.
    pub fn peel(
        writer: Writer<'cell>,
        value: Value<'cell, X>,
        identity: KType,
    ) -> &'cell Tagged<'cell, X> {
        match value {
            Value::Tagged(tagged) => tagged.with_type(writer, identity),
            other => Self::hold(writer, other, identity),
        }
    }
}

impl<'cell, X: Knotted> Tagged<'cell, X, Link<'cell, X>> {
    /// A knot's tagged data node over `payload`, under the identity the tie checked.
    pub fn linked(writer: Writer<'cell>, payload: Link<'cell, X>, identity: KType) -> &'cell Self {
        Self::from_payload(
            writer,
            payload,
            identity,
            Weight::flat::<Self>().plus(payload.referent_weight()),
            ContentDigest::NONE,
        )
    }
}

impl<'cell, X: Copy, C: Copy> Tagged<'cell, X, C> {
    /// A tagged value over a payload already resident in `writer`'s region, under an identity and
    /// weight the caller already knows — the deep copy's arm.
    pub(crate) fn from_payload(
        writer: Writer<'cell>,
        payload: C,
        ktype: KType,
        weight: Weight,
        contents: ContentDigest,
    ) -> &'cell Self {
        resident(
            writer,
            Tagged {
                payload,
                ktype,
                weight,
                contents,
                member: PhantomData,
            },
        )
    }

    /// The same payload under another identity.
    pub fn with_type(&self, writer: Writer<'cell>, ktype: KType) -> &'cell Self {
        resident(writer, Tagged { ktype, ..*self })
    }

    /// Its payload's digest, as laid down.
    pub(crate) fn contents(&self) -> ContentDigest {
        self.contents
    }

    pub fn payload(&self) -> &C {
        &self.payload
    }

    pub fn ktype(&self) -> KType {
        self.ktype
    }

    pub fn weight(&self) -> Weight {
        self.weight
    }
}
