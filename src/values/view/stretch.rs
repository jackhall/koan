//! A list view's leaves: each a [`Stretch`] of one flat run of values or links, read forward or
//! backward, its summary its length, the bytes its flat copy writes and how many of its cells are
//! an own run's edges.
//!
//! A run of a knot's data node resolves its edges through a holder, the knot member that reads
//! it. A knot's own list is laid before its knot exists, so its runs name no holder: an **own**
//! run's edges resolve through the holder its reader brings. A view of other lists over it is held
//! by one branch naming its holder ([`held`](super::held)), the tree's lazy tag, so every read and
//! edit stays O(log n).
//!
//! A flat run keeps a [`Prefix`] sum of its cells' weights every [`BLOCK`] cells
//! ([list.rs](super::super::list) lays it), so a stretch of it is weighed in O(1): a view weighs
//! what its flat copy writes. A view's type, bits and hides bit are its builder's; nothing here
//! reads a type.

use crate::memory::Writer;

use super::super::{Knotted, Link, Value};
use super::{Leaf, Node, Summary, Tree, step};

/// How many cells of a flat run one [`Prefix`] entry spans.
pub(in crate::values) const BLOCK: usize = 16;

/// The weight of a flat run's cells before a block boundary: the bytes their copy writes past the
/// list's own struct, and how many of them are edges, whose copy also copies their holder's knot.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Prefix {
    bytes: usize,
    edges: usize,
}

impl Prefix {
    /// The bytes its cells' copy writes, edges' knots aside.
    pub(in crate::values) fn bytes(&self) -> usize {
        self.bytes
    }
}

/// One cell's share of a [`Prefix`]: its bytes, and whether it is an edge.
pub(in crate::values) trait Weighed {
    fn weighed(&self) -> (usize, bool);
}

impl<X: Knotted> Weighed for Value<'_, X> {
    fn weighed(&self) -> (usize, bool) {
        (self.weight().bytes(), false)
    }
}

impl<X: Knotted> Weighed for Link<'_, X> {
    fn weighed(&self) -> (usize, bool) {
        (self.weight().bytes(), matches!(self, Link::Edge(_)))
    }
}

/// The prefix sums of `cells`, laid through `writer`: entry `b` sums cells `[0, BLOCK·b)`, for `b`
/// in `0..=len / BLOCK`.
pub(in crate::values) fn prefixes<'cell, C: Weighed>(
    writer: Writer<'cell, '_>,
    cells: &[C],
) -> &'cell [Prefix] {
    let mut sum = Prefix::default();
    writer.fill(cells.len() / BLOCK + 1, |block| {
        let at = sum;
        if let Some(cells) = cells.get(block * BLOCK..(block + 1) * BLOCK) {
            sum = add(sum, cells);
        }
        at
    })
}

/// `sum` plus every cell of `cells`, saturating.
fn add<C: Weighed>(sum: Prefix, cells: &[C]) -> Prefix {
    cells.iter().fold(sum, |sum, cell| {
        let (bytes, edge) = cell.weighed();
        Prefix {
            bytes: sum.bytes.saturating_add(bytes),
            edges: sum.edges + usize::from(edge),
        }
    })
}

/// What cells `[from, to)` of a flat run weigh: the prefix difference over the whole blocks
/// between them, and a direct sum over the at most `BLOCK - 1` cells on each side. Saturates.
fn stretch_weight<C: Weighed>(cells: &[C], prefix: &[Prefix], from: usize, to: usize) -> Prefix {
    let (first, last) = (from.div_ceil(BLOCK), to / BLOCK);
    if first > last {
        return add(Prefix::default(), &cells[from..to]);
    }
    let (low, high) = (prefix[first], prefix[last]);
    let whole = Prefix {
        bytes: match high.bytes {
            usize::MAX => usize::MAX,
            bytes => bytes - low.bytes,
        },
        edges: high.edges - low.edges,
    };
    let sides = add(
        add(whole, &cells[from..first * BLOCK]),
        &cells[last * BLOCK..to],
    );
    step();
    sides
}

/// One flat run a stretch reads: plain value words, or a data node's links. A run of links resolves
/// its edges through the holder it names, or, where it names none, through the holder its reader
/// brings: an **own** run, a knot's list laid before its knot exists.
#[derive(Clone, Copy, Debug)]
pub(in crate::values) enum Run<'cell, X> {
    Plain(&'cell [Value<'cell, X>]),
    Linked(&'cell [Link<'cell, X>], Option<X>),
}

impl<'cell, X: Knotted> Run<'cell, X> {
    /// The cell at `at`, an edge resolved to a value through the run's holder or else `holder`;
    /// an own run's edge with neither stays an edge, for its reader to resolve.
    fn link(self, at: usize, holder: Option<X>) -> Link<'cell, X> {
        match self {
            Run::Plain(cells) => Link::Value(cells[at]),
            Run::Linked(cells, own) => match own.or(holder) {
                Some(holder) => Link::Value(cells[at].resolve(holder)),
                None => cells[at],
            },
        }
    }

    /// What cells `[from, to)` of this run weigh: the bytes their copy writes — an edge's also its
    /// holder's knot, where the run names it — and how many of them are an own run's edges.
    fn weight(self, prefix: &[Prefix], from: usize, to: usize) -> (usize, usize) {
        match self {
            Run::Plain(cells) => (stretch_weight(cells, prefix, from, to).bytes, 0),
            Run::Linked(cells, holder) => {
                let Prefix { bytes, edges } = stretch_weight(cells, prefix, from, to);
                match holder {
                    Some(holder) => (with_knots(bytes, edges, holder), 0),
                    None => (bytes, edges),
                }
            }
        }
    }
}

/// `bytes` plus `edges` copies of `holder`'s knot, saturating.
fn with_knots<X: Knotted>(bytes: usize, edges: usize, holder: X) -> usize {
    bytes.saturating_add(edges.saturating_mul(holder.weight().bytes()))
}

/// `len` cells of one flat run from `start`, read forward or backward. Never empty.
#[derive(Clone, Copy, Debug)]
pub(in crate::values) struct Stretch<'cell, X> {
    run: Run<'cell, X>,
    prefix: &'cell [Prefix],
    start: usize,
    len: usize,
    backward: bool,
    weight: usize,
    /// How many of its cells are an own run's edges.
    edges: usize,
}

impl<'cell, X: Knotted> Stretch<'cell, X> {
    /// Cells `[start, start + len)` of `run`, forward.
    fn of(run: Run<'cell, X>, prefix: &'cell [Prefix], start: usize, len: usize) -> Self {
        debug_assert!(len > 0, "a stretch is never empty");
        let (weight, edges) = run.weight(prefix, start, start + len);
        Stretch {
            run,
            prefix,
            start,
            len,
            backward: false,
            weight,
            edges,
        }
    }

    /// The cell index of the element at `at`, read `backward` or not.
    fn cell(&self, at: usize, backward: bool) -> usize {
        match backward {
            false => self.start + at,
            true => self.start + self.len - 1 - at,
        }
    }
}

/// A list view's tree: a [`Node`] over stretches, needing no bound on `X` to be named.
pub(in crate::values) type ListNode<'cell, X> = Node<'cell, Stretch<'cell, X>, Cells, X>;

/// A list view's summary of its stretches: their length, the bytes their flat copy writes past the
/// list's own struct and prefix, and how many of their cells are an own run's edges, waiting for a
/// holder.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::values) struct Cells {
    len: usize,
    weight: usize,
    edges: usize,
}

impl<X: Knotted> Summary<X> for Cells {
    fn len(&self) -> usize {
        self.len
    }

    fn joined(self, right: Self) -> Self {
        Cells {
            len: self.len + right.len,
            weight: self.weight.saturating_add(right.weight),
            edges: self.edges + right.edges,
        }
    }

    fn reversed(self) -> Self {
        self
    }

    fn waits(&self) -> bool {
        self.edges > 0
    }

    /// Each waiting edge's copy also copies `holder`'s knot.
    fn under(self, holder: X) -> Self {
        Cells {
            weight: with_knots(self.weight, self.edges, holder),
            edges: 0,
            ..self
        }
    }
}

impl<X: Knotted> Leaf for Stretch<'_, X> {
    type Summary = Cells;
    type Lazy = X;

    fn summary(&self) -> Cells {
        Cells {
            len: self.len,
            weight: self.weight,
            edges: self.edges,
        }
    }

    fn split(self, at: usize) -> (Option<Self>, Option<Self>) {
        let part = |start, len| {
            (len > 0).then(|| Stretch {
                backward: self.backward,
                ..Stretch::of(self.run, self.prefix, start, len)
            })
        };
        let rest = self.len - at;
        match self.backward {
            false => (part(self.start, at), part(self.start + at, rest)),
            true => (part(self.start + rest, at), part(self.start, rest)),
        }
    }

    fn reversed(self) -> Self {
        Stretch {
            backward: !self.backward,
            ..self
        }
    }

    fn under(self, holder: X) -> Self {
        let Run::Linked(cells, None) = self.run else {
            unreachable!("only an own run's edges wait for a holder")
        };
        Stretch {
            run: Run::Linked(cells, Some(holder)),
            weight: with_knots(self.weight, self.edges, holder),
            edges: 0,
            ..self
        }
    }

    #[cfg(test)]
    fn holds_its_laws(&self) -> bool {
        (self.weight, self.edges)
            == self
                .run
                .weight(self.prefix, self.start, self.start + self.len)
    }
}

/// A one-leaf tree over cells `[start, start + len)` of `run`, `len > 0`.
pub(in crate::values) fn leaf<'cell, X: Knotted>(
    writer: Writer<'cell, '_>,
    run: Run<'cell, X>,
    prefix: &'cell [Prefix],
    start: usize,
    len: usize,
) -> &'cell ListNode<'cell, X> {
    super::of(writer, Stretch::of(run, prefix, start, len))
}

impl<X> ListNode<'_, X> {
    /// Its length, read where `X` is no member yet: [`len`](Tree::len) without the bound.
    pub(in crate::values) fn count(&self) -> usize {
        match self {
            Node::Leaf(stretch) => stretch.len,
            Node::Branch { summary, .. } => summary.len,
        }
    }
}

impl<'cell, X: Knotted> Tree<'cell, Stretch<'cell, X>> {
    /// The bytes its cells' flat copy writes, past the list's own struct and prefix.
    pub(in crate::values) fn weight(&self) -> usize {
        self.summary().weight
    }

    /// The element at `at`, below [`len`](Self::len): a value, or an edge of an own run no holder
    /// above it resolves, which its reader resolves. Lays nothing.
    pub(in crate::values) fn link(&self, at: usize) -> Link<'cell, X> {
        let (stretch, at, flip, holder) = self.locate(at);
        (stretch.run).link(stretch.cell(at, stretch.backward ^ flip), holder)
    }

    /// Every element, in reading order, as [`link`](Self::link) reads it.
    pub(in crate::values) fn links(&'cell self) -> Links<'cell, X> {
        Links {
            leaves: self.leaves(),
            leaf: None,
            next: 0,
        }
    }
}

/// A view's elements in reading order: each leaf's cells in turn. O(1) amortized per element;
/// lays nothing.
pub(in crate::values) struct Links<'cell, X> {
    leaves: super::Leaves<'cell, Stretch<'cell, X>, Cells, X>,
    /// The leaf being read, whether it reads backward, its holder, and how many of its elements
    /// are read.
    leaf: Option<(Stretch<'cell, X>, bool, Option<X>)>,
    next: usize,
}

impl<'cell, X: Knotted> Iterator for Links<'cell, X> {
    type Item = Link<'cell, X>;

    fn next(&mut self) -> Option<Link<'cell, X>> {
        loop {
            if let Some((stretch, backward, holder)) = self.leaf {
                if self.next < stretch.len {
                    self.next += 1;
                    let at = stretch.cell(self.next - 1, backward);
                    return Some(stretch.run.link(at, holder));
                }
                self.leaf = None;
            }
            let (stretch, flip, holder) = self.leaves.next()?;
            self.leaf = Some((stretch, stretch.backward ^ flip, holder));
            self.next = 0;
        }
    }
}
