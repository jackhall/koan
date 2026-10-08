//! The knot layer: functions, modules and circular data as values — what closes
//! [`Value`](crate::values::Value)'s knot-member parameter.
//!
//! A knot member is one node of a [`Knot`](crate::memory::Knot) laid down in a cell's region.
//! [`Knotted`] is that node, the `(knot, index)` pair, and [`Node`] is what one holds: a
//! [function](crate::knot::function), a [builtin](crate::knot::builtin) overload, a quote's
//! [code](crate::knot::code), a data node — a [`Circular`] over links — a
//! [module](crate::knot::module), or a barrier over a function member of an opaque view. A function
//! or a quote that names no fellow is a one-node knot, and so is every builtin, every module and
//! every barrier; a deferred-only component of value binders is born together as one
//! knot by [`tie`], each mention of a fellow member an edge into it. A `FN` a data member holds that
//! captures a fellow member is a node of that knot, and so is a quote whose `$` name reads one. Any
//! other callable no binder names is born alone through [`lambda`], and any other quote through
//! [`quote`], where the evaluator meets it.
//!
//! A module's node is the carrier, not a claim about cycles: `m.f` is not a knot edge but an index
//! into the run, and the run holds members of knots the module does not own. A mention reached from
//! a module binder's root is eager whatever body it sits in, so a module is never in a cycle.
//!
//! [`Knotted`] is sixteen bytes, so a value holding one stays one twenty-four-byte word. A member
//! copies at a crossing by re-tying its whole knot at the destination, each held value deep-copied
//! and each edge carried verbatim, priced by the knot's memoized weight under the ordinary verdict.
//! A member's content digest is computed on demand: its knot's, over each node's content with an
//! edge to a sibling by its index, beside its index there — so values that reach one another
//! digest as one knot. A member lists the values its knot holds and hashes the knot over their
//! digests; `values` walks them on its own stack, so a chain of knots never nests a call per knot.
//! A function compares by its shape's address — one per `FN` written, so a copy keeps it — and its
//! captures, a builtin by its record's address, a quote by its code and bindings, and a module or a
//! barrier is
//! [`Incomparable`](crate::values::Incomparable).
//!
//! **What sits where.** This file is the vocabulary every node kind shares: the member, the node,
//! the value and activation spelled at it, and the [`Supplied`] and [`Untieable`] a birth answers
//! with. [`function`], [`builtin`], [`code`] and [`module`] each own one node kind — its payload, its doors and,
//! for a module, everything that reads one by name — and none names another; [`data`] owns
//! the data node; [`tie`](crate::knot::tie()) births a component over them, and `copy` re-ties a whole knot.
//! A submodule reaches this vocabulary through the facade, never a sibling.
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::elaborate`,
//! `crate::memory`, `crate::parse`, `crate::scope`, `crate::symbols`, `crate::type_lattice` and
//! `crate::values`, and nothing else in the crate; `tests::boundary` reads the source to hold it
//! there. Neither `values` nor `scope` names this module.
//!
//! See [knot/README.md](knot/README.md).

mod builtin;
mod code;
mod copy;
mod data;
mod function;
pub mod module;
mod tie;

#[cfg(test)]
pub(crate) mod tests;

pub use builtin::{BuiltinFunction, builtin};
pub use code::{Code, UsingRefused, quote, using};
pub use function::{Function, instance, lambda};
pub use module::{Coerced, Module, body_activation};
pub use tie::tie;

use std::fmt;

use crate::memory::{BumpAllocator, DropFree, Edge, Member, Writer, covariant, reattachable};
use crate::parse::ExpressionPart;
use crate::scope::Elaboration;
use crate::scope::{Activation, ActivationView, BodyShape, Builtins, CaptureSlot, Site};
use crate::symbols::{BinderSymbol, SymbolInterner};
use crate::type_lattice::{DeclaredType, KType, TypeRegistry, display_name};
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{
    self, Circular, ConstructionRefused, ContentDigest, KeyRejected, Resolved, Seen, Value,
    ValueCarrier, ValueFamily, Weight,
};

/// The payload of a knot node.
#[derive(Clone, Copy)]
pub enum Node<'graph, 'cell> {
    Function(Function<'graph, 'cell, Knotted<'graph, 'cell>>),
    /// A builtin overload, its record in program storage.
    Builtin(&'graph BuiltinFunction),
    /// A data node: a member's right-hand side, or an anonymous constructor below one on the path to
    /// a sibling mention.
    Data {
        circular: Circular<'cell, Knotted<'graph, 'cell>>,
        /// What rebuilding the whole knot this node sits in writes, the same on every node.
        knot_weight: Weight,
    },
    /// A module: its self-signature, its members in layout order, and the same weight.
    Module(Module<'graph, 'cell>),
    /// A function member behind an opaque view's barrier: the barrier sits beside the node in the
    /// same region and the node points at it, since its six fields would otherwise be the widest
    /// arm and every node in the program pays for that.
    Coerced(&'cell Coerced<'graph, 'cell>),
    /// A quote's code, homed beside the node for the reason a barrier is.
    Code(&'cell Code<'graph, 'cell>),
}

const _: () = assert!(!std::mem::needs_drop::<Node<'static, 'static>>());

/// Hand `each` every capture of `links`, a closure of `shape`, that its code digest does not name
/// — every one but a read of the program's top level, and every type capture — beside its slot:
/// what a closure's or a module's content composes over its code.
fn composed<L>(shape: &BodyShape<'_>, links: &[L], mut each: impl FnMut(usize, &L)) {
    for (index, link) in links.iter().enumerate() {
        if shape.composes(CaptureSlot(index as u32)) {
            each(index, link);
        }
    }
}

/// A knot member: one node of a knot — a function, a data node, a module or a barrier over a
/// function. Its equality and hash are node identity.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Knotted<'graph, 'cell>(Member<'cell, Node<'graph, 'cell>>);

const _: () = assert!(size_of::<Knotted<'static, 'static>>() == 16);
const _: () = assert!(size_of::<KValue<'static, 'static>>() == 24);
const _: () = assert!(size_of::<KActivation<'static, 'static>>() == 80);
/// The module arm sets the node's width; a barrier's six fields would widen every node in the
/// program, so [`Node::Coerced`] points at them instead.
const _: () = assert!(size_of::<Node<'static, 'static>>() == 64);

impl<'graph, 'cell> Knotted<'graph, 'cell> {
    /// The knot node this member is.
    pub fn member(self) -> Member<'cell, Node<'graph, 'cell>> {
        self.0
    }

    pub fn node(self) -> &'cell Node<'graph, 'cell> {
        self.0.payload()
    }

    /// The function this member runs, if it is a function's node.
    pub fn function(self) -> Option<&'cell Function<'graph, 'cell, Self>> {
        match self.node() {
            Node::Function(function) => Some(function),
            _ => None,
        }
    }

    /// The builtin overload this member is, if it is a builtin's node.
    pub fn builtin(self) -> Option<&'graph BuiltinFunction> {
        match self.node() {
            Node::Builtin(builtin) => Some(builtin),
            _ => None,
        }
    }

    /// The module this member is, if it is a module's node.
    pub fn module(self) -> Option<&'cell Module<'graph, 'cell>> {
        match self.node() {
            Node::Module(module) => Some(module),
            _ => None,
        }
    }

    /// The code this member is, if it is a quote's node.
    pub fn code(self) -> Option<&'cell Code<'graph, 'cell>> {
        match self.node() {
            Node::Code(code) => Some(code),
            _ => None,
        }
    }

    /// The barrier this member sits behind, if it is a coerced function's node.
    pub fn coerced(self) -> Option<&'cell Coerced<'graph, 'cell>> {
        match self.node() {
            Node::Coerced(coerced) => Some(coerced),
            _ => None,
        }
    }

    /// The barriers stacked at this member, outermost first: one per view of a view.
    pub fn barriers(self) -> impl Iterator<Item = &'cell Coerced<'graph, 'cell>> {
        std::iter::successors(self.coerced(), |barrier| barrier.underlying().coerced())
    }

    /// The member behind every barrier stacked here: itself where none is.
    pub fn behind_barriers(self) -> Self {
        self.barriers()
            .last()
            .map_or(self, |barrier| barrier.underlying())
    }

    /// Every value this node holds that its content covers, in the order
    /// [`content`](Self::content) asks for their digests. A builtin's node is a knot of its own and
    /// digests as its overload, never through here.
    fn content_parts<'a>(
        self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        out: &mut dyn FnMut(Seen<'a, Self>),
    ) where
        Self: 'a,
    {
        match self.node() {
            Node::Function(function) => function.content_parts(out),
            Node::Builtin(_) => unreachable!("a builtin digests as its overload"),
            Node::Data { .. } => Circular::content_parts(self, types, scratch, out),
            Node::Module(_) => {}
            Node::Coerced(coerced) => coerced.content_parts(out),
            Node::Code(code) => code.content_parts(out),
        }
    }

    /// The node's content inside its knot's digest, `parts` answering each value
    /// [`content_parts`](Self::content_parts) listed with its digest: a data node's read through
    /// the door at its memo.
    fn content(
        self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        parts: &mut dyn FnMut() -> ContentDigest,
    ) -> ContentDigest {
        match self.node() {
            Node::Function(function) => function.content(parts),
            Node::Builtin(_) => unreachable!("a builtin digests as its overload"),
            Node::Data { .. } => Circular::content(self, types, scratch, parts),
            Node::Module(module) => module.content(),
            Node::Coerced(coerced) => coerced.content(parts),
            Node::Code(code) => code.content(parts),
        }
    }
}

impl fmt::Debug for Knotted<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Knotted")
            .field("knot", &self.0.knot().len())
            .field("index", &self.0.index().index())
            .finish()
    }
}

impl values::Knotted for Knotted<'_, '_> {
    fn ktype(&self) -> DeclaredType<KType> {
        match self.node() {
            Node::Function(function) => function.ktype(),
            Node::Builtin(builtin) => builtin.ktype().into(),
            Node::Data { circular, .. } => circular.ktype().into(),
            Node::Module(module) => module.ktype().into(),
            Node::Coerced(coerced) => coerced.ktype(),
            Node::Code(code) => code.ktype().into(),
        }
    }

    fn weight(&self) -> Weight {
        match self.node() {
            Node::Function(function) => function.knot_weight(),
            Node::Builtin(_) => BuiltinFunction::knot_weight(),
            Node::Data { knot_weight, .. } => *knot_weight,
            Node::Module(module) => module.knot_weight(),
            Node::Coerced(coerced) => coerced.knot_weight(),
            Node::Code(code) => code.knot_weight(),
        }
    }

    /// Every value each node of the knot holds that its content covers, nodes in index order. A
    /// builtin holds none.
    fn held<'a>(
        &self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        out: &mut dyn FnMut(Seen<'a, Self>),
    ) where
        Self: 'a,
    {
        if self.builtin().is_some() {
            return;
        }
        for member in self.0.knot().members() {
            Knotted(member).content_parts(types, scratch, out);
        }
    }

    /// A builtin's knot is its overload — its native and its shape — and every other knot is a
    /// [`Tag::Knot`] over its node count and each node's content in index order, an edge to a
    /// sibling hashed as its index, so values that reach one another digest as the one knot they
    /// are tied into.
    fn digest_held(
        &self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        parts: &mut dyn FnMut() -> ContentDigest,
    ) -> ContentDigest {
        if let Node::Builtin(builtin) = self.node() {
            return DigestHasher::new(Tag::Builtin)
                .feed(builtin.id())
                .feed(builtin.ktype())
                .finished();
        }
        let knot = self.0.knot();
        let mut hasher = DigestHasher::new(Tag::Knot);
        hasher.count(knot.len() as usize);
        for member in knot.members() {
            hasher.digest(Knotted(member).content(types, scratch, parts));
        }
        hasher.finished()
    }

    fn sibling(&self, edge: Edge) -> Self {
        Knotted(self.0.follow(edge))
    }

    fn index(&self) -> Edge {
        self.0.index()
    }

    fn root(&self) -> Self {
        Knotted(self.0.knot().members().next().expect("a knot holds a node"))
    }

    fn resolve<'a>(&self) -> Resolved<'a, Self>
    where
        Self: 'a,
    {
        match self.node() {
            // One shape per function written, so its address is the function's identity.
            Node::Function(function) => Resolved::Function {
                identity: std::ptr::from_ref(function.shape()).addr(),
                instance: function.instance().unwrap_or(&[]),
                closure: function.closure().links(),
            },
            // One record per overload, so its address is the builtin's identity.
            Node::Builtin(builtin) => Resolved::Function {
                identity: std::ptr::from_ref(*builtin).addr(),
                instance: &[],
                closure: &[],
            },
            Node::Data { circular, .. } => Resolved::Circular(*circular),
            Node::Module(_) => Resolved::Module,
            Node::Coerced(_) => Resolved::Barrier,
            Node::Code(code) => Resolved::Code(code.view()),
        }
    }
}

/// The value `source` holds under `name`: a record's field its type shows, restamped in `writer`'s
/// region at the type it is seen at, or a module's member. `None` for any other value, or a name
/// `source` does not show.
fn field<'graph, 'cell>(
    writer: Writer<'cell>,
    source: KValue<'graph, 'cell>,
    name: BinderSymbol,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Option<KValue<'graph, 'cell>> {
    match source {
        Value::Record(_) => Some(
            source
                .field(name.symbol(), types, scratch)?
                .restamped(writer),
        ),
        Value::Knotted(member) => module::layout::member(member, name, types, scratch),
        _ => None,
    }
}

/// What the caller supplies for a part only it can produce: an evaluated value for a data member's
/// part, or a module binder's body already run.
#[derive(Clone, Copy)]
pub enum Supplied<'graph, 'cell> {
    Value(KValue<'graph, 'cell>),
    /// A module binder's body, run: its activation with every slot bound.
    Body(&'cell KActivation<'graph, 'cell>),
}

/// What a tie's caller answers a part only it can evaluate with, asked by site with the part itself
/// — or `None` for a module body, which the caller runs.
pub type Eager<'e, 'graph, 'cell> =
    dyn FnMut(Site, Option<&'graph ExpressionPart<'graph>>) -> Option<Supplied<'graph, 'cell>> + 'e;

/// Why a birth refused: a component [`tie`] could not tie, or a lambda [`lambda`] could not birth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Untieable<'x> {
    /// A member's signature did not elaborate.
    Type(Elaboration),
    /// A member that is neither a callable binder, nor a module binder, nor a `LET` of a value name
    /// whose right-hand side, through one-part groups, is a list, dict or record literal or a
    /// nominal construction.
    Opaque { name: BinderSymbol },
    /// Data member `name`'s right-hand side has a part at `site` the caller must evaluate first.
    Eager { name: BinderSymbol, site: Site },
    /// A dict key in data member `name` at `site` evaluated to something no key can be.
    Key {
        name: BinderSymbol,
        site: Site,
        rejected: KeyRejected,
    },
    /// A construction at `site` in data member `name` the construction rule refuses.
    Construction {
        name: BinderSymbol,
        site: Site,
        refused: ConstructionRefused,
    },
    /// Derived nodes — containers and constructions through a family — that reach one another with
    /// no function node or cut between them, so no finite type memoizes them: the members whose
    /// right-hand sides hold them, in component order.
    TypeCycle { names: &'x [BinderSymbol] },
}

impl<'x> Untieable<'x> {
    /// The refusal as an error value's message, with its names spelled through `symbols` and its
    /// types through `types`.
    pub fn display<'d, 'run>(
        &'d self,
        symbols: &'d SymbolInterner,
        types: &'d TypeRegistry<'run>,
    ) -> UntieableDisplay<'d, 'x, 'run> {
        UntieableDisplay {
            error: self,
            symbols,
            types,
        }
    }
}

/// An [`Untieable`] beside the interner and registry it renders through.
pub struct UntieableDisplay<'d, 'x, 'run> {
    error: &'d Untieable<'x>,
    symbols: &'d SymbolInterner,
    types: &'d TypeRegistry<'run>,
}

impl fmt::Display for UntieableDisplay<'_, '_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = |name: &BinderSymbol| self.symbols.display(name.symbol());
        let ktype = |handle: &KType| display_name(*handle, self.types, self.symbols);
        match self.error {
            Untieable::Type(error) => write!(f, "{}", error.display(self.symbols, self.types)),
            Untieable::Opaque { name: member } => {
                write!(f, "`{}` is bound to nothing a knot holds", name(member))
            }
            Untieable::Eager { name: member, .. } => {
                write!(f, "`{}` needs a part evaluated first", name(member))
            }
            Untieable::Key { rejected, .. } => match rejected {
                KeyRejected::NotAScalar(handle) => {
                    write!(f, "{} cannot be a dict key", ktype(handle))
                }
                KeyRejected::NaN => f.write_str("NaN cannot be a dict key"),
            },
            Untieable::Construction { refused, .. } => write!(
                f,
                "{}",
                refused_construction(refused, self.symbols, self.types)
            ),
            Untieable::TypeCycle { names } => {
                f.write_str("these bindings build values of no finite type:")?;
                for member in names.iter() {
                    write!(f, " `{}`", name(member))?;
                }
                Ok(())
            }
        }
    }
}

/// A construction the construction rule refused, as an error value's message: what a tie of a
/// data member and an evaluated construction both report.
pub fn refused_construction<'d, 'run>(
    refused: &'d ConstructionRefused,
    symbols: &'d SymbolInterner,
    types: &'d TypeRegistry<'run>,
) -> RefusedConstruction<'d, 'run> {
    RefusedConstruction {
        refused,
        symbols,
        types,
    }
}

/// A [`ConstructionRefused`] beside the interner and registry it renders through.
pub struct RefusedConstruction<'d, 'run> {
    refused: &'d ConstructionRefused,
    symbols: &'d SymbolInterner,
    types: &'d TypeRegistry<'run>,
}

impl fmt::Display for RefusedConstruction<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ktype = |handle: &KType| display_name(*handle, self.types, self.symbols);
        match self.refused {
            ConstructionRefused::NotConstructible(head) => {
                write!(f, "{} is not callable", ktype(head))
            }
            ConstructionRefused::Misfit {
                identity,
                representation,
                payload,
            } => write!(
                f,
                "{} cannot wrap {}: its representation is {}",
                ktype(identity),
                ktype(payload),
                ktype(representation)
            ),
            ConstructionRefused::Unsolved { family, payload } => write!(
                f,
                "{} cannot be solved against {}",
                ktype(family),
                ktype(payload)
            ),
        }
    }
}

/// The family of [`Knotted`], which `values` crosses a callable through.
pub struct KnottedFamily;

reattachable!(KnottedFamily => Knotted<'graph, 'cell>);

impl DropFree for KnottedFamily {}

/// A value that may hold a function.
pub type KValue<'graph, 'cell> = Value<'cell, Knotted<'graph, 'cell>>;

/// The family of [`KValue`].
pub type KValueFamily = ValueFamily<KnottedFamily>;

// A value crosses between cells, so its family carries the covariance witness the crossing doors
// ask for.
covariant!(KValueFamily);

/// A carrier of a [`KValue`].
pub type KValueCarrier<'graph, 'home> = ValueCarrier<'graph, 'home, KnottedFamily>;

/// An activation whose values may hold functions.
pub type KActivation<'graph, 'cell> = Activation<'graph, 'cell, KnottedFamily>;

/// The read half of a [`KActivation`]. The member is spelled out rather than left to the view's
/// default: a type holding one in a field is then covariant in `'cell`, which the projection the
/// default names would not be.
pub type KActivationView<'graph, 'cell> =
    ActivationView<'graph, 'cell, KnottedFamily, Knotted<'graph, 'cell>>;

/// A builtin table whose values may hold functions.
pub type KBuiltins<'graph, 'cell> = Builtins<'cell, Knotted<'graph, 'cell>>;
