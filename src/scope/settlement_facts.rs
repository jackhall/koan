//! A **settlement's facts**: what one walk rooted at a body decided under one binding, over its
//! parent's. It keeps the binding, the records of the units the binding woke and the closings the
//! walk wrote, the index of the facts those records hold, and every closed tree they computed by
//! its identity, each found as the load finds its own ([`records`](super::records)). The body's
//! folds stay the load's, so a read of one falls to the load.
//!
//! A settlement whose walk reached a refusal keeps the refusal's wording instead of records, so a
//! frame born under it faults as the load would, and a second ask refuses alike. The arena owns
//! each settlement beside its parent; a step reads one only through [`Facts`](super::Facts), at
//! its own lifetime.

use crate::memory::{BumpAllocator, Lays};

use super::facts::Binding;
use super::records::Records;
use super::shapes::Shapes;
use super::writes::KeyedWrites;

/// One settlement's facts over its parent's, the root's where it has none.
#[derive(Clone, Copy)]
pub struct Settlement<'a> {
    parent: Option<&'a Settlement<'a>>,
    records: Records<'a>,
    refusal: Option<&'a str>,
}

impl<'a> Settlement<'a> {
    /// The facts a walk over `shapes` under `binding` wrote into `writes`, over `parent`'s, laid
    /// through `lay`.
    pub fn of(
        lay: impl Lays<'a>,
        scratch: BumpAllocator<'_>,
        shapes: &Shapes<'_>,
        parent: Option<&'a Settlement<'a>>,
        binding: Binding<'a>,
        writes: &KeyedWrites<'a, '_>,
    ) -> Self {
        let (units, closings) = writes.sorted(scratch);
        Settlement {
            parent,
            records: Records::of(lay, scratch, shapes, None, binding, &units, &closings),
            refusal: None,
        }
    }

    /// A settlement over `parent`'s binding that holds no fact of its own.
    pub fn empty(parent: Option<&'a Settlement<'a>>) -> Self {
        Settlement {
            parent,
            records: Records::bare(parent.map_or(Binding::EMPTY, |parent| parent.binding())),
            refusal: None,
        }
    }

    /// A settlement whose walk under `binding` refused, worded `refusal`: it holds no record.
    pub fn refused(
        parent: Option<&'a Settlement<'a>>,
        binding: Binding<'a>,
        refusal: &'a str,
    ) -> Self {
        Settlement {
            parent,
            records: Records::bare(binding),
            refusal: Some(refusal),
        }
    }

    /// The wording of the refusal its walk reached, if it reached one.
    pub fn refusal(&self) -> Option<&'a str> {
        self.refusal
    }

    /// The binding it settled under: its parent's and more.
    pub fn binding(&self) -> Binding<'a> {
        self.records.binding()
    }

    /// Hand `each` every type its own records name.
    #[cfg(test)]
    pub fn each_type(
        &self,
        each: &mut dyn FnMut(crate::type_lattice::DeclaredType<crate::type_lattice::Parametric>),
    ) {
        self.records.each_type(each);
    }

    pub(super) fn parent(&self) -> Option<&'a Settlement<'a>> {
        self.parent
    }

    pub(super) fn records(&self) -> &Records<'a> {
        &self.records
    }
}
