//! A list: one run of cells in the region, typed by the union of its elements' filled types
//! ([fill.rs](super::fill)) — or `List<Any>` for a
//! key's candidates, which dispatch alone reads. Its cells are value words, or [`Link`]s when the
//! list is a knot's data node. A flat run lays the prefix sums of its cells' weights beside it.
//!
//! A slice or a concatenation is a view instead ([view.rs](super::view)): a tree over the runs of
//! the lists it was built from, laying none of their cells. A slice carries its source's type, a
//! concatenation the type a flat list of its elements would carry, derived from its sources'
//! element types alone. Every reader outside this file reads elements through [`List::get`] or
//! [`List::elements`], so a view reads as the flat list it stands for.
//!
//! A knot's data node that spreads a list is a view too ([`List::linked_pieces`]): its own cells
//! and its fellows' runs are **own** runs, laid before the knot exists, so their edges resolve
//! through the member its reader holds ([`List::link`], [`List::links`]). A view of other lists
//! over it holds its tree once, under that member ([`view::held`]).

use std::cell::Cell;
use std::marker::PhantomData;

use crate::memory::{Bump, BumpAllocator, BumpVec, Writer, resident};
use crate::type_lattice::{KType, TypeNode};

use super::fill::hides_more;
use super::hiding::{held_in, unhidden_cells};
use super::settled::{composite_bits, part_hides, restamp_bits};
use super::view::{self, BLOCK, ListNode, Prefix, Run, Weighed, prefixes};
use super::{Chain, Circular, Knotted, Link, MemoBits, Nothing, Value, Weight, list_type};

/// A list value, resident in the region its cells live in: one flat run, or a view over the runs
/// of the lists it was sliced or concatenated from ([view.rs](super::view)).
#[derive(Debug)]
pub struct List<'cell, X = Nothing, C = Value<'cell, X>> {
    storage: Storage<'cell, X, C>,
    ktype: KType,
    weight: Cell<Weight>,
    /// The knot member a cell's word may hold, which a link cell names only through `C`.
    member: PhantomData<Value<'cell, X>>,
}

/// Where a list's elements live.
#[derive(Clone, Copy, Debug)]
enum Storage<'cell, X, C> {
    /// One run of cells, and the prefix sums of their weights every [`BLOCK`] cells.
    Flat {
        cells: &'cell [C],
        prefix: &'cell [Prefix],
    },
    /// A tree of stretches of other lists' flat runs. A knot's data node that spreads a list is
    /// one, its own runs' edges resolved through the member its reader holds.
    View(&'cell ListNode<'cell, X>),
}

/// What a list over `len` elements weighing `cells` bytes writes: its struct, its cells and the
/// prefix sums beside them. A view weighs what its flat copy writes.
fn list_weight<L>(cells: usize, len: usize) -> Weight {
    Weight::flat::<L>()
        .plus(Weight::text(cells))
        .plus(Weight::run::<Prefix>(len / BLOCK + 1))
}

/// `items` written as a run of cells into `writer`'s region.
fn filled<'cell, X: Knotted>(
    writer: Writer<'cell, '_>,
    items: impl ExactSizeIterator<Item = Value<'cell, X>>,
) -> &'cell [Value<'cell, X>] {
    let mut items = items;
    writer.fill(items.len(), |_| {
        items
            .next()
            .expect("an exact-size iterator yields its reported length")
    })
}

impl<'cell, X: Knotted> List<'cell, X> {
    /// Lay down `items` as the list's cells. The element type is the union of the cells' filled
    /// types ([`list_type`]) — `Never` for an empty list — and the weight is summed in the same
    /// pass. A cell that would show a field it hides is copied without it ([`unhidden_cells`]).
    pub fn new(
        writer: Writer<'cell, '_>,
        items: impl ExactSizeIterator<Item = Value<'cell, X>>,
        types: &Chain<'_, 'cell, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> &'cell List<'cell, X> {
        let cells = filled(writer, items);
        let ktype = list_type(types, scratch, cells.iter().map(Value::concrete_ktype));
        let cells = unhidden_cells(writer, types, scratch, cells, ktype);
        Self::weighed(writer, cells, ktype, types)
    }

    /// Lay down a key's candidates — the functions a `USING` hole or an `EVAL` offer gathers at
    /// one key — typed `List<Any>` without reading any function's type. Dispatch alone reads such
    /// a list, each function by its own type; a quantified registration's type is a scheme, which
    /// no list type joins.
    pub fn of_candidates(
        writer: Writer<'cell, '_>,
        functions: impl ExactSizeIterator<Item = Value<'cell, X>>,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> &'cell List<'cell, X> {
        Self::typed(writer, functions, KType::LIST_OF_ANY, types)
    }

    /// Lay down `items` under `ktype`, a type the caller knows they satisfy — a view's rebuilt
    /// list, at the type its slot declares — so no type is interned.
    pub fn typed(
        writer: Writer<'cell, '_>,
        items: impl ExactSizeIterator<Item = Value<'cell, X>>,
        ktype: KType,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> &'cell List<'cell, X> {
        let cells = filled(writer, items);
        let cells = unhidden_cells(writer, types, &Bump::new(), cells, ktype);
        Self::weighed(writer, cells, ktype, types)
    }

    /// Elements `[from, to)` of `source`, a list plain or a knot's data node, read backward where
    /// `backward` is set, at `source`'s own type: a view over its runs, or a flat empty list where
    /// it holds none. A slice is typed by its source, so its type costs nothing.
    pub fn sliced(
        writer: Writer<'cell, '_>,
        source: Value<'cell, X>,
        (from, to): (usize, usize),
        backward: bool,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> &'cell Self {
        let ktype = source.concrete_ktype();
        let bits = composite_bits(types, ktype, [&source]);
        let (tree, hides) = tree(writer, source);
        let node = tree.and_then(|tree| view::sliced(writer, tree, from, to));
        let node = match backward {
            true => node.map(|node| view::reversed(writer, node)),
            false => node,
        };
        Self::viewed(writer, node, ktype, bits, hides, types)
    }

    /// `pieces` in order, at the list type of their elements' types — each spread source's element
    /// type standing for its elements: a view over the spread sources' runs and a flat run of the
    /// items between them. A source whose cells may hide a field the new element type would show
    /// is copied flat at it instead, as [`new`](Self::new) copies an item.
    pub fn concatenated(
        writer: Writer<'cell, '_>,
        pieces: &[Piece<'_, 'cell, X>],
        types: &Chain<'_, 'cell, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> &'cell Self {
        let element = |source: Value<'cell, X>| match types.node(source.concrete_ktype()) {
            TypeNode::List { element } => element,
            _ => unreachable!("a spread source is a list"),
        };
        let mut elements = BumpVec::new_in(scratch);
        for piece in pieces {
            match piece {
                Piece::Items(items) => elements.extend(items.iter().map(Value::concrete_ktype)),
                Piece::Spread(source) => elements.push(element(*source)),
            }
        }
        let ktype = list_type(types, scratch, elements.into_iter());
        let TypeNode::List { element: new } = types.node(ktype) else {
            unreachable!("a list type")
        };
        // Items, and a source copied flat, as one leaf over a fresh run.
        let flat = |cells: &'cell [Value<'cell, X>]| {
            let cells = unhidden_cells(writer, types, scratch, cells, ktype);
            let node = (!cells.is_empty()).then(|| {
                view::leaf(
                    writer,
                    Run::Plain(cells),
                    prefixes(writer, cells),
                    0,
                    cells.len(),
                )
            });
            (cells, node)
        };
        let (mut joined, mut hides) = (None, false);
        let mut parts = BumpVec::new_in(scratch);
        for piece in pieces {
            let node =
                match *piece {
                    Piece::Items(items) => {
                        let (cells, node) = flat(filled(writer, items.iter().copied()));
                        hides |= cells.iter().any(part_hides);
                        parts.extend(cells.iter().copied());
                        node
                    }
                    Piece::Spread(source) => {
                        parts.push(source);
                        let (node, source_hides) = tree(writer, source);
                        if source_hides && element(source) != new {
                            let surface = source.surface(types, scratch).expect("a list opens");
                            let mut read = BumpVec::with_capacity_in(surface.len(), scratch);
                            read.extend((0..surface.len()).map(|at| {
                                surface.child(at, types, scratch).restamped(writer, types)
                            }));
                            let (cells, node) = flat(filled(writer, read.into_iter()));
                            hides |= cells.iter().any(part_hides);
                            node
                        } else {
                            hides |= source_hides;
                            node
                        }
                    }
                };
            joined = view::joined(writer, joined, node);
        }
        let bits = composite_bits(types, ktype, parts.iter());
        Self::viewed(writer, joined, ktype, bits, hides, types)
    }

    /// A view over `node` under `ktype`, `bits` and `hides`, or a flat empty list where `node` is
    /// `None`.
    fn viewed(
        writer: Writer<'cell, '_>,
        node: Option<&'cell ListNode<'cell, X>>,
        ktype: KType,
        bits: MemoBits,
        hides: bool,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> &'cell Self {
        let Some(node) = node else {
            return Self::weighed(writer, &[], ktype, types);
        };
        let weight = list_weight::<Self>(node.weight(), node.len());
        resident(
            writer,
            List {
                storage: Storage::View(node),
                ktype,
                weight: Cell::new(weight.with_bits(bits).with_hides(hides)),
                member: PhantomData,
            },
        )
    }

    /// A list over value cells already resident in `writer`'s region under `ktype`, weighed as
    /// [`new`](Self::new) weighs them — a retyped data node's arm.
    pub(crate) fn weighed(
        writer: Writer<'cell, '_>,
        cells: &'cell [Value<'cell, X>],
        ktype: KType,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> &'cell Self {
        let prefix = prefixes(writer, cells);
        let bits = composite_bits(types, ktype, cells);
        let hides = cells.iter().any(part_hides);
        Self::from_run(
            writer,
            cells,
            prefix,
            ktype,
            flat_weight::<Self, _>(cells, prefix)
                .with_bits(bits)
                .with_hides(hides),
        )
    }
}

impl<'cell, X: Knotted> List<'cell, X, Link<'cell, X>> {
    /// Lay down `cells` as a knot's data node under the finished memo `ktype`, which the tie
    /// derived from the cells and the members their edges name.
    pub fn linked(
        writer: Writer<'cell, '_>,
        cells: &[Link<'cell, X>],
        ktype: KType,
    ) -> &'cell Self {
        let cells = writer.fill(cells.len(), |at| cells[at]);
        let prefix = prefixes(writer, cells);
        Self::from_run(
            writer,
            cells,
            prefix,
            ktype,
            flat_weight::<Self, _>(cells, prefix),
        )
    }

    /// `pieces` in order as a knot's data node under the finished memo `ktype`, beside whether its
    /// cells may hide a field: flat where it spreads nothing, else a view over its own cells, the
    /// runs of the lists spread into it and its fellows' own runs, as
    /// [`concatenated`](List::concatenated) builds one. A source whose cells may hide a field the
    /// memo's element type would show is copied flat at it instead, a fellow's edges kept. Its
    /// own value cells are held at the memo already.
    pub fn linked_pieces(
        writer: Writer<'cell, '_>,
        pieces: &[LinkedPiece<'_, 'cell, X>],
        ktype: KType,
        types: &Chain<'_, 'cell, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> (&'cell Self, bool) {
        let cells_hide = |cells: &[Link<'cell, X>]| {
            (cells.iter()).any(|cell| matches!(cell, Link::Value(value) if part_hides(value)))
        };
        match pieces {
            [] => return (Self::linked(writer, &[], ktype), false),
            [LinkedPiece::Items(cells)] => {
                return (Self::linked(writer, cells, ktype), cells_hide(cells));
            }
            _ => {}
        }
        let element = |ktype: KType| match types.node(ktype) {
            TypeNode::List { element } => element,
            _ => unreachable!("a list's memo is a list type"),
        };
        let new = element(ktype);
        let own = |cells: &'cell [Link<'cell, X>]| {
            (!cells.is_empty()).then(|| {
                let run = Run::Linked(cells, None);
                view::leaf(writer, run, prefixes(writer, cells), 0, cells.len())
            })
        };
        // A value seen at the source's element type, held at the memo's.
        let held = |value| held_in(writer, types, scratch, value, ktype);
        // A run laid flat at the memo's element type, beside whether its cells hide a field.
        let copied = |cells: &'cell [Link<'cell, X>]| (own(cells), cells_hide(cells));
        let (mut joined, mut hides) = (None, false);
        for piece in pieces {
            let (node, piece_hides) = match *piece {
                LinkedPiece::Items(items) => copied(writer.fill(items.len(), |at| items[at])),
                LinkedPiece::Spread(source) => match tree(writer, source) {
                    (_, true) if element(source.concrete_ktype()) != new => {
                        let surface = source.surface(types, scratch).expect("a list opens");
                        copied(writer.fill(surface.len(), |at| {
                            let child = surface.child(at, types, scratch);
                            Link::Value(held(child.restamped(writer, types)))
                        }))
                    }
                    shared => shared,
                },
                LinkedPiece::Fellow(fellow, true) if element(fellow.ktype) != new => {
                    let mut links = fellow.links();
                    copied(writer.fill(fellow.len(), |_| {
                        match links.next().expect("as many links as its length") {
                            Link::Value(value) => Link::Value(held(value)),
                            edge => edge,
                        }
                    }))
                }
                LinkedPiece::Fellow(fellow, hides) => (fellow.own_tree(writer), hides),
            };
            hides |= piece_hides;
            joined = view::joined(writer, joined, node);
        }
        let Some(node) = joined else {
            return (Self::linked(writer, &[], ktype), false);
        };
        let list = resident(
            writer,
            List {
                storage: Storage::View(node),
                ktype,
                weight: Cell::new(list_weight::<Self>(node.weight(), node.len())),
                member: PhantomData,
            },
        );
        (list, hides)
    }

    /// Its tree with its own runs waiting for a holder: a view's own, or one leaf over its run;
    /// `None` where it holds nothing.
    fn own_tree(&self, writer: Writer<'cell, '_>) -> Option<&'cell ListNode<'cell, X>> {
        match self.storage {
            Storage::Flat { cells, prefix } => (!cells.is_empty())
                .then(|| view::leaf(writer, Run::Linked(cells, None), prefix, 0, cells.len())),
            Storage::View(node) => Some(node),
        }
    }

    /// The link at `at`, below [`len`](Self::len): a value, or an edge into its own knot.
    pub fn link(&self, at: usize) -> Link<'cell, X> {
        match self.storage {
            Storage::Flat { cells, .. } => cells[at],
            Storage::View(node) => node.link(at),
        }
    }

    /// Every link, in order.
    pub fn links(&self) -> Links<'cell, X> {
        match self.storage {
            Storage::Flat { cells, .. } => Links {
                flat: cells.iter(),
                view: None,
            },
            Storage::View(node) => Links {
                flat: [].iter(),
                view: Some(node.links()),
            },
        }
    }

    /// This node rebuilt flat in `writer`'s region, each link through [`Link::copied`], type and
    /// weight's bytes carried over: a data node keeps no memo bits.
    pub fn copied<'to, Y: Knotted>(
        &self,
        writer: Writer<'to, '_>,
        copy: &mut impl FnMut(&Value<'cell, X>) -> Value<'to, Y>,
    ) -> &'to List<'to, Y, Link<'to, Y>> {
        let weight = self.weight().without_bits();
        match self.storage {
            Storage::Flat { cells, prefix } => {
                let cells = writer.fill(cells.len(), |at| cells[at].copied(copy));
                let prefix = writer.fill(prefix.len(), |at| prefix[at]);
                List::from_run(writer, cells, prefix, self.ktype, weight)
            }
            Storage::View(_) => {
                let mut links = self.links();
                let cells = writer.fill(self.len(), |_| {
                    (links.next().expect("as many links as its length")).copied(copy)
                });
                List::from_run(writer, cells, prefixes(writer, cells), self.ktype, weight)
            }
        }
    }
}

/// A piece of a knot's data node: its own cells, a list spread into it from outside its knot, or
/// a fellow data node of its knot, laid already, beside whether its cells may hide a field.
#[derive(Clone, Copy)]
pub enum LinkedPiece<'a, 'cell, X> {
    Items(&'a [Link<'cell, X>]),
    Spread(Value<'cell, X>),
    Fellow(&'cell List<'cell, X, Link<'cell, X>>, bool),
}

/// A knot's data node's links in order: its flat run's, or its view's.
pub struct Links<'cell, X> {
    flat: std::slice::Iter<'cell, Link<'cell, X>>,
    view: Option<view::Links<'cell, X>>,
}

impl<'cell, X: Knotted> Iterator for Links<'cell, X> {
    type Item = Link<'cell, X>;

    fn next(&mut self) -> Option<Link<'cell, X>> {
        match &mut self.view {
            Some(view) => view.next(),
            None => self.flat.next().copied(),
        }
    }
}

impl<'cell, X: Copy, C: Copy> List<'cell, X, C> {
    /// A list over cells and their prefix sums already resident in `writer`'s region, under a type
    /// and weight the caller already knows — the deep copy's arm.
    pub(crate) fn from_run(
        writer: Writer<'cell, '_>,
        cells: &'cell [C],
        prefix: &'cell [Prefix],
        ktype: KType,
        weight: Weight,
    ) -> &'cell Self {
        resident(
            writer,
            List {
                storage: Storage::Flat { cells, prefix },
                ktype,
                weight: Cell::new(weight),
                member: PhantomData,
            },
        )
    }

    /// The same cells under `ktype` — an ascription's retype, sharing the run, its bits
    /// recomputed from the new memo.
    pub fn with_type(
        &self,
        writer: Writer<'cell, '_>,
        ktype: KType,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> &'cell Self {
        let weight = self.weight.get();
        let weight = weight
            .with_bits(restamp_bits(weight.bits(), types, ktype))
            .with_hides(weight.hides() || hides_more(types, self.ktype, ktype));
        resident(
            writer,
            List {
                storage: self.storage,
                ktype,
                weight: Cell::new(weight),
                member: PhantomData,
            },
        )
    }

    /// How many elements it holds: a flat run's length, or a view's memo. O(1).
    pub fn len(&self) -> usize {
        match self.storage {
            Storage::Flat { cells, .. } => cells.len(),
            Storage::View(node) => node.count(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn ktype(&self) -> KType {
        self.ktype
    }

    pub fn weight(&self) -> Weight {
        self.weight.get()
    }

    /// Where the value's type memos resolve, as its weight records it.
    pub fn bits(&self) -> MemoBits {
        self.weight.get().bits()
    }

    /// Record that every memo the value reaches resolves in its home half's branch — an exit's
    /// mark on what it adopted. Keeps root-closed.
    pub(crate) fn settle(&self) {
        let weight = self.weight.get();
        self.weight.set(weight.with_bits(MemoBits {
            settled: true,
            ..weight.bits()
        }));
    }

    /// Record that a cell hides a field at its view under the value's own type — the mark a
    /// holder rebuilt over a copy takes ([hiding.rs](super::hiding)) where its parts' bits miss it.
    pub(super) fn hide(&self) {
        self.weight.set(self.weight.get().with_hides(true));
    }
}

/// A piece of a concatenation: items laid flat, or the elements of a list spread into it.
#[derive(Clone, Copy)]
pub enum Piece<'a, 'cell, X> {
    Items(&'a [Value<'cell, X>]),
    Spread(Value<'cell, X>),
}

/// The tree of `source`'s elements, a list plain or a knot's data node, beside whether its cells
/// may hide a field: a view's own tree, or one leaf over a flat run, a data node's held by its
/// member; `None` where it holds none. A data node keeps no hides bit, so one is taken to hide.
fn tree<'cell, X: Knotted>(
    writer: Writer<'cell, '_>,
    source: Value<'cell, X>,
) -> (Option<&'cell ListNode<'cell, X>>, bool) {
    let flat = |run, prefix, len| (len > 0).then(|| view::leaf(writer, run, prefix, 0, len));
    match source {
        Value::List(list) => {
            let tree = match list.storage {
                Storage::Flat { cells, prefix } => flat(Run::Plain(cells), prefix, cells.len()),
                Storage::View(node) => Some(node),
            };
            (tree, part_hides(&source))
        }
        _ => match source.as_circular() {
            Some((member, Circular::List(list))) => (
                (list.own_tree(writer)).map(|node| view::held(writer, node, member)),
                true,
            ),
            _ => unreachable!("a list's elements are read off a list"),
        },
    }
}

/// What a flat list over `cells`, prefix sums `prefix`, weighs.
fn flat_weight<L, C: Weighed>(cells: &[C], prefix: &[Prefix]) -> Weight {
    let bytes = prefix.last().expect("a prefix run is never empty");
    let tail = &cells[(prefix.len() - 1) * BLOCK..];
    let bytes = tail.iter().fold(bytes.bytes(), |sum, cell| {
        sum.saturating_add(cell.weighed().0)
    });
    list_weight::<L>(bytes, cells.len())
}

/// The elements a read outside `values` reaches only through [the door](super::surface).
impl<'cell, X: Knotted> List<'cell, X> {
    /// The element at `at`, below [`len`](Self::len): O(1) flat, O(log n) in a view.
    pub(super) fn get(&self, at: usize) -> Value<'cell, X> {
        match self.storage {
            Storage::Flat { cells, .. } => cells[at],
            Storage::View(node) => plain(node.link(at)),
        }
    }

    /// Every element, in order.
    pub(super) fn elements(&self) -> Elements<'cell, X> {
        let (flat, view): (&[Value<'cell, X>], _) = match self.storage {
            Storage::Flat { cells, .. } => (cells, None),
            Storage::View(node) => (&[], Some(node.links())),
        };
        Elements {
            flat: flat.iter(),
            view,
            left: self.len(),
        }
    }

    /// Whether `self` and `other` hold one storage: one flat run, or one view's tree.
    pub(super) fn same_run(&self, other: &Self) -> bool {
        match (self.storage, other.storage) {
            (Storage::Flat { cells: a, .. }, Storage::Flat { cells: b, .. }) => {
                std::ptr::eq(a.as_ptr(), b.as_ptr())
            }
            (Storage::View(a), Storage::View(b)) => std::ptr::eq(a, b),
            _ => false,
        }
    }

    /// The flat run, in a test that built one.
    #[cfg(test)]
    pub(crate) fn cells(&self) -> &'cell [Value<'cell, X>] {
        match self.storage {
            Storage::Flat { cells, .. } => cells,
            Storage::View(_) => panic!("a view has no run of its own"),
        }
    }
}

#[cfg(test)]
impl<X: Knotted, C> List<'_, X, C> {
    /// Whether it is a view.
    pub(crate) fn is_view(&self) -> bool {
        matches!(self.storage, Storage::View(_))
    }

    /// Whether a view's tree is balanced and every memo in it is its recomputed value; a flat list
    /// holds no tree.
    pub(crate) fn holds_its_laws(&self) -> bool {
        match self.storage {
            Storage::Flat { .. } => true,
            Storage::View(node) => node.holds_its_laws(),
        }
    }
}

/// A list's elements in order: its flat run's, or its view's.
pub(super) struct Elements<'cell, X> {
    flat: std::slice::Iter<'cell, Value<'cell, X>>,
    view: Option<view::Links<'cell, X>>,
    left: usize,
}

impl<'cell, X: Knotted> Iterator for Elements<'cell, X> {
    type Item = Value<'cell, X>;

    fn next(&mut self) -> Option<Value<'cell, X>> {
        let next = match &mut self.view {
            Some(view) => view.next().map(plain),
            None => self.flat.next().copied(),
        };
        self.left -= usize::from(next.is_some());
        next
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.left, Some(self.left))
    }
}

impl<X: Knotted> ExactSizeIterator for Elements<'_, X> {}

/// A plain list's element: a plain tree resolves every edge through a holder.
fn plain<X>(link: Link<'_, X>) -> Value<'_, X> {
    match link {
        Link::Value(value) => value,
        Link::Edge(_) => unreachable!("only a knot's own list waits for a holder"),
    }
}
