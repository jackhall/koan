//! The audit for a **lend cycle**: cells whose disposals wait on one another through
//! the counts a lend leaves, so that none of them disposes after every release
//! ([../lend.rs](../lend.rs)).
//!
//! A tree cell's lend names a cell older than itself, and a child is younger than its parent, so
//! the waits those two leave all point from an older cell to a younger one and never close. A
//! tenant's lend does not: its lender may be younger than its host, and the count it leaves in its
//! host's scratch holds the lender until that scratch resets, which waits on the tree children
//! borrowing it. A tenant of `H` lent the region of `H`'s own child, which borrows `H`'s scratch,
//! leaves the child waiting on the scratch and the scratch waiting on the child. The substrate
//! refuses no such lend: which lends an embedder takes is its own election, and a check at
//! creation would have to walk this same graph. It ships this walk instead, so an embedder can
//! state a law against cycles in its own tests.
//!
//! The walk reads the structures disposal reads: the tree links, each tree cell's lend, the held
//! counts, and the live tenants' lends, which become held counts at their release. A wait on a
//! tenant, or on a scratch state at rest, is left out: both end at a release or a step the
//! embedder drives, never at another cell's disposal.

use std::collections::{HashMap, HashSet};

use super::*;
use crate::tree::{Ancestor, TreeState};

/// One thing a disposal can wait on: a cell's disposal, or the next reset of a write home's
/// scratch.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Wait {
    Disposal(CellHome),
    Reset(CellHome),
}

impl<
    'graph,
    C: Reattachable<'graph>,
    S: ReattachableOverBoth<'graph>,
    D: Delivery<'graph>,
    const W: usize,
    H: HabitatHeader<'graph>,
> CellGraph<'graph, C, S, D, W, H>
{
    /// A lend cycle, if the graph holds one: the slab and tree cells on it, in wait order, each
    /// once. Cells on a cycle never dispose, however they are released, and so neither does
    /// anything under them; a graph with none empties once everything in it is released.
    ///
    /// Linear in the graph's size, and changes nothing: an audit for an embedder's tests, not a
    /// door its steps should call.
    pub fn lend_cycle(&self) -> Option<Vec<CellHandle>> {
        self.cells.lend_cycle()
    }
}

impl<
    'graph,
    C: Reattachable<'graph>,
    S: ReattachableOverBoth<'graph>,
    D: Delivery<'graph>,
    const W: usize,
    H: HabitatHeader<'graph>,
> Cells<'graph, C, S, D, W, H>
{
    fn lend_cycle(&self) -> Option<Vec<CellHandle>> {
        let trees: Vec<u32> = (self.trees.occupied())
            .filter(|index| matches!(self.trees.state(*index), TreeState::Live | TreeState::Dead))
            .collect();
        // Every edge, from waiter to waited-on. A tree cell waits on nothing it lends, so the
        // starts are the lenders a scratch holds or will hold.
        let mut edges: HashMap<Wait, Vec<Wait>> = HashMap::new();
        let mut edge = |from: Wait, to: Wait| edges.entry(from).or_default().push(to);
        for &index in &trees {
            let cell = Wait::Disposal(CellHome::Tree(index));
            let parent = match self.trees.parent(index) {
                Ancestor::Root => CellHome::Slab(self.trees.root(index)),
                Ancestor::Tree(parent) => CellHome::Tree(parent),
            };
            edge(Wait::Disposal(parent), cell);
            match self.trees.lent(index) {
                Lent::Region(lender) => edge(Wait::Disposal(lender), cell),
                Lent::Scratch(lender) => edge(Wait::Reset(lender), cell),
            }
        }
        let tenant_lends =
            (self.tenants.census()).filter_map(|(host, _, lent)| lent.map(|lent| (host, lent)));
        let mut starts = Vec::new();
        for (host, lent) in tenant_lends.chain(self.held.iter().copied()) {
            if self.counted(host, lent) {
                let lender = Wait::Disposal(lent.lender());
                edge(lender, Wait::Reset(host));
                starts.push(lender);
            }
        }
        cycle_from(&edges, &starts).map(|cycle| {
            let mut cells: Vec<CellHandle> = Vec::new();
            for wait in cycle {
                let (Wait::Disposal(home) | Wait::Reset(home)) = wait;
                let cell = match home {
                    CellHome::Slab(slot) => CellHandle::Slab(self.occupant(slot)),
                    CellHome::Tree(index) => CellHandle::Tree(self.trees.occupant(index)),
                };
                if !cells.contains(&cell) {
                    cells.push(cell);
                }
            }
            cells
        })
    }
}

/// The nodes of a cycle reachable from `starts` over `edges`, in order, or `None`. An iterative
/// depth-first search, since a tree's depth bounds nothing.
fn cycle_from(edges: &HashMap<Wait, Vec<Wait>>, starts: &[Wait]) -> Option<Vec<Wait>> {
    // A node in `done` reaches no cycle; one in `open` is on the current path.
    let mut done: HashSet<Wait> = HashSet::new();
    for &start in starts {
        if done.contains(&start) {
            continue;
        }
        let mut open: HashSet<Wait> = HashSet::from([start]);
        let mut path: Vec<(Wait, usize)> = vec![(start, 0)];
        while let Some(&mut (node, ref mut next)) = path.last_mut() {
            let out = edges.get(&node).map_or(&[][..], Vec::as_slice);
            match out.get(*next) {
                None => {
                    done.insert(node);
                    open.remove(&node);
                    path.pop();
                }
                Some(&to) => {
                    *next += 1;
                    if open.contains(&to) {
                        let at = path.iter().position(|(on, _)| *on == to)?;
                        return Some(path[at..].iter().map(|(on, _)| *on).collect());
                    }
                    if !done.contains(&to) {
                        open.insert(to);
                        path.push((to, 0));
                    }
                }
            }
        }
    }
    None
}
