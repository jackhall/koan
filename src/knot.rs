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
//! [`Knotted`] is sixteen bytes, so a value holding one stays one twenty-four-byte word. Every node
//! of a knot points at its [`KnotFacts`], written once beside them: the weight a rebuild of the
//! whole knot writes, and the knot's content digest over each node's content, an edge to a sibling
//! by its index — so values that reach one another digest as one knot, and a member's digest is
//! its knot's beside its index. A member copies at a crossing by re-tying its whole knot at the
//! destination, each held value deep-copied, each edge carried verbatim and the facts with them,
//! priced by the knot's memoized weight under the ordinary verdict.
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

use crate::memory::{
    BumpAllocator, DropFree, Edge, Member, Writer, covariant, reattachable, resident,
};
use crate::parse::ExpressionPart;
use crate::scope::Elaboration;
use crate::scope::{Activation, ActivationView, BodyShape, Builtins, CaptureSlot, Site};
use crate::symbols::{BinderSymbol, SymbolInterner};
use crate::type_lattice::{DeclaredType, KType, TypeRegistry, display_name};
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{
    self, Circular, ConstructionRefused, ContentDigest, KeyRejected, Link, Resolved, Value,
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
        /// What every node of the knot shares.
        facts: &'cell KnotFacts,
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

/// What every node of one knot shares, written once beside its nodes and pointed at by each: what
/// rebuilding the whole knot writes, and the knot's content digest — a [`Tag::Knot`] over its node
/// count and each node's content in index order, an edge to a sibling hashed as its index, so
/// values that reach one another digest as the one knot they are tied into.
#[derive(Clone, Copy, Debug)]
pub struct KnotFacts {
    weight: Weight,
    digest: ContentDigest,
}

/// Feed `hasher` the digest of each capture of `links`, a closure of `shape`, that its code digest
/// does not name — every one but a read of the program's top level, and every type capture — beside
/// its slot: what a closure's or a module's content composes over its code.
fn composed(hasher: &mut DigestHasher, shape: &BodyShape<'_>, links: &[Link<'_, Knotted<'_, '_>>]) {
    for (index, link) in links.iter().enumerate() {
        if shape.composes(CaptureSlot(index as u32)) {
            hasher.count(index).digest(link.digest());
        }
    }
}

impl KnotFacts {
    /// The facts of a knot whose nodes weigh `weight` and whose contents, in index order, are
    /// `contents`, laid down in `writer`'s region: a rebuild writes the facts too, so they count.
    pub(crate) fn laid<'cell>(
        writer: Writer<'cell>,
        weight: Weight,
        contents: &[ContentDigest],
    ) -> &'cell KnotFacts {
        let weight = weight.plus(Weight::flat::<KnotFacts>());
        let mut hasher = DigestHasher::new(Tag::Knot);
        hasher.count(contents.len());
        for content in contents {
            hasher.digest(*content);
        }
        resident(
            writer,
            KnotFacts {
                weight,
                digest: hasher.finished(),
            },
        )
    }

    /// The same facts laid down in `writer`'s region — a copy's, which keeps its digest.
    pub(crate) fn copied<'to>(&self, writer: Writer<'to>) -> &'to KnotFacts {
        resident(writer, *self)
    }

    /// What rebuilding the whole knot writes.
    pub fn weight(&self) -> Weight {
        self.weight
    }

    /// The knot's content digest.
    pub fn digest(&self) -> ContentDigest {
        self.digest
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

    /// What every node of this member's knot shares; `None` for a builtin, whose record is its
    /// own.
    pub fn facts(self) -> Option<&'cell KnotFacts> {
        Some(match self.node() {
            Node::Function(function) => function.facts(),
            Node::Builtin(_) => return None,
            Node::Data { facts, .. } => facts,
            Node::Module(module) => module.facts(),
            Node::Coerced(coerced) => coerced.facts(),
            Node::Code(code) => code.facts(),
        })
    }

    /// The barrier this member sits behind, if it is a coerced function's node.
    pub fn coerced(self) -> Option<&'cell Coerced<'graph, 'cell>> {
        match self.node() {
            Node::Coerced(coerced) => Some(coerced),
            _ => None,
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
        match self.facts() {
            Some(facts) => facts.weight(),
            None => BuiltinFunction::knot_weight(),
        }
    }

    /// A builtin's digest is its overload's — its native and its shape — and every other member's
    /// is its knot's beside its index.
    fn digest(&self) -> ContentDigest {
        match (self.node(), self.facts()) {
            (Node::Builtin(builtin), _) => DigestHasher::new(Tag::Builtin)
                .feed(builtin.id())
                .feed(builtin.ktype())
                .finished(),
            (_, Some(facts)) => DigestHasher::new(Tag::Member)
                .digest(facts.digest())
                .count(self.0.index().index() as usize)
                .finished(),
            (_, None) => unreachable!("only a builtin's node has no knot facts"),
        }
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
