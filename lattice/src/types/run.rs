//! [`Run`] and [`Elements`] — the views a node's runs are read through.
//!
//! A node stores each run as a slice of raw [`Handle`]s in the run region. Read through a typed
//! handle, the run hands its children back as that handle's [`Child`](TypeHandle::Child) — a
//! concrete type's union members are concrete, a parametric type's are parametric — wrapping each
//! one as it is read, so a view copies nothing. Read raw, inside the lattice, a run is its slice.
//! A shape's element run is stored with each slot's priority class beside its handle, so
//! [`Elements`] reads both the elements and the ranking off one slice.
//!
//! [`Record`](super::record::Record) is the third view, over a run of named fields.

use std::fmt;
use std::marker::PhantomData;
use std::ops::Deref;

use super::handle::{Handle, KType, Parametric, TypeHandle, wrap};
use super::shape::{Classes, DispatchTokenElement, StoredElement};

/// A node's run of child handles — a union's members, a meet's applications — read as `H`.
pub struct Run<'run, H = Handle> {
    handles: &'run [Handle],
    view: PhantomData<H>,
}

impl<H> Clone for Run<'_, H> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<H> Copy for Run<'_, H> {}

impl<'run, H> Run<'run, H> {
    /// The run over `handles`, which already live where the node does.
    pub(super) fn over(handles: &'run [Handle]) -> Self {
        Run {
            handles,
            view: PhantomData,
        }
    }

    /// The raw slice.
    pub(super) fn raw(self) -> &'run [Handle] {
        self.handles
    }
}

impl<H> fmt::Debug for Run<'_, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.handles).finish()
    }
}

/// Two runs are equal when they hold the same handles in the same order.
impl<H> PartialEq for Run<'_, H> {
    fn eq(&self, other: &Self) -> bool {
        self.handles == other.handles
    }
}

impl<H> Eq for Run<'_, H> {}

impl<'run> Deref for Run<'run, Handle> {
    type Target = [Handle];

    fn deref(&self) -> &[Handle] {
        self.handles
    }
}

/// An expression shape's element run — keywords and typed slots in call order — read as `H`.
/// The run is stored with each slot's priority class beside it, read through
/// [`classes`](Self::classes).
pub struct Elements<'run, H = Handle> {
    elements: &'run [StoredElement],
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
    pub(super) fn over(elements: &'run [StoredElement]) -> Self {
        Elements {
            elements,
            view: PhantomData,
        }
    }

    /// The raw slice.
    pub(super) fn raw(self) -> &'run [StoredElement] {
        self.elements
    }
}

impl<'run, H: TypeHandle> Elements<'run, H> {
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

/// The typed readers, written once for each typed handle: the raw view reads its slice instead.
macro_rules! typed_runs {
    ($($handle:ty),*) => {$(
        impl<'run> Run<'run, $handle> {
            pub fn iter(
                self,
            ) -> impl DoubleEndedIterator<Item = $handle> + ExactSizeIterator + 'run {
                self.handles.iter().map(|raw| wrap(*raw))
            }

            pub fn len(self) -> usize {
                self.handles.len()
            }

            pub fn is_empty(self) -> bool {
                self.handles.is_empty()
            }

            pub fn get(self, index: usize) -> Option<$handle> {
                self.handles.get(index).map(|raw| wrap(*raw))
            }

            pub fn contains(self, handle: $handle) -> bool {
                self.handles.contains(&handle.raw())
            }
        }

        impl<'run> IntoIterator for Run<'run, $handle> {
            type Item = $handle;
            type IntoIter = std::iter::Map<
                std::iter::Copied<std::slice::Iter<'run, Handle>>,
                fn(Handle) -> $handle,
            >;

            fn into_iter(self) -> Self::IntoIter {
                self.handles.iter().copied().map(wrap as fn(Handle) -> $handle)
            }
        }

    )*};
}

typed_runs!(KType, Parametric);
