//! The **knot**: a group of values that refer to each other, laid down together in a cell's region
//! as one run of nodes whose sibling references are indices into the run rather than pointers.
//!
//! A value is born from finished parts, so a field cannot point at a value that does not exist yet.
//! A knot sidesteps that without a placeholder: an [`Edge`] names a node by its index, and an index
//! exists before the node does. No node holds a pointer to a sibling, so a copy of the run carries
//! every edge verbatim and a copier never follows one.
//!
//! **Construction.** A [`KnotPlan`] fixes the count before the first payload and mints edges below
//! it. Payloads whose edges name nodes not yet written are finished in the consumer's own scratch
//! and copied out by [`KnotPlan::tie`], which consumes the plan and lays every node down in index
//! order through [`Writer::thin_run`]. A knot never grows. Because the plan is neither `Copy` nor
//! `Clone`, one plan ties exactly one knot.
//!
//! **Reading.** A node is read as a [`Member`], the `(knot, index)` pair, and an edge is resolved
//! only through a member or its knot, never against a bare run. Resolving an edge against a knot
//! whose count it does not fit panics, like a slice index. The case the knot cannot see — an edge
//! minted by one plan placed in a payload another plan ties, whose count it happens to fit — is a
//! consumer bug, documented on [`KnotPlan::edge`].
//!
//! **Graphs built across steps.** A node of a later knot may hold a [`Member`] of a finished knot
//! as an ordinary pointer-carrying payload field, which this module neither mints nor sees. Knots
//! therefore form a DAG and cycles live only inside one; a finished knot never gains an edge to a
//! newer node, which is what write-once values already require.
//!
//! **Drop-freeness.** The region runs no destructor. `thin_run` asserts that for its element type,
//! and [`KnotPlan::tie`] restates the assert so a payload bringing drop glue fails the build naming
//! the knot.
//!
//! See [README.md § The knot](README.md#the-knot).

use super::substrate::{ThinRun, Writer};

/// A node index inside one knot: minted only by a [`KnotPlan`] against its count, and resolved only
/// through the knot holding the node it sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Edge(u32);

impl Edge {
    /// The node index this edge names.
    pub fn index(self) -> u32 {
        self.0
    }
}

/// A knot's size, settled before its first payload: mints edges in range, and is consumed by the
/// tie so its count is never shared by two knots.
pub struct KnotPlan {
    count: u32,
}

impl KnotPlan {
    pub fn new(count: u32) -> Self {
        KnotPlan { count }
    }

    /// How many nodes the knot this plan ties will hold.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// The edge naming node `target`, or `None` when `target` is not below the count.
    ///
    /// An edge is meaningful only in the knot this plan ties. Placing it in a payload another plan
    /// ties is a consumer bug the knot cannot see when the counts happen to fit — the same class as
    /// indexing one `Vec` with another's index; it panics at resolve only when they do not.
    pub fn edge(&self, target: u32) -> Option<Edge> {
        (target < self.count).then_some(Edge(target))
    }

    /// Lay the knot down in `writer`'s region: `build` is called once per node, in index order,
    /// with that node's own edge, and returns its finished payload.
    ///
    /// `build` may write into the same region — a payload's own run of edges through `writer` —
    /// since the node run is claimed before the first payload is built.
    pub fn tie<'cell, T>(
        self,
        writer: Writer<'cell>,
        mut build: impl FnMut(Edge) -> T,
    ) -> Knot<'cell, T> {
        const {
            assert!(
                !std::mem::needs_drop::<T>(),
                "a knot payload must carry no drop glue: the region runs no destructor"
            )
        };
        let run = writer.thin_run(self.count as usize, |index| build(Edge(index as u32)));
        Knot { run }
    }
}

/// A group of nodes laid down together, whose sibling edges are indices into the group. One
/// pointer wide and `Copy`.
pub struct Knot<'cell, T> {
    run: ThinRun<'cell, T>,
}

impl<T> Clone for Knot<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Knot<'_, T> {}

/// Two knots are equal when they are the same run.
impl<T> PartialEq for Knot<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.run.as_slice(), other.run.as_slice())
    }
}

impl<T> Eq for Knot<'_, T> {}

impl<'cell, T> Knot<'cell, T> {
    /// How many nodes the knot holds.
    pub fn len(self) -> u32 {
        self.run.len() as u32
    }

    /// Whether the knot holds no node.
    pub fn is_empty(self) -> bool {
        self.run.is_empty()
    }

    /// The node `edge` names.
    ///
    /// Panics when `edge` is not below this knot's count — an edge minted for a knot of another
    /// size.
    pub fn member(self, edge: Edge) -> Member<'cell, T> {
        assert!(
            edge.0 < self.len(),
            "edge {} resolved against a knot of {} nodes",
            edge.0,
            self.len()
        );
        Member {
            knot: self,
            index: edge,
        }
    }

    /// Every node, in index order.
    pub fn members(self) -> impl Iterator<Item = Member<'cell, T>> {
        (0..self.len()).map(move |index| Member {
            knot: self,
            index: Edge(index),
        })
    }
}

/// One node of a knot: the knot and the node's index. Sixteen bytes and `Copy`.
pub struct Member<'cell, T> {
    knot: Knot<'cell, T>,
    index: Edge,
}

impl<T> Clone for Member<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Member<'_, T> {}

/// Two members are equal when they are the same node: one knot, one index.
impl<T> PartialEq for Member<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        self.knot == other.knot && self.index == other.index
    }
}

impl<T> Eq for Member<'_, T> {}

impl<'cell, T> Member<'cell, T> {
    /// The knot this node belongs to.
    pub fn knot(self) -> Knot<'cell, T> {
        self.knot
    }

    /// This node's own edge.
    pub fn index(self) -> Edge {
        self.index
    }

    /// The node's payload, borrowed from the region.
    pub fn payload(self) -> &'cell T {
        &self.knot.run.as_slice()[self.index.0 as usize]
    }

    /// Resolve an edge read out of this node's payload against this node's own knot.
    pub fn follow(self, edge: Edge) -> Member<'cell, T> {
        self.knot.member(edge)
    }
}

const _: () = assert!(size_of::<Knot<'static, ()>>() == size_of::<usize>());
const _: () = assert!(size_of::<Member<'static, ()>>() == 16);

#[cfg(test)]
mod tests;
