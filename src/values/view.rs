//! A list view: a list that lays down none of its sources' cells, built by a slice or a
//! concatenation over the flat runs they hold. See
//! [README.md § Views](README.md#views).
//!
//! A view is an AVL tree of [`Stretch`]es, each a stretch of one flat run read forward or backward.
//! Every [`Node`] memoizes its subtree's length, weight and height, and a branch may read its two
//! children in reverse, so a reversal is one node. Every operation is persistent: it lays new nodes
//! in the writer's region and never writes an old one. Concatenation is an AVL join without a
//! middle key, and a slice is two splits over it, so a view built by n edits reads an element in
//! O(log n) steps and costs O(log n) nodes per edit.
//!
//! A run of a knot's data node resolves its edges through a holder, the knot member that reads
//! it. A knot's own list is laid before its knot exists, so its runs name no holder: an **own**
//! run's edges resolve through the holder its reader brings. A view of other lists over it is held
//! by one branch naming its holder ([`held`]), pushed down lazily as a reversal is, so every read
//! and edit stays O(log n).
//!
//! A flat run keeps a [`Prefix`] sum of its cells' weights every [`BLOCK`] cells
//! ([list.rs](super::list) lays it), so a stretch of it is weighed in O(1): a view weighs what its
//! flat copy writes. A view's type, bits and hides bit are its builder's ([list.rs](super::list));
//! nothing here reads a type.

use crate::memory::{Writer, resident};

use super::{Knotted, Link, Value};

/// How many cells of a flat run one [`Prefix`] entry spans.
pub(super) const BLOCK: usize = 16;

/// The tallest tree [`Links`] walks: an AVL tree of more leaves than a `usize` counts stands
/// lower than this.
const TALLEST: usize = 96;

// [`Links`] keeps one direction bit per pending subtree in a `u128`.
const _: () = assert!(TALLEST <= 128);

/// The weight of a flat run's cells before a block boundary: the bytes their copy writes past the
/// list's own struct, and how many of them are edges, whose copy also copies their holder's knot.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Prefix {
    bytes: usize,
    edges: usize,
}

impl Prefix {
    /// The bytes its cells' copy writes, edges' knots aside.
    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }
}

/// One cell's share of a [`Prefix`]: its bytes, and whether it is an edge.
pub(super) trait Weighed {
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
pub(super) fn prefixes<'cell, C: Weighed>(
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
    #[cfg(test)]
    VIEW_STEPS.with(|steps| steps.set(steps.get() + 1));
    sides
}

/// One flat run a stretch reads: plain value words, or a data node's links. A run of links resolves
/// its edges through the holder it names, or, where it names none, through the holder its reader
/// brings: an **own** run, a knot's list laid before its knot exists.
#[derive(Clone, Copy, Debug)]
pub(super) enum Run<'cell, X> {
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
pub(super) struct Stretch<'cell, X> {
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
    pub(super) fn of(
        run: Run<'cell, X>,
        prefix: &'cell [Prefix],
        start: usize,
        len: usize,
    ) -> Self {
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

    /// Its first `at` elements and the rest, in reading order; either `None` where empty.
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
}

/// A view's tree: a stretch, or two subtrees read left then right, or right then left each
/// reversed where `backward` is set. A branch naming a `holder` resolves the own runs below it
/// through it — those below no nearer holder — as `backward` reverses what lies below it, and
/// counts none of their edges; otherwise `edges` counts every own run's edge below it.
#[derive(Debug)]
pub(super) enum Node<'cell, X> {
    Leaf(Stretch<'cell, X>),
    Branch {
        left: &'cell Node<'cell, X>,
        right: &'cell Node<'cell, X>,
        height: u8,
        len: usize,
        weight: usize,
        edges: usize,
        backward: bool,
        holder: Option<X>,
    },
}

#[cfg(test)]
thread_local! {
    /// How many nodes the view operations have visited or laid, and stretches weighed: what a test
    /// reads to see a read or an edit cost O(log n).
    pub(crate) static VIEW_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Count one step of a view operation, in a test build.
fn step() {
    #[cfg(test)]
    VIEW_STEPS.with(|steps| steps.set(steps.get() + 1));
}

impl<'cell, X> Node<'cell, X> {
    pub(super) fn len(&self) -> usize {
        match self {
            Node::Leaf(stretch) => stretch.len,
            Node::Branch { len, .. } => *len,
        }
    }

    /// The bytes its cells' flat copy writes, past the list's own struct and prefix.
    pub(super) fn weight(&self) -> usize {
        match self {
            Node::Leaf(stretch) => stretch.weight,
            Node::Branch { weight, .. } => *weight,
        }
    }

    /// How many own runs' edges below it wait for a holder.
    pub(super) fn edges(&self) -> usize {
        match self {
            Node::Leaf(stretch) => stretch.edges,
            Node::Branch { edges, .. } => *edges,
        }
    }

    fn height(&self) -> u8 {
        match self {
            Node::Leaf(_) => 1,
            Node::Branch { height, .. } => *height,
        }
    }
}

impl<'cell, X: Knotted> Node<'cell, X> {
    /// The element at `at`, below [`len`](Self::len), descending with the reversals above it
    /// folded into one bit and the nearest holder above it: a value, or an edge of an own run no
    /// holder above it resolves, which its reader resolves. Lays nothing.
    pub(super) fn link(&self, at: usize) -> Link<'cell, X> {
        let (mut node, mut at, mut flip, mut holder) = (self, at, false, None);
        loop {
            step();
            match *node {
                Node::Leaf(stretch) => {
                    return (stretch.run).link(stretch.cell(at, stretch.backward ^ flip), holder);
                }
                Node::Branch {
                    left,
                    right,
                    backward,
                    holder: held,
                    ..
                } => {
                    flip ^= backward;
                    holder = held.or(holder);
                    let (first, second) = if flip { (right, left) } else { (left, right) };
                    if at < first.len() {
                        node = first;
                    } else {
                        at -= first.len();
                        node = second;
                    }
                }
            }
        }
    }

    /// Every element, in reading order, as [`link`](Self::link) reads it.
    pub(super) fn links(&'cell self) -> Links<'cell, X> {
        let mut links = Links {
            pending: [None; TALLEST],
            holders: [None; TALLEST],
            flips: 0,
            depth: 0,
            leaf: None,
            next: 0,
        };
        links.push(self, false, None);
        links
    }
}

/// A one-leaf tree over cells `[start, start + len)` of `run`, `len > 0`.
pub(super) fn leaf<'cell, X: Knotted>(
    writer: Writer<'cell, '_>,
    run: Run<'cell, X>,
    prefix: &'cell [Prefix],
    start: usize,
    len: usize,
) -> &'cell Node<'cell, X> {
    step();
    resident(writer, Node::Leaf(Stretch::of(run, prefix, start, len)))
}

/// A branch reading `left` then `right`.
fn branch<'cell, X: Knotted>(
    writer: Writer<'cell, '_>,
    left: &'cell Node<'cell, X>,
    right: &'cell Node<'cell, X>,
) -> &'cell Node<'cell, X> {
    step();
    resident(
        writer,
        Node::Branch {
            left,
            right,
            height: 1 + left.height().max(right.height()),
            len: left.len() + right.len(),
            weight: left.weight().saturating_add(right.weight()),
            edges: left.edges() + right.edges(),
            backward: false,
            holder: None,
        },
    )
}

/// `node` with the own runs below it resolving through `holder`: one node, or `node` itself where
/// no edge below it waits for a holder. Its weight gains each such edge's copy of `holder`'s knot.
pub(super) fn held<'cell, X: Knotted>(
    writer: Writer<'cell, '_>,
    node: &'cell Node<'cell, X>,
    holder: X,
) -> &'cell Node<'cell, X> {
    let edges = node.edges();
    if edges == 0 {
        return node;
    }
    step();
    let weight = with_knots(node.weight(), edges, holder);
    resident(
        writer,
        match *node {
            Node::Leaf(stretch) => {
                let Run::Linked(cells, None) = stretch.run else {
                    unreachable!("only an own run's edges wait for a holder")
                };
                Node::Leaf(Stretch {
                    run: Run::Linked(cells, Some(holder)),
                    weight,
                    edges: 0,
                    ..stretch
                })
            }
            Node::Branch {
                left,
                right,
                height,
                len,
                backward,
                ..
            } => Node::Branch {
                left,
                right,
                height,
                len,
                weight,
                edges: 0,
                backward,
                holder: Some(holder),
            },
        },
    )
}

/// `node` read in reverse: one node with its direction toggled.
pub(super) fn reversed<'cell, X: Knotted>(
    writer: Writer<'cell, '_>,
    node: &'cell Node<'cell, X>,
) -> &'cell Node<'cell, X> {
    step();
    resident(
        writer,
        match *node {
            Node::Leaf(stretch) => Node::Leaf(Stretch {
                backward: !stretch.backward,
                ..stretch
            }),
            Node::Branch {
                left,
                right,
                height,
                len,
                weight,
                edges,
                backward,
                holder,
            } => Node::Branch {
                left,
                right,
                height,
                len,
                weight,
                edges,
                backward: !backward,
                holder,
            },
        },
    )
}

/// A branch's two children in reading order, each reversed where the branch reads backward and
/// held where it names a holder.
fn oriented<'cell, X: Knotted>(
    writer: Writer<'cell, '_>,
    node: &'cell Node<'cell, X>,
) -> (&'cell Node<'cell, X>, &'cell Node<'cell, X>) {
    step();
    let Node::Branch {
        left,
        right,
        backward,
        holder,
        ..
    } = *node
    else {
        unreachable!("only a branch has children")
    };
    let (left, right) = match holder {
        Some(holder) => (held(writer, left, holder), held(writer, right, holder)),
        None => (left, right),
    };
    match backward {
        false => (left, right),
        true => (reversed(writer, right), reversed(writer, left)),
    }
}

/// `left` then `right`, rebalanced where their heights differ by two: one rotation, single or
/// double.
fn balanced<'cell, X: Knotted>(
    writer: Writer<'cell, '_>,
    left: &'cell Node<'cell, X>,
    right: &'cell Node<'cell, X>,
) -> &'cell Node<'cell, X> {
    let (hl, hr) = (left.height(), right.height());
    if hl > hr + 1 {
        let (outer, inner) = oriented(writer, left);
        if outer.height() >= inner.height() {
            return branch(writer, outer, branch(writer, inner, right));
        }
        let (low, high) = oriented(writer, inner);
        return branch(
            writer,
            branch(writer, outer, low),
            branch(writer, high, right),
        );
    }
    if hr > hl + 1 {
        let (inner, outer) = oriented(writer, right);
        if outer.height() >= inner.height() {
            return branch(writer, branch(writer, left, inner), outer);
        }
        let (low, high) = oriented(writer, inner);
        return branch(
            writer,
            branch(writer, left, low),
            branch(writer, high, outer),
        );
    }
    branch(writer, left, right)
}

/// `left` then `right`: an AVL join without a middle key, descending the taller tree's facing
/// spine to a subtree within one of the shorter's height. O(|h(left) − h(right)|) nodes.
pub(super) fn concat<'cell, X: Knotted>(
    writer: Writer<'cell, '_>,
    left: &'cell Node<'cell, X>,
    right: &'cell Node<'cell, X>,
) -> &'cell Node<'cell, X> {
    let (hl, hr) = (left.height(), right.height());
    if hl > hr + 1 {
        let (outer, inner) = oriented(writer, left);
        return balanced(writer, outer, concat(writer, inner, right));
    }
    if hr > hl + 1 {
        let (inner, outer) = oriented(writer, right);
        return balanced(writer, concat(writer, left, inner), outer);
    }
    branch(writer, left, right)
}

/// [`concat`] over either side missing.
pub(super) fn joined<'cell, X: Knotted>(
    writer: Writer<'cell, '_>,
    left: Option<&'cell Node<'cell, X>>,
    right: Option<&'cell Node<'cell, X>>,
) -> Option<&'cell Node<'cell, X>> {
    match (left, right) {
        (Some(left), Some(right)) => Some(concat(writer, left, right)),
        (left, right) => left.or(right),
    }
}

/// `node`'s first `at` elements and the rest, `at` at most its length; either `None` where empty.
/// O(log n) nodes: each level joins back what it cut past, and the joins' heights telescope.
pub(super) fn split<'cell, X: Knotted>(
    writer: Writer<'cell, '_>,
    node: &'cell Node<'cell, X>,
    at: usize,
) -> (Option<&'cell Node<'cell, X>>, Option<&'cell Node<'cell, X>>) {
    if at == 0 {
        return (None, Some(node));
    }
    if at == node.len() {
        return (Some(node), None);
    }
    let laid = |stretch: Option<Stretch<'cell, X>>| {
        stretch.map(|stretch| {
            step();
            resident(writer, Node::Leaf(stretch))
        })
    };
    match node {
        Node::Leaf(stretch) => {
            let (first, rest) = stretch.split(at);
            (laid(first), laid(rest))
        }
        Node::Branch { .. } => {
            let (left, right) = oriented(writer, node);
            if at <= left.len() {
                let (first, rest) = split(writer, left, at);
                (first, joined(writer, rest, Some(right)))
            } else {
                let (first, rest) = split(writer, right, at - left.len());
                (joined(writer, Some(left), first), rest)
            }
        }
    }
}

/// Elements `[from, to)` of `node`, forward, `from ≤ to ≤ len`; `None` where empty.
pub(super) fn sliced<'cell, X: Knotted>(
    writer: Writer<'cell, '_>,
    node: &'cell Node<'cell, X>,
    from: usize,
    to: usize,
) -> Option<&'cell Node<'cell, X>> {
    let (first, _) = split(writer, node, to);
    split(writer, first?, from).1
}

/// A view's elements in reading order, walked over a stack of the subtrees still to read, each
/// beside whether it reads reversed — one bit apiece of `flips` — and the holder its own runs
/// resolve through. O(1) amortized per element; lays nothing.
pub(super) struct Links<'cell, X> {
    pending: [Option<&'cell Node<'cell, X>>; TALLEST],
    holders: [Option<X>; TALLEST],
    flips: u128,
    depth: usize,
    /// The leaf being read, whether it reads backward, its holder, and how many of its elements
    /// are read.
    leaf: Option<(Stretch<'cell, X>, bool, Option<X>)>,
    next: usize,
}

impl<'cell, X: Knotted> Links<'cell, X> {
    fn push(&mut self, node: &'cell Node<'cell, X>, flip: bool, holder: Option<X>) {
        self.pending[self.depth] = Some(node);
        self.holders[self.depth] = holder;
        self.flips = (self.flips & !(1 << self.depth)) | (u128::from(flip) << self.depth);
        self.depth += 1;
    }
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
            if self.depth == 0 {
                return None;
            }
            self.depth -= 1;
            let node = self.pending[self.depth].take().expect("a pending subtree");
            let flip = self.flips & (1 << self.depth) != 0;
            let holder = self.holders[self.depth];
            match *node {
                Node::Leaf(stretch) => {
                    self.leaf = Some((stretch, stretch.backward ^ flip, holder));
                    self.next = 0;
                }
                Node::Branch {
                    left,
                    right,
                    backward,
                    holder: held,
                    ..
                } => {
                    let (flip, holder) = (flip ^ backward, held.or(holder));
                    let (first, second) = if flip { (right, left) } else { (left, right) };
                    // The second is read after the first, so it is pushed first.
                    self.push(second, flip, holder);
                    self.push(first, flip, holder);
                }
            }
        }
    }
}

#[cfg(test)]
impl<'cell, X: Knotted> Node<'cell, X> {
    /// Whether every branch's children differ in height by at most one, and every memo equals its
    /// recomputed value.
    pub(crate) fn holds_its_laws(&self) -> bool {
        match self {
            Node::Leaf(stretch) => {
                stretch.len > 0
                    && (stretch.weight, stretch.edges)
                        == stretch.run.weight(
                            stretch.prefix,
                            stretch.start,
                            stretch.start + stretch.len,
                        )
            }
            Node::Branch {
                left,
                right,
                height,
                len,
                weight,
                edges,
                holder,
                ..
            } => {
                let (below, waiting) = (
                    left.weight().saturating_add(right.weight()),
                    left.edges() + right.edges(),
                );
                let memos = match *holder {
                    Some(holder) => {
                        waiting > 0 && *edges == 0 && *weight == with_knots(below, waiting, holder)
                    }
                    None => *edges == waiting && *weight == below,
                };
                left.height().abs_diff(right.height()) <= 1
                    && *height == 1 + left.height().max(right.height())
                    && *len == left.len() + right.len()
                    && memos
                    && left.holds_its_laws()
                    && right.holds_its_laws()
            }
        }
    }
}
