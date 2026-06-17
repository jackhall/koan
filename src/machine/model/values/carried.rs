//! `Carried` — the scheduler's value currency: what a node produces and the node store
//! holds. A produced result is either a runtime [`KObject`] (the `Object` arm) or a type
//! flowing raw in the type channel (the `Type` arm), so a type-operator returns a `&KType`
//! without boxing it into a `KObject`.
//!
//! See [execution-model.md § `KObject` and the model/core boundary](../../../../design/execution-model.md#kobject-and-the-modelcore-boundary).

use std::rc::Rc;

use crate::machine::core::Reattachable;
use crate::machine::model::types::{KType, Parseable};
use crate::machine::KError;

use super::KObject;

/// Two-arm value currency. `Copy` like the `&'a` references it wraps, so it threads through
/// the `Ok` arm of a node result, the `Outcome::Done` value, and the lift path without clones.
#[derive(Clone, Copy)]
pub enum Carried<'a> {
    Object(&'a KObject<'a>),
    Type(&'a KType<'a>),
}

/// `Reattachable` family for [`Carried`] — the value channel's erase/reattach owner (`ErasedValue`,
/// `pin_carried_to_run`, `deps_for_builtin`). Layout-invariant: a `Carried<'a>` is two `&'a`
/// references, whose representation does not depend on `'a`.
pub struct CarriedFamily;

// SAFETY: `Carried<'r>` is one type generic only in `'r`; its layout (two pointers) is identical
// for every `'r`.
unsafe impl Reattachable for CarriedFamily {
    type At<'r> = Carried<'r>;
}

/// `Reattachable` family for a dep terminal `Result<Carried, KError>` — the element type the
/// dep-delivery slice retypes (`deps_at_step`). Layout-invariant for the same reason as
/// [`CarriedFamily`] (`KError` carries no lifetime).
pub struct ResultCarriedFamily;

// SAFETY: `Result<Carried<'r>, KError>` is generic only in `'r` (the `KError` arm is lifetime-free);
// its layout does not depend on `'r`.
unsafe impl Reattachable for ResultCarriedFamily {
    type At<'r> = Result<Carried<'r>, KError>;
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

/// Owned form of the value currency, for storage that must survive a [`KFuture`] lift: the
/// `Object` arm rides an [`Rc`] (lift-stable, like the bundle's pre-existing `Rc<KObject>`),
/// the `Type` arm is an owned [`KType`] (`Clone`-stable — recursive sets ride `Rc`, module
/// refs carry their own frame anchor). The form a bound argument record (`Record<ArgValue>`) holds,
/// and that [`ExpressionPart::resolve_for`](crate::machine::model::ast::ExpressionPart::resolve_for)
/// produces. The borrowed [`Carried`] is the channel currency; this is its owned dual.
///
/// [`KFuture`]: crate::machine::core::KFuture
pub enum ArgValue<'a> {
    Object(Rc<KObject<'a>>),
    Type(KType<'a>),
}

impl<'a> ArgValue<'a> {
    /// Independent copy: `Rc::clone` the object arm, `clone` the type arm.
    pub fn deep_clone(&self) -> ArgValue<'a> {
        match self {
            ArgValue::Object(rc) => ArgValue::Object(Rc::new(rc.deep_clone())),
            ArgValue::Type(kt) => ArgValue::Type(kt.clone()),
        }
    }

    /// The `Object` arm as a borrow, if this is one.
    pub fn as_object(&self) -> Option<&KObject<'a>> {
        match self {
            ArgValue::Object(rc) => Some(rc),
            ArgValue::Type(_) => None,
        }
    }

    /// The `Type` arm, if this is one.
    pub fn as_type(&self) -> Option<&KType<'a>> {
        match self {
            ArgValue::Type(kt) => Some(kt),
            ArgValue::Object(_) => None,
        }
    }

    /// Surface rendering of either arm — an object's `summarize` or a type's `name`.
    pub fn summarize(&self) -> String {
        match self {
            ArgValue::Object(o) => Parseable::summarize(&**o),
            ArgValue::Type(t) => t.name(),
        }
    }
}

/// Owned by-value cell of a `List` / `Dict` / `Record`: a runtime object or a type carried
/// as a first-class aggregate element. The by-value dual of [`Carried`] for aggregate
/// storage — distinct from [`ArgValue`], whose `Object` arm is `Rc`-shared for per-call
/// bundle cloning; an aggregate owns its cells inline (by value, not `Rc`-shared).
pub enum Held<'a> {
    Object(KObject<'a>),
    Type(KType<'a>),
}

impl<'a> Held<'a> {
    /// Owned-ify a borrowed channel [`Carried`] into a cell: deep-clone the object arm,
    /// clone the type arm. Used by the literal-aggregate `AwaitDeps` finish.
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
