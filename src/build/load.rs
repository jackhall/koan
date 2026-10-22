//! The **load's store**: where the load's walk keeps its records. It binds nothing in production,
//! so every unit the walk reaches runs, and it lays every record through the writer it is given,
//! into program storage at the load. It keeps no base, so a test may run it after the freeze over a
//! binding of its own and compare its facts with a resumption's
//! ([the store](README.md#the-store)).

use crate::memory::{BumpAllocator, Lays};
use crate::scope::{
    Binding, Bound, Closing, Folds, Frontier, LoadFacts, Segment, ShapeError, Shapes, Store,
    UnitFacts, UnitId, Unknown, Writes,
};

/// The load's store over `shapes`, laying through `L`.
pub struct LoadStore<'o, 'p, L> {
    lay: L,
    binding: Binding<'o>,
    writes: Writes<'o, 'p>,
}

impl<'o, 'p, L: Lays<'o>> LoadStore<'o, 'p, L> {
    /// A store binding `binding`, its writes kept in `scratch` until it lays them.
    pub fn new(
        lay: L,
        scratch: BumpAllocator<'p>,
        shapes: &'o Shapes<'o>,
        binding: Binding<'o>,
    ) -> Self {
        LoadStore {
            lay,
            binding,
            writes: Writes::new(shapes, scratch),
        }
    }

    /// Every record the walk wrote, beside `folds` by segment: none where it is empty.
    pub fn facts(&self, scratch: BumpAllocator<'_>, folds: &[Folds<'o>]) -> LoadFacts<'o> {
        self.writes
            .laid(self.lay, scratch, None, self.binding, folds)
    }
}

impl<'o, L: Lays<'o>> Store<'o> for LoadStore<'o, '_, L> {
    type Lay = L;

    fn lay(&self) -> L {
        self.lay
    }

    fn binding(&self, unknown: Unknown) -> Option<Bound<'o>> {
        self.binding.bound(unknown)
    }

    fn fact(&self, unit: UnitId) -> Option<(&'o UnitFacts<'o>, Frontier<'o>)> {
        self.writes.unit(unit)
    }

    fn close(&self, body: Segment) -> Option<(&'o Closing<'o>, Frontier<'o>)> {
        self.writes.closing(body)
    }

    /// The load keeps no record from before its walk, so every unit runs.
    fn wakes(&self, _: Frontier<'_>) -> bool {
        true
    }

    fn write(&self, unit: UnitId, facts: &'o UnitFacts<'o>, frontier: Frontier<'o>) {
        self.writes.write(unit, facts, frontier);
    }

    fn write_close(&self, body: Segment, closing: &'o Closing<'o>, frontier: Frontier<'o>) {
        self.writes.write_close(body, closing, frontier);
    }

    fn refuse(&self, error: ShapeError<'o>) -> ShapeError<'o> {
        error
    }
}
