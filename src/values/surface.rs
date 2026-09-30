//! The one door every read of a container or tagged value goes through: a value's type is its
//! surface. See [README.md § The type memo and `satisfies`](README.md#the-type-memo-and-satisfies).
//!
//! A read sees a value at a type ([`Seen`]): at the top, the value's own memo; below, what the
//! holder's seen type names for the part there — a field, element or entry type, or a tagged
//! value's [representation](super::representation) — narrowed as a retype narrows. A retype
//! restamps the top node alone, so a read applies it on the way down, and a nested part obeys it
//! all the same without the retype ever walking the value.
//!
//! [`Surface`] is a container or tagged value opened at the type it is seen at, plain or a knot'x
//! data node: a record shows only the fields its seen type names, and every part it hands back is
//! seen at its type there. Equality, rendering, the mark pass and the deep copy walk it; a reader
//! that hands a part on as a value of its own [restamps](Seen::restamped) it.

use crate::memory::{BumpAllocator, Writer};
use crate::symbols::Symbol;
use crate::type_lattice::{KType, TypeNode, TypeRegistry, is_subtype_of, meet};

use super::admission::type_satisfies;
use super::circular::{Circular, Resolved};
use super::{Dict, Key, Knotted, Link, List, Nothing, Record, Tagged, Value, representation};

/// A value beside the type a read sees it at.
#[derive(Clone, Copy, Debug)]
pub struct Seen<'cell, X = Nothing> {
    value: Value<'cell, X>,
    ktype: KType,
}

impl<'cell, X: Knotted + 'cell> Seen<'cell, X> {
    /// `value` at its own memo: the top of every read.
    pub fn of(value: Value<'cell, X>) -> Self {
        Seen {
            value,
            ktype: value.ktype(),
        }
    }

    /// The value, at its own memo.
    pub fn value(&self) -> Value<'cell, X> {
        self.value
    }

    /// The type the read sees the value at.
    pub fn ktype(&self) -> KType {
        self.ktype
    }

    /// Whether `slot` takes the value at the type it is seen at: [`satisfies`](super::satisfies)
    /// over that type.
    pub fn satisfies(
        &self,
        slot: KType,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> bool {
        type_satisfies(slot, self.ktype, types, scratch)
    }

    /// The value seen where a holder's type names `declared` for it, after the caller has checked
    /// it satisfies `declared`. The target is read member by member — `declared`'s members, or
    /// `declared` alone — as the meet of the members of the value's own kind that its seen type lies
    /// under: a list, dict or record node for a container, a node naming its constructor for a
    /// tagged value. Where `declared` has none, and for every other value, the seen type stands.
    pub fn seen_at(
        self,
        declared: KType,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> Self {
        let own = self.ktype;
        if declared == own {
            return self;
        }
        let Some(kind) = kind(self.value) else {
            return self;
        };
        let constructor = |handle: KType| match types.node(handle) {
            TypeNode::ConstructorApply { constructor, .. } => constructor,
            _ => handle,
        };
        let of_kind = |member: KType| match (kind, types.node(member)) {
            (Kind::List, TypeNode::List { .. })
            | (Kind::Dict, TypeNode::Dict { .. })
            | (Kind::Record, TypeNode::Record { .. }) => true,
            (Kind::Tagged, _) => constructor(member) == constructor(own),
            _ => false,
        };
        let members = match types.node(declared) {
            TypeNode::Union { members } => members,
            _ => std::slice::from_ref(&declared),
        };
        let target = members
            .iter()
            .copied()
            .filter(|member| of_kind(*member) && is_subtype_of(types, scratch, own, *member))
            .reduce(|lower, member| {
                if is_subtype_of(types, scratch, lower, member) {
                    lower
                } else if is_subtype_of(types, scratch, member, lower) {
                    member
                } else {
                    meet(types, scratch, lower, member)
                }
            });
        match target {
            Some(ktype) => Seen {
                value: self.value,
                ktype,
            },
            None => self,
        }
    }

    /// The value handed on as a value of its own, restamped at its seen type: a plain value takes
    /// it as its handle over the same cells; a knot's data node, whose memo its knot cannot
    /// restamp, is laid down as a plain value of its kind over its cells, each edge resolved to its
    /// sibling.
    pub fn restamped(self, writer: Writer<'cell>) -> Value<'cell, X> {
        let target = self.ktype;
        if target == self.value.ktype() {
            return self.value;
        }
        match self.value {
            Value::List(list) => Value::List(list.with_type(writer, target)),
            Value::Dict(dict) => Value::Dict(dict.with_type(writer, target)),
            Value::Record(record) => Value::Record(record.with_type(writer, target)),
            Value::Tagged(tagged) => Value::Tagged(tagged.with_type(writer, target)),
            other => match other.as_circular() {
                Some((member, node)) => laid_down(writer, member, node, target),
                None => unreachable!("only a value of a kind is seen at another type"),
            },
        }
    }

    /// The value opened at its seen type — the one door. `None` for a scalar, a string, a type, and
    /// every knot member but a data node.
    pub fn surface<'x>(
        &self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'x>,
    ) -> Option<Surface<'x, 'cell, X>> {
        let (node, raw) = raw(self.value)?;
        let ktype = self.ktype;
        let unexpected = || unreachable!("a value is seen only at a type of its own kind");
        let parts = match raw {
            Raw::List(cells) => match types.node(ktype) {
                TypeNode::List { element } => Parts::List { element, cells },
                _ => unexpected(),
            },
            Raw::Dict(keys, cells) => match types.node(ktype) {
                TypeNode::Dict { value, .. } => Parts::Dict { value, keys, cells },
                _ => unexpected(),
            },
            Raw::Record(names, cells) => match types.node(ktype) {
                TypeNode::Record { fields } => {
                    let fields = scratch.alloc_slice_fill_iter(
                        fields.iter().map(|(name, ktype)| (name.symbol(), ktype)),
                    );
                    fields.sort_unstable_by_key(|(name, _)| *name);
                    Parts::Record {
                        fields,
                        names,
                        cells,
                    }
                }
                _ => unexpected(),
            },
            Raw::Tagged(payload) => Parts::Tagged {
                payload,
                representation: representation(types, scratch, ktype),
            },
        };
        Some(Surface { ktype, node, parts })
    }

    /// The field `name` of a record, or a record data node, seen at a record type naming it, seen
    /// at its type there. `None` otherwise — a tagged value included: its caller peels it through
    /// [`surface`](Self::surface).
    pub fn field(
        &self,
        name: Symbol,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> Option<Self> {
        let Some((_, Raw::Record(names, cells))) = raw(self.value) else {
            return None;
        };
        let TypeNode::Record { fields } = types.node(self.ktype) else {
            unreachable!("a record is seen only at a record type")
        };
        let declared = fields.get(name)?;
        Some(Seen::of(cells.get(held_at(names, name))).seen_at(declared, types, scratch))
    }
}

impl<'cell, X: Knotted + 'cell> Value<'cell, X> {
    /// This value opened at its own memo: [`Seen::surface`].
    pub fn surface<'x>(
        &self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'x>,
    ) -> Option<Surface<'x, 'cell, X>> {
        Seen::of(*self).surface(types, scratch)
    }

    /// This value's field `name`, read at its own memo: [`Seen::field`].
    pub fn field(
        &self,
        name: Symbol,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> Option<Seen<'cell, X>> {
        Seen::of(*self).field(name, types, scratch)
    }

    /// The value viewed at `declared`, after the caller has checked it satisfies `declared`: an
    /// ascription, a parameter binding its argument, a frame returning under its contract.
    /// [`Seen::seen_at`], then [`Seen::restamped`].
    pub fn retyped(
        self,
        writer: Writer<'cell>,
        declared: KType,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> Value<'cell, X> {
        Seen::of(self)
            .seen_at(declared, types, scratch)
            .restamped(writer)
    }
}

/// A container or tagged value opened at the type it is seen at, beside the data node it is when
/// it is one. Its parts are the fields that type names, or every element, entry or payload, each
/// seen at its type there. Scratch-borrowed at `'x`, its cells at `'a`.
#[derive(Clone, Copy)]
pub struct Surface<'x, 'a, X> {
    ktype: KType,
    node: Option<X>,
    parts: Parts<'x, 'a, X>,
}

/// What a surface holds, by kind, beside the types its seen type names for its parts.
#[derive(Clone, Copy)]
pub(super) enum Parts<'x, 'a, X> {
    List {
        element: KType,
        cells: Cells<'a, X>,
    },
    Dict {
        value: KType,
        keys: &'a [Key<'a>],
        cells: Cells<'a, X>,
    },
    /// `fields` is what the seen type names, sorted by symbol as `names` is; `names` and `cells`
    /// are the record's whole runs.
    Record {
        fields: &'x [(Symbol, KType)],
        names: &'a [Symbol],
        cells: Cells<'a, X>,
    },
    /// `representation` is `None` for an identity with none, whose payload is seen at its own
    /// memo.
    Tagged {
        payload: Value<'a, X>,
        representation: Option<KType>,
    },
}

impl<'x, 'a, X: Knotted + 'a> Surface<'x, 'a, X> {
    /// The type the value is seen at.
    pub fn ktype(&self) -> KType {
        self.ktype
    }

    /// How many parts it shows: its elements or entries, the fields its seen type names, or a
    /// tagged value's one payload.
    pub fn len(&self) -> usize {
        match self.parts {
            Parts::List { cells, .. } | Parts::Dict { cells, .. } => cells.len(),
            Parts::Record { fields, .. } => fields.len(),
            Parts::Tagged { .. } => 1,
        }
    }

    /// Whether it shows no part.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The part at `at`, below [`len`](Self::len), seen at its type there.
    pub fn child(
        &self,
        at: usize,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> Seen<'a, X> {
        match self.parts {
            Parts::List { element, cells } => {
                Seen::of(cells.get(at)).seen_at(element, types, scratch)
            }
            Parts::Dict { value, cells, .. } => {
                Seen::of(cells.get(at)).seen_at(value, types, scratch)
            }
            Parts::Record {
                fields,
                names,
                cells,
            } => {
                let (name, declared) = fields[at];
                Seen::of(cells.get(held_at(names, name))).seen_at(declared, types, scratch)
            }
            Parts::Tagged {
                payload,
                representation,
            } => {
                debug_assert_eq!(at, 0, "a tagged value holds one payload");
                match representation {
                    Some(representation) => {
                        Seen::of(payload).seen_at(representation, types, scratch)
                    }
                    None => Seen::of(payload),
                }
            }
        }
    }

    /// A record's field name at `at`, in symbol order.
    pub fn name(&self, at: usize) -> Symbol {
        match self.parts {
            Parts::Record { fields, .. } => fields[at].0,
            _ => unreachable!("only a record names its parts"),
        }
    }

    /// A dict's key at `at`, in key order.
    pub fn key(&self, at: usize) -> &'a Key<'a> {
        match self.parts {
            Parts::Dict { keys, .. } => &keys[at],
            _ => unreachable!("only a dict keys its parts"),
        }
    }

    /// The data node it is, when it is one.
    pub(super) fn node(&self) -> Option<X> {
        self.node
    }

    pub(super) fn parts(&self) -> Parts<'x, 'a, X> {
        self.parts
    }
}

/// Where a record holds the field `name` its seen type names.
fn held_at(names: &[Symbol], name: Symbol) -> usize {
    names
        .binary_search(&name)
        .expect("a record holds every field its type names")
}

/// A run of cells read as values: plain words, or links resolved through their holder.
#[derive(Clone, Copy)]
pub(super) enum Cells<'a, X> {
    Plain(&'a [Value<'a, X>]),
    Linked(&'a [Link<'a, X>], X),
}

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
}

/// A container or tagged value's runs, before a type opens them.
enum Raw<'a, X> {
    List(Cells<'a, X>),
    Dict(&'a [Key<'a>], Cells<'a, X>),
    Record(&'a [Symbol], Cells<'a, X>),
    Tagged(Value<'a, X>),
}

/// `value`'s runs, beside the data node it is when it is one. `None` for a scalar, a string, a
/// type and every knot member but a data node.
fn raw<'a, X: Knotted + 'a>(value: Value<'a, X>) -> Option<(Option<X>, Raw<'a, X>)> {
    let plain = match value {
        Value::List(list) => Raw::List(Cells::Plain(list.cells())),
        Value::Dict(dict) => Raw::Dict(dict.keys(), Cells::Plain(dict.cells())),
        Value::Record(record) => Raw::Record(record.names(), Cells::Plain(record.cells())),
        Value::Tagged(tagged) => Raw::Tagged(*tagged.payload()),
        Value::Knotted(member) => {
            let Resolved::Circular(node) = member.resolve() else {
                return None;
            };
            let linked = match node {
                Circular::List(list) => Raw::List(Cells::Linked(list.cells(), member)),
                Circular::Dict(dict) => Raw::Dict(dict.keys(), Cells::Linked(dict.cells(), member)),
                Circular::Record(record) => {
                    Raw::Record(record.names(), Cells::Linked(record.cells(), member))
                }
                Circular::Tagged(tagged) => Raw::Tagged(tagged.payload().resolve(member)),
            };
            return Some((Some(member), linked));
        }
        _ => return None,
    };
    Some((None, plain))
}

/// The kinds of value a read narrows its seen type within.
#[derive(Clone, Copy)]
enum Kind {
    List,
    Dict,
    Record,
    Tagged,
}

/// `value`'s kind, plain or a data node; `None` for every other value.
fn kind<X: Knotted>(value: Value<'_, X>) -> Option<Kind> {
    let node = match value {
        Value::List(_) => return Some(Kind::List),
        Value::Dict(_) => return Some(Kind::Dict),
        Value::Record(_) => return Some(Kind::Record),
        Value::Tagged(_) => return Some(Kind::Tagged),
        Value::Knotted(member) => match member.resolve() {
            Resolved::Circular(node) => node,
            _ => return None,
        },
        _ => return None,
    };
    Some(match node {
        Circular::List(_) => Kind::List,
        Circular::Dict(_) => Kind::Dict,
        Circular::Record(_) => Kind::Record,
        Circular::Tagged(_) => Kind::Tagged,
    })
}

/// A knot's data node laid down as a plain value of its kind under `target`: each cell resolved
/// through `member`, so an edge becomes the sibling it names; a dict's keys and a record's names
/// shared with the node.
fn laid_down<'cell, X: Knotted + 'cell>(
    writer: Writer<'cell>,
    member: X,
    node: Circular<'cell, X>,
    target: KType,
) -> Value<'cell, X> {
    let resolved =
        |cells: &[Link<'cell, X>]| writer.fill(cells.len(), |at| cells[at].resolve(member));
    match node {
        Circular::List(list) => Value::List(List::weighed(writer, resolved(list.cells()), target)),
        Circular::Dict(dict) => Value::Dict(Dict::weighed(
            writer,
            dict.keys(),
            resolved(dict.cells()),
            target,
        )),
        Circular::Record(record) => Value::Record(Record::weighed(
            writer,
            record.names(),
            resolved(record.cells()),
            target,
        )),
        Circular::Tagged(tagged) => Value::Tagged(Tagged::hold(
            writer,
            tagged.payload().resolve(member),
            target,
        )),
    }
}
