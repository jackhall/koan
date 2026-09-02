//! `Carried` — the scheduler's value currency: what a node produces and the node store
//! holds. A produced result is either a runtime [`KObject`] (the `Object` arm), a type
//! flowing raw in the type channel (the `Type` arm), so a type-operator returns a `KType`
//! handle without boxing it into a `KObject`, or a surface type name the bind seam could not
//! lower to a type (the `UnresolvedType` arm).
//!
//! `UnresolvedType` carries the token's [`TypeSymbol`] verbatim: no type handle ever denotes an
//! unresolved name. [`ExpressionPart::resolve_for`](crate::machine::model::ast::ExpressionPart::resolve_for)
//! mints it for a bare user name, and the park-capable
//! [`Scope::resolve_type_identifier`](crate::machine::core::Scope::resolve_type_identifier)
//! consumes it.
//!
//! See [execution/calls-and-values.md § `KObject` and the model/core boundary](../../../../design/execution/calls-and-values.md#kobject-and-the-modelcore-boundary).

use crate::machine::model::labels::{BinderSymbol, TypeSymbol};
use crate::machine::model::types::{KKind, KType, TypeRegistry, display_label};
use crate::witnessed::reattachable;

use super::KObject;
use crate::machine::model::RunRegistries;
use crate::machine::model::ast::ProgramNode;

/// Three-arm value currency. `Copy` — the object arms wrap `&'a` references and the `Type` arm a
/// `Copy` [`KType`] handle, so it threads through node results and the lift path without clones.
#[derive(Clone, Copy)]
pub enum Carried<'a> {
    Object(&'a KObject<'a>),
    Type(KType),
    /// A surface type name the bind seam left unlowered; resolved by scope walk at the consumer.
    /// Held by value: a [`TypeSymbol`] is a `Copy` digest, so there is nothing for a reference
    /// to add and the carrier borrows nothing from the storage that parsed the token.
    UnresolvedType(TypeSymbol),
}

/// `Reattachable` family for [`Carried`] — the value channel's erase/reattach owner and the
/// scheduler's `Workload::Value`, stored in a `Witnessed<CarriedFamily, _>` slot and re-anchored on read.
pub struct CarriedFamily;

// A `Carried<'r>` is a tag plus `&'r` references and a lifetime-free `KType` handle, layout
// identical for every `'r`; the shared `reattachable!` macro discharges that obligation once.
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
                    "expected an Object value, got a Type arm: 0x{:032x}",
                    t.digest().0
                )
            }
            Carried::UnresolvedType(ti) => {
                panic!(
                    "expected an Object value, got an unresolved type name: 0x{:032x}",
                    ti.symbol().0
                )
            }
        }
    }

    /// Surface rendering of any arm, written straight into `f` — an object's summary, a type's
    /// name, or the unlowered name's surface form.
    pub fn write_summary(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        registries: &RunRegistries,
    ) -> std::fmt::Result {
        match self {
            Carried::Object(o) => o.write_summary(f, registries),
            Carried::Type(t) => t.write_name(f, registries),
            Carried::UnresolvedType(ti) => {
                write!(f, "{}", display_label(ti.symbol(), registries))
            }
        }
    }

    /// [`write_summary`](Self::write_summary) as a `Display` view.
    pub fn summary<'x>(&'x self, registries: &'x RunRegistries) -> CarriedSummary<'x, 'a> {
        CarriedSummary {
            carried: self,
            registries,
        }
    }

    /// The carried value's surface as an owned `String`.
    pub fn summarize(&self, registries: &RunRegistries) -> String {
        self.summary(registries).to_string()
    }

    /// The shallow type tag of the carried value: an object's `ktype()`, or a type-channel
    /// arm's own `OfKind` classification.
    pub fn ktype(&self, types: &TypeRegistry) -> KType {
        match self {
            Carried::Object(o) => o.ktype(),
            Carried::Type(t) => KType::of_kind(t.kind_of(types)),
            // An unlowered name denotes a proper type once resolved.
            Carried::UnresolvedType(_) => KType::of_kind(KKind::ProperType),
        }
    }
}

/// Owned by-value cell — the owned dual of the borrowed [`Carried`], holding each arm inline (no `Rc`).
/// The cell type of a `List` / `Dict` / `Record` substrate, and the value half of a builtin's bound
/// argument slot (`BoundArg`). The `Name` and `RecordType` arms are the two with no [`Carried`]
/// peer: each is minted only by a part-kind-exact capture slot, so it reaches a bound slot and no
/// substrate.
///
/// `Copy` for the same reason [`KObject`] is: every arm is a scalar handle or a region borrow, so a
/// cell owns no allocation and runs no `Drop` at region death. The bound is what lets a `Held` cell
/// ride the `T: Copy` bump doors.
#[derive(Clone, Copy)]
pub enum Held<'a> {
    Object(KObject<'a>),
    Type(KType),
    /// The owned dual of [`Carried::UnresolvedType`] — the bind seam's carrier for a bare type
    /// name that is not a builtin leaf. Consumers resolve it against their scope chain.
    UnresolvedType(TypeSymbol),
    /// The bind seam's carrier for a captured name token: the classified symbol the parse minted,
    /// with the class taken from the part variant (`Identifier` part → `Value`, `Type` part →
    /// `Type`). Minted only for the raw name-capture slots (`:Identifier`, `NameToken`,
    /// `TypeNameToken`); a binder position never resolves, so no consumer re-derives the class
    /// from a rendering. There is no [`Carried`] peer: a captured name is never a produced
    /// result, only a bound argument.
    Name(BinderSymbol),
    /// The bind seam's carrier for a `:{…}` captured raw at a [`KType::RECORD_TYPE`] slot. A sigil
    /// capture (`:(…)`) rides [`KObject::KExpression`], so the two raw type-expression shapes stay
    /// distinguishable in a body under a union carrier slot — the inner expression cannot be
    /// sniffed for the difference, since `:( :{…} )` makes a sigil's inner a lone record part.
    /// There is no [`Carried`] peer: `RECORD_TYPE` is part-kind-exact, so no resolved cell ever
    /// lands here.
    RecordType(ProgramNode<'a>),
}

impl<'a> Held<'a> {
    /// Owned-ify a borrowed [`Carried`] into a cell: deep-clone the object arm, copy the
    /// type-channel arms.
    pub fn from_carried(c: Carried<'a>) -> Held<'a> {
        match c {
            Carried::Object(o) => Held::Object(o.deep_clone()),
            Carried::Type(t) => Held::Type(t),
            Carried::UnresolvedType(ti) => Held::UnresolvedType(ti),
        }
    }

    /// The `Object` arm as a borrow, if this is one.
    pub fn as_object(&self) -> Option<&KObject<'a>> {
        match self {
            Held::Object(o) => Some(o),
            Held::Type(_) | Held::UnresolvedType(_) | Held::Name(_) | Held::RecordType(_) => None,
        }
    }

    /// The `Type` arm, if this is one.
    pub fn as_type(&self) -> Option<KType> {
        match self {
            Held::Type(t) => Some(*t),
            Held::Object(_) | Held::UnresolvedType(_) | Held::Name(_) | Held::RecordType(_) => None,
        }
    }

    /// The `Object` arm, panicking on a `Type` arm — for value-only consumers (a site that
    /// by construction handles only a runtime object, e.g. a dict-key carrier).
    pub fn object(&self) -> &KObject<'a> {
        match self {
            Held::Object(o) => o,
            Held::Type(t) => panic!(
                "expected an Object cell, got a Type arm: 0x{:032x}",
                t.digest().0
            ),
            Held::UnresolvedType(ti) => panic!(
                "expected an Object cell, got an unresolved type name: 0x{:032x}",
                ti.symbol().0
            ),
            Held::Name(b) => panic!(
                "expected an Object cell, got a captured name: 0x{:032x}",
                b.symbol().0
            ),
            Held::RecordType(_) => panic!("expected an Object cell, got a captured record type"),
        }
    }

    /// Independent copy: deep-clone the object arm, copy the type-channel arms.
    pub fn deep_clone(&self) -> Held<'a> {
        match self {
            Held::Object(o) => Held::Object(o.deep_clone()),
            Held::Type(t) => Held::Type(*t),
            Held::UnresolvedType(ti) => Held::UnresolvedType(*ti),
            Held::Name(b) => Held::Name(*b),
            Held::RecordType(e) => Held::RecordType(*e),
        }
    }

    /// The cell's shallow type tag: an object's `ktype()`, or a type-channel arm's own
    /// `OfKind` classification (mirrors [`Carried::ktype`]).
    pub fn ktype(&self, types: &TypeRegistry) -> KType {
        match self {
            Held::Object(o) => o.ktype(),
            Held::Type(t) => KType::of_kind(t.kind_of(types)),
            Held::UnresolvedType(_) => KType::of_kind(KKind::ProperType),
            Held::Name(BinderSymbol::Value(_)) => KType::IDENTIFIER,
            Held::Name(BinderSymbol::Type(_)) => KType::NAME_TOKEN,
            Held::RecordType(_) => KType::RECORD_TYPE,
        }
    }

    /// Surface rendering of any arm, written straight into `f` — an object's summary, a type's
    /// name, or the unlowered name's surface form.
    pub fn write_summary(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        registries: &RunRegistries,
    ) -> std::fmt::Result {
        match self {
            Held::Object(o) => o.write_summary(f, registries),
            Held::Type(t) => t.write_name(f, registries),
            Held::UnresolvedType(ti) => write!(f, "{}", display_label(ti.symbol(), registries)),
            Held::Name(b) => write!(f, "{}", display_label(b.symbol(), registries)),
            Held::RecordType(e) => e.write_summary(f, &registries.labels),
        }
    }

    /// [`write_summary`](Self::write_summary) as a `Display` view.
    pub fn summary<'x>(&'x self, registries: &'x RunRegistries) -> HeldSummary<'x, 'a> {
        HeldSummary {
            held: self,
            registries,
        }
    }

    /// The cell's surface as an owned `String`.
    pub fn summarize(&self, registries: &RunRegistries) -> String {
        self.summary(registries).to_string()
    }
}

/// A [`Carried::summary`] view: one borrowed cell plus the registries it resolves through.
pub struct CarriedSummary<'x, 'a> {
    carried: &'x Carried<'a>,
    registries: &'x RunRegistries,
}

impl std::fmt::Display for CarriedSummary<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.carried.write_summary(f, self.registries)
    }
}

/// A [`Held::summary`] view: one owned cell plus the registries it resolves through.
pub struct HeldSummary<'x, 'a> {
    held: &'x Held<'a>,
    registries: &'x RunRegistries,
}

impl std::fmt::Display for HeldSummary<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.held.write_summary(f, self.registries)
    }
}

impl<'a> From<KObject<'a>> for Held<'a> {
    fn from(o: KObject<'a>) -> Held<'a> {
        Held::Object(o)
    }
}
