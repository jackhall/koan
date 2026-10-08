//! A knot's data node, read through the member holding it: a list, dict, record or tagged value
//! whose cells are [`Link`]s, so a cell may name a sibling node of the same knot. What a member holds
//! is its [`Resolved`] form: beside a data node, a function or a quote's code, each read through
//! the links it holds, or a module or a barrier, opaque to `values`.
//!
//! A data node is read like a plain value of its kind, through [the door](super::surface), its
//! links resolved through the member holding it; its [content](Circular::content) inside its knot's
//! digest is read through the door at its memo, an edge kept as its index and each value link's
//! digest handed in by the walk, which [`Circular::content_parts`] lists them for.
//! [`Circular::held`] lists the values a data node's [`copied`](Circular::copied) asks for, in its
//! order, for a knot family's [`held`](super::KnottedFamily::held). The lifetimes of a node's run
//! shorten to the borrow a member hands out, which is sound because every resident is covariant in
//! `'cell` and so is every member.

use crate::memory::{BumpAllocator, Writer, collect};
use crate::parse::KExpression;
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, TypeRegistry};

use super::digest::{ContentDigest, DigestHasher, Tag, edge, scalar};
use super::surface::Parts;
use super::{
    DeepCopy, Dict, Knotted, Link, List, Nothing, Record, Seen, Surface, Tagged, Value, Weight,
};

/// What a knot member holds.
#[derive(Clone, Copy)]
pub enum Resolved<'a, X> {
    /// A function, rendered as its type's name. It compares by `identity` — one per `FN`, `EXPR` or
    /// `OP` written, so a copy keeps it — by the solution an instance of a quantified function was
    /// made at, empty for anything else, and by its closure bindings, read through the member.
    Function {
        identity: usize,
        instance: &'a [KType],
        closure: &'a [Link<'a, X>],
    },
    /// A module: opaque to `values`, incomparable, rendered as its type's name.
    Module,
    /// A function behind an opaque view's barrier: called as a function, opaque as a module.
    Barrier,
    /// A data node.
    Circular(Circular<'a, X>),
    /// A quote's code.
    Code(CodeView<'a, X>),
}

/// A quote's code as `values` reads it: its body as written, and the names its code binds, each
/// beside a link read through the member — its `$` names and the holes a `USING` filled, each run
/// sorted by name.
pub struct CodeView<'a, X> {
    pub body: &'a KExpression<'a>,
    pub bound: &'a [(BinderSymbol, Link<'a, X>)],
    pub supplied: &'a [(BinderSymbol, Link<'a, X>)],
}

impl<X> Clone for CodeView<'_, X> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<X> Copy for CodeView<'_, X> {}

/// A data node of a knot. `Copy`.
pub enum Circular<'cell, X = Nothing> {
    List(&'cell List<'cell, X, Link<'cell, X>>),
    Dict(&'cell Dict<'cell, X, Link<'cell, X>>),
    Record(&'cell Record<'cell, X, Link<'cell, X>>),
    Tagged(&'cell Tagged<'cell, X, Link<'cell, X>>),
}

impl<X> Clone for Circular<'_, X> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<X> Copy for Circular<'_, X> {}

impl<'cell, X: Knotted> Circular<'cell, X> {
    /// The node's memoized type, exact and finite: the tie derived it.
    pub fn ktype(&self) -> KType {
        match self {
            Circular::List(list) => list.ktype(),
            Circular::Dict(dict) => dict.ktype(),
            Circular::Record(record) => record.ktype(),
            Circular::Tagged(tagged) => tagged.ktype(),
        }
    }

    /// What this node's own resident writes — not its knot's.
    pub fn weight(&self) -> Weight {
        match self {
            Circular::List(list) => list.weight(),
            Circular::Dict(dict) => dict.weight(),
            Circular::Record(record) => record.weight(),
            Circular::Tagged(tagged) => tagged.weight(),
        }
    }

    /// Every value link of `holder`'s data node, seen through [the door](super::surface) at the
    /// node's memo, in the order [`content`](Self::content) asks for their digests: what its
    /// knot [holds](super::Knotted::held) of this node.
    pub fn content_parts<'a>(
        holder: X,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        out: &mut dyn FnMut(Seen<'a, X>),
    ) where
        X: 'a,
    {
        let surface = opened(holder, types, scratch);
        for at in 0..surface.len() {
            if surface.edge(at).is_none() {
                out(surface.child(at, types, scratch));
            }
        }
    }

    /// The content of `holder`'s data node inside its knot's digest, read through the door at the
    /// node's memo: its memo, its kind, and each part the memo shows — a dict's key beside it, a
    /// record's name beside it — an edge by its index and a value link by its digest, which
    /// `parts` answers in [`content_parts`](Self::content_parts)' order.
    pub fn content(
        holder: X,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        parts: &mut dyn FnMut() -> ContentDigest,
    ) -> ContentDigest {
        let surface = opened(holder, types, scratch);
        let kind = match surface.parts() {
            Parts::List { .. } => Tag::List,
            Parts::Dict { .. } => Tag::Dict,
            Parts::Record { .. } => Tag::Record,
            Parts::Tagged { .. } => Tag::Tagged,
        };
        let mut hasher = DigestHasher::new(Tag::Data);
        hasher.feed(surface.ktype()).tag(kind).count(surface.len());
        for at in 0..surface.len() {
            match kind {
                Tag::Dict => {
                    hasher.digest(scalar(surface.key(at).value::<Nothing>()));
                }
                Tag::Record => {
                    hasher.feed(surface.name(at));
                }
                _ => {}
            }
            hasher.digest(surface.edge(at).map_or_else(&mut *parts, edge));
        }
        hasher.finished()
    }

    /// Every value this node's links hold, in the order [`copied`](Self::copied) asks for them.
    pub fn held(&self, out: &mut dyn FnMut(Value<'cell, X>)) {
        let mut links = |cells: &[Link<'cell, X>]| {
            for link in cells {
                if let Link::Value(value) = link {
                    out(*value);
                }
            }
        };
        match *self {
            Circular::List(list) => links(list.cells()),
            Circular::Dict(dict) => links(dict.cells()),
            Circular::Record(record) => links(record.cells()),
            Circular::Tagged(tagged) => links(std::slice::from_ref(tagged.payload())),
        }
    }

    /// This node rebuilt in `writer`'s region: each link through [`Link::copied`], keys rehomed,
    /// names collected, type and weight carried over.
    pub fn copied<'to, Y: Knotted>(
        &self,
        writer: Writer<'to>,
        copy: &mut DeepCopy<'_, 'cell, 'to, X, Y>,
    ) -> Circular<'to, Y> {
        let mut copy = |value: &Value<'cell, X>| copy(value);
        match *self {
            Circular::List(list) => {
                let source = list.cells();
                let cells = writer.fill(source.len(), |at| source[at].copied(&mut copy));
                Circular::List(List::from_run(writer, cells, list.ktype(), list.weight()))
            }
            Circular::Dict(dict) => {
                let (source_keys, source) = (dict.keys(), dict.cells());
                let keys = writer.fill(source_keys.len(), |at| source_keys[at].rehomed(writer));
                let cells = writer.fill(source.len(), |at| source[at].copied(&mut copy));
                Circular::Dict(Dict::from_runs(
                    writer,
                    keys,
                    cells,
                    dict.ktype(),
                    dict.weight(),
                ))
            }
            Circular::Record(record) => {
                let source = record.cells();
                let names = collect(writer, record.names().iter().copied());
                let cells = writer.fill(source.len(), |at| source[at].copied(&mut copy));
                Circular::Record(Record::from_runs(
                    writer,
                    names,
                    cells,
                    record.ktype(),
                    record.weight(),
                ))
            }
            Circular::Tagged(tagged) => Circular::Tagged(Tagged::from_payload(
                writer,
                tagged.payload().copied(&mut copy),
                tagged.ktype(),
                tagged.weight(),
            )),
        }
    }
}

/// `holder`'s data node opened through the door at its memo.
fn opened<'x, 'a, X: Knotted + 'a>(
    holder: X,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> Surface<'x, 'a, X> {
    Seen::of(Value::Knotted(holder))
        .surface(types, scratch)
        .expect("a data node opens")
}
