//! A knot's data node, read through the member holding it: a list, dict, record or tagged value
//! whose cells are [`Link`]s, so a cell may name a sibling node of the same knot. What a member holds
//! is its [`Resolved`] form.
//!
//! [`Composite`] is the one reading of a container or tagged value that equality, rendering and the
//! mark pass share: its cells as values, whether they are plain words or links resolved through the
//! node holding them. The lifetimes of a node's run shorten to the borrow a member hands out, which
//! is sound because every resident is covariant in `'graph` and `'cell`.

use crate::memory::{Writer, collect};
use crate::parse::Symbol;
use crate::type_lattice::KType;

use super::{DeepCopy, Dict, Key, Knotted, Link, List, Nothing, Record, Tagged, Value, Weight};

/// What a knot member holds.
#[derive(Clone, Copy)]
pub enum Resolved<'a, X> {
    /// A function: opaque to `values`, incomparable, rendered as its type's name.
    Function,
    /// A data node.
    Circular(Circular<'a, 'a, X>),
}

/// A data node of a knot. `Copy`.
pub enum Circular<'graph, 'cell, X = Nothing> {
    List(&'cell List<'graph, 'cell, X, Link<'graph, 'cell, X>>),
    Dict(&'cell Dict<'graph, 'cell, X, Link<'graph, 'cell, X>>),
    Record(&'cell Record<'graph, 'cell, X, Link<'graph, 'cell, X>>),
    Tagged(&'cell Tagged<'graph, 'cell, X, Link<'graph, 'cell, X>>),
}

impl<X> Clone for Circular<'_, '_, X> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<X> Copy for Circular<'_, '_, X> {}

impl<'graph, 'cell, X: Knotted> Circular<'graph, 'cell, X> {
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

    /// This node rebuilt in `writer`'s region: each link through [`Link::copied`], keys rehomed,
    /// names collected, type and weight carried over.
    pub fn copied<'to, Y: Knotted>(
        &self,
        writer: Writer<'to>,
        copy: &mut DeepCopy<'_, 'graph, 'cell, 'to, X, Y>,
    ) -> Circular<'graph, 'to, Y> {
        let mut copy = |value: &Value<'graph, 'cell, X>| copy(value);
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
        payload: Value<'a, 'a, X>,
    },
}

/// A run of cells read as values: plain words, or links resolved through their holder.
pub(super) enum Cells<'a, X> {
    Plain(&'a [Value<'a, 'a, X>]),
    Linked(&'a [Link<'a, 'a, X>], X),
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

    pub(super) fn get(self, at: usize) -> Value<'a, 'a, X> {
        match self {
            Cells::Plain(cells) => cells[at],
            Cells::Linked(cells, holder) => cells[at].resolve(holder),
        }
    }

    pub(super) fn iter(self) -> impl ExactSizeIterator<Item = Value<'a, 'a, X>> {
        (0..self.len()).map(move |at| self.get(at))
    }
}

impl<'graph, 'cell, X: Knotted> Value<'graph, 'cell, X> {
    /// The composite this value reads as, beside the data node it is when it is one. `None` for a
    /// scalar, a string, a quote, a type and a function.
    pub(super) fn composite<'a>(&self) -> Option<(Option<X>, Composite<'a, X>)>
    where
        'cell: 'a,
        'graph: 'a,
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

impl<'a, X: Knotted> Circular<'a, 'a, X> {
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
