//! A knot's data node, read through the member holding it: a list, dict, record or tagged value
//! whose cells are [`Link`]s, so a cell may name a sibling node of the same knot. What a member holds
//! is its [`Resolved`] form: beside a data node, a function or a quote's code, each read through
//! the links it holds, or a module or a barrier, opaque to `values`.
//!
//! [`Composite`] is the one reading of a container or tagged value that equality, rendering, the
//! mark pass and the deep copy share: its cells as values, whether they are plain words or links
//! resolved through the node holding them. [`Circular::held`] lists the values a data node's
//! [`copied`](Circular::copied) asks for, in its order, for a knot family's
//! [`held`](super::KnottedFamily::held). The lifetimes of a node's run shorten to the borrow a member hands out, which
//! is sound because every resident is covariant in `'cell` and so is every member.

use crate::memory::{Writer, collect};
use crate::parse::KExpression;
use crate::symbols::{BinderSymbol, Symbol};
use crate::type_lattice::KType;

use super::{DeepCopy, Dict, Key, Knotted, Link, List, Nothing, Record, Tagged, Value, Weight};

/// What a knot member holds.
#[derive(Clone, Copy)]
pub enum Resolved<'a, X> {
    /// A function, rendered as its type's name. It compares by `identity` — one per `FN`, `EXPR` or
    /// `OP` written, so a copy keeps it — and by its closure bindings, read through the member.
    Function {
        identity: usize,
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

/// A container or tagged value read one way, plain or a data node.
pub(super) enum Composite<'a, X> {
    List {
        ktype: KType,
        cells: Cells<'a, X>,
    },
    Dict {
        ktype: KType,
        keys: &'a [Key<'a>],
        cells: Cells<'a, X>,
    },
    Record {
        ktype: KType,
        names: &'a [Symbol],
        cells: Cells<'a, X>,
    },
    Tagged {
        ktype: KType,
        payload: Value<'a, X>,
    },
}

impl<'a, X: Knotted> Composite<'a, X> {
    /// How many values this composite holds: its cells, or a tagged value's one payload.
    pub(super) fn len(&self) -> usize {
        match self {
            Composite::List { cells, .. }
            | Composite::Dict { cells, .. }
            | Composite::Record { cells, .. } => cells.len(),
            Composite::Tagged { .. } => 1,
        }
    }

    /// The value at `at`, below [`len`](Self::len): a cell, or a tagged value's payload.
    pub(super) fn child(&self, at: usize) -> Value<'a, X> {
        match self {
            Composite::List { cells, .. }
            | Composite::Dict { cells, .. }
            | Composite::Record { cells, .. } => cells.get(at),
            Composite::Tagged { payload, .. } => {
                debug_assert_eq!(at, 0, "a tagged value holds one payload");
                *payload
            }
        }
    }
}

/// A run of cells read as values: plain words, or links resolved through their holder.
pub(super) enum Cells<'a, X> {
    Plain(&'a [Value<'a, X>]),
    Linked(&'a [Link<'a, X>], X),
}

impl<X: Knotted> Clone for Cells<'_, X> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<X: Knotted> Copy for Cells<'_, X> {}

impl<'a, X: Knotted> Cells<'a, X> {
    pub(super) fn len(self) -> usize {
        match self {
            Cells::Plain(cells) => cells.len(),
            Cells::Linked(cells, _) => cells.len(),
        }
    }

    pub(super) fn get(self, at: usize) -> Value<'a, X> {
        match self {
            Cells::Plain(cells) => cells[at],
            Cells::Linked(cells, holder) => cells[at].resolve(holder),
        }
    }

    pub(super) fn iter(self) -> impl ExactSizeIterator<Item = Value<'a, X>> {
        (0..self.len()).map(move |at| self.get(at))
    }
}

impl<'cell, X: Knotted> Value<'cell, X> {
    /// The composite this value reads as, beside the data node it is when it is one. `None` for a
    /// scalar, a string, a type and every knot member but a data node.
    pub(super) fn composite<'a>(&self) -> Option<(Option<X>, Composite<'a, X>)>
    where
        'cell: 'a,
        X: 'a,
    {
        let plain = match *self {
            Value::List(list) => Composite::List {
                ktype: list.ktype(),
                cells: Cells::Plain(list.cells()),
            },
            Value::Dict(dict) => Composite::Dict {
                ktype: dict.ktype(),
                keys: dict.keys(),
                cells: Cells::Plain(dict.cells()),
            },
            Value::Record(record) => Composite::Record {
                ktype: record.ktype(),
                names: record.names(),
                cells: Cells::Plain(record.cells()),
            },
            Value::Tagged(tagged) => Composite::Tagged {
                ktype: tagged.ktype(),
                payload: *tagged.payload(),
            },
            Value::Knotted(member) => {
                let Resolved::Circular(circular) = member.resolve() else {
                    return None;
                };
                return Some((Some(member), circular.composite(member)));
            }
            _ => return None,
        };
        Some((None, plain))
    }
}

impl<'a, X: Knotted> Circular<'a, X> {
    /// This node read as a composite, its links resolved through `holder`.
    fn composite(self, holder: X) -> Composite<'a, X> {
        match self {
            Circular::List(list) => Composite::List {
                ktype: list.ktype(),
                cells: Cells::Linked(list.cells(), holder),
            },
            Circular::Dict(dict) => Composite::Dict {
                ktype: dict.ktype(),
                keys: dict.keys(),
                cells: Cells::Linked(dict.cells(), holder),
            },
            Circular::Record(record) => Composite::Record {
                ktype: record.ktype(),
                names: record.names(),
                cells: Cells::Linked(record.cells(), holder),
            },
            Circular::Tagged(tagged) => Composite::Tagged {
                ktype: tagged.ktype(),
                payload: tagged.payload().resolve(holder),
            },
        }
    }
}
