//! `Carried` — the scheduler's value currency: what a node produces and the node store
//! holds. A produced result is either a runtime [`KObject`] (the `Object` arm), a type
//! flowing raw in the type channel (the `Type` arm), so a type-operator returns a `&KType`
//! without boxing it into a `KObject`, or a surface type name the bind seam could not lower
//! to a type (the `UnresolvedType` arm).
//!
//! `UnresolvedType` carries a [`TypeIdentifier`] verbatim: no type handle ever denotes an
//! unresolved name. [`ExpressionPart::resolve_for`](crate::machine::model::ast::ExpressionPart::resolve_for)
//! mints it for a bare user name, and the park-capable
//! [`Scope::resolve_type_identifier`](crate::machine::core::Scope::resolve_type_identifier)
//! consumes it.
//!
//! See [execution/calls-and-values.md § `KObject` and the model/core boundary](../../../../design/execution/calls-and-values.md#kobject-and-the-modelcore-boundary).

use crate::machine::model::types::{KKind, KType, TypeRegistry};
use crate::machine::model::TypeIdentifier;
use crate::witnessed::reattachable;

use super::KObject;

/// Three-arm value currency. `Copy` like the `&'a` references it wraps, so it threads through node
/// results and the lift path without clones.
#[derive(Clone, Copy)]
pub enum Carried<'a> {
    Object(&'a KObject<'a>),
    Type(&'a KType),
    /// A surface type name the bind seam left unlowered; resolved by scope walk at the consumer.
    UnresolvedType(&'a TypeIdentifier),
}

/// `Reattachable` family for [`Carried`] — the value channel's erase/reattach owner and the
/// scheduler's `Workload::Value`, stored in a `Witnessed<CarriedFamily, _>` slot and re-anchored on read.
pub struct CarriedFamily;

// A `Carried<'r>` is a tag plus `&'r` references, layout identical for every `'r`; the shared
// `reattachable!` macro discharges that obligation once.
reattachable! {
    CarriedFamily => Carried<'r>,
}

impl<'a> Carried<'a> {
    /// The `Object` arm, if this is one.
    pub fn as_object(self) -> Option<&'a KObject<'a>> {
        match self {
            Carried::Object(o) => Some(o),
            Carried::Type(_) | Carried::UnresolvedType(_) => None,
        }
    }

    /// The `Object` arm, panicking on a `Type` arm. For value-consumers — a site that by
    /// construction only ever handles a runtime object (not a type flowing in the type
    /// channel).
    pub fn object(self) -> &'a KObject<'a> {
        match self {
            Carried::Object(o) => o,
            Carried::Type(t) => {
                panic!(
                    "expected an Object value, got a Type arm: {}",
                    t.name_without_registry()
                )
            }
            Carried::UnresolvedType(ti) => {
                panic!(
                    "expected an Object value, got an unresolved type name: {}",
                    ti.render()
                )
            }
        }
    }

    /// Surface rendering of any arm — an object's `summarize`, a type's `name`, or the
    /// unlowered name's surface form.
    pub fn summarize(&self, types: &TypeRegistry) -> String {
        self.render_summary(Some(types))
    }

    /// The registry-free twin of [`Self::summarize`], for the `Formatter`-only renderers that
    /// have no registry to hand: [`std::fmt::Debug`] and the surface-only
    /// [`ExpressionPart::summarize`](crate::machine::model::ast::ExpressionPart::summarize).
    pub fn summarize_without_registry(&self) -> String {
        self.render_summary(None)
    }

    pub(crate) fn render_summary(&self, types: Option<&TypeRegistry>) -> String {
        match self {
            Carried::Object(o) => o.render_summary(types),
            Carried::Type(t) => t.name_or_without_registry(types),
            Carried::UnresolvedType(ti) => ti.render(),
        }
    }

    /// The shallow type tag of the carried value: an object's `ktype()`, or a type-channel
    /// arm's own `OfKind` classification.
    pub fn ktype(&self, types: &TypeRegistry) -> KType {
        match self {
            Carried::Object(o) => o.ktype(),
            Carried::Type(t) => KType::OfKind(t.kind_of(types)),
            // An unlowered name denotes a proper type once resolved.
            Carried::UnresolvedType(_) => KType::OfKind(KKind::ProperType),
        }
    }
}

/// Owned by-value cell — the owned dual of the borrowed [`Carried`], holding each arm inline (no `Rc`).
/// The cell type of a `List` / `Dict` / `Record` and the currency a builtin's bound argument record
/// (`Record<Held>`) holds.
pub enum Held<'a> {
    Object(KObject<'a>),
    Type(KType),
    /// The owned dual of [`Carried::UnresolvedType`] — the bind seam's carrier for a bare type
    /// name that is not a builtin leaf. Consumers resolve it against their scope chain.
    UnresolvedType(TypeIdentifier),
}

impl<'a> Held<'a> {
    /// Owned-ify a borrowed [`Carried`] into a cell: deep-clone the object arm, clone the
    /// type-channel arms.
    pub fn from_carried(c: Carried<'a>) -> Held<'a> {
        match c {
            Carried::Object(o) => Held::Object(o.deep_clone()),
            Carried::Type(t) => Held::Type(t.clone()),
            Carried::UnresolvedType(ti) => Held::UnresolvedType(ti.clone()),
        }
    }

    /// The `Object` arm as a borrow, if this is one.
    pub fn as_object(&self) -> Option<&KObject<'a>> {
        match self {
            Held::Object(o) => Some(o),
            Held::Type(_) | Held::UnresolvedType(_) => None,
        }
    }

    /// The `Type` arm, if this is one.
    pub fn as_type(&self) -> Option<&KType> {
        match self {
            Held::Type(t) => Some(t),
            Held::Object(_) | Held::UnresolvedType(_) => None,
        }
    }

    /// The `Object` arm, panicking on a `Type` arm — for value-only consumers (a site that
    /// by construction handles only a runtime object, e.g. a dict-key carrier).
    pub fn object(&self) -> &KObject<'a> {
        match self {
            Held::Object(o) => o,
            Held::Type(t) => panic!(
                "expected an Object cell, got a Type arm: {}",
                t.name_without_registry()
            ),
            Held::UnresolvedType(ti) => panic!(
                "expected an Object cell, got an unresolved type name: {}",
                ti.render()
            ),
        }
    }

    /// Independent copy: deep-clone the object arm, clone the type-channel arms.
    pub fn deep_clone(&self) -> Held<'a> {
        match self {
            Held::Object(o) => Held::Object(o.deep_clone()),
            Held::Type(t) => Held::Type(t.clone()),
            Held::UnresolvedType(ti) => Held::UnresolvedType(ti.clone()),
        }
    }

    /// The cell's shallow type tag: an object's `ktype()`, or a type-channel arm's own
    /// `OfKind` classification (mirrors [`Carried::ktype`]).
    pub fn ktype(&self, types: &TypeRegistry) -> KType {
        match self {
            Held::Object(o) => o.ktype(),
            Held::Type(t) => KType::OfKind(t.kind_of(types)),
            Held::UnresolvedType(_) => KType::OfKind(KKind::ProperType),
        }
    }

    /// Surface rendering of any arm — an object's `summarize`, a type's `name`, or the
    /// unlowered name's surface form.
    pub fn summarize(&self, types: &TypeRegistry) -> String {
        self.render_summary(Some(types))
    }

    pub(crate) fn render_summary(&self, types: Option<&TypeRegistry>) -> String {
        match self {
            Held::Object(o) => o.render_summary(types),
            Held::Type(t) => t.name_or_without_registry(types),
            Held::UnresolvedType(ti) => ti.render(),
        }
    }
}

impl<'a> From<KObject<'a>> for Held<'a> {
    fn from(o: KObject<'a>) -> Held<'a> {
        Held::Object(o)
    }
}
