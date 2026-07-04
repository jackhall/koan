//! `Carried` — the scheduler's value currency: what a node produces and the node store
//! holds. A produced result is either a runtime [`KObject`] (the `Object` arm) or a type
//! flowing raw in the type channel (the `Type` arm), so a type-operator returns a `&KType`
//! without boxing it into a `KObject`.
//!
//! See [execution/calls-and-values.md § `KObject` and the model/core boundary](../../../../design/execution/calls-and-values.md#kobject-and-the-modelcore-boundary).

use crate::machine::model::types::{KType, Parseable};
use crate::witnessed::reattachable;

use super::KObject;

/// Two-arm value currency. `Copy` like the `&'a` references it wraps, so it threads through node
/// results and the lift path without clones.
#[derive(Clone, Copy)]
pub enum Carried<'a> {
    Object(&'a KObject<'a>),
    Type(&'a KType<'a>),
}

/// `Reattachable` family for [`Carried`] — the value channel's erase/reattach owner and the
/// scheduler's `Workload::Value`, stored in a `Witnessed<CarriedFamily, _>` slot and re-anchored on read.
pub struct CarriedFamily;

// A `Carried<'r>` is two `&'r` references, layout identical for every `'r`; the shared
// `reattachable!` macro discharges that obligation once.
reattachable! {
    CarriedFamily => Carried<'r>,
}

impl<'a> Carried<'a> {
    /// The `Object` arm, if this is one.
    pub fn as_object(self) -> Option<&'a KObject<'a>> {
        match self {
            Carried::Object(o) => Some(o),
            Carried::Type(_) => None,
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

/// Owned by-value cell — the owned dual of the borrowed [`Carried`], holding each arm inline (no `Rc`).
/// The cell type of a `List` / `Dict` / `Record` and the currency a builtin's bound argument record
/// (`Record<Held>`) holds.
pub enum Held<'a> {
    Object(KObject<'a>),
    Type(KType<'a>),
}

impl<'a> Held<'a> {
    /// Owned-ify a borrowed [`Carried`] into a cell: deep-clone the object arm, clone the type arm.
    pub fn from_carried(c: Carried<'a>) -> Held<'a> {
        match c {
            Carried::Object(o) => Held::Object(o.deep_clone()),
            Carried::Type(t) => Held::Type(t.clone()),
        }
    }

    /// The `Object` arm as a borrow, if this is one.
    pub fn as_object(&self) -> Option<&KObject<'a>> {
        match self {
            Held::Object(o) => Some(o),
            Held::Type(_) => None,
        }
    }

    /// The `Type` arm, if this is one.
    pub fn as_type(&self) -> Option<&KType<'a>> {
        match self {
            Held::Type(t) => Some(t),
            Held::Object(_) => None,
        }
    }

    /// The `Object` arm, panicking on a `Type` arm — for value-only consumers (a site that
    /// by construction handles only a runtime object, e.g. a dict-key carrier).
    pub fn object(&self) -> &KObject<'a> {
        match self {
            Held::Object(o) => o,
            Held::Type(t) => panic!("expected an Object cell, got a Type arm: {}", t.name()),
        }
    }

    /// Independent copy: deep-clone the object arm, clone the type arm.
    pub fn deep_clone(&self) -> Held<'a> {
        match self {
            Held::Object(o) => Held::Object(o.deep_clone()),
            Held::Type(t) => Held::Type(t.clone()),
        }
    }

    /// The cell's shallow type tag: an object's `ktype()`, or a type arm's own `OfKind`
    /// classification (mirrors [`Carried::ktype`]).
    pub fn ktype(&self) -> KType<'a> {
        match self {
            Held::Object(o) => o.ktype(),
            Held::Type(t) => KType::OfKind(t.kind_of()),
        }
    }

    /// Surface rendering of either arm — an object's `summarize` or a type's `name`.
    pub fn summarize(&self) -> String {
        match self {
            Held::Object(o) => Parseable::summarize(o),
            Held::Type(t) => t.name(),
        }
    }
}

impl<'a> From<KObject<'a>> for Held<'a> {
    fn from(o: KObject<'a>) -> Held<'a> {
        Held::Object(o)
    }
}
