//! The **wake memo**: whether a record re-runs under a binding, answered once per frontier. A
//! frontier's unknowns are its own and each read's past its cut, transitively
//! ([the store](README.md#the-store)), and frontiers share structure, so the memo keeps, per
//! frontier by identity, the latest segment among the bound unknowns it reaches. A read's cut keeps
//! the unknowns at segments at least its own, so a read meets the binding where that segment lies
//! past its cut. Frontiers read only ended ones, so they form a DAG; a chain of k reads is k deep,
//! so the post-order keeps its own stack, as the folds' summary does.

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, bump_table};
use crate::scope::{Binding, Frontier, Segment};

/// Whether a frontier meets `binding`, memoized per frontier.
pub struct Wakes<'o, 'p> {
    binding: Binding<'o>,
    scratch: BumpAllocator<'p>,
    /// The latest segment among the bound unknowns each frontier reaches, by identity: `None` where
    /// it reaches none. Taken out while an ask fills it.
    latest: Cell<BumpBackedMap<'p, (usize, usize), Option<Segment>>>,
}

impl<'o, 'p> Wakes<'o, 'p> {
    /// The memo of what `binding` wakes, kept in `scratch`.
    pub fn new(binding: Binding<'o>, scratch: BumpAllocator<'p>) -> Self {
        Wakes {
            binding,
            scratch,
            latest: Cell::new(bump_table(scratch)),
        }
    }

    /// Whether `frontier` reaches an unknown the binding binds.
    pub fn wakes(&self, frontier: Frontier<'_>) -> bool {
        self.latest(frontier).is_some()
    }

    /// The latest segment among the bound unknowns `frontier` reaches.
    fn latest(&self, frontier: Frontier<'_>) -> Option<Segment> {
        if frontier.is_empty() || self.binding.is_empty() {
            return None;
        }
        let mut latest = self.latest.replace(bump_table(self.scratch));
        let mut stack = BumpVec::new_in(self.scratch);
        stack.push((frontier, false));
        while let Some((at, expanded)) = stack.pop() {
            if latest.contains_key(&at.identity()) {
                continue;
            }
            if !expanded {
                stack.push((at, true));
                stack.extend((at.reads().iter()).map(|cut| (cut.frontier, false)));
                continue;
            }
            let own = (at.own().iter())
                .filter(|unknown| self.binding.bound(**unknown).is_some())
                .map(|unknown| unknown.segment);
            let read = (at.reads().iter()).filter_map(|cut| {
                latest[&cut.frontier.identity()].filter(|segment| *segment >= cut.from)
            });
            let found = own.chain(read).max();
            latest.insert(at.identity(), found);
        }
        let found = latest[&frontier.identity()];
        self.latest.set(latest);
        found
    }
}
