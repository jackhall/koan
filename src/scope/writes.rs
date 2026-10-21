//! The **writes** one walk makes into its [`Store`](super::Store): each unit's record and each
//! owning body's closing record, beside its frontier. Every unit the walk can run is known from the
//! shape table, so the table is dense: one cell per unit, numbered by segment, then by unit. A write
//! sets its cell once, a read costs one index, and nothing needs a lock or a map. Each store keeps
//! one, and lays it into a [`LoadFacts`] once its walk returns.

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpVec, Lays};

use super::facts::{Binding, Frontier, UnitId};
use super::records::{Closing, Folds, LoadFacts, UnitFacts};
use super::shape::{Segment, UnitIndex};
use super::shapes::Shapes;

/// A unit's record beside its frontier.
type Unit<'o> = (&'o UnitFacts<'o>, Frontier<'o>);

/// An owning body's closing record beside its frontier.
type Close<'o> = (&'o Closing<'o>, Frontier<'o>);

/// One walk's writes over `shapes`, in the walk's scratch.
pub struct Writes<'o, 'p> {
    shapes: &'o Shapes<'o>,
    /// Where each segment's units start in `units`: its entry, then each of its run units.
    starts: &'p [u32],
    units: &'p [Cell<Option<Unit<'o>>>],
    closings: &'p [Cell<Option<Close<'o>>>],
}

impl<'o, 'p> Writes<'o, 'p> {
    /// No writes yet, over every unit of `shapes`.
    pub fn new(shapes: &'o Shapes<'o>, scratch: BumpAllocator<'p>) -> Self {
        let bodies = shapes.bodies();
        let mut next = 0;
        let starts = scratch.alloc_slice_fill_with(bodies.len(), |at| {
            let start = next;
            next += bodies[at].units().len() as u32 + 1;
            start
        });
        Writes {
            shapes,
            starts,
            units: scratch.alloc_slice_fill_with(next as usize, |_| Cell::new(None)),
            closings: scratch.alloc_slice_fill_with(bodies.len(), |_| Cell::new(None)),
        }
    }

    fn cell(&self, unit: UnitId) -> &Cell<Option<Unit<'o>>> {
        let start = self.starts[unit.segment.index() as usize];
        &self.units[(start + unit.unit.0) as usize]
    }

    /// The record this walk wrote for `unit`, where it wrote one.
    pub fn unit(&self, unit: UnitId) -> Option<Unit<'o>> {
        self.cell(unit).get()
    }

    /// The closing record this walk wrote for the owning body `body`, where it wrote one.
    pub fn closing(&self, body: Segment) -> Option<Close<'o>> {
        self.closings[body.index() as usize].get()
    }

    pub fn write(&self, unit: UnitId, facts: &'o UnitFacts<'o>, frontier: Frontier<'o>) {
        let cell = self.cell(unit);
        debug_assert!(cell.get().is_none(), "a walk runs each unit once");
        cell.set(Some((facts, frontier)));
    }

    pub fn write_close(&self, body: Segment, closing: &'o Closing<'o>, frontier: Frontier<'o>) {
        let cell = &self.closings[body.index() as usize];
        debug_assert!(cell.get().is_none(), "a walk closes each body once");
        cell.set(Some((closing, frontier)));
    }

    /// Every write, laid through `lay` as the facts of a walk over `base` under `binding`, beside
    /// `folds` by segment: none where it is empty.
    pub fn laid(
        &self,
        lay: impl Lays<'o>,
        scratch: BumpAllocator<'_>,
        base: Option<&'o LoadFacts<'o>>,
        binding: Binding<'o>,
        folds: &[Folds<'o>],
    ) -> LoadFacts<'o> {
        let mut units = BumpVec::new_in(scratch);
        for (at, shape) in self.shapes.bodies().iter().enumerate() {
            let start = self.starts[at] as usize;
            for (index, cell) in self.units[start..=start + shape.units().len()]
                .iter()
                .enumerate()
            {
                if let Some(record) = cell.get() {
                    let unit = UnitIndex(index as u32);
                    units.push((
                        UnitId {
                            segment: shape.segment(),
                            unit,
                        },
                        record,
                    ));
                }
            }
        }
        let mut closings = BumpVec::new_in(scratch);
        for (at, cell) in self.closings.iter().enumerate() {
            if let Some(record) = cell.get() {
                closings.push((self.shapes.bodies()[at].segment(), record));
            }
        }
        LoadFacts::of_records(
            lay,
            scratch,
            self.shapes,
            base,
            binding,
            &mut units,
            &mut closings,
            folds,
        )
    }
}
