//! The one nominal wrap: a payload tagged with a type identity — a newtype construction, a union
//! variant, an abstract-type re-tag, a lowered error.

use crate::memory::Writer;
use crate::type_lattice::KType;

use super::{Value, Weight, resident};

/// A tagged value. The identity is its type, so no tag symbol rides beside the payload.
#[derive(Clone, Copy, Debug)]
pub struct Tagged<'graph, 'cell> {
    payload: Value<'graph, 'cell>,
    ktype: KType,
    weight: Weight,
}

impl<'graph, 'cell> Tagged<'graph, 'cell> {
    /// A construction: the payload rides verbatim, nested tagged layers included, so a newtype over
    /// another keeps every layer.
    pub fn hold(
        writer: Writer<'cell>,
        payload: Value<'graph, 'cell>,
        identity: KType,
    ) -> &'cell Tagged<'graph, 'cell> {
        let weight = Weight::flat::<Self>().plus(payload.referent_weight());
        resident(
            writer,
            Tagged {
                payload,
                ktype: identity,
                weight,
            },
        )
    }

    /// A re-tag: a tagged value's own layer is replaced rather than wrapped, so the payload is never
    /// itself tagged.
    pub fn peel(
        writer: Writer<'cell>,
        value: Value<'graph, 'cell>,
        identity: KType,
    ) -> &'cell Tagged<'graph, 'cell> {
        match value {
            Value::Tagged(tagged) => tagged.with_type(writer, identity),
            other => Self::hold(writer, other, identity),
        }
    }

    /// The same payload under another identity.
    pub fn with_type(&self, writer: Writer<'cell>, ktype: KType) -> &'cell Tagged<'graph, 'cell> {
        resident(writer, Tagged { ktype, ..*self })
    }

    pub fn payload(&self) -> &Value<'graph, 'cell> {
        &self.payload
    }

    pub fn ktype(&self) -> KType {
        self.ktype
    }

    pub fn weight(&self) -> Weight {
        self.weight
    }
}
