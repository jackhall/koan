//! Koan's data values, laid down in a cell's region through `memory`'s shapes and nothing else.
//!
//! A [`Value`] is one `Copy` word: a scalar, a string or a quoted expression borrowed where it lives,
//! or a borrow of a per-kind resident struct — [`List`], [`Dict`], [`Record`], [`Tagged`],
//! [`TypeValue`]. Every composite is born through a door that takes the region's
//! [`Writer`](crate::memory::Writer), stores its type as a memoized lattice handle and its copy
//! [`Weight`], and is `Drop`-free, so a region releases it whole.
//!
//! A value borrows at two lifetimes. What it holds of program storage — a quoted expression's
//! node — sits at `'graph`, which the cell graph never retypes; what a writer laid down sits at
//! `'cell`. [`cross`] moves a value between regions over the substrate's placement doors, rebuilding
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
pub use list::List;
pub use record::Record;
pub use tagged::Tagged;
pub use type_value::TypeValue;
pub use weight::Weight;
pub use working::{WorkingExpression, WorkingPart};

use crate::memory::{DropFree, Ready, Writer, reattachable};
use crate::parse::ProgramNode;
use crate::type_lattice::{KType, TypeNode, TypeRegistry};

/// The value family the cell graph carries: [`Value`] at a region lifetime, beside the graph's.
pub struct ValueFamily;

reattachable!(ValueFamily => Value<'graph, 'cell>);

impl DropFree for ValueFamily {}

/// A value carrier at rest in `'home` — what a placement door hands back and a step reads.
pub type ValueCarrier<'graph, 'home> = Ready<'graph, 'home, ValueFamily>;

/// Koan's value: 24 bytes, `Copy`, with no type handle inline — every handle lives in the resident
/// struct an arm points at. The size is asserted below, so an arm that widens the word fails to
/// compile.
#[derive(Clone, Copy, Debug)]
pub enum Value<'graph, 'cell> {
    Number(f64),
    Bool(bool),
    Null,
    /// Bytes written into the region the value lives in.
    Str(&'cell str),
    /// A `#(...)` body: a node in program storage, the same node on either side of every crossing.
    Expression(ProgramNode<'graph>),
    /// A first-class type: the handle and its own `OfKind` type.
    Type(&'cell TypeValue),
    List(&'cell List<'graph, 'cell>),
    Dict(&'cell Dict<'graph, 'cell>),
    Record(&'cell Record<'graph, 'cell>),
    Tagged(&'cell Tagged<'graph, 'cell>),
}

const _: () = assert!(size_of::<Value<'static, 'static>>() == 24);

impl<'graph, 'cell> Value<'graph, 'cell> {
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
        }
    }

    /// What rebuilding this value at a destination writes: the word itself and what it points at.
    pub fn weight(&self) -> Weight {
        Weight::flat::<Value<'static, 'static>>().plus(self.referent_weight())
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
        }
    }

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

    pub fn as_list(&self) -> Option<&'cell List<'graph, 'cell>> {
        match self {
            Value::List(list) => Some(list),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&'cell Dict<'graph, 'cell>> {
        match self {
            Value::Dict(dict) => Some(dict),
            _ => None,
        }
    }

    pub fn as_record(&self) -> Option<&'cell Record<'graph, 'cell>> {
        match self {
            Value::Record(record) => Some(record),
            _ => None,
        }
    }

    pub fn as_tagged(&self) -> Option<&'cell Tagged<'graph, 'cell>> {
        match self {
            Value::Tagged(tagged) => Some(tagged),
            _ => None,
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
    ) -> Value<'graph, 'cell> {
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

/// Store one value in the region and hand back its resident borrow — `fill` at length one.
pub fn resident<'cell, T: Copy>(writer: Writer<'cell>, value: T) -> &'cell T {
    &writer.fill(1, |_| value)[0]
}

/// Copy an exact-length run into the region — `fill` driven by the iterator, with no growth path.
pub fn collect<'cell, T>(
    writer: Writer<'cell>,
    items: impl ExactSizeIterator<Item = T>,
) -> &'cell [T] {
    let mut items = items;
    writer.fill(items.len(), |_| {
        items
            .next()
            .expect("an exact-size iterator yields its reported length")
    })
}

/// A string value whose bytes are written into the region.
pub fn text<'graph, 'cell>(writer: Writer<'cell>, text: &str) -> Value<'graph, 'cell> {
    Value::Str(writer.text(text))
}
