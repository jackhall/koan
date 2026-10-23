//! The **writes** one walk makes into its [`Store`](super::Store): each unit's record and each
//! owning body's closing record, beside its frontier. Every unit the walk can run is known from the
//! shape table, so the table is dense: one cell per unit, numbered by segment, then by unit. A write
//! sets its cell once, a read costs one index, and nothing needs a lock or a map. Each store keeps
//! one, and lays it into a [`LoadFacts`] once its walk returns. The walk keeps a [`UnitTable`] of
//! its own over the same cells, to find the state of a unit it ran.

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpVec, Lays};

use super::facts::{Binding, Frontier, UnitId};
use super::records::{Closing, LoadFacts, UnitFacts};
use super::shape::{Segment, UnitIndex};
use super::shapes::Shapes;

/// A unit's record beside its frontier.
type Unit<'o> = (&'o UnitFacts<'o>, Frontier<'o>);

/// An owning body's closing record beside its frontier.
type Close<'o> = (&'o Closing<'o>, Frontier<'o>);

/// One cell per unit of a shape table, numbered by segment, then by unit: set once, read by index.
pub struct UnitTable<'p, T> {
    /// Where each segment's units start in `cells`: its entry, then each of its run units.
    starts: &'p [u32],
    cells: &'p [Cell<Option<T>>],
}

impl<'p, T: Copy> UnitTable<'p, T> {
    /// Every cell empty, over every unit of `shapes`.
    pub fn new(shapes: &Shapes<'_>, scratch: BumpAllocator<'p>) -> Self {
        let bodies = shapes.bodies();
        let mut next = 0;
        let starts = scratch.alloc_slice_fill_with(bodies.len(), |at| {
            let start = next;
            next += bodies[at].units().len() as u32 + 1;
            start
        });
        UnitTable {
            starts,
            cells: scratch.alloc_slice_fill_with(next as usize, |_| Cell::new(None)),
        }
    }

    fn cell(&self, unit: UnitId) -> &Cell<Option<T>> {
        let start = self.starts[unit.segment.index() as usize];
        &self.cells[(start + unit.unit.0) as usize]
    }

    /// `unit`'s cell, where it is set.
    pub fn get(&self, unit: UnitId) -> Option<T> {
        self.cell(unit).get()
    }

    /// Set `unit`'s cell.
    pub fn set(&self, unit: UnitId, value: T) {
        self.cell(unit).set(Some(value));
    }

    /// Every set cell, by unit id.
    pub fn each(&self, shapes: &Shapes<'_>, mut each: impl FnMut(UnitId, T)) {
        for (at, shape) in shapes.bodies().iter().enumerate() {
            let start = self.starts[at] as usize;
            let cells = &self.cells[start..=start + shape.units().len()];
            for (index, cell) in cells.iter().enumerate() {
                if let Some(value) = cell.get() {
                    let unit = UnitIndex(index as u32);
                    each(
                        UnitId {
                            segment: shape.segment(),
                            unit,
                        },
                        value,
                    );
                }
            }
        }
    }
}

/// One walk's writes over `shapes`, in the walk's scratch.
pub struct Writes<'o, 'p> {
    shapes: &'o Shapes<'o>,
    units: UnitTable<'p, Unit<'o>>,
    closings: &'p [Cell<Option<Close<'o>>>],
}

impl<'o, 'p> Writes<'o, 'p> {
    /// No writes yet, over every unit of `shapes`.
    pub fn new(shapes: &'o Shapes<'o>, scratch: BumpAllocator<'p>) -> Self {
        Writes {
            shapes,
            units: UnitTable::new(shapes, scratch),
            closings: scratch.alloc_slice_fill_with(shapes.bodies().len(), |_| Cell::new(None)),
        }
    }

    /// The record this walk wrote for `unit`, where it wrote one.
    pub fn unit(&self, unit: UnitId) -> Option<Unit<'o>> {
        self.units.get(unit)
    }

    /// The closing record this walk wrote for the owning body `body`, where it wrote one.
    pub fn closing(&self, body: Segment) -> Option<Close<'o>> {
        self.closings[body.index() as usize].get()
    }

    pub fn write(&self, unit: UnitId, facts: &'o UnitFacts<'o>, frontier: Frontier<'o>) {
        debug_assert!(self.units.get(unit).is_none(), "a walk runs each unit once");
        self.units.set(unit, (facts, frontier));
    }

    pub fn write_close(&self, body: Segment, closing: &'o Closing<'o>, frontier: Frontier<'o>) {
        let cell = &self.closings[body.index() as usize];
        debug_assert!(cell.get().is_none(), "a walk closes each body once");
        cell.set(Some((closing, frontier)));
    }

    /// Every write, laid through `lay` as the facts of a walk over `base` under `binding`, not yet
    /// folded. The units come out by segment, then by unit: sorted by id.
    pub fn laid(
        &self,
        lay: impl Lays<'o>,
        scratch: BumpAllocator<'_>,
        base: Option<&'o LoadFacts<'o>>,
        binding: Binding<'o>,
    ) -> LoadFacts<'o> {
        let mut units = BumpVec::new_in(scratch);
        self.units
            .each(self.shapes, |unit, record| units.push((unit, record)));
        let mut closings = BumpVec::new_in(scratch);
        for (at, cell) in self.closings.iter().enumerate() {
            if let Some(record) = cell.get() {
                closings.push((self.shapes.bodies()[at].segment(), record));
            }
        }
        LoadFacts::of_records(lay, scratch, self.shapes, base, binding, &units, &closings)
    }
}
