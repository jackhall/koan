//! [`OnceRun`] — a run of write-once slots laid down in a region, each holding its value erased so
//! the run can be read through a [`OnceView`] that is covariant in the region brand.
//!
//! A `Cell<V::At<'cell>>` is invariant in `'cell`, so no safe read view over live slots can be
//! shortened. Here a slot holds its value as an [`Erased`], which names only `'graph`, and a read
//! reattaches it at the view's brand — the crate's one retype seam, the way [`ThinRun`] is its one
//! hand-built layout. The run carries no vocabulary of its own: what a slot means is the
//! embedder's, built as safe code over it. See [../README.md](../README.md) § The cell.
//!
//! [`ThinRun`]: crate::ThinRun

use std::cell::Cell;
use std::marker::PhantomData;

use crate::reattach::{Covariant, Erased, Reattachable};
use crate::region::Writer;

#[cfg(test)]
mod tests;

/// One slot of a run: empty, or a value set once, erased to `'graph`.
type OnceSlot<'graph, V> = Cell<Option<Erased<'graph, V>>>;

impl<'cell> Writer<'cell> {
    /// A run of `len` empty slots, each set once at this region's brand and read at it or any
    /// shorter one — the shape a table of bindings is built over.
    pub fn once_run<'graph, V>(self, len: usize) -> OnceRun<'graph, 'cell, V>
    where
        V: Reattachable<'graph>,
        Erased<'graph, V>: Copy,
    {
        OnceRun {
            slots: self.fill(len, |_| Cell::new(None)),
            _brand: PhantomData,
        }
    }
}

/// The write handle of a run [`Writer::once_run`] laid down.
///
/// **Invariant** in `'cell`: a value set here is at the run's own brand, so no copy of the handle
/// can set a value borrowed for less than the run's readers read it at. `Copy`, since the slots
/// are region state its holder names.
pub struct OnceRun<'graph, 'cell, V: Reattachable<'graph>> {
    slots: &'cell [OnceSlot<'graph, V>],
    _brand: PhantomData<fn(&'cell ()) -> &'cell ()>,
}

impl<'graph, V: Reattachable<'graph>> Clone for OnceRun<'graph, '_, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'graph, V: Reattachable<'graph>> Copy for OnceRun<'graph, '_, V> {}

/// A slot already set: a run's one refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Written;

impl<'graph, 'cell, V> OnceRun<'graph, 'cell, V>
where
    V: Reattachable<'graph>,
    Erased<'graph, V>: Copy,
{
    pub fn len(self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(self) -> bool {
        self.slots.is_empty()
    }

    /// Set `slot`, once. Refused when it already holds a value; out of range panics like a slice
    /// index.
    pub fn set(self, slot: usize, value: V::At<'cell>) -> Result<(), Written>
    where
        'graph: 'cell,
    {
        let cell = &self.slots[slot];
        if cell.get().is_some() {
            return Err(Written);
        }
        cell.set(Some(Erased::erase(value)));
        Ok(())
    }

    /// What `slot` holds, at the run's own brand, or `None` while it is empty. Out of range panics
    /// like a slice index.
    pub fn get(self, slot: usize) -> Option<V::At<'cell>>
    where
        'graph: 'cell,
    {
        // SAFETY: every value in the run was erased by `set` at this run's own brand `'cell`, and
        // the run is invariant, so it comes back at the brand it was set at.
        self.slots[slot]
            .get()
            .map(|erased| unsafe { erased.reattach::<'cell>() })
    }

    /// The read handle over the same slots, at the run's own brand.
    ///
    /// `V` must be [`Covariant`]: the view shortens, so a value comes back through it at a brand
    /// shorter than the one it was set at, and a form that could take a `'cell` borrow in would let
    /// the reader store a short-lived borrow in a longer-lived region. The bound is proven here,
    /// where the only view of a run is made, so a reader of one never restates it.
    pub fn view(self) -> OnceView<'graph, 'cell, V>
    where
        V: Covariant<'graph>,
    {
        OnceView(self.slots)
    }
}

/// The read handle of a [`OnceRun`]: covariant in `'cell`, so a value crossing into a
/// shorter-lived cell may hold one, and without a door that sets a slot.
///
/// ```compile_fail,E0599
/// use cellgraph::{OnceView, reattachable};
/// struct Number;
/// reattachable!(Number => &'cell u32);
/// fn set<'cell>(view: OnceView<'static, 'cell, Number>, value: &'cell u32) {
///     view.set(0, value);
/// }
/// ```
///
/// A view shortens to any brand its run outlives:
///
/// ```
/// use cellgraph::{OnceView, reattachable};
/// struct Number;
/// reattachable!(Number => &'cell u32);
/// fn shorten<'long: 'short, 'short>(
///     view: OnceView<'static, 'long, Number>,
/// ) -> OnceView<'static, 'short, Number> {
///     view
/// }
/// ```
pub struct OnceView<'graph, 'cell, V: Reattachable<'graph>>(&'cell [OnceSlot<'graph, V>]);

impl<'graph, V: Reattachable<'graph>> Clone for OnceView<'graph, '_, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'graph, V: Reattachable<'graph>> Copy for OnceView<'graph, '_, V> {}

impl<'graph, 'cell, V> OnceView<'graph, 'cell, V>
where
    V: Reattachable<'graph>,
    Erased<'graph, V>: Copy,
{
    pub fn len(self) -> usize {
        self.0.len()
    }

    pub fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    /// What `slot` holds, at this view's brand, or `None` while it is empty. Out of range panics
    /// like a slice index.
    pub fn get(self, slot: usize) -> Option<V::At<'cell>>
    where
        'graph: 'cell,
    {
        // SAFETY: every value in the run was erased by `OnceRun::set` at the run's own brand `'x`,
        // and `OnceRun` is invariant, so none was set at a shorter one. This view was made by
        // `OnceRun::view` at `'x` and reached here by covariance alone, so `'x: 'cell`, and the
        // region referents a value set at `'x` holds outlive `'cell`. `view` required `V` to be
        // covariant, so a form re-anchored at the shorter brand admits no borrow in.
        self.0[slot]
            .get()
            .map(|erased| unsafe { erased.reattach::<'cell>() })
    }
}
