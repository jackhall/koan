//! The **slot array**: a fixed run of write-once binding slots laid down in a cell's region,
//! addressed by index rather than by key — for a table whose key set is fixed before its first
//! write, such as a call's value bindings, sized by the body's shape.
//!
//! A slot is empty until its binder writes it, and bound once. It is safe code over `cellgraph`'s
//! [`OnceRun`]: each slot holds its value erased, so the array hands out a [`SlotView`] that is
//! covariant in the region brand — a reader in a shorter-lived cell may hold one — while the array
//! itself, which binds, is invariant. The binding vocabulary is this file's; the run beneath it
//! knows nothing of names or binders.
//!
//! The payload is a family, `V`, because the run erases a value to its form at `'graph`: the array
//! names its payload at any brand through the family rather than as one type.

use super::substrate::{Covariant, Erased, OnceRun, OnceView, Reattachable, Writer};

/// `len` write-once binding slots in a cell's region. Invariant in `'cell`: it binds.
pub struct SlotArray<'graph, 'cell, V: Reattachable<'graph>>(OnceRun<'graph, 'cell, V>);

impl<'graph, V: Reattachable<'graph>> Clone for SlotArray<'graph, '_, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'graph, V: Reattachable<'graph>> Copy for SlotArray<'graph, '_, V> {}

/// A slot already bound: binding is once.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SlotConflict;

impl<'graph, 'cell, V> SlotArray<'graph, 'cell, V>
where
    V: Reattachable<'graph>,
    Erased<'graph, V>: Copy,
{
    /// `len` empty slots, laid down in the region `writer` names.
    pub fn new(writer: Writer<'cell>, len: usize) -> Self {
        SlotArray(writer.once_run(len))
    }

    pub fn len(self) -> usize {
        self.0.len()
    }

    pub fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    /// Bind `slot`, once. Out of range panics like a slice index.
    pub fn bind(self, slot: usize, value: V::At<'cell>) -> Result<(), SlotConflict>
    where
        'graph: 'cell,
    {
        self.0.set(slot, value).map_err(|_| SlotConflict)
    }

    /// The read half over the same slots.
    pub fn view(self) -> SlotView<'graph, 'cell, V>
    where
        V: Covariant<'graph>,
    {
        SlotView(self.0.view())
    }
}

/// The read half of a [`SlotArray`]: covariant in `'cell`, so a reader in a shorter-lived cell may
/// hold it, and without a door that binds.
pub struct SlotView<'graph, 'cell, V: Reattachable<'graph>>(OnceView<'graph, 'cell, V>);

impl<'graph, V: Reattachable<'graph>> Clone for SlotView<'graph, '_, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'graph, V: Reattachable<'graph>> Copy for SlotView<'graph, '_, V> {}

impl<'graph, 'cell, V> SlotView<'graph, 'cell, V>
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

    /// What `slot` is bound to, at this view's brand, or `None` while it is empty.
    pub fn get(self, slot: usize) -> Option<V::At<'cell>>
    where
        'graph: 'cell,
    {
        self.0.get(slot)
    }
}

#[cfg(test)]
mod tests;
