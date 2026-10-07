//! Koan's data values, laid down in a cell's region through `memory`'s shapes and nothing else.
//!
//! A [`Value`] is one `Copy` word: a scalar, a string borrowed where it lives, or a borrow of a
//! per-kind resident struct — [`List`], [`Dict`], [`Record`], [`Tagged`], [`TypeValue`] — or a
//! member of a knot, which `values` does not define: the arm holds a type parameter a layer above
//! closes, and [`Knotted`] and [`KnottedFamily`] are what `values` asks of it. A member is a
//! function, a quote's code, a module, a barrier, or a data node — a [`Circular`] container or
//! tagged value whose cells are [`Link`]s, each a value word or an edge to a sibling — which
//! equality, rendering and the deep copy read through ([`Resolved`]). The parameter defaults to [`Nothing`], so a value
//! spelled without it holds no knot member. Every
//! composite is born through a door that takes the region's
//! [`Writer`](crate::memory::Writer), stores its type as a memoized lattice handle and its copy
//! [`Weight`], and is `Drop`-free, so a region releases it whole.
//!
//! A value borrows at `'cell`: what a writer laid down, through `memory`'s
//! [`resident`](crate::memory::resident) and [`collect`](crate::memory::collect) shapes. What a
//! knot member holds of program storage sits inside the member. [`cross`] moves a value between
//! regions over the substrate's placement doors, rebuilding it under a copy, and [`verdict`] is the
//! copy-or-pin policy a graph is built with. Every read of a container or tagged value — equality,
//! rendering, the deep copy, a field read — goes through one door, [`Seen`] and [`Surface`], which
//! shows only what the value's type names. [`working`] is the scheduler's per-dispatch copy of an
//! expression, built in the executing cell's region.
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
pub mod digest;
mod equality;
mod link;
mod list;
mod lower;
mod record;
mod render;
mod surface;
mod tagged;
mod type_value;
mod weight;
pub mod working;

#[cfg(test)]
pub(crate) mod tests;

pub use admission::{
    ConstructionRefused, SealRefused, admits, admits_part, construction, dict_type, list_type,
    part_ktype, record_type, representation, satisfies, sealing, solves_identity,
};
pub use circular::{Circular, CodeView, Resolved};
pub use crossing::{COPY_RATIO, copy_severed, cross, cross_here, cross_view, verdict};
pub use dict::{Dict, Key, KeyRejected, kept_entries};
pub use digest::{ContentDigest, Digests};
pub use equality::Incomparable;
pub use link::Link;
pub use list::List;
pub use lower::literal_digest;
pub use record::Record;
pub use surface::{Seen, Surface};
pub use tagged::Tagged;
pub use type_value::TypeValue;
pub use weight::Weight;
pub use working::{WorkingExpression, WorkingPart};

use std::hash::Hash;
use std::marker::PhantomData;

use crate::memory::{DropFree, Edge, Ready, Writer, covariant, reattachable};
use crate::type_lattice::{DeclaredType, KType};

/// What `values` asks of a knot member at one region lifetime — a function or a data node of a
/// knot: its memoized type, what rebuilding its knot at a destination writes, the fellow member an
/// edge of its own names, and what the node holds. A member is `Copy`, so it carries no drop glue
/// and may rest in a region, and its equality and hash are node identity.
pub trait Knotted: Copy + Eq + Hash {
    /// The member's type: a function's own type — a scheme where a `FOR ALL` group quantifies
    /// it — and every other member's concrete type.
    fn ktype(&self) -> DeclaredType<KType>;

    /// The bytes a rebuild of this member's knot at a destination writes, past the value word
    /// holding it.
    fn weight(&self) -> Weight;

    /// The member's content digest, through `memo`: its knot's, beside its index there.
    fn digest(&self, memo: &mut Digests) -> ContentDigest;

    /// The member `edge` names among this one's own siblings.
    fn sibling(&self, edge: Edge) -> Self;

    /// This member's own edge in its knot.
    fn index(&self) -> Edge;

    /// The member at index 0 of this one's knot: one member names the whole knot, so a copy keys
    /// the knots it has rebuilt by it.
    fn root(&self) -> Self;

    /// What the node holds: a function or a quote's code, read through the links it holds; a module
    /// or a barrier, opaque to `values`; or a data node read through its cells.
    fn resolve<'a>(&self) -> Resolved<'a, Self>
    where
        Self: 'a;
}

/// A knot member across region lifetimes: its form at each, and the copy from one to another.
///
/// The copy is two walks over the knot. [`held`](Self::held) lists every value the knot holds, and
/// the crossing's worklist copies each of them; [`copy_into`](Self::copy_into) then ties the knot
/// once, asking for each finished copy in the same order. The worklist owns the recursion, so a
/// knot holding values that hold knots never nests a call per level.
pub trait KnottedFamily<'graph> {
    /// The member at `'cell`: one type up to `'cell`, as an associated type is.
    type Closed<'cell>: Knotted + 'cell
    where
        'graph: 'cell;

    /// Every value `member`'s knot holds, handed to `out` in the order `copy_into` asks `copy` for
    /// them: nodes in index order, and within a node exactly the values its rebuild asks for.
    fn held<'from>(
        member: &Self::Closed<'from>,
        out: &mut dyn FnMut(Value<'from, Self::Closed<'from>>),
    ) where
        'graph: 'from;

    /// `member`'s knot rebuilt in `writer`'s region, and the member at its own index. `copy`
    /// answers each value [`held`](Self::held) listed with its finished copy, in `held`'s order,
    /// and answers nothing else.
    fn copy_into<'from, 'to>(
        writer: Writer<'to>,
        member: &Self::Closed<'from>,
        copy: &mut DeepCopy<'_, 'from, 'to, Self::Closed<'from>, Self::Closed<'to>>,
    ) -> Self::Closed<'to>
    where
        'graph: 'from,
        'graph: 'to;
}

/// The finished copy of each value a member's knot holds, as its family's rebuild is handed it.
pub type DeepCopy<'copy, 'from, 'to, X, Y> = dyn FnMut(&Value<'from, X>) -> Value<'to, Y> + 'copy;

/// The knot member of a value that holds none: uninhabited, so its arm cannot be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Nothing {}

impl Knotted for Nothing {
    fn ktype(&self) -> DeclaredType<KType> {
        match *self {}
    }

    fn weight(&self) -> Weight {
        match *self {}
    }

    fn digest(&self, _: &mut Digests) -> ContentDigest {
        match *self {}
    }

    fn sibling(&self, _: Edge) -> Self {
        match *self {}
    }

    fn index(&self) -> Edge {
        match *self {}
    }

    fn root(&self) -> Self {
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

    fn held<'from>(member: &Nothing, _: &mut dyn FnMut(Value<'from, Nothing>))
    where
        'graph: 'from,
    {
        match *member {}
    }

    fn copy_into<'from, 'to>(
        _: Writer<'to>,
        member: &Nothing,
        _: &mut DeepCopy<'_, 'from, 'to, Nothing, Nothing>,
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

reattachable!(ValueFamily<XF: KnottedFamily<'graph>> => Value<'cell, XF::Closed<'cell>>);

/// A knot member is `Copy`, so no value carries drop glue.
impl<XF> DropFree for ValueFamily<XF> {}

/// A value carrier at rest in `'home` — what a placement door hands back and a step reads.
pub type ValueCarrier<'graph, 'home, XF = NoKnot> = Ready<'graph, 'home, ValueFamily<XF>>;

/// Koan's value: 24 bytes, `Copy`, with no type handle inline — every handle lives in the resident
/// struct an arm points at, or in the knot member. The size is asserted below, so an arm that widens
/// the word fails to compile.
#[derive(Clone, Copy, Debug)]
pub enum Value<'cell, X = Nothing> {
    Number(f64),
    Bool(bool),
    Null,
    /// Bytes written into the region the value lives in.
    Str(&'cell str),
    /// A first-class type: the handle and its own `OfKind` type.
    Type(&'cell TypeValue),
    List(&'cell List<'cell, X>),
    Dict(&'cell Dict<'cell, X>),
    Record(&'cell Record<'cell, X>),
    Tagged(&'cell Tagged<'cell, X>),
    /// A member of a knot a layer above `values` ties: a function, a quote's code, a module, a
    /// barrier, or a data node whose cells may name its siblings. [`Knotted::resolve`] says which.
    Knotted(X),
}

const _: () = assert!(size_of::<Value<'static>>() == 24);

/// Why a value's type is concrete where [`Value::concrete_ktype`] reads it: a quantified callable
/// is read only at the head of a call, or as a module member's binding, and an instance site reads
/// its instance, so no other read reaches one
/// ([scope/README.md § Resolution](scope/README.md#resolution)).
pub const CALL_ONLY: &str = "a quantified callable is read only at the head of a call, or as the \
                             binding of a module member; an instance site reads its instance";

impl<'cell, X: Knotted> Value<'cell, X> {
    /// The value's type: a constant for a leaf and the stored handle for everything else — a
    /// concrete type for every value but a quantified callable, which answers its scheme. Reads no
    /// registry and walks nothing.
    pub fn ktype(&self) -> DeclaredType<KType> {
        match self {
            Value::Number(_) => KType::NUMBER.into(),
            Value::Bool(_) => KType::BOOL.into(),
            Value::Null => KType::NULL.into(),
            Value::Str(_) => KType::STR.into(),
            Value::Type(value) => value.ktype().into(),
            Value::List(list) => list.ktype().into(),
            Value::Dict(dict) => dict.ktype().into(),
            Value::Record(record) => record.ktype().into(),
            Value::Tagged(tagged) => tagged.ktype().into(),
            Value::Knotted(member) => member.ktype(),
        }
    }

    /// The value's concrete type, where the load keeps a quantified callable out — every read but
    /// a call's head, which an instance site reads instantiated ([`CALL_ONLY`]).
    pub fn concrete_ktype(&self) -> KType {
        self.ktype().as_type().expect(CALL_ONLY)
    }

    /// What rebuilding this value at a destination writes: the word itself and what it points at.
    pub fn weight(&self) -> Weight {
        Weight::flat::<Self>().plus(self.referent_weight())
    }

    /// The value's content digest ([`digest`]), computed now: a scalar's from its payload, a
    /// composite's from its type and its contents, a knot member's its knot's.
    pub fn digest(&self) -> ContentDigest {
        self.digest_in(&mut Digests::default())
    }

    /// [`digest`](Self::digest) inside a demand already walking, through its `memo`.
    pub fn digest_in(&self, memo: &mut Digests) -> ContentDigest {
        use digest::{DigestHasher, Tag};
        match self {
            Value::Number(number) => DigestHasher::new(Tag::Number)
                .feed(number.to_bits())
                .finished(),
            Value::Bool(flag) => DigestHasher::new(Tag::Bool).feed(flag).finished(),
            Value::Null => DigestHasher::new(Tag::Null).finished(),
            Value::Str(text) => DigestHasher::new(Tag::Str).text(text.as_bytes()).finished(),
            Value::Type(value) => DigestHasher::new(Tag::Type).feed(value.handle()).finished(),
            Value::List(list) => list.digest(memo),
            Value::Dict(dict) => dict.digest(memo),
            Value::Record(record) => record.digest(memo),
            Value::Tagged(tagged) => tagged.digest(memo),
            Value::Knotted(member) => member.digest(memo),
        }
    }

    /// The part of [`weight`](Self::weight) past the word — what a holder that stores the word
    /// inline adds for it.
    pub(crate) fn referent_weight(&self) -> Weight {
        match self {
            Value::Number(_) | Value::Bool(_) | Value::Null => Weight::ZERO,
            Value::Str(text) => Weight::text(text.len()),
            Value::Type(_) => Weight::flat::<TypeValue>(),
            Value::List(list) => list.weight(),
            Value::Dict(dict) => dict.weight(),
            Value::Record(record) => record.weight(),
            Value::Tagged(tagged) => tagged.weight(),
            Value::Knotted(member) => member.weight(),
        }
    }
}

impl<'cell, X: Copy> Value<'cell, X> {
    pub fn as_str(&self) -> Option<&'cell str> {
        match self {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_type(&self) -> Option<&'cell TypeValue> {
        match self {
            Value::Type(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&'cell List<'cell, X>> {
        match self {
            Value::List(list) => Some(list),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&'cell Dict<'cell, X>> {
        match self {
            Value::Dict(dict) => Some(dict),
            _ => None,
        }
    }

    pub fn as_record(&self) -> Option<&'cell Record<'cell, X>> {
        match self {
            Value::Record(record) => Some(record),
            _ => None,
        }
    }

    pub fn as_tagged(&self) -> Option<&'cell Tagged<'cell, X>> {
        match self {
            Value::Tagged(tagged) => Some(tagged),
            _ => None,
        }
    }
}

impl<'cell, X: Knotted> Value<'cell, X> {
    /// The function this value is, if it is one — a barrier over one included, which calls the
    /// same way.
    pub fn as_callable(&self) -> Option<X> {
        match self {
            Value::Knotted(member)
                if matches!(
                    member.resolve(),
                    Resolved::Function { .. } | Resolved::Barrier
                ) =>
            {
                Some(*member)
            }
            _ => None,
        }
    }

    /// The quote's code this value is, if it is one.
    pub fn as_code(&self) -> Option<X> {
        match self {
            Value::Knotted(member) if matches!(member.resolve(), Resolved::Code(_)) => {
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

    /// The knot member this value is when its node is opaque to `values` — a module or a barrier.
    /// Equality refuses such a pair.
    pub fn as_opaque(&self) -> Option<X> {
        match self {
            Value::Knotted(member) => match member.resolve() {
                Resolved::Module | Resolved::Barrier => Some(*member),
                Resolved::Function { .. } | Resolved::Circular(_) | Resolved::Code(_) => None,
            },
            _ => None,
        }
    }

    /// The data node this value is, if it is one, beside the member it is read through.
    pub fn as_circular(&self) -> Option<(X, Circular<'cell, X>)>
    where
        X: 'cell,
    {
        match self {
            Value::Knotted(member) => match member.resolve() {
                Resolved::Circular(circular) => Some((*member, circular)),
                _ => None,
            },
            _ => None,
        }
    }
}

/// A string value whose bytes are written into the region.
pub fn text<'cell, X>(writer: Writer<'cell>, text: &str) -> Value<'cell, X> {
    Value::Str(writer.text(text))
}
