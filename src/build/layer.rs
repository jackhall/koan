//! A **layer**: a resumption's store over the facts it extends. Its binding extends its base's,
//! a unit re-runs where its frontier meets what the layer binds that its base leaves unbound
//! ([`Wakes`]), and every other read falls to the base. It writes only what re-runs, so its writes
//! are keyed, sized by what it wrote ([the store](README.md#the-store)).

use crate::memory::{BumpAllocator, BumpVec, Lays};
use crate::scope::{
    Binding, Bound, Closing, Frontier, KeyedWrites, RecordSource, Segment, Store, UnitFacts,
    UnitId, Unknown,
};

use super::wake::Wakes;

/// A resumption's store over `base`, laying through `L`.
pub(crate) struct Layer<'o, 'p, L, B> {
    lay: L,
    base: B,
    binding: Binding<'o>,
    /// What `binding` binds that the base's does not wakes a record.
    wakes: Wakes<'o, 'p>,
    writes: KeyedWrites<'o, 'p>,
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "no settlement runs a layer yet")
)]
impl<'o, 'p, L: Lays<'o>, B: RecordSource<'o>> Layer<'o, 'p, L, B> {
    /// A layer over `base` binding `binding`, which binds every unknown the base's binds alike.
    pub(crate) fn new(lay: L, scratch: BumpAllocator<'p>, base: B, binding: Binding<'o>) -> Self {
        debug_assert!(
            binding.extends(base.binding()),
            "a layer binds what its base binds"
        );
        let mut fresh = BumpVec::new_in(scratch);
        fresh.extend(
            (binding.entries().iter())
                .filter(|(unknown, _)| base.binding().bound(*unknown).is_none())
                .copied(),
        );
        Layer {
            lay,
            base,
            binding,
            wakes: Wakes::new(Binding::laid(lay, &mut fresh), scratch),
            writes: KeyedWrites::new(scratch),
        }
    }

    /// What this layer binds: its base's binding and more.
    pub(crate) fn bound(&self) -> Binding<'o> {
        self.binding
    }

    /// The records this layer's walk wrote.
    pub(crate) fn writes(&self) -> &KeyedWrites<'o, 'p> {
        &self.writes
    }
}

impl<'o, L: Lays<'o>, B: RecordSource<'o>> Store<'o> for Layer<'o, '_, L, B> {
    type Lay = L;

    fn lay(&self) -> L {
        self.lay
    }

    fn binding(&self, unknown: Unknown) -> Option<Bound<'o>> {
        self.binding.bound(unknown)
    }

    fn fact(&self, unit: UnitId) -> Option<(&'o UnitFacts<'o>, Frontier<'o>)> {
        self.writes.unit(unit).or_else(|| self.base.unit(unit))
    }

    fn close(&self, body: Segment) -> Option<(&'o Closing<'o>, Frontier<'o>)> {
        (self.writes.closing(body)).or_else(|| self.base.closing(body))
    }

    fn wakes(&self, frontier: Frontier<'_>) -> bool {
        self.wakes.wakes(frontier)
    }

    fn write(&self, unit: UnitId, facts: &'o UnitFacts<'o>, frontier: Frontier<'o>) {
        self.writes.write(unit, facts, frontier);
    }

    fn write_close(&self, body: Segment, closing: &'o Closing<'o>, frontier: Frontier<'o>) {
        self.writes.write_close(body, closing, frontier);
    }
}
