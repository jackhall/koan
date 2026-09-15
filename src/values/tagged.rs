//! The one nominal wrap: a payload tagged with a type identity — a newtype construction, a union
//! variant, an abstract-type re-tag, a lowered error.

use std::marker::PhantomData;

use crate::memory::{BumpAllocator, Writer, resident};
use crate::type_lattice::{KType, TypeRegistry};

use super::{ConstructionRefused, Knotted, Link, Nothing, TypeValue, Value, Weight, construction};

/// A tagged value. The identity is its type, so no tag symbol rides beside the payload. Its payload
/// is a value word, or a [`Link`] when the tagged value is a knot's data node.
#[derive(Clone, Copy, Debug)]
pub struct Tagged<'graph, 'cell, X = Nothing, C = Value<'graph, 'cell, X>> {
    payload: C,
    ktype: KType,
    weight: Weight,
    /// The knot member a cell's word may hold, which a link cell names only through `C`.
    member: PhantomData<Value<'graph, 'cell, X>>,
}

impl<'graph, 'cell, X: Knotted> Tagged<'graph, 'cell, X> {
    /// The newtype construction `(head payload)`: [`construction`] checks the head names a newtype
    /// whose representation the payload's type satisfies, and the payload is held under it.
    pub fn construct(
        writer: Writer<'cell>,
        head: &TypeValue,
        payload: Value<'graph, 'cell, X>,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> Result<&'cell Tagged<'graph, 'cell, X>, ConstructionRefused> {
        let identity = construction(types, scratch, head.handle(), payload.ktype())?;
        Ok(Self::hold(writer, payload, identity))
    }

    /// The raw wrap — a union variant, a lowered error, a retype: the payload rides verbatim,
    /// nested tagged layers included, so a newtype over another keeps every layer.
    pub fn hold(
        writer: Writer<'cell>,
        payload: Value<'graph, 'cell, X>,
        identity: KType,
    ) -> &'cell Tagged<'graph, 'cell, X> {
        let weight = Weight::flat::<Self>().plus(payload.referent_weight());
        Self::from_payload(writer, payload, identity, weight)
    }

    /// A re-tag: a tagged value's own layer is replaced rather than wrapped, so the payload is never
    /// itself tagged.
    pub fn peel(
        writer: Writer<'cell>,
        value: Value<'graph, 'cell, X>,
        identity: KType,
    ) -> &'cell Tagged<'graph, 'cell, X> {
        match value {
            Value::Tagged(tagged) => tagged.with_type(writer, identity),
            other => Self::hold(writer, other, identity),
        }
    }
}

impl<'graph, 'cell, X: Knotted> Tagged<'graph, 'cell, X, Link<'graph, 'cell, X>> {
    /// A knot's tagged data node over `payload`, under the identity the tie checked.
    pub fn linked(
        writer: Writer<'cell>,
        payload: Link<'graph, 'cell, X>,
        identity: KType,
    ) -> &'cell Self {
        Self::from_payload(
            writer,
            payload,
            identity,
            Weight::flat::<Self>().plus(payload.referent_weight()),
        )
    }
}

impl<'graph, 'cell, X: Copy, C: Copy> Tagged<'graph, 'cell, X, C> {
    /// A tagged value over a payload already resident in `writer`'s region, under an identity and
    /// weight the caller already knows — the deep copy's arm.
    pub(crate) fn from_payload(
        writer: Writer<'cell>,
        payload: C,
        ktype: KType,
        weight: Weight,
    ) -> &'cell Self {
        resident(
            writer,
            Tagged {
                payload,
                ktype,
                weight,
                member: PhantomData,
            },
        )
    }

    /// The same payload under another identity.
    pub fn with_type(&self, writer: Writer<'cell>, ktype: KType) -> &'cell Self {
        resident(writer, Tagged { ktype, ..*self })
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
