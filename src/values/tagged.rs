//! The one nominal wrap: a payload tagged with a type identity — a newtype construction, a union
//! variant, an abstract-type re-tag, a lowered error.

use std::cell::Cell;
use std::marker::PhantomData;

use crate::memory::{BumpAllocator, Writer, resident};
use crate::type_lattice::KType;

use super::settled::{composite_bits, restamp_bits};
use super::{
    Chain, ConstructionRefused, Knotted, Link, MemoBits, Nothing, SealRefused, TypeValue, Value,
    Weight, construction, sealing,
};

/// A tagged value. The identity is its type, so no tag symbol rides beside the payload. Its payload
/// is a value word, or a [`Link`] when the tagged value is a knot's data node.
#[derive(Debug)]
pub struct Tagged<'cell, X = Nothing, C = Value<'cell, X>> {
    payload: C,
    ktype: KType,
    weight: Cell<Weight>,
    /// The knot member a cell's word may hold, which a link cell names only through `C`.
    member: PhantomData<Value<'cell, X>>,
}

impl<'cell, X: Knotted> Tagged<'cell, X> {
    /// A construction `(head payload)`: [`construction`] checks the payload against what the head
    /// names — a newtype or a family — and the payload is held under the identity it answers.
    pub fn construct(
        writer: Writer<'cell, '_>,
        head: &TypeValue,
        payload: Value<'cell, X>,
        types: &Chain<'_, 'cell, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Result<&'cell Tagged<'cell, X>, ConstructionRefused> {
        let identity = construction(types, scratch, head.handle(), payload.concrete_ktype())?;
        Ok(Self::hold(writer, payload, identity, types))
    }

    /// The raw wrap — a union variant, a lowered error, a retype: the payload rides verbatim,
    /// nested tagged layers included, so a newtype over another keeps every layer.
    pub fn hold(
        writer: Writer<'cell, '_>,
        payload: Value<'cell, X>,
        identity: KType,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> &'cell Tagged<'cell, X> {
        let weight = Weight::flat::<Self>().plus(payload.referent_weight());
        let bits = composite_bits(types, identity, [&payload]);
        Self::from_payload(writer, payload, identity, weight.with_bits(bits))
    }

    /// A member sealed behind an opaque view's barrier: [`sealing`] checks the identity is a
    /// carrier and the payload satisfies what the source binds the member to, and the payload takes
    /// the carrier as its one tagged layer.
    pub fn seal(
        writer: Writer<'cell, '_>,
        value: Value<'cell, X>,
        carrier: KType,
        witness: KType,
        types: &Chain<'_, 'cell, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Result<&'cell Tagged<'cell, X>, SealRefused> {
        let identity = sealing(types, scratch, carrier, witness, value.ktype())?;
        Ok(Self::peel(writer, value, identity, types))
    }

    /// A re-tag: a tagged value's own layer is replaced rather than wrapped, so the payload is never
    /// itself tagged.
    pub fn peel(
        writer: Writer<'cell, '_>,
        value: Value<'cell, X>,
        identity: KType,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> &'cell Tagged<'cell, X> {
        match value {
            Value::Tagged(tagged) => tagged.with_type(writer, identity, types),
            other => Self::hold(writer, other, identity, types),
        }
    }
}

impl<'cell, X: Knotted> Tagged<'cell, X, Link<'cell, X>> {
    /// A knot's tagged data node over `payload`, under the identity the tie checked.
    pub fn linked(
        writer: Writer<'cell, '_>,
        payload: Link<'cell, X>,
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

impl<'cell, X: Copy, C: Copy> Tagged<'cell, X, C> {
    /// A tagged value over a payload already resident in `writer`'s region, under an identity and
    /// weight the caller already knows — the deep copy's arm.
    pub(crate) fn from_payload(
        writer: Writer<'cell, '_>,
        payload: C,
        ktype: KType,
        weight: Weight,
    ) -> &'cell Self {
        resident(
            writer,
            Tagged {
                payload,
                ktype,
                weight: Cell::new(weight),
                member: PhantomData,
            },
        )
    }

    /// The same payload under another identity, its bits recomputed from the new memo.
    pub fn with_type(
        &self,
        writer: Writer<'cell, '_>,
        ktype: KType,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> &'cell Self {
        let weight = self.weight.get();
        let weight = weight.with_bits(restamp_bits(weight.bits(), types, ktype));
        Self::from_payload(writer, self.payload, ktype, weight)
    }

    pub fn payload(&self) -> &C {
        &self.payload
    }

    pub fn ktype(&self) -> KType {
        self.ktype
    }

    pub fn weight(&self) -> Weight {
        self.weight.get()
    }

    /// Where the value's type memos resolve, as its weight records it.
    pub fn bits(&self) -> MemoBits {
        self.weight.get().bits()
    }

    /// Record that every memo the value reaches resolves in its home half's branch — an exit's
    /// mark on what it adopted. Keeps root-closed.
    pub(crate) fn settle(&self) {
        let weight = self.weight.get();
        self.weight.set(weight.with_bits(MemoBits {
            settled: true,
            ..weight.bits()
        }));
    }
}
