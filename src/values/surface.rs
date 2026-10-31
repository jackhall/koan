//! The one door every read of a container or tagged value goes through: a value's type is its
//! surface. See [README.md § The type memo and `satisfies`](README.md#the-type-memo-and-satisfies).
//!
//! A read sees a value at a type ([`Seen`]): at the top, the value's own memo; below, what the
//! holder's seen type names for the part there — a field, element or entry type, or a tagged
//! value's [representation](super::representation) — narrowed as a retype narrows. A retype
//! restamps the top node alone, so a read applies it on the way down, and a nested part obeys it
//! all the same without the retype ever walking the value.
//!
//! [`Surface`] is a container or tagged value opened at the type it is seen at, plain or a knot's
//! data node: a record shows only the fields its seen type names, and every part it hands back is
//! seen at its type there. Equality, rendering, the mark pass, the deep copy and the content digest
//! walk it; a reader that hands a part on as a value of its own [restamps](Seen::restamped) it.
//!
//! A container's type is the fill of its cells' types ([fill.rs](super::fill)), which names, at
//! `Null`, fields a cell does not store. So a field seen at exactly `Null` reads `null` without its
//! cell, and a container is taken at a member of the type it is read at that its own type fits
//! once filled against it. A record seen at a record type keeps at `Null` each field its own type
//! lacks or has at exactly `Null`.
//!
//! The static type of a value retyped to a type ([`retyped_to`]) and of one at most a type
//! ([`under`]) live here too, beside the retype they describe: the load reads the same predicate
//! `seen_at` retypes by. Where the load closes a retype's declared type it records the
//! [`Retype`], and the run re-stamps by it ([`Seen::seen_by`]) without computing the target.

use crate::memory::{Bump, BumpAllocator, BumpVec, Edge, Writer};
use crate::symbols::{BinderSymbol, Symbol, TypeSymbol};
use crate::type_lattice::{
    DeclaredType, FitsFailure, Interval, KType, Members, Parametric, TypeNode, bound_above, fits,
    fits_application, is_subtype_of, meet, satisfied_by,
};

use super::circular::{Circular, Resolved};
use super::fill::filled_against;
use super::hiding::unhidden;
use super::{Chain, Key, Knotted, Link, List, Nothing, Retyped, Value, representation};

/// Why the type a container, a tagged value or a data node is seen at is concrete: its memo is a
/// type, and so is every type a holder names for a part. Only a callable is typed by a scheme.
const SEEN: &str = "a value of a kind is seen at a type, never a scheme";

/// One step from a value down to one of its parts: a record field's label, read as
/// [`Value::along`] reads it; a capture slot of a function, read as [`Value::captured`] reads it;
/// a member slot of a module, which `values` sees as opaque and the knot reads; or a tagged
/// value's payload, read as [`Value::untagged`] reads it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PathSegment {
    Label(Symbol),
    Capture(u32),
    Member(u32),
    Payload,
}

/// A path of segments from a value down to one of its parts.
pub type PartPath<'cell> = &'cell [PathSegment];

/// A value beside the type a read sees it at: a quantified callable — a closure's capture, a
/// candidate list's cell — at its scheme, every other value at a type.
#[derive(Clone, Copy, Debug)]
pub struct Seen<'cell, X = Nothing> {
    value: Value<'cell, X>,
    ktype: DeclaredType<KType>,
}

impl<'cell, X: Knotted + 'cell> Seen<'cell, X> {
    /// `value` at its own memo: the top of every read.
    pub fn of(value: Value<'cell, X>) -> Self {
        Seen {
            value,
            ktype: value.ktype(),
        }
    }

    /// `function`, a registration member, at `shape`, the expression shape it registers at: what a
    /// keyworded use reads it by, and what a header retypes it from.
    pub fn registered(function: Value<'cell, X>, shape: DeclaredType<KType>) -> Self {
        debug_assert!(
            function.as_callable().is_some(),
            "a registration member is a function"
        );
        Seen {
            value: function,
            ktype: shape,
        }
    }

    /// The value, at its own memo.
    pub fn value(&self) -> Value<'cell, X> {
        self.value
    }

    /// The type the read sees the value at.
    pub fn ktype(&self) -> DeclaredType<KType> {
        self.ktype
    }

    /// The type a value of a kind is seen at ([`SEEN`]).
    fn concrete(&self) -> KType {
        self.ktype.as_type().expect(SEEN)
    }

    /// Whether `slot` takes the value at the type it is seen at: [`satisfies`](super::satisfies)
    /// over that type.
    pub fn satisfies(
        &self,
        slot: KType,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> bool {
        satisfied_by(types, scratch, slot, self.ktype)
    }

    /// The value seen where a holder's type names `declared` for it, after the caller has checked
    /// it satisfies `declared`. The target is read member by member — `declared`'s members, or
    /// `declared` alone — as the meet of what the members of the value's own kind take it at
    /// ([`target`]). A function declared at a scheme is seen at that scheme. Where `declared` has
    /// none, and for every other value, the seen type stands.
    pub fn seen_at(
        self,
        declared: impl Into<DeclaredType<KType>>,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Self {
        let Some(kind) = kind(self.value) else {
            return self;
        };
        let ktype = match declared.into() {
            DeclaredType::Type(declared) => {
                target(types, scratch, kind, self.ktype, declared).map(DeclaredType::from)
            }
            scheme => (kind == Kind::Function
                && self.ktype != scheme
                && fits(types, scratch, self.ktype, scheme))
            .then_some(scheme),
        };
        match ktype {
            Some(ktype) => Seen {
                value: self.value,
                ktype,
            },
            None => self,
        }
    }

    /// [`seen_at`](Self::seen_at) `declared`, by `retype` where the load recorded one for it.
    pub fn seen_as(
        self,
        declared: KType,
        retype: Option<Retype>,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Self {
        let Some(retype) = retype else {
            return self.seen_at(declared, types, scratch);
        };
        #[cfg(debug_assertions)]
        if let Some(kind) = kind(self.value) {
            let seen = |target: Option<KType>| target.map_or(self.ktype, DeclaredType::from);
            debug_assert_eq!(
                seen(target_by(types, scratch, kind, self.ktype, retype)),
                seen(computed(types, scratch, kind, self.ktype, declared)),
                "a recorded retype re-stamps where the target is"
            );
        }
        self.seen_by(retype, types, scratch)
    }

    /// The value seen as `retype`, the load's record of a retype to a closed declared type, after
    /// the caller has checked it satisfies that type: [`seen_at`](Self::seen_at) that type, with
    /// the target read off the record — computed only for a union's members or a module's fit.
    pub fn seen_by(
        self,
        retype: Retype,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Self {
        let Some(kind) = kind(self.value) else {
            return self;
        };
        match target_by(types, scratch, kind, self.ktype, retype) {
            Some(ktype) => Seen {
                value: self.value,
                ktype: ktype.into(),
            },
            None => self,
        }
    }

    /// The value handed on as a value of its own, restamped at its seen type: a plain value takes
    /// it as its handle over the same cells, once each part the seen type would show a field of
    /// that it hides is copied without it ([`unhidden`]); a knot member, whose node every holder
    /// shares, takes a [`Retyped`] header over its bare member, or is that bare member where the
    /// seen type is its own.
    pub fn restamped(
        self,
        writer: Writer<'cell, '_>,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> Value<'cell, X> {
        if self.value.ktype() == self.ktype {
            return self.value;
        }
        // A part the seen type would show a field of that its view so far hides is copied first.
        let value = match self.value {
            Value::List(_) | Value::Dict(_) | Value::Record(_) | Value::Tagged(_) => {
                unhidden(writer, types, &Bump::new(), self.value, self.concrete())
            }
            other => other,
        };
        match value {
            Value::List(list) => Value::List(list.with_type(writer, self.concrete(), types)),
            Value::Dict(dict) => Value::Dict(dict.with_type(writer, self.concrete(), types)),
            Value::Record(record) => {
                Value::Record(record.with_type(writer, self.concrete(), types))
            }
            Value::Tagged(tagged) => {
                Value::Tagged(tagged.with_type(writer, self.concrete(), types))
            }
            Value::Type(value) => Value::Type(value.with_type(writer, self.concrete())),
            other => match other.knot_member() {
                Some(member) if member.ktype() == self.ktype => Value::Knotted(member),
                Some(member) => Value::Retyped(Retyped::of(writer, member, self.ktype)),
                None => unreachable!("only a value of a kind is seen at another type"),
            },
        }
    }

    /// The value opened at its seen type — the one door. `None` for a scalar, a string, a type, and
    /// every knot member but a data node.
    pub fn surface<'x>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'x>,
    ) -> Option<Surface<'x, 'cell, X>> {
        let (node, raw) = raw(self.value)?;
        let ktype = self.concrete();
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
    /// at its type there — `null` where that type is exactly `Null`, stored or not. `None`
    /// otherwise — a tagged value included: its caller peels it through
    /// [`surface`](Self::surface).
    pub fn field(
        &self,
        name: Symbol,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Option<Self> {
        let Some((_, Raw::Record(names, cells))) = raw(self.value) else {
            return None;
        };
        let TypeNode::Record { fields } = types.node(self.concrete()) else {
            unreachable!("a record is seen only at a record type")
        };
        let declared = fields.get(name)?;
        if declared == KType::NULL {
            return Some(Seen::of(Value::Null));
        }
        Some(Seen::of(cells.get(held_at(names, name))).seen_at(declared, types, scratch))
    }
}

impl<'cell, X: Knotted + 'cell> Value<'cell, X> {
    /// This value opened at its own memo: [`Seen::surface`].
    pub fn surface<'x>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'x>,
    ) -> Option<Surface<'x, 'cell, X>> {
        Seen::of(*self).surface(types, scratch)
    }

    /// This value's field `name`, read at its own memo: [`Seen::field`].
    pub fn field(
        &self,
        name: Symbol,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Option<Seen<'cell, X>> {
        Seen::of(*self).field(name, types, scratch)
    }

    /// The value viewed at `declared`, after the caller has checked it satisfies `declared`: an
    /// ascription, a parameter binding its argument, a frame returning under its contract.
    /// [`Seen::seen_at`], then [`Seen::restamped`].
    pub fn retyped(
        self,
        writer: Writer<'cell, '_>,
        declared: KType,
        types: &Chain<'_, 'cell, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Value<'cell, X> {
        Seen::of(self)
            .seen_at(declared, types, scratch)
            .restamped(writer, types)
    }

    /// The part the record field `label` stores, read through every tagged payload first — a
    /// knot's data node's included — at its own memo: the stored part a field read restamps.
    /// Reads, never writes. `None` where nothing along the way is a record storing `label` — a
    /// field a fill gave a record, at `Null`, among them ([`Value::named`]).
    pub fn along(self, label: Symbol) -> Option<Value<'cell, X>> {
        let mut value = self;
        loop {
            match raw(value)?.1 {
                Raw::Tagged(payload) => value = payload.get(0),
                Raw::Record(names, cells) => {
                    return Some(cells.get(names.binary_search(&label).ok()?));
                }
                Raw::List(_) | Raw::Dict(..) => return None,
            }
        }
    }

    /// The payload one tagged layer below this value, as [`along`](Self::along) reads through it:
    /// `None` for a value that is not tagged.
    pub fn untagged(self) -> Option<Value<'cell, X>> {
        match raw(self)?.1 {
            Raw::Tagged(payload) => Some(payload.get(0)),
            Raw::Record(..) | Raw::List(_) | Raw::Dict(..) => None,
        }
    }

    /// The field `label` of this record, one its type names, as [`along`](Self::along) reads it:
    /// `null` where the record stores none, a field a fill gave it at `Null`, which the door reads
    /// as `null`. What a call by name hands each parameter of a record held whole.
    pub fn named(self, label: Symbol) -> Value<'cell, X> {
        self.along(label).unwrap_or(Value::Null)
    }

    /// The value capture slot `slot` of the function this value is holds — a value word, or the
    /// fellow member of its knot an edge names — as an activation of it reads that capture.
    /// `None` for a value that is no function, and past its closure run.
    pub fn captured(self, slot: u32) -> Option<Value<'cell, X>> {
        let member = self.knot_member()?;
        let Resolved::Function { closure, .. } = member.resolve() else {
            return None;
        };
        Some(closure.get(slot as usize)?.resolve(member))
    }

    /// The value restamped at `ktype`, a type a restamp of the same storage carried: what a value
    /// handed on through its true home re-lays where it lands. [`Seen::restamped`] at that type.
    pub fn restamped_at(
        self,
        writer: Writer<'cell, '_>,
        ktype: DeclaredType<KType>,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> Value<'cell, X> {
        Seen { value: self, ktype }.restamped(writer, types)
    }

    /// Whether `self` and `other` are one storage: the same scalar or resident, restamps of the
    /// same runs, tagged values over one storage, or headers over one knot member. Runs compare by
    /// address, so two containers of no cells answer `true` wherever their empty runs share one;
    /// that is intended, since such a container holds no storage beside its header and either
    /// stands for the other. Never walks a value.
    pub fn same_storage(&self, other: &Value<'_, X>) -> bool {
        let runs = |a: *const (), b: *const ()| std::ptr::eq(a, b);
        match (self, other) {
            (Value::Number(a), Value::Number(b)) => a.to_bits() == b.to_bits(),
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Null, Value::Null) => true,
            (Value::Str(a), Value::Str(b)) => runs(a.as_ptr().cast(), b.as_ptr().cast()),
            (Value::StrView(a), Value::StrView(b)) => std::ptr::eq(*a, *b),
            (Value::Type(a), Value::Type(b)) => a.handle() == b.handle(),
            (Value::List(a), Value::List(b)) => a.same_run(b),
            (Value::Dict(a), Value::Dict(b)) => {
                runs(a.cells().as_ptr().cast(), b.cells().as_ptr().cast())
            }
            (Value::Record(a), Value::Record(b)) => {
                runs(a.cells().as_ptr().cast(), b.cells().as_ptr().cast())
            }
            (Value::Tagged(a), Value::Tagged(b)) => a.payload().same_storage(b.payload()),
            _ => match (self.knot_member(), other.knot_member()) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            },
        }
    }

    /// [`retyped`](Self::retyped) to `declared`, re-stamped by `retype` where the load recorded
    /// one for it ([`Seen::seen_by`]).
    pub fn retyped_as(
        self,
        writer: Writer<'cell, '_>,
        declared: KType,
        retype: Option<Retype>,
        types: &Chain<'_, 'cell, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Value<'cell, X> {
        Seen::of(self)
            .seen_as(declared, retype, types, scratch)
            .restamped(writer, types)
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
    /// `payload` is a run of one; `representation` is `None` for an identity with none, whose
    /// payload is seen at its own memo.
    Tagged {
        payload: Cells<'a, X>,
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

    /// The part at `at`, below [`len`](Self::len), seen at its type there: a field seen at exactly
    /// `Null` is `null`, whether the record stores it or a fill gave it.
    pub fn child(
        &self,
        at: usize,
        types: &Chain<'_, '_, '_, '_>,
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
                if declared == KType::NULL {
                    return Seen::of(Value::Null);
                }
                Seen::of(cells.get(held_at(names, name))).seen_at(declared, types, scratch)
            }
            Parts::Tagged {
                payload,
                representation,
            } => {
                debug_assert_eq!(at, 0, "a tagged value holds one payload");
                match representation {
                    Some(representation) => {
                        Seen::of(payload.get(0)).seen_at(representation, types, scratch)
                    }
                    None => Seen::of(payload.get(0)),
                }
            }
        }
    }

    /// The part at `at`, below [`len`](Self::len), as its cell holds it, at its own memo — before a
    /// read sees it at the type its holder names.
    pub(super) fn raw(&self, at: usize) -> Value<'a, X> {
        match self.parts {
            Parts::List { cells, .. } | Parts::Dict { cells, .. } => cells.get(at),
            Parts::Record {
                fields,
                names,
                cells,
            } => match fields[at] {
                (_, declared) if declared == KType::NULL => Value::Null,
                (name, _) => cells.get(held_at(names, name)),
            },
            Parts::Tagged { payload, .. } => payload.get(0),
        }
    }

    /// The edge the part at `at` is, when it is a data node's link naming a sibling of its own knot
    /// rather than a value word.
    pub(super) fn edge(&self, at: usize) -> Option<Edge> {
        match self.parts {
            Parts::List { cells, .. } | Parts::Dict { cells, .. } => cells.edge(at),
            Parts::Record {
                fields,
                names,
                cells,
            } => match fields[at] {
                (_, declared) if declared == KType::NULL => None,
                (name, _) => cells.edge(held_at(names, name)),
            },
            Parts::Tagged { payload, .. } => payload.edge(0),
        }
    }

    /// Whether a record stores the field at `at`: one its seen type names at `Null` that a fill
    /// gave it stores none.
    pub(super) fn stores(&self, at: usize) -> bool {
        match self.parts {
            Parts::Record { fields, names, .. } => names.binary_search(&fields[at].0).is_ok(),
            _ => true,
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

    /// Where a dict holds `key`, in key order; `None` where it holds none. O(log n).
    pub fn key_index(&self, key: &Key<'_>) -> Option<usize> {
        match self.parts {
            Parts::Dict { keys, .. } => keys.binary_search_by(|probe| probe.cmp(key)).ok(),
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

/// Where a record holds the field `name` its seen type names at a type other than `Null`.
fn held_at(names: &[Symbol], name: Symbol) -> usize {
    names
        .binary_search(&name)
        .expect("a record holds every field its type names at a type other than `Null`")
}

/// A run of cells read as values: plain words, links resolved through their holder, or a plain
/// list's elements, flat or a view, read through the list.
#[derive(Clone, Copy)]
pub(super) enum Cells<'a, X> {
    Plain(&'a [Value<'a, X>]),
    Linked(&'a [Link<'a, X>], X),
    List(&'a List<'a, X>),
    /// A knot's list, flat or a view, read through its member.
    Knotted(&'a List<'a, X, Link<'a, X>>, X),
}

impl<'a, X: Knotted> Cells<'a, X> {
    pub(super) fn len(self) -> usize {
        match self {
            Cells::Plain(cells) => cells.len(),
            Cells::Linked(cells, _) => cells.len(),
            Cells::List(list) => list.len(),
            Cells::Knotted(list, _) => list.len(),
        }
    }

    pub(super) fn get(self, at: usize) -> Value<'a, X> {
        match self {
            Cells::Plain(cells) => cells[at],
            Cells::Linked(cells, holder) => cells[at].resolve(holder),
            Cells::List(list) => list.get(at),
            Cells::Knotted(list, holder) => list.link(at).resolve(holder),
        }
    }

    /// The edge the cell at `at` holds, if it holds one. A plain list's element never is one.
    fn edge(self, at: usize) -> Option<Edge> {
        match self {
            Cells::Linked(cells, _) => match cells[at] {
                Link::Edge(edge) => Some(edge),
                Link::Value(_) => None,
            },
            Cells::Knotted(list, _) => match list.link(at) {
                Link::Edge(edge) => Some(edge),
                Link::Value(_) => None,
            },
            Cells::Plain(_) | Cells::List(_) => None,
        }
    }
}

/// A container or tagged value's runs, before a type opens them.
enum Raw<'a, X> {
    List(Cells<'a, X>),
    Dict(&'a [Key<'a>], Cells<'a, X>),
    Record(&'a [Symbol], Cells<'a, X>),
    Tagged(Cells<'a, X>),
}

/// `value`'s runs, beside the data node it is when it is one. `None` for a scalar, a string, a
/// type and every knot member but a data node.
fn raw<'a, X: Knotted + 'a>(value: Value<'a, X>) -> Option<(Option<X>, Raw<'a, X>)> {
    let plain = match value {
        Value::List(list) => Raw::List(Cells::List(list)),
        Value::Dict(dict) => Raw::Dict(dict.keys(), Cells::Plain(dict.cells())),
        Value::Record(record) => Raw::Record(record.names(), Cells::Plain(record.cells())),
        Value::Tagged(tagged) => Raw::Tagged(Cells::Plain(std::slice::from_ref(tagged.payload()))),
        Value::Knotted(_) | Value::Retyped(_) => {
            let member = value.knot_member().expect("a knot member");
            let Resolved::Circular(node) = member.resolve() else {
                return None;
            };
            let linked = match node {
                Circular::List(list) => Raw::List(Cells::Knotted(list, member)),
                Circular::Dict(dict) => Raw::Dict(dict.keys(), Cells::Linked(dict.cells(), member)),
                Circular::Record(record) => {
                    Raw::Record(record.names(), Cells::Linked(record.cells(), member))
                }
                Circular::Tagged(tagged) => Raw::Tagged(Cells::Linked(
                    std::slice::from_ref(tagged.payload()),
                    member,
                )),
            };
            return Some((Some(member), linked));
        }
        _ => return None,
    };
    Some((None, plain))
}

/// Every part `value` holds, hidden or not, at its own memo: a container's cells, a tagged value's
/// payload. Nothing for a knot member, whose parts its knot lists. What the crossing's carried-type
/// walk reads.
pub(super) fn raw_parts<'a, X: Knotted + 'a>(
    value: Value<'a, X>,
    out: &mut impl FnMut(Value<'a, X>),
) {
    if value.knot_member().is_some() {
        return;
    }
    let Some((_, raw)) = raw(value) else {
        return;
    };
    let cells = match raw {
        // A view's elements are walked in order, rather than found one by one.
        Raw::List(Cells::List(list)) => return list.elements().for_each(out),
        Raw::List(cells) | Raw::Dict(_, cells) | Raw::Record(_, cells) | Raw::Tagged(cells) => {
            cells
        }
    };
    for at in 0..cells.len() {
        out(cells.get(at));
    }
}

/// The kinds of value a read narrows its seen type within.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    List,
    Dict,
    Record,
    Tagged,
    /// A function or a barrier over one.
    Function,
    /// A module, body-born or a view.
    Module,
    /// A quote's code.
    Code,
    /// A type value, typed by its kind.
    Type,
}

/// `value`'s kind, plain or a knot member; `None` for every other value.
fn kind<X: Knotted>(value: Value<'_, X>) -> Option<Kind> {
    let node = match value {
        Value::List(_) => return Some(Kind::List),
        Value::Dict(_) => return Some(Kind::Dict),
        Value::Record(_) => return Some(Kind::Record),
        Value::Tagged(_) => return Some(Kind::Tagged),
        Value::Type(_) => return Some(Kind::Type),
        Value::Knotted(_) | Value::Retyped(_) => {
            match value.knot_member().expect("a knot member").resolve() {
                Resolved::Circular(node) => node,
                Resolved::Function { .. } | Resolved::Barrier => return Some(Kind::Function),
                Resolved::Module => return Some(Kind::Module),
                Resolved::Code => return Some(Kind::Code),
            }
        }
        _ => return None,
    };
    Some(match node {
        Circular::List(_) => Kind::List,
        Circular::Dict(_) => Kind::Dict,
        Circular::Record(_) => Kind::Record,
        Circular::Tagged(_) => Kind::Tagged,
    })
}

/// What a retype to a closed declared type re-stamps a value satisfying it at, as the load records
/// it where it closes that type, so the run re-stamps without computing the target ([`target`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retype {
    /// This type, whatever the value: the declared type re-stamps every value of its kind at
    /// itself ([`retyped_to`] is a point), or the type an exact module is pinned at.
    Point(KType),
    /// The value's own type: the declared type re-stamps no kind — `Any`, a scalar, `Module` or
    /// `Code`.
    Keeps,
    /// The target computed at the run: the meet of what the members of this union of the value's
    /// kind take it at, or this record type with a field `Null` lies strictly under, whose `Null`s
    /// the value keeps.
    Computed(KType),
    /// What *fits* pins a module at under this signature type, which leaves a head parameter
    /// unpinned.
    Fit(KType),
}

impl Retype {
    /// The retype to `declared`, read off its node: the classes [`retyped_to`] reads.
    pub fn of(types: &Chain<'_, '_, '_, '_>, scratch: BumpAllocator<'_>, declared: KType) -> Self {
        if let TypeNode::Union { .. } = types.node(declared) {
            return Retype::Computed(declared);
        }
        match retyped_kind(types, declared.into()) {
            None => Retype::Keeps,
            Some(Kind::Module) if !pins_every_parameter(types, declared.into()) => {
                Retype::Fit(declared)
            }
            Some(Kind::Record) if !record_is_point(types, scratch, declared.into()) => {
                Retype::Computed(declared)
            }
            Some(_) => Retype::Point(declared),
        }
    }
}

/// The type a value of `kind` and type `own` is re-stamped at by `retype`: [`target`]'s answer,
/// computed only for a union's members or a module's fit. `None` where it keeps `own`.
fn target_by(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    kind: Kind,
    own: DeclaredType<KType>,
    retype: Retype,
) -> Option<KType> {
    match retype {
        Retype::Point(point) => Some(point),
        Retype::Keeps => None,
        Retype::Computed(declared) | Retype::Fit(declared) => {
            target(types, scratch, kind, own, declared)
        }
    }
}

#[cfg(test)]
thread_local! {
    /// How many retype targets the run computed rather than read off a record, past a value
    /// already of the declared type.
    pub(crate) static TARGETS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The type a value of `kind` and type `own` is retyped to where a holder declares `declared`, after
/// the caller has checked it satisfies `declared`: the meet of what `declared`'s members — or
/// `declared` alone — of that kind take it at. A container, a tagged value, a function, a quote's
/// code or a type value is taken at each such member its type fits, a module at each application it fits, pinned
/// ([`pinned_at_fit`]). `None` where none is.
fn target(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    kind: Kind,
    own: DeclaredType<KType>,
    declared: KType,
) -> Option<KType> {
    #[cfg(test)]
    if own != declared.into() {
        TARGETS.with(|targets| targets.set(targets.get() + 1));
    }
    computed(types, scratch, kind, own, declared)
}

/// What the door takes a part of type `own` at where its holder names `declared`: [`target`],
/// uncounted, read off the type's kind; `own` where it keeps its own type.
pub(super) fn taken_at(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    own: KType,
    declared: KType,
) -> KType {
    retyped_kind(types, own.into())
        .and_then(|kind| computed(types, scratch, kind, own.into(), declared))
        .unwrap_or(own)
}

/// [`target`], uncounted: what a recorded retype is checked against.
fn computed(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    kind: Kind,
    own: DeclaredType<KType>,
    declared: KType,
) -> Option<KType> {
    if own == declared.into() {
        return None;
    }
    let constructor = |handle: KType| match types.node(handle) {
        TypeNode::ConstructorApply { constructor, .. } => constructor,
        _ => handle,
    };
    // A container is taken at a member its own type fits once filled against it: a part a
    // container's fill names a field for that it lacks reads that field as `null`.
    let filled_fits = |member: KType| {
        let own = own.as_type().expect(SEEN);
        fits(
            types,
            scratch,
            filled_against(types, scratch, own, member),
            member,
        )
    };
    let taken = |member: KType| {
        if retyped_kind(types, member.into()) != Some(kind) {
            return None;
        }
        match kind {
            Kind::Module => pinned_at_fit(types, scratch, own.as_type().expect(SEEN), member),
            Kind::Tagged if constructor(member) != constructor(own.as_type().expect(SEEN)) => None,
            Kind::List | Kind::Dict | Kind::Record => {
                (fits(types, scratch, own, member) || filled_fits(member)).then_some(member)
            }
            _ => fits(types, scratch, own, member).then_some(member),
        }
    };
    let mut members = BumpVec::new_in(scratch);
    match types.node(declared) {
        TypeNode::Union { members: held } => members.extend(held.iter()),
        _ => members.push(declared),
    }
    let target = members
        .iter()
        .filter_map(|member| taken(*member))
        .reduce(|lower, member| {
            if fits(types, scratch, lower, member) {
                lower
            } else if fits(types, scratch, member, lower) {
                member
            } else {
                meet(types, scratch, lower, member)
            }
        });
    match (kind, target) {
        (Kind::Record, Some(target)) => Some(keeping_null(types, scratch, own, target)),
        _ => target,
    }
}

/// `target`, a record retype's target, with each field the record's own type `own` lacks or has
/// at exactly `Null` at `Null`: a record seen at a type keeps the `Null`s its own type has. Its
/// own fields only; a nested record keeps its own as the door reads it.
fn keeping_null(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    own: DeclaredType<KType>,
    target: KType,
) -> KType {
    let (TypeNode::Record { fields: own }, TypeNode::Record { fields }) =
        (types.node(own.as_type().expect(SEEN)), types.node(target))
    else {
        return target;
    };
    let kept = |name: BinderSymbol, field: KType| {
        field != KType::NULL && own.get(name.symbol()).is_none_or(|own| own == KType::NULL)
    };
    if !fields.iter().any(|(name, field)| kept(name, field)) {
        return target;
    }
    let mut rebuilt = BumpVec::with_capacity_in(fields.len(), scratch);
    rebuilt.extend(fields.iter().map(|(name, field)| {
        (
            name,
            if kept(name, field) {
                KType::NULL
            } else {
                field
            },
        )
    }));
    types.record(scratch, &rebuilt)
}

/// The applications the signature type `carried` holds, in `scratch`: a meet's members, or
/// `carried` alone.
pub fn applications<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    carried: KType,
) -> BumpVec<'x, KType> {
    let mut applications = BumpVec::new_in(scratch);
    match types.node(carried) {
        TypeNode::SignatureMeet { members } => applications.extend(members.iter()),
        _ => applications.push(carried),
    }
    applications
}

/// The type a module of signature `own` is retyped to at the signature type `declared`: each
/// application `declared` holds, its head parameters pinned at what *fits* solves them to — the
/// signature itself where it has none — and the meet of those for a meet. `None` where `own` fits
/// one of them not, or `declared` is no signature type. The load pins an exact operand with it, so
/// a header's type is the one the load computes.
pub fn pinned_at_fit(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    own: KType,
    declared: KType,
) -> Option<KType> {
    let (signature, pinned) = match types.node(declared) {
        TypeNode::SignatureMeet { members } => {
            let mut pinned = BumpVec::with_capacity_in(members.len(), scratch);
            for member in members.iter() {
                pinned.push(pinned_at_fit(types, scratch, own, member)?);
            }
            return Some(types.signature_meet(scratch, &pinned));
        }
        TypeNode::Signature { schema, .. } if schema.parameters.is_empty() => {
            return fits(types, scratch, own, declared).then_some(declared);
        }
        TypeNode::Signature { .. } => (declared, None),
        TypeNode::SignatureApply { signature, pins } => (signature, Some(pins)),
        _ => return None,
    };
    let mut pins = BumpVec::new_in(scratch);
    pins.extend(pinned.iter().flat_map(|pins| pins.iter()));
    let solution = fit_solution(types, scratch, own, signature, &pins).ok()?;
    for (name, solved) in solution.iter() {
        let name = BinderSymbol::Type(*name);
        if pins.iter().all(|(pinned, _)| *pinned != name) {
            pins.push((name, *solved));
        }
    }
    Some(types.signature_apply(scratch, signature, &pins))
}

/// What *fits* solves the head parameters of `signature` to, under `pins`, for a module of type
/// `own`: each concrete, since a module's type offers no stand-in to the solve. Refused where `own`
/// does not fit the application. What both a retype's pins and a view's `from` are read from.
pub fn fit_solution<'run: 'x, 'x>(
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    own: KType,
    signature: KType,
    pins: &[(BinderSymbol, KType)],
) -> Result<Members<'x, TypeSymbol, KType>, FitsFailure<'run, 'x>> {
    let solution = fits_application(types, scratch, own, signature, pins)?;
    Ok(Members::from_pairs(
        scratch,
        solution.iter().map(|(name, solved)| {
            let solved = types
                .concrete(*solved)
                .expect("a module's type offers no stand-in to what fits solves");
            (*name, solved)
        }),
    ))
}

/// The type a value of type `own` is retyped to where a holder declares `declared`, after the
/// caller has checked it satisfies `declared`: [`Seen::seen_at`]'s target, its kind read off
/// `own`'s node, and `own` itself where nothing re-stamps — what a knot's tie births an annotated
/// member at.
pub fn retype_target(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    own: DeclaredType<KType>,
    declared: KType,
) -> DeclaredType<KType> {
    node_kind(own.view(types).body)
        .and_then(|kind| target(types, scratch, kind, own, declared))
        .map_or(own, DeclaredType::from)
}

/// [`retype_target`] to `declared`, read off `retype` where the load recorded one for it.
pub fn retype_target_as(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    own: DeclaredType<KType>,
    declared: KType,
    retype: Option<Retype>,
) -> DeclaredType<KType> {
    let Some(retype) = retype else {
        return retype_target(types, scratch, own, declared);
    };
    let kind = node_kind(own.view(types).body);
    let seen = |target: Option<KType>| target.map_or(own, DeclaredType::from);
    let by = seen(kind.and_then(|kind| target_by(types, scratch, kind, own, retype)));
    debug_assert_eq!(
        by,
        seen(kind.and_then(|kind| computed(types, scratch, kind, own, declared))),
        "a recorded retype re-stamps where the target is"
    );
    by
}

/// The kind of value a retype to `declared` re-stamps at `declared` itself: a list, dict or
/// record type for a container of that kind, a nominal type or a family's application for a
/// tagged value under its constructor, a function type or an expression shape for a function or a
/// barrier, a signature type for a module, a code kind below `Code` for a quote's code, and a kind
/// for a type value. `None` for every other type, which a value satisfying it keeps its own type
/// under — `Module`, every module's type, and `Code`, every code kind's, among them, as `Any` is
/// every value's.
fn retyped_kind(types: &Chain<'_, '_, '_, '_>, declared: Parametric) -> Option<Kind> {
    if declared == KType::EMPTY_SIGNATURE.into() {
        return None;
    }
    node_kind(types.node(declared))
}

/// Whether a retype to `declared` lays a header over a function, a module or a quote's code — the
/// knot members a header stands over beside a data node: [`retyped_kind`] answers one of their
/// kinds at `declared`, or at a member of the union `declared` is. What a content tree retypes.
pub fn retypes_by_header(types: &Chain<'_, '_, '_, '_>, declared: KType) -> bool {
    let header = |member: KType| {
        matches!(
            retyped_kind(types, member.into()),
            Some(Kind::Function | Kind::Module | Kind::Code)
        )
    };
    match types.node(declared) {
        TypeNode::Union { members } => members.iter().any(header),
        _ => header(declared),
    }
}

/// [`retyped_kind`] of a type's node.
fn node_kind(node: TypeNode<'_, Parametric>) -> Option<Kind> {
    match node {
        TypeNode::List { .. } => Some(Kind::List),
        TypeNode::Dict { .. } => Some(Kind::Dict),
        TypeNode::Record { .. } => Some(Kind::Record),
        TypeNode::ConstructorApply { .. } | TypeNode::SetMember { .. } => Some(Kind::Tagged),
        TypeNode::KFunction { .. } | TypeNode::ExpressionShape { .. } => Some(Kind::Function),
        TypeNode::Signature { .. }
        | TypeNode::SignatureApply { .. }
        | TypeNode::SignatureMeet { .. } => Some(Kind::Module),
        TypeNode::Block
        | TypeNode::Expression
        | TypeNode::Declaration
        | TypeNode::Binder
        | TypeNode::Literal
        | TypeNode::Symbol
        | TypeNode::Name
        | TypeNode::Identifier
        | TypeNode::TypeNameToken
        | TypeNode::Keyword
        | TypeNode::SigiledTypeExpr
        | TypeNode::RecordType
        | TypeNode::CodeNeeding { .. } => Some(Kind::Code),
        TypeNode::OfKind(_) => Some(Kind::Type),
        _ => None,
    }
}

/// The static type of a value retyped to `declared` (an ascription's, a parameter's, a frame's
/// return): exactly `declared` where every value satisfying it is retyped to it — a list, dict or
/// record type, a family or its application, a newtype or a union's variant, a function type, an
/// expression shape, a code kind below `Code`, a kind, or a signature type pinning every head
/// parameter — and at most `declared`
/// otherwise; a union keeps each variant's own type. A module is pinned afresh at each run where
/// `declared` leaves a parameter unpinned, and two pins of one parameter are unordered. It reads
/// the predicate [`Seen::seen_at`] retypes by. A record type with a field `Null` lies strictly
/// under is retyped to at most itself, and at least itself with each such field at `Null`, since a
/// record keeps the `Null`s its own type has; where such a field is parametric, at most itself.
pub fn retyped_to(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    declared: Parametric,
) -> Interval {
    match retyped_kind(types, declared) {
        Some(Kind::Module) if !pins_every_parameter(types, declared) => under(declared),
        Some(Kind::Record) => record_retyped_to(types, scratch, declared),
        Some(_) => Interval::point(declared),
        None => under(declared),
    }
}

/// Whether `field`, a record type's field, is one a record seen at it may keep at `Null`: `Null`
/// lies strictly under it — under its bound above where it holds a variable.
fn nullable(types: &Chain<'_, '_, '_, '_>, scratch: BumpAllocator<'_>, field: Parametric) -> bool {
    field != KType::NULL.into()
        && match types.concrete(field) {
            Some(field) => is_subtype_of(types, scratch, KType::NULL, field),
            None => is_subtype_of(
                types,
                scratch,
                KType::NULL,
                bound_above(types, scratch, field),
            ),
        }
}

/// Whether a retype to the record type `declared` re-stamps every record at `declared` itself: no
/// field of it is [`nullable`].
fn record_is_point(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    declared: Parametric,
) -> bool {
    let TypeNode::Record { fields } = types.node(declared) else {
        unreachable!("a record retype's type is a record type")
    };
    !fields.values().any(|field| nullable(types, scratch, field))
}

/// [`retyped_to`] a record type.
fn record_retyped_to(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    declared: Parametric,
) -> Interval {
    let TypeNode::Record { fields } = types.node(declared) else {
        unreachable!("a record retype's type is a record type")
    };
    let mut lower = BumpVec::with_capacity_in(fields.len(), scratch);
    let mut keeps = false;
    for (name, field) in fields.iter() {
        if !nullable(types, scratch, field) {
            lower.push((name, field));
        } else if types.concrete(field).is_some() {
            keeps = true;
            lower.push((name, KType::NULL.into()));
        } else {
            return under(declared);
        }
    }
    if !keeps {
        return Interval::point(declared);
    }
    Interval {
        lower: types.record(scratch, &lower),
        upper: declared,
    }
}

/// Whether every application the signature type `declared` holds pins each of its head
/// parameters, a lexical variable counting as a pin.
fn pins_every_parameter(types: &Chain<'_, '_, '_, '_>, declared: Parametric) -> bool {
    let pinned = |application: Parametric| match types.node(application) {
        TypeNode::Signature { schema, .. } => schema.parameters.is_empty(),
        TypeNode::SignatureApply { signature, pins } => match types.node(signature) {
            TypeNode::Signature { schema, .. } => (schema.parameters.iter())
                .all(|(name, _)| pins.get(BinderSymbol::Type(*name).symbol()).is_some()),
            _ => false,
        },
        _ => false,
    };
    match types.node(declared) {
        TypeNode::SignatureMeet { members } => members.iter().all(pinned),
        _ => pinned(declared),
    }
}

/// A static type under `upper` and bounded below by nothing — exact where `upper` is `Number`,
/// `Str`, `Bool` or `Null`, since no value carries a type strictly under one.
pub fn under(upper: Parametric) -> Interval {
    let leaf = [KType::NUMBER, KType::STR, KType::BOOL, KType::NULL]
        .into_iter()
        .any(|leaf| upper == leaf.into());
    if leaf {
        Interval::point(upper)
    } else {
        Interval {
            lower: KType::NEVER.into(),
            upper,
        }
    }
}

/// `[Never, Any]`: what the load cannot bound.
pub fn unknown() -> Interval {
    under(KType::ANY.into())
}
