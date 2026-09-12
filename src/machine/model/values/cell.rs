//! The value-channel cells: [`Carried`], the scheduler's value currency — what a node produces and
//! the node store holds — and [`Held`], its owned by-value dual, the cell type of a container
//! substrate and of a builtin's bound argument slot.
//!
//! A produced result is either a runtime [`KObject`] (the `Object` arm), a type flowing raw in the
//! type channel (the `Type` arm), so a type-operator returns a `KType` handle without boxing it
//! into a `KObject`, or a surface type name the bind seam could not lower to a type (the
//! `UnresolvedType` arm).
//!
//! `UnresolvedType` carries the token's [`TypeSymbol`] verbatim: no type handle ever denotes an
//! unresolved name. [`ExpressionPart::resolve_for`](crate::parse::ExpressionPart::resolve_for)
//! mints it for a bare user name, and the park-capable
//! [`Scope::resolve_type_identifier`](crate::machine::core::Scope::resolve_type_identifier)
//! consumes it.
//!
//! The carrier states a value cell travels and rests in are here too, beside the cell they carry:
//! [`DeliveredCarried`] is the in-transit envelope, [`SplicedCell`] the resting form a working
//! expression holds, and [`read_resting`] the pin-free read over one. Each is a Koan-bound alias
//! from [`memory`](crate::memory) applied to [`CarriedFamily`] — an instantiation of the substrate,
//! which is why it lives with the payload rather than with the substrate.
//!
//! A cell answers no question that needs a registry: its type tag is
//! [`TypeRegistry::ktype_of`](crate::machine::model::TypeRegistry::ktype_of) and its rendering
//! [`RunRegistries::held_summary`](crate::machine::model::RunRegistries::held_summary), both owned
//! by the registry that holds the answer.
//!
//! See [execution/calls-and-values.md § `KObject` and the model/core boundary](../../../../old_design/execution/calls-and-values.md#kobject-and-the-modelcore-boundary).

use crate::machine::model::KObject;
use crate::machine::model::types::KType;
use crate::memory::{Delivered, FoldingBrand, Sealed, reattachable};
use crate::parse::ProgramNode;
use crate::parse::{BinderSymbol, TypeSymbol};

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

/// Koan's **delivery envelope** for a value cell: the library [`Delivered`](crate::memory::Delivered)
/// carrying a witnessed [`Carried`] paired with its retained frame owner. The in-transit form of a
/// value's liveness — from a scheduler pull (or a resident seal) to its adoption. The retained frame
/// is private to the envelope and materializes into a minted reach set only through the envelope's
/// own verbs (`adopt_into` / `open_adopted` / `transfer_into`), so koan never holds a bare frame pin
/// at a consumer site. The envelope's member set pins the value's home region alongside everything
/// else it reaches, and the residence itself is the host of the description the carrier references —
/// so a site that needs the home back reads it off the value's own record rather than off a side
/// channel on the envelope, and a relocation derives what it still reaches from the product it built
/// ([`product_reaches_region`](super::kobject::product_reaches_region)) rather than choosing a
/// bundle up front.
pub type DeliveredCarried = Delivered<CarriedFamily>;

/// A resolved sub-result **at rest** inside a working expression: the producer's sealed value
/// carrier alone, `Copy` and `Drop`-free, with the pins that keep its backing alive lodged one level
/// down in the region the cell was rested into
/// ([`Delivered::rest_in`](crate::memory::Delivered::rest_in), reached through
/// [`Scope::rest_delivered`](crate::machine::core::Scope::rest_delivered)). The resting form of a
/// [`DeliveredCarried`]: same carrier, ownership relocated — which is what lets a
/// [`WorkingPart`](crate::machine::model::WorkingPart) hold one without becoming heap-shaped.
///
/// Reading one names its coverage, as every reference-only carrier does: the reach-carrying route is
/// [`Scope::lift_spliced`](crate::machine::core::Scope::lift_spliced), back to an envelope for an
/// adoption; a verdict-only reader opens the cell at its own brand through [`read_resting`].
pub type SplicedCell<'home> = Sealed<'home, CarriedFamily>;

/// Read a resting splice cell at a site with **no pin vocabulary** — the registry-free renderers
/// ([`WorkingPart`](crate::machine::model::WorkingPart)'s `Debug` / `summarize`) and the slot
/// classifier `KType::accepts_cell`). Each is a pure probe over a part the caller already holds,
/// reached from signatures that carry no scope and (for `Debug::fmt`) could not be given one.
///
/// The coverage is the step's, not the reader's: a probe runs synchronously inside the step holding
/// the expression, and a cell rests in that step's own cart — the splice and every read of it happen
/// on one side of a tail hop, never across one. So the pointee outlives the read for a reason
/// outside it, which is exactly what `NoPins` names. Stated once here so the assertion has one home
/// rather than one per call site. A reader that holds a scope names a pin instead:
/// [`Scope::read_spliced`](crate::machine::core::Scope::read_spliced) for another verdict,
/// [`Scope::lift_spliced`](crate::machine::core::Scope::lift_spliced) when it goes on to *adopt* the
/// value, which owns the reach rather than merely naming it.
pub(crate) fn read_resting<R>(
    cell: &SplicedCell<'_>,
    read: impl for<'b> FnOnce(Carried<'b>) -> R,
) -> R {
    cell.open(read)
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

// `Held`'s own erase/reattach registration. A `Held<'r>` is a tag plus a `KObject<'r>`, a
// lifetime-free `KType` or symbol, or a `ProgramNode<'r>` — layout identical for every `'r`, which
// is the obligation the shared `reattachable!` macro discharges once.
//
// The macro's `!needs_drop` backstop is where `Held`'s structural `Drop`-freedom is proved: every
// arm is a scalar handle or a region borrow, so the assert compiling *is* the claim that the bump —
// which runs no destructor — loses nothing by hosting a cell. An arm that later brings glue back
// fails the build here, and the aggregate folds' bumped cell runs depend on it not doing so.
reattachable! {
    Held<'static> => Held<'r>,
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
}

impl<'a> From<KObject<'a>> for Held<'a> {
    fn from(o: KObject<'a>) -> Held<'a> {
        Held::Object(o)
    }
}

impl<'a> FoldingBrand<'a> {
    /// Store one container cell at this fold's own brand, handing back the resident `&'a Held<'a>`
    /// borrow the sectioned alloc door takes as its payload
    /// ([`Sectioned::build`](crate::memory::Sectioned::build)). One line over
    /// [`FoldingBrand::alloc_folded`], which carries the rank-2 soundness argument. Residing the cell
    /// before the door runs is what ties it to the same `'a` the container's run descriptions are
    /// interned at, so one pin covers a projected cell and its reach together.
    pub(crate) fn alloc_cell_folded(self, cell: Held<'a>) -> &'a Held<'a> {
        self.alloc_folded(cell)
    }
}
