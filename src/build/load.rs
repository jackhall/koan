//! The **load's store**: where the load's walk keeps its records. It binds nothing in production,
//! so every unit the walk reaches runs, and it lays every record through the writer it is given,
//! into program storage at the load. It keeps no base, so a test may run it after the freeze over a
//! binding of its own and compare its facts with a resumption's
//! ([the store](README.md#the-store)). It lays every [derived name](README.md#derived-names) the
//! walk meets, named by its term's content through the load's [`Spellings`], in its base layer
//! beside its records.

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpVec, Lays};
use crate::scope::{
    Binding, Bound, Closing, Derived, DerivedName, DerivedTerm, Frontier, LoadFacts, Segment,
    Shapes, Spellings, Store, UnitFacts, UnitId, UnitRecord, Unknown, Wake, Writes,
};

/// The load's store over `shapes`, laying through `L`.
pub struct LoadStore<'o, 'p, L> {
    lay: L,
    binding: Binding<'o>,
    writes: Writes<'o, 'p>,
    /// Each derived name the walk laid, in the order it met their keys.
    derived: Cell<BumpVec<'p, DerivedName<'p>>>,
    spellings: Spellings<'p>,
    scratch: BumpAllocator<'p>,
}

impl<'o, 'p, L: Lays<'o>> LoadStore<'o, 'p, L> {
    /// A store binding `binding`, its writes kept in `scratch` until it lays them, naming derived
    /// names through `spellings`.
    pub fn new(
        lay: L,
        scratch: BumpAllocator<'p>,
        (shapes, spellings): (&'o Shapes<'o>, Spellings<'p>),
        binding: Binding<'o>,
    ) -> Self {
        LoadStore {
            lay,
            binding,
            writes: Writes::new(shapes, scratch),
            derived: Cell::new(BumpVec::new_in(scratch)),
            spellings,
            scratch,
        }
    }

    /// Every record the walk wrote, not yet folded, beside the derived names it laid.
    pub fn facts(&self, scratch: BumpAllocator<'_>) -> LoadFacts<'o> {
        let derived = self.names(|names| Derived::sorted(self.lay, scratch, names));
        self.writes
            .laid(self.lay, scratch, None, derived, self.binding)
    }
}

impl<'o, 'p, L> LoadStore<'o, 'p, L> {
    /// `read` of the derived names laid so far, which it holds meanwhile.
    fn names<R>(&self, read: impl FnOnce(&mut BumpVec<'p, DerivedName<'p>>) -> R) -> R {
        let mut names = self.derived.replace(BumpVec::new_in(self.scratch));
        let read = read(&mut names);
        self.derived.set(names);
        read
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
        self.writes.unit(unit).and_then(UnitRecord::typed)
    }

    fn close(&self, body: Segment) -> Option<(&'o Closing<'o>, Frontier<'o>)> {
        self.writes.closing(body)
    }

    /// The load keeps no record from before its walk, so every unit runs.
    fn wakes(&self, _: Frontier<'_>, _: &crate::values::Chain<'_, '_, '_, '_>) -> Wake {
        Wake::Open
    }

    fn traces(&self) -> bool {
        true
    }

    /// A name met again is the one laid; else it is named now and laid.
    fn derived(
        &self,
        types: &crate::values::Chain<'_, '_, '_, '_>,
        term: DerivedTerm<'_>,
    ) -> DerivedName<'_> {
        if let Some(held) = self.derived_name(term.id()) {
            return held;
        }
        let term = term.laid(self.scratch);
        let name = (self.spellings).named((types, self.scratch), term);
        self.names(|names| names.push(name));
        name
    }

    fn derived_name(&self, id: crate::scope::DerivedId) -> Option<DerivedName<'_>> {
        self.names(|names| names.iter().find(|name| name.id == id).copied())
    }

    fn write(&self, unit: UnitId, record: UnitRecord<'o>) {
        self.writes.write(unit, record);
    }

    fn write_close(&self, body: Segment, closing: &'o Closing<'o>, frontier: Frontier<'o>) {
        self.writes.write_close(body, closing, frontier);
    }
}
