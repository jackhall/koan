//! The submission table: units of work with no cell yet, and the counts that decide when each
//! gets one.
//!
//! This is what a binder dependency is waited on with. A body's reference graph is known before it
//! runs, so a unit is submitted with a count of dependencies not yet met, the count falls as each
//! producing unit finishes, and the drain creates the cell only when it reaches zero. A reader
//! therefore never observes a binding whose binder has not run, and the drain's bookkeeping is one
//! record per *unit* rather than one per parked cell.
//!
//! See [scheduler/README.md](README.md#the-two-ways-a-cell-waits).

use crate::scheduler::action::Placement;
use crate::scheduler::continuation::{CellPlace, NativeStep, State};

/// One unit in the table, handed back by [`Submissions::submit`] to wire edges against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UnitId(usize);

/// A unit of work with no cell yet: where it will be born, and what it will run.
///
/// It names no destination. A submitted unit reports through the table rather than through a
/// receipt run — its dependents are wired by edges and released by [`Submissions::settle`] — which
/// is the whole difference between the two ways a cell waits.
#[derive(Clone, Copy)]
pub struct Unit<'graph> {
    /// Where the cell is born: under a parent, or in the slab.
    pub place: CellPlace,
    /// Which door it is born through, when it is born under a parent.
    pub placement: Placement,
    /// The step it runs first.
    pub step: NativeStep<'graph>,
    /// What it is born holding, at `'graph`.
    pub state: State<'graph, 'graph>,
}

/// One submitted unit's record.
struct Entry<'graph> {
    /// The work, taken when the unit becomes ready — so a unit is launched once, however many
    /// edges name it and however the counts fall.
    unit: Option<Unit<'graph>>,
    /// Dependencies not yet satisfied.
    unmet: usize,
    /// Head of this unit's dependent list, an index into `edges`.
    dependents: Option<usize>,
}

/// One dependency edge. They live in a flat arena threaded by `next` rather than a list per unit,
/// so a table of any width costs a handful of amortized allocations rather than one per unit.
struct Edge {
    dependent: UnitId,
    next: Option<usize>,
}

/// Every unit the drain has been given, and the counts that decide when each gets a cell.
///
/// A [`UnitId`] is an index into `entries`, so a settled unit's entry stays where it is with its
/// work taken out of it: the arena is sized by the units submitted over the run rather than by the
/// units outstanding at any moment.
pub struct Submissions<'graph> {
    entries: Vec<Entry<'graph>>,
    edges: Vec<Edge>,
    /// Units whose count has reached zero and whose cell the drain has not created yet.
    ready: Vec<UnitId>,
    /// Submitted units that have not finished.
    outstanding: usize,
}

impl<'graph> Submissions<'graph> {
    pub(crate) fn new() -> Self {
        Submissions {
            entries: Vec::new(),
            edges: Vec::new(),
            ready: Vec::new(),
            outstanding: 0,
        }
    }

    /// Register a unit that runs once `dependencies` producing units have finished. A unit with
    /// none is ready the moment it is submitted.
    pub fn submit(&mut self, unit: Unit<'graph>, dependencies: usize) -> UnitId {
        let id = UnitId(self.entries.len());
        self.entries.push(Entry {
            unit: Some(unit),
            unmet: dependencies,
            dependents: None,
        });
        self.outstanding += 1;
        if dependencies == 0 {
            self.ready.push(id);
        }
        id
    }

    /// Record that finishing `producer` satisfies one of `dependent`'s dependencies.
    ///
    /// The count came from [`submit`](Self::submit), so the edges are what decide *which* finishes
    /// answer for it. Wire every edge before the run: one added after its producer has settled
    /// never fires, and its dependent is left in the table for the drain to report.
    pub fn edge(&mut self, producer: UnitId, dependent: UnitId) {
        let index = self.edges.len();
        self.edges.push(Edge {
            dependent,
            next: self.entries[producer.0].dependents,
        });
        self.entries[producer.0].dependents = Some(index);
    }

    /// A unit finished: decrement each of its dependents, and make ready those that reached zero.
    pub fn settle(&mut self, finished: UnitId) {
        self.outstanding -= 1;
        let mut edge = self.entries[finished.0].dependents;
        while let Some(index) = edge {
            let dependent = self.edges[index].dependent;
            let entry = &mut self.entries[dependent.0];
            entry.unmet -= 1;
            if entry.unmet == 0 {
                self.ready.push(dependent);
            }
            edge = self.edges[index].next;
        }
    }

    /// The next unit whose dependencies are all met, for the drain to give a cell to.
    pub(crate) fn ready(&mut self) -> Option<(UnitId, Unit<'graph>)> {
        let id = self.ready.pop()?;
        let unit = self.entries[id.0]
            .unit
            .take()
            .expect("a unit reaches the ready list once");
        Some((id, unit))
    }

    /// Whether every submitted unit has finished. False with a unit still waiting on a dependency
    /// that will never be satisfied, which is how the drain reports a cycle.
    pub fn is_empty(&self) -> bool {
        self.outstanding == 0
    }
}
