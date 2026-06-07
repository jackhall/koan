//! `Carried` — the scheduler's value currency: what a node produces and the node store
//! holds. A produced result is either a runtime [`KObject`] (the `Object` arm) or a type
//! flowing raw in the type channel (the `Type` arm), so a type-operator returns a `&KType`
//! without boxing it into a `KObject`.
//!
//! See the roadmap item `type_language/types-in-value-channel.md`.

use crate::machine::model::types::{KType, Parseable};

use super::KObject;

/// Two-arm value currency. `Copy` like the `&'a` references it wraps, so it threads through
/// `BodyResult::Value`, `NodeOutput::Value`, and the lift path without clones.
#[derive(Clone, Copy)]
pub enum Carried<'a> {
    Object(&'a KObject<'a>),
    Type(&'a KType<'a>),
}

impl<'a> Carried<'a> {
    /// The `Object` arm, if this is one.
    pub fn as_object(self) -> Option<&'a KObject<'a>> {
        match self {
            Carried::Object(o) => Some(o),
            Carried::Type(_) => None,
        }
    }

    /// The `Type` arm, if this is one.
    pub fn as_type(self) -> Option<&'a KType<'a>> {
        match self {
            Carried::Type(t) => Some(t),
            Carried::Object(_) => None,
        }
    }

    /// The `Object` arm, panicking on a `Type` arm. For value-consumers — a site that by
    /// construction only ever handles a runtime object (not a type flowing in the type
    /// channel).
    pub fn object(self) -> &'a KObject<'a> {
        match self {
            Carried::Object(o) => o,
            Carried::Type(t) => {
                panic!("expected an Object value, got a Type arm: {}", t.name())
            }
        }
    }

    /// Surface rendering of either arm — an object's `summarize` or a type's `name`.
    pub fn summarize(&self) -> String {
        match self {
            Carried::Object(o) => Parseable::summarize(*o),
            Carried::Type(t) => t.name(),
        }
    }

    /// The shallow type tag of the carried value: an object's `ktype()`, or a type arm's
    /// own `OfKind` classification.
    pub fn ktype(&self) -> KType<'a> {
        match self {
            Carried::Object(o) => o.ktype(),
            Carried::Type(t) => KType::OfKind(t.kind_of()),
        }
    }
}
