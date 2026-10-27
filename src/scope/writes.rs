//! The **writes** one walk makes into its [`Store`](super::Store): each unit's record and each
//! owning body's closing record, beside its frontier. The load writes every unit the shape table
//! holds, so its [`Writes`] are dense: one cell per unit, numbered by segment, then by unit. A write
//! sets its cell once, a read costs one index, and nothing needs a lock or a map. A layer writes the
//! few units its binding wakes, so its [`KeyedWrites`] are a table keyed by unit, sized by what it
//! wrote. Each store keeps one, and lays it into a [`LoadFacts`] once its walk returns.

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, Lays, bump_table};

use super::facts::{Binding, Frontier, UnitId};
use super::records::{Closing, LoadFacts, UnitFacts};
use super::shape::{Segment, UnitIndex};
use super::shapes::Shapes;

/// A unit's record beside its frontier.
type Unit<'o> = (&'o UnitFacts<'o>, Frontier<'o>);

/// An owning body's closing record beside its frontier.
type Close<'o> = (&'o Closing<'o>, Frontier<'o>);

/// One walk's writes sorted by id: its units, then its closings.
type Sorted<'x, 'o> = (
    BumpVec<'x, (UnitId, Unit<'o>)>,
    BumpVec<'x, (Segment, Close<'o>)>,
);

/// One cell per unit of a shape table, numbered by segment, then by unit: set once, read by index.
struct UnitTable<'p, T> {
    /// Where each segment's units start in `cells`: its entry, then each of its run units.
    starts: &'p [u32],
    cells: &'p [Cell<Option<T>>],
}

impl<'p, T: Copy> UnitTable<'p, T> {
    /// Every cell empty, over every unit of `shapes`.
    fn new(shapes: &Shapes<'_>, scratch: BumpAllocator<'p>) -> Self {
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
    fn get(&self, unit: UnitId) -> Option<T> {
        self.cell(unit).get()
    }

    /// Set `unit`'s cell.
    fn set(&self, unit: UnitId, value: T) {
        self.cell(unit).set(Some(value));
    }

    /// Every set cell, by unit id.
    fn each(&self, shapes: &Shapes<'_>, mut each: impl FnMut(UnitId, T)) {
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

/// One walk's writes keyed by unit and by body, in the walk's scratch: a layer's, which writes only
/// the units its binding wakes. Each table is taken out while a read or a write holds it.
pub struct KeyedWrites<'o, 'p> {
    scratch: BumpAllocator<'p>,
    units: Cell<BumpBackedMap<'p, UnitId, Unit<'o>>>,
    closings: Cell<BumpBackedMap<'p, Segment, Close<'o>>>,
}

impl<'o, 'p> KeyedWrites<'o, 'p> {
    /// No writes yet.
    pub fn new(scratch: BumpAllocator<'p>) -> Self {
        KeyedWrites {
            scratch,
            units: Cell::new(bump_table(scratch)),
            closings: Cell::new(bump_table(scratch)),
        }
    }

    /// `read` of the unit table, which it holds meanwhile.
    fn units<R>(&self, read: impl FnOnce(&mut BumpBackedMap<'p, UnitId, Unit<'o>>) -> R) -> R {
        let mut units = self.units.replace(bump_table(self.scratch));
        let read = read(&mut units);
        self.units.set(units);
        read
    }

    /// `read` of the closing table, which it holds meanwhile.
    fn closings<R>(&self, read: impl FnOnce(&mut BumpBackedMap<'p, Segment, Close<'o>>) -> R) -> R {
        let mut closings = self.closings.replace(bump_table(self.scratch));
        let read = read(&mut closings);
        self.closings.set(closings);
        read
    }

    /// The record this walk wrote for `unit`, where it wrote one.
    pub fn unit(&self, unit: UnitId) -> Option<Unit<'o>> {
        self.units(|units| units.get(&unit).copied())
    }

    /// The closing record this walk wrote for the owning body `body`, where it wrote one.
    pub fn closing(&self, body: Segment) -> Option<Close<'o>> {
        self.closings(|closings| closings.get(&body).copied())
    }

    pub fn write(&self, unit: UnitId, facts: &'o UnitFacts<'o>, frontier: Frontier<'o>) {
        let held = self.units(|units| units.insert(unit, (facts, frontier)));
        debug_assert!(held.is_none(), "a walk runs each unit once");
    }

    pub fn write_close(&self, body: Segment, closing: &'o Closing<'o>, frontier: Frontier<'o>) {
        let held = self.closings(|closings| closings.insert(body, (closing, frontier)));
        debug_assert!(held.is_none(), "a walk closes each body once");
    }

    /// Every write, sorted by id: the units, then the closings.
    pub fn sorted<'x>(&self, scratch: BumpAllocator<'x>) -> Sorted<'x, 'o> {
        let mut units = BumpVec::new_in(scratch);
        self.units(|held| units.extend(held.iter().map(|(unit, record)| (*unit, *record))));
        units.sort_unstable_by_key(|(unit, _)| *unit);
        let mut closings = BumpVec::new_in(scratch);
        self.closings(|held| closings.extend(held.iter().map(|(body, record)| (*body, *record))));
        closings.sort_unstable_by_key(|(body, _)| *body);
        (units, closings)
    }

    /// Every write, laid through `lay` as the facts of a walk over `shapes` and `base` under
    /// `binding`, not yet folded.
    pub fn laid(
        &self,
        lay: impl Lays<'o>,
        scratch: BumpAllocator<'_>,
        shapes: &'o Shapes<'o>,
        base: Option<&'o LoadFacts<'o>>,
        binding: Binding<'o>,
    ) -> LoadFacts<'o> {
        let (units, closings) = self.sorted(scratch);
        LoadFacts::of_records(lay, scratch, shapes, base, binding, &units, &closings)
    }
}
