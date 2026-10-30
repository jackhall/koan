//! A view's tree: a persistent AVL tree over leaves of one kind, each a stretch of some flat
//! run. A list view's leaves are stretches of flat runs of values or links
//! ([stretch.rs](view/stretch.rs)). See [README.md § Views](README.md#views).
//!
//! Every [`Node`] memoizes its subtree's height and its leaves' [`Summary`] — their length and
//! whatever the leaf kind adds, a monoid joined left to right — and a branch may read its two
//! children in reverse, so a reversal is one node. Every operation is persistent: it lays new nodes
//! in the writer's region and never writes an old one. Concatenation is an AVL join without a
//! middle key, and a slice is two splits over it, so a view built by n edits reads an element in
//! O(log n) steps and costs O(log n) nodes per edit.
//!
//! A branch may also carry a leaf kind's **lazy** tag down to the leaves below it, pushed down as
//! a reversal is, so every read and edit stays O(log n): a list's holder, the knot member its own
//! runs' edges resolve through ([`held`]). A leaf kind that needs none uses `()`.

use std::fmt;

use crate::memory::{Writer, resident};

mod stretch;

pub(super) use stretch::{BLOCK, Links, ListNode, Prefix, Run, Weighed, leaf, prefixes};

/// The tallest tree [`Leaves`] walks: an AVL tree of more leaves than a `usize` counts stands
/// lower than this.
const TALLEST: usize = 96;

// [`Leaves`] keeps one direction bit per pending subtree in a `u128`.
const _: () = assert!(TALLEST <= 128);

/// A leaf of a view's tree: a non-empty stretch of elements, read in its own direction.
pub(crate) trait Leaf: Copy {
    /// What a subtree memoizes of its leaves.
    type Summary: Summary<Self::Lazy>;
    /// What a branch carries down to the leaves below it.
    type Lazy: Copy;

    /// Its elements' summary, in reading order.
    fn summary(&self) -> Self::Summary;

    /// Its first `at` elements and the rest, in reading order; either `None` where empty.
    fn split(self, at: usize) -> (Option<Self>, Option<Self>);

    /// The same elements read in reverse.
    fn reversed(self) -> Self;

    /// The leaf under `lazy`, where its summary [waits](Summary::waits) for one.
    fn under(self, lazy: Self::Lazy) -> Self;

    /// Whether its memo equals its recomputed value.
    #[cfg(test)]
    fn holds_its_laws(&self) -> bool;
}

/// What a subtree memoizes of its leaves: a monoid under [`joined`](Self::joined), its length
/// among what it counts.
pub(crate) trait Summary<Lazy>: Copy + PartialEq {
    fn len(&self) -> usize;

    /// This summary, then `right`'s.
    fn joined(self, right: Self) -> Self;

    /// The summary of the same elements read in reverse: `a·b` reversed is `b` reversed, then `a`.
    fn reversed(self) -> Self;

    /// Whether some leaf below waits for a lazy tag, which [`held`] then lays.
    fn waits(&self) -> bool;

    /// The summary of the same elements under `lazy`.
    fn under(self, lazy: Lazy) -> Self;
}

/// A view's tree: a leaf, or two subtrees read left then right, or right then left each reversed
/// where `backward` is set. A branch naming a `lazy` tag carries it to the leaves below it — those
/// below no nearer one — as `backward` reverses what lies below it, and its summary is theirs
/// under it.
pub(crate) enum Node<'cell, L, S, Z> {
    Leaf(L),
    Branch {
        left: &'cell Node<'cell, L, S, Z>,
        right: &'cell Node<'cell, L, S, Z>,
        height: u8,
        summary: S,
        backward: bool,
        lazy: Option<Z>,
    },
}

/// The tree over leaves `L`: a [`Node`] memoizing their summary and carrying their lazy tag.
pub(crate) type Tree<'cell, L> = Node<'cell, L, <L as Leaf>::Summary, <L as Leaf>::Lazy>;

impl<L, S, Z> fmt::Debug for Node<'_, L, S, Z> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Node::Leaf(_) => "Leaf",
            Node::Branch { .. } => "Branch",
        })
    }
}

#[cfg(test)]
thread_local! {
    /// How many nodes the view operations have visited or laid, and stretches weighed: what a test
    /// reads to see a read or an edit cost O(log n).
    pub(crate) static VIEW_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Count one step of a view operation, in a test build.
pub(super) fn step() {
    #[cfg(test)]
    VIEW_STEPS.with(|steps| steps.set(steps.get() + 1));
}

impl<'cell, L: Leaf> Tree<'cell, L> {
    /// Its leaves' summary, in reading order.
    pub(crate) fn summary(&self) -> L::Summary {
        match self {
            Node::Leaf(leaf) => leaf.summary(),
            Node::Branch { summary, .. } => *summary,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.summary().len()
    }

    fn height(&self) -> u8 {
        match self {
            Node::Leaf(_) => 1,
            Node::Branch { height, .. } => *height,
        }
    }

    /// The leaf holding element `at`, below [`len`](Self::len), beside `at`'s offset in it read
    /// in the tree's reading order, whether the reversals above it flip it, and the nearest lazy
    /// tag above it. Lays nothing.
    pub(crate) fn locate(&self, at: usize) -> (L, usize, bool, Option<L::Lazy>) {
        let (mut node, mut at, mut flip, mut lazy) = (self, at, false, None);
        loop {
            step();
            match *node {
                Node::Leaf(leaf) => return (leaf, at, flip, lazy),
                Node::Branch {
                    left,
                    right,
                    backward,
                    lazy: held,
                    ..
                } => {
                    flip ^= backward;
                    lazy = held.or(lazy);
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

    /// Every leaf, in reading order, as [`locate`](Self::locate) reads it.
    pub(crate) fn leaves(&'cell self) -> Leaves<'cell, L, L::Summary, L::Lazy> {
        let mut leaves = Leaves {
            pending: [None; TALLEST],
            lazies: [None; TALLEST],
            flips: 0,
            depth: 0,
        };
        leaves.push(self, false, None);
        leaves
    }
}

/// A one-leaf tree.
pub(crate) fn of<'cell, L: Leaf>(writer: Writer<'cell, '_>, leaf: L) -> &'cell Tree<'cell, L> {
    step();
    resident(writer, Node::Leaf(leaf))
}

/// A branch reading `left` then `right`.
fn branch<'cell, L: Leaf>(
    writer: Writer<'cell, '_>,
    left: &'cell Tree<'cell, L>,
    right: &'cell Tree<'cell, L>,
) -> &'cell Tree<'cell, L> {
    step();
    resident(
        writer,
        Node::Branch {
            left,
            right,
            height: 1 + left.height().max(right.height()),
            summary: left.summary().joined(right.summary()),
            backward: false,
            lazy: None,
        },
    )
}

/// `node` with the leaves below it under `lazy`: one node, or `node` itself where no leaf below
/// it waits for one.
pub(crate) fn held<'cell, L: Leaf>(
    writer: Writer<'cell, '_>,
    node: &'cell Tree<'cell, L>,
    lazy: L::Lazy,
) -> &'cell Tree<'cell, L> {
    if !node.summary().waits() {
        return node;
    }
    step();
    resident(
        writer,
        match *node {
            Node::Leaf(leaf) => Node::Leaf(leaf.under(lazy)),
            Node::Branch {
                left,
                right,
                height,
                summary,
                backward,
                ..
            } => Node::Branch {
                left,
                right,
                height,
                summary: summary.under(lazy),
                backward,
                lazy: Some(lazy),
            },
        },
    )
}

/// `node` read in reverse: one node with its direction toggled.
pub(crate) fn reversed<'cell, L: Leaf>(
    writer: Writer<'cell, '_>,
    node: &'cell Tree<'cell, L>,
) -> &'cell Tree<'cell, L> {
    step();
    resident(
        writer,
        match *node {
            Node::Leaf(leaf) => Node::Leaf(leaf.reversed()),
            Node::Branch {
                left,
                right,
                height,
                summary,
                backward,
                lazy,
            } => Node::Branch {
                left,
                right,
                height,
                summary: summary.reversed(),
                backward: !backward,
                lazy,
            },
        },
    )
}

/// A branch's two children in reading order, each reversed where the branch reads backward and
/// held where it names a lazy tag.
fn oriented<'cell, L: Leaf>(
    writer: Writer<'cell, '_>,
    node: &'cell Tree<'cell, L>,
) -> (&'cell Tree<'cell, L>, &'cell Tree<'cell, L>) {
    step();
    let Node::Branch {
        left,
        right,
        backward,
        lazy,
        ..
    } = *node
    else {
        unreachable!("only a branch has children")
    };
    let (left, right) = match lazy {
        Some(lazy) => (held(writer, left, lazy), held(writer, right, lazy)),
        None => (left, right),
    };
    match backward {
        false => (left, right),
        true => (reversed(writer, right), reversed(writer, left)),
    }
}

/// `left` then `right`, rebalanced where their heights differ by two: one rotation, single or
/// double.
fn balanced<'cell, L: Leaf>(
    writer: Writer<'cell, '_>,
    left: &'cell Tree<'cell, L>,
    right: &'cell Tree<'cell, L>,
) -> &'cell Tree<'cell, L> {
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
pub(crate) fn concat<'cell, L: Leaf>(
    writer: Writer<'cell, '_>,
    left: &'cell Tree<'cell, L>,
    right: &'cell Tree<'cell, L>,
) -> &'cell Tree<'cell, L> {
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
pub(crate) fn joined<'cell, L: Leaf>(
    writer: Writer<'cell, '_>,
    left: Option<&'cell Tree<'cell, L>>,
    right: Option<&'cell Tree<'cell, L>>,
) -> Option<&'cell Tree<'cell, L>> {
    match (left, right) {
        (Some(left), Some(right)) => Some(concat(writer, left, right)),
        (left, right) => left.or(right),
    }
}

/// `node`'s first `at` elements and the rest, `at` at most its length; either `None` where empty.
/// O(log n) nodes: each level joins back what it cut past, and the joins' heights telescope.
pub(crate) fn split<'cell, L: Leaf>(
    writer: Writer<'cell, '_>,
    node: &'cell Tree<'cell, L>,
    at: usize,
) -> (Option<&'cell Tree<'cell, L>>, Option<&'cell Tree<'cell, L>>) {
    if at == 0 {
        return (None, Some(node));
    }
    if at == node.len() {
        return (Some(node), None);
    }
    let laid = |leaf: Option<L>| leaf.map(|leaf| of(writer, leaf));
    match node {
        Node::Leaf(leaf) => {
            let (first, rest) = leaf.split(at);
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
pub(crate) fn sliced<'cell, L: Leaf>(
    writer: Writer<'cell, '_>,
    node: &'cell Tree<'cell, L>,
    from: usize,
    to: usize,
) -> Option<&'cell Tree<'cell, L>> {
    let (first, _) = split(writer, node, to);
    split(writer, first?, from).1
}

/// A view's leaves in reading order, walked over a stack of the subtrees still to read, each
/// beside whether it reads reversed — one bit apiece of `flips` — and the lazy tag it is under.
/// Each leaf comes beside whether the reversals above it flip it, and the nearest tag above it.
/// O(1) amortized per leaf; lays nothing.
pub(crate) struct Leaves<'cell, L, S, Z> {
    pending: [Option<&'cell Node<'cell, L, S, Z>>; TALLEST],
    lazies: [Option<Z>; TALLEST],
    flips: u128,
    depth: usize,
}

impl<'cell, L: Leaf> Leaves<'cell, L, L::Summary, L::Lazy> {
    fn push(&mut self, node: &'cell Tree<'cell, L>, flip: bool, lazy: Option<L::Lazy>) {
        self.pending[self.depth] = Some(node);
        self.lazies[self.depth] = lazy;
        self.flips = (self.flips & !(1 << self.depth)) | (u128::from(flip) << self.depth);
        self.depth += 1;
    }
}

impl<'cell, L: Leaf> Iterator for Leaves<'cell, L, L::Summary, L::Lazy> {
    type Item = (L, bool, Option<L::Lazy>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.depth == 0 {
                return None;
            }
            self.depth -= 1;
            let node = self.pending[self.depth].take().expect("a pending subtree");
            let flip = self.flips & (1 << self.depth) != 0;
            let lazy = self.lazies[self.depth];
            match *node {
                Node::Leaf(leaf) => return Some((leaf, flip, lazy)),
                Node::Branch {
                    left,
                    right,
                    backward,
                    lazy: held,
                    ..
                } => {
                    let (flip, lazy) = (flip ^ backward, held.or(lazy));
                    let (first, second) = if flip { (right, left) } else { (left, right) };
                    // The second is read after the first, so it is pushed first.
                    self.push(second, flip, lazy);
                    self.push(first, flip, lazy);
                }
            }
        }
    }
}

#[cfg(test)]
impl<L: Leaf> Tree<'_, L> {
    /// Whether every branch's children differ in height by at most one, and every memo equals its
    /// recomputed value.
    pub(crate) fn holds_its_laws(&self) -> bool {
        match self {
            Node::Leaf(leaf) => leaf.summary().len() > 0 && leaf.holds_its_laws(),
            Node::Branch {
                left,
                right,
                height,
                summary,
                backward,
                lazy,
            } => {
                let below = left.summary().joined(right.summary());
                let below = if *backward { below.reversed() } else { below };
                let memo = match *lazy {
                    Some(lazy) => below.waits() && *summary == below.under(lazy),
                    None => *summary == below,
                };
                left.height().abs_diff(right.height()) <= 1
                    && *height == 1 + left.height().max(right.height())
                    && memo
                    && left.holds_its_laws()
                    && right.holds_its_laws()
            }
        }
    }
}
