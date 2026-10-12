//! [`Run`] and [`Elements`] — the views a node's runs are read through.
//!
//! A node stores each run as a slice of references to its children's nodes in the run region. Read
//! through a typed handle, the run hands its children back as that handle's
//! [`Child`](TypeHandle::Child) — a concrete type's union members are concrete, a parametric type's
//! are parametric — wrapping each one's handle as it is read, so a view copies nothing. Read inside
//! the lattice, a run is its slice of references. A shape's element run is stored with each slot's
//! priority class beside its reference, so [`Elements`] reads both the elements and the ranking off
//! one slice.
//!
//! [`Record`](super::record::Record) is the third view, over a run of named fields.

use std::fmt;
use std::marker::PhantomData;
use std::ops::Deref;

use super::handle::{Handle, KType, Naming, Parametric};
use super::shape::{Classes, DispatchTokenElement, StoredElement};
use super::stored::Linked;

/// A node's run of children — a union's members, a meet's applications — read as `H`.
pub struct Run<'run, H = Handle> {
    nodes: &'run [Linked<'run>],
    view: PhantomData<H>,
}

impl<H> Clone for Run<'_, H> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<H> Copy for Run<'_, H> {}

impl<'run, H> Run<'run, H> {
    /// The run over `nodes`, which already live where the node does.
    pub(super) fn over(nodes: &'run [Linked<'run>]) -> Self {
        Run {
            nodes,
            view: PhantomData,
        }
    }

    /// The slice of references.
    pub(super) fn raw(self) -> &'run [Linked<'run>] {
        self.nodes
    }
}

impl<H> fmt::Debug for Run<'_, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.nodes).finish()
    }
}

/// Two runs are equal when they hold the same types in the same order.
impl<H> PartialEq for Run<'_, H> {
    fn eq(&self, other: &Self) -> bool {
        self.nodes == other.nodes
    }
}

impl<H> Eq for Run<'_, H> {}

impl<'run> Deref for Run<'run, Linked<'run>> {
    type Target = [Linked<'run>];

    fn deref(&self) -> &[Linked<'run>] {
        self.nodes
    }
}

/// An expression shape's element run — keywords and typed slots in call order — read as `H`.
/// The run is stored with each slot's priority class beside it, read through
/// [`classes`](Self::classes).
pub struct Elements<'run, H = Handle> {
    elements: &'run [StoredElement<'run>],
    view: PhantomData<H>,
}

impl<H> Clone for Elements<'_, H> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<H> Copy for Elements<'_, H> {}

impl<'run, H> Elements<'run, H> {
    /// The run over `elements`, which already live where the node does.
    pub(super) fn over(elements: &'run [StoredElement<'run>]) -> Self {
        Elements {
            elements,
            view: PhantomData,
        }
    }

    /// The stored slice.
    pub(super) fn raw(self) -> &'run [StoredElement<'run>] {
        self.elements
    }
}

impl<'run, H: Naming<'run>> Elements<'run, H> {
    pub fn iter(
        self,
    ) -> impl DoubleEndedIterator<Item = DispatchTokenElement<H>> + ExactSizeIterator + 'run {
        self.elements.iter().map(|element| element.element())
    }

    pub fn len(self) -> usize {
        self.elements.len()
    }

    pub fn is_empty(self) -> bool {
        self.elements.is_empty()
    }

    /// Each slot's priority class, in slot order.
    pub fn classes(self) -> Classes<'run> {
        Classes::over(self.elements)
    }
}

impl<H> fmt::Debug for Elements<'_, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.elements).finish()
    }
}

/// The typed readers, written once for each typed handle: the lattice's own view reads its slice
/// instead.
macro_rules! typed_runs {
    ($($handle:ty),*) => {$(
        impl<'run> Run<'run, $handle> {
            pub fn iter(
                self,
            ) -> impl DoubleEndedIterator<Item = $handle> + ExactSizeIterator + 'run {
                self.nodes.iter().map(|node| <$handle as Naming>::read(*node))
            }

            pub fn len(self) -> usize {
                self.nodes.len()
            }

            pub fn is_empty(self) -> bool {
                self.nodes.is_empty()
            }

            pub fn get(self, index: usize) -> Option<$handle> {
                self.nodes.get(index).map(|node| <$handle as Naming>::read(*node))
            }

            pub fn contains(self, handle: $handle) -> bool {
                self.nodes.iter().any(|node| node.digest() == handle.digest())
            }
        }

        impl<'run> IntoIterator for Run<'run, $handle> {
            type Item = $handle;
            type IntoIter = std::iter::Map<
                std::iter::Copied<std::slice::Iter<'run, Linked<'run>>>,
                fn(Linked<'run>) -> $handle,
            >;

            fn into_iter(self) -> Self::IntoIter {
                self.nodes
                    .iter()
                    .copied()
                    .map(<$handle as Naming>::read as fn(Linked<'run>) -> $handle)
            }
        }

    )*};
}

typed_runs!(KType, Parametric);

// The suite reads a node through a raw handle.
#[cfg(test)]
typed_runs!(Handle);
