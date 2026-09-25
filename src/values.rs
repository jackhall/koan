//! Koan's data values, laid down in a cell's region through `memory`'s shapes and nothing else.
//!
//! A [`Value`] is one `Copy` word: a scalar, a string or a quoted expression borrowed where it lives,
//! or a borrow of a per-kind resident struct — [`List`], [`Dict`], [`Record`], [`Tagged`],
//! [`TypeValue`] — or a member of a knot, which `values` does not define: the arm holds a type
//! parameter a layer above closes, and [`Knotted`] and [`KnottedFamily`] are what `values` asks of
//! it. A member is a function, opaque here, or a data node — a [`Circular`] container or tagged
//! value whose cells are [`Link`]s, each a value word or an edge to a sibling — which equality,
//! rendering and the deep copy read through. The parameter defaults to [`Nothing`], so a value
//! spelled without it holds no knot member. Every
//! composite is born through a door that takes the region's
//! [`Writer`](crate::memory::Writer), stores its type as a memoized lattice handle and its copy
//! [`Weight`], and is `Drop`-free, so a region releases it whole.
//!
//! A value borrows at two lifetimes. What it holds of program storage — a quoted expression's
//! node — sits at `'graph`, which the cell graph never retypes; what a writer laid down sits at
//! `'cell`, through `memory`'s [`resident`](crate::memory::resident) and
//! [`collect`](crate::memory::collect) shapes. [`cross`] moves a value between regions over the substrate's placement doors, rebuilding
//! only the `'cell` parts under a copy, and [`verdict`] is the copy-or-pin policy a graph is built
//! with. [`working`] is the scheduler's per-dispatch expression form, built in the executing cell's
//! region.
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::memory`,
//! `crate::parse`, `crate::source` and `crate::type_lattice`, and nothing else in the crate;
//! [`tests::boundary`] reads the source to hold it there.
//!
//! See [values/README.md](values/README.md).

mod admission;
mod circular;
mod crossing;
mod dict;
mod equality;
mod link;
mod list;
mod lower;
mod record;
mod render;
mod tagged;
mod type_value;
mod weight;
pub mod working;

#[cfg(test)]
mod tests;

pub use admission::{
    ConstructionRefused, SealRefused, admits, admits_part, construction, dict_type, list_type,
    part_ktype, record_type, satisfies, sealing, solves_identity, unsealed,
};
pub use circular::{Circular, Resolved};
pub use crossing::{COPY_RATIO, copy_severed, cross, cross_here, cross_view, verdict};
pub use dict::{Dict, Key, KeyRejected, kept_entries};
pub use equality::Incomparable;
pub use link::Link;
pub use list::List;
pub use record::Record;
pub use tagged::Tagged;
pub use type_value::TypeValue;
pub use weight::Weight;
pub use working::{WorkingExpression, WorkingPart};

use std::hash::Hash;
use std::marker::PhantomData;

use crate::memory::{DropFree, Edge, Ready, Writer, covariant, reattachable};
use crate::parse::ProgramNode;
use crate::type_lattice::{KType, TypeNode, TypeRegistry};

/// What `values` asks of a knot member at one region lifetime — a function or a data node of a
/// knot: its memoized type, what rebuilding its knot at a destination writes, the fellow member an
/// edge of its own names, and what the node holds. A member is `Copy`, so it carries no drop glue
/// and may rest in a region, and its equality and hash are node identity.
pub trait Knotted: Copy + Eq + Hash {
    fn ktype(&self) -> KType;

    /// The bytes a rebuild of this member's knot at a destination writes, past the value word
    /// holding it.
    fn weight(&self) -> Weight;

    /// The member `edge` names among this one's own siblings.
    fn sibling(&self, edge: Edge) -> Self;

    /// What the node holds: a function or a module, both opaque to `values`, or a data node read
    /// through its cells.
    fn resolve<'a>(&self) -> Resolved<'a, Self>
    where
        Self: 'a;
}

/// A knot member across region lifetimes: its form at each, and the copy from one to another.
///
/// The copy is handed the deep copy of a value, so a member holding values rebuilds them through
/// the one copy a crossing priced.
pub trait KnottedFamily<'graph> {
    /// The member at `'cell`: one type up to `'cell`, as an associated type is.
    type Closed<'cell>: Knotted + 'cell
    where
        'graph: 'cell;

    /// `member`'s knot rebuilt in `writer`'s region, each value it holds through `copy`, and the
    /// member at its own index.
    fn copy_into<'from, 'to>(
        writer: Writer<'to>,
        member: &Self::Closed<'from>,
        copy: &mut DeepCopy<'_, 'graph, 'from, 'to, Self::Closed<'from>, Self::Closed<'to>>,
    ) -> Self::Closed<'to>
    where
        'graph: 'from,
        'graph: 'to;
}

/// The deep copy of a value from `'from` to `'to`, as a member's family is handed it.
pub type DeepCopy<'copy, 'graph, 'from, 'to, X, Y> =
    dyn FnMut(&Value<'graph, 'from, X>) -> Value<'graph, 'to, Y> + 'copy;

/// The knot member of a value that holds none: uninhabited, so its arm cannot be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Nothing {}

impl Knotted for Nothing {
    fn ktype(&self) -> KType {
        match *self {}
    }

    fn weight(&self) -> Weight {
        match *self {}
    }

    fn sibling(&self, _: Edge) -> Self {
        match *self {}
    }

    fn resolve<'a>(&self) -> Resolved<'a, Self> {
        match *self {}
    }
}

/// The family of [`Nothing`].
pub struct NoKnot;

impl<'graph> KnottedFamily<'graph> for NoKnot {
    type Closed<'cell>
        = Nothing
    where
        'graph: 'cell;

    fn copy_into<'from, 'to>(
        _: Writer<'to>,
        member: &Nothing,
        _: &mut DeepCopy<'_, 'graph, 'from, 'to, Nothing, Nothing>,
    ) -> Nothing
    where
        'graph: 'from,
        'graph: 'to,
    {
        match *member {}
    }
}

// A knot-free value crosses between cells like any other, so its family carries the covariance
// witness too.
covariant!(ValueFamily<NoKnot>);

/// The value family the cell graph carries: [`Value`] at a region lifetime, beside the graph's,
/// closed over the knot members of `XF`.
pub struct ValueFamily<XF = NoKnot>(PhantomData<XF>);

reattachable!(ValueFamily<XF: KnottedFamily<'graph>> => Value<'graph, 'cell, XF::Closed<'cell>>);

/// A knot member is `Copy`, so no value carries drop glue.
impl<XF> DropFree for ValueFamily<XF> {}

/// A value carrier at rest in `'home` — what a placement door hands back and a step reads.
pub type ValueCarrier<'graph, 'home, XF = NoKnot> = Ready<'graph, 'home, ValueFamily<XF>>;

/// Koan's value: 24 bytes, `Copy`, with no type handle inline — every handle lives in the resident
/// struct an arm points at, or in the knot member. The size is asserted below, so an arm that widens
/// the word fails to compile.
#[derive(Clone, Copy, Debug)]
pub enum Value<'graph, 'cell, X = Nothing> {
    Number(f64),
    Bool(bool),
    Null,
    /// Bytes written into the region the value lives in.
    Str(&'cell str),
    /// A `#(...)` body: a node in program storage, the same node on either side of every crossing.
    Expression(ProgramNode<'graph>),
    /// A first-class type: the handle and its own `OfKind` type.
    Type(&'cell TypeValue),
    List(&'cell List<'graph, 'cell, X>),
    Dict(&'cell Dict<'graph, 'cell, X>),
    Record(&'cell Record<'graph, 'cell, X>),
    Tagged(&'cell Tagged<'graph, 'cell, X>),
    /// A member of a knot a layer above `values` ties: a function, or a data node whose cells may
    /// name its siblings. [`Knotted::resolve`] says which.
    Knotted(X),
}

const _: () = assert!(size_of::<Value<'static, 'static>>() == 24);

impl<'graph, 'cell, X: Knotted> Value<'graph, 'cell, X> {
    /// The value's type: a constant for a leaf, the stored handle for everything else. Reads no
    /// registry and walks nothing.
    pub fn ktype(&self) -> KType {
        match self {
            Value::Number(_) => KType::NUMBER,
            Value::Bool(_) => KType::BOOL,
            Value::Null => KType::NULL,
            Value::Str(_) => KType::STR,
            Value::Expression(_) => KType::KEXPRESSION,
            Value::Type(value) => value.ktype(),
            Value::List(list) => list.ktype(),
            Value::Dict(dict) => dict.ktype(),
            Value::Record(record) => record.ktype(),
            Value::Tagged(tagged) => tagged.ktype(),
            Value::Knotted(member) => member.ktype(),
        }
    }

    /// What rebuilding this value at a destination writes: the word itself and what it points at.
    pub fn weight(&self) -> Weight {
        Weight::flat::<Self>().plus(self.referent_weight())
    }

    /// The part of [`weight`](Self::weight) past the word — what a holder that stores the word
    /// inline adds for it.
    pub(crate) fn referent_weight(&self) -> Weight {
        match self {
            Value::Number(_) | Value::Bool(_) | Value::Null | Value::Expression(_) => Weight::ZERO,
            Value::Str(text) => Weight::text(text.len()),
            Value::Type(_) => Weight::flat::<TypeValue>(),
            Value::List(list) => list.weight(),
            Value::Dict(dict) => dict.weight(),
            Value::Record(record) => record.weight(),
            Value::Tagged(tagged) => tagged.weight(),
            Value::Knotted(member) => member.weight(),
        }
    }

    /// Ascription stamping at an annotated boundary, after the caller has checked the value
    /// satisfies `declared`. A container against a declared node of its own kind takes `declared`
    /// as its handle over the same cells, so downstream dispatch sees the contract rather than the
    /// contents' incidental precision. A tagged value against a union takes the member it inhabits —
    /// the member naming the same constructor — and keeps its own handle when the union declares
    /// none. Everything else, and a value already of the declared type, passes through unwritten —
    /// a knot member among them: a knot never grows a node, so a data node is never restamped.
    pub fn retyped(
        self,
        writer: Writer<'cell>,
        declared: KType,
        types: &TypeRegistry<'_>,
    ) -> Value<'graph, 'cell, X> {
        if declared == self.ktype() {
            return self;
        }
        match (self, types.node(declared)) {
            (Value::List(list), TypeNode::List { .. }) => {
                Value::List(list.with_type(writer, declared))
            }
            (Value::Dict(dict), TypeNode::Dict { .. }) => {
                Value::Dict(dict.with_type(writer, declared))
            }
            (Value::Record(record), TypeNode::Record { .. }) => {
                Value::Record(record.with_type(writer, declared))
            }
            (Value::Tagged(tagged), TypeNode::Union { members }) => {
                let constructor = |handle: KType| match types.node(handle) {
                    TypeNode::ConstructorApply { constructor, .. } => constructor,
                    _ => handle,
                };
                let inhabited = constructor(tagged.ktype());
                match members
                    .iter()
                    .find(|member| constructor(**member) == inhabited)
                {
                    Some(member) => Value::Tagged(tagged.with_type(writer, *member)),
                    None => self,
                }
            }
            (other, _) => other,
        }
    }
}

impl<'graph, 'cell, X: Copy> Value<'graph, 'cell, X> {
    pub fn as_str(&self) -> Option<&'cell str> {
        match self {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_expression(&self) -> Option<ProgramNode<'graph>> {
        match self {
            Value::Expression(node) => Some(*node),
            _ => None,
        }
    }

    pub fn as_type(&self) -> Option<&'cell TypeValue> {
        match self {
            Value::Type(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&'cell List<'graph, 'cell, X>> {
        match self {
            Value::List(list) => Some(list),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&'cell Dict<'graph, 'cell, X>> {
        match self {
            Value::Dict(dict) => Some(dict),
            _ => None,
        }
    }

    pub fn as_record(&self) -> Option<&'cell Record<'graph, 'cell, X>> {
        match self {
            Value::Record(record) => Some(record),
            _ => None,
        }
    }

    pub fn as_tagged(&self) -> Option<&'cell Tagged<'graph, 'cell, X>> {
        match self {
            Value::Tagged(tagged) => Some(tagged),
            _ => None,
        }
    }
}

impl<'graph, 'cell, X: Knotted> Value<'graph, 'cell, X> {
    /// The function this value is, if it is one.
    pub fn as_callable(&self) -> Option<X> {
        match self {
            Value::Knotted(member) if matches!(member.resolve(), Resolved::Function) => {
                Some(*member)
            }
            _ => None,
        }
    }

    /// The module this value is, if it is one.
    pub fn as_module(&self) -> Option<X> {
        match self {
            Value::Knotted(member) if matches!(member.resolve(), Resolved::Module) => Some(*member),
            _ => None,
        }
    }

    /// The knot member this value is when its node is opaque to `values` — a function or a module.
    /// Equality refuses such a pair and rendering writes its type's name.
    pub fn as_opaque(&self) -> Option<X> {
        match self {
            Value::Knotted(member) => match member.resolve() {
                Resolved::Function | Resolved::Module => Some(*member),
                Resolved::Circular(_) => None,
            },
            _ => None,
        }
    }

    /// The data node this value is, if it is one, beside the member it is read through.
    pub fn as_circular(&self) -> Option<(X, Circular<'cell, 'cell, X>)>
    where
        X: 'cell,
    {
        match self {
            Value::Knotted(member) => match member.resolve() {
                Resolved::Circular(circular) => Some((*member, circular)),
                Resolved::Function | Resolved::Module => None,
            },
            _ => None,
        }
    }
}

/// A string value whose bytes are written into the region.
pub fn text<'graph, 'cell, X>(writer: Writer<'cell>, text: &str) -> Value<'graph, 'cell, X> {
    Value::Str(writer.text(text))
}
