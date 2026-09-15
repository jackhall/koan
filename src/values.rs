//! Koan's data values, laid down in a cell's region through `memory`'s shapes and nothing else.
//!
//! A [`Value`] is one `Copy` word: a scalar, a string or a quoted expression borrowed where it lives,
//! or a borrow of a per-kind resident struct — [`List`], [`Dict`], [`Record`], [`Tagged`],
//! [`TypeValue`] — or a callable, which `values` does not define: the arm holds a type parameter a
//! layer above closes, and [`Callable`] and [`CallableFamily`] are what `values` asks of it. The
//! parameter defaults to [`Nothing`], so a value spelled without it holds no callable. Every
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
mod crossing;
mod dict;
mod equality;
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

pub use admission::{admits, admits_part, part_ktype, satisfies};
pub use crossing::{COPY_RATIO, cross, cross_here, verdict};
pub use dict::{Dict, Key, KeyRejected};
pub use equality::Incomparable;
pub use list::List;
pub use record::Record;
pub use tagged::Tagged;
pub use type_value::TypeValue;
pub use weight::Weight;
pub use working::{WorkingExpression, WorkingPart};

use std::fmt;
use std::marker::PhantomData;

use crate::memory::{DropFree, Edge, Ready, Writer, reattachable};
use crate::parse::{LabelInterner, ProgramNode};
use crate::type_lattice::{KType, TypeNode, TypeRegistry};

/// What `values` asks of a closed callable at one region lifetime: its memoized type, what
/// rebuilding it at a destination writes, its surface, and the fellow member an edge of its own
/// names. A callable is `Copy`, so it carries no drop glue and may rest in a region.
pub trait Callable: Copy {
    fn ktype(&self) -> KType;

    /// The bytes a rebuild of this callable at a destination writes, past the value word holding it.
    fn weight(&self) -> Weight;

    fn render(
        &self,
        out: &mut impl fmt::Write,
        types: &TypeRegistry<'_>,
        labels: &LabelInterner,
    ) -> fmt::Result;

    /// The callable `edge` names among this one's own siblings.
    fn sibling(&self, edge: Edge) -> Self;
}

/// A closed callable across region lifetimes: its form at each, and the copy from one to another.
///
/// The copy is handed the deep copy of a value, so a callable holding values rebuilds them through
/// the one copy a crossing priced.
pub trait CallableFamily<'graph> {
    /// The closed callable at `'cell`: one type up to `'cell`, as an associated type is.
    type Closed<'cell>: Callable + 'cell
    where
        'graph: 'cell;

    /// `callable` rebuilt in `writer`'s region, each value it holds through `copy`.
    fn copy_into<'from, 'to>(
        writer: Writer<'to>,
        callable: &Self::Closed<'from>,
        copy: &mut DeepCopy<'_, 'graph, 'from, 'to, Self::Closed<'from>, Self::Closed<'to>>,
    ) -> Self::Closed<'to>
    where
        'graph: 'from,
        'graph: 'to;
}

/// The deep copy of a value from `'from` to `'to`, as a callable's family is handed it.
pub type DeepCopy<'copy, 'graph, 'from, 'to, X, Y> =
    dyn FnMut(&Value<'graph, 'from, X>) -> Value<'graph, 'to, Y> + 'copy;

/// The callable of a value that holds none: uninhabited, so its arm cannot be built.
#[derive(Clone, Copy, Debug)]
pub enum Nothing {}

impl Callable for Nothing {
    fn ktype(&self) -> KType {
        match *self {}
    }

    fn weight(&self) -> Weight {
        match *self {}
    }

    fn render(
        &self,
        _: &mut impl fmt::Write,
        _: &TypeRegistry<'_>,
        _: &LabelInterner,
    ) -> fmt::Result {
        match *self {}
    }

    fn sibling(&self, _: Edge) -> Self {
        match *self {}
    }
}

/// The family of [`Nothing`].
pub struct NoCallable;

impl<'graph> CallableFamily<'graph> for NoCallable {
    type Closed<'cell>
        = Nothing
    where
        'graph: 'cell;

    fn copy_into<'from, 'to>(
        _: Writer<'to>,
        callable: &Nothing,
        _: &mut DeepCopy<'_, 'graph, 'from, 'to, Nothing, Nothing>,
    ) -> Nothing
    where
        'graph: 'from,
        'graph: 'to,
    {
        match *callable {}
    }
}

/// The value family the cell graph carries: [`Value`] at a region lifetime, beside the graph's,
/// closed over the callables of `XF`.
pub struct ValueFamily<XF = NoCallable>(PhantomData<XF>);

reattachable!(ValueFamily<XF: CallableFamily<'graph>> => Value<'graph, 'cell, XF::Closed<'cell>>);

/// A callable is `Copy`, so no value carries drop glue.
impl<XF> DropFree for ValueFamily<XF> {}

/// A value carrier at rest in `'home` — what a placement door hands back and a step reads.
pub type ValueCarrier<'graph, 'home, XF = NoCallable> = Ready<'graph, 'home, ValueFamily<XF>>;

/// Koan's value: 24 bytes, `Copy`, with no type handle inline — every handle lives in the resident
/// struct an arm points at, or in the callable. The size is asserted below, so an arm that widens
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
    /// A callable, which a layer above `values` defines.
    Callable(X),
}

const _: () = assert!(size_of::<Value<'static, 'static>>() == 24);

impl<'graph, 'cell, X: Callable> Value<'graph, 'cell, X> {
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
            Value::Callable(callable) => callable.ktype(),
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
            Value::Callable(callable) => callable.weight(),
        }
    }

    /// Ascription stamping at an annotated boundary, after the caller has checked the value
    /// satisfies `declared`. A container against a declared node of its own kind takes `declared`
    /// as its handle over the same cells, so downstream dispatch sees the contract rather than the
    /// contents' incidental precision. A tagged value against a union takes the member it inhabits —
    /// the member naming the same constructor — and keeps its own handle when the union declares
    /// none. Everything else, and a value already of the declared type, passes through unwritten.
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

    pub fn as_callable(&self) -> Option<X> {
        match self {
            Value::Callable(callable) => Some(*callable),
            _ => None,
        }
    }
}

/// A string value whose bytes are written into the region.
pub fn text<'graph, 'cell, X>(writer: Writer<'cell>, text: &str) -> Value<'graph, 'cell, X> {
    Value::Str(writer.text(text))
}
