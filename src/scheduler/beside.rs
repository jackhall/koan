//! A kept cell read beside a step ([README.md § Faults](README.md#faults)): the drain enters a
//! cell that ended with [`read_beside`](super::Step::read_beside) through `cellgraph`'s
//! `enter_beside`, and the step reads the kept cell's region header through
//! [`beside`](super::Step::beside) at a brand the read quantifies, unrelated to the step's own. So
//! nothing read there can be laid in the step's region: a value reaches it only by the deep copy a
//! crossing makes, and a type only adopted from the kept cell's branch. The drain releases the kept
//! cell once the step ends.
//!
//! The header reaches the step through [`ReadsBeside`], an erased door, so a `Step` names no brand
//! of the kept cell's.

use crate::knot::{KLink, KRegionHeader, KValue, KnottedFamily};
use crate::memory::{Active, Bump, BumpAllocator, BumpVec, Writer};
use crate::scope::{HalfHeader, Named};
use crate::type_lattice::{Branch, DeclaredType, Parametric};
use crate::values::{Chain, TypeTiers, copy_beside};

/// What a step entered beside a kept cell reads of it, at `'kept`, a brand of the read's own: the
/// cell's link, a chain in which every type the link names resolves, and the doors that carry what
/// the step takes into its own region by value.
pub struct Beside<'x, 'kept, 'graph> {
    link: &'kept KLink<'graph, 'kept>,
    half: &'kept HalfHeader<'kept, KValue<'graph, 'kept>>,
    types: &'x Chain<'x, 'x, 'x, 'x>,
}

impl<'x, 'kept, 'graph> Beside<'x, 'kept, 'graph> {
    /// The outermost link of the fault the kept cell ended with.
    pub fn link(&self) -> &'kept KLink<'graph, 'kept> {
        self.link
    }

    /// A chain over the graph's tiers in which every type the link names resolves, adopted from
    /// the kept cell's branch: what wording the link reads.
    pub fn types(&self) -> &'x Chain<'x, 'x, 'x, 'x> {
        self.types
    }

    /// Make `kt`, a type the kept cell's region resolves, resolve in `types`, the step's own chain.
    pub fn adopt_into(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        kt: impl Into<DeclaredType<Parametric>>,
    ) {
        types.adopt(self.branch(), kt);
    }

    /// `values`, read out of the kept cell, rebuilt deep through `writer` in the step's region, all
    /// through one copy, every type they carry adopted into `types`, listed in `scratch`.
    pub fn copy_into<'here, 'list>(
        &self,
        writer: Writer<'here, '_>,
        types: &Chain<'here, 'here, '_, '_>,
        values: &[KValue<'graph, 'kept>],
        scratch: BumpAllocator<'list>,
    ) -> BumpVec<'list, KValue<'graph, 'here>> {
        copy_beside::<KnottedFamily, KRegionHeader>(
            writer,
            types,
            Active::new(self.half),
            values,
            scratch,
        )
    }

    fn branch(&self) -> Option<&'kept Branch<'kept>> {
        self.half.branch()
    }
}

/// The door a [`Step`](super::Step) reads a kept cell's header through, its brand erased: the read
/// is handed a [`Beside`] at a brand of its own, or never called where the header holds no link.
pub(super) trait ReadsBeside<'graph> {
    fn read(&self, read: &mut dyn for<'x, 'kept> FnMut(Beside<'x, 'kept, 'graph>));
}

/// A kept cell's region header as `enter_beside` hands it, at its brand `'kept`, beside the graph's
/// type tiers.
pub(super) struct KeptHeader<'graph, 'kept> {
    pub(super) half: Option<&'kept HalfHeader<'kept, KValue<'graph, 'kept>>>,
    pub(super) tiers: TypeTiers<'graph>,
}

impl<'graph> ReadsBeside<'graph> for KeptHeader<'graph, '_> {
    fn read(&self, read: &mut dyn for<'x, 'kept> FnMut(Beside<'x, 'kept, 'graph>)) {
        let Some(half) = self.half else {
            return;
        };
        let Some(link) = half.link() else {
            return;
        };
        let bump = Bump::new();
        let types = self.tiers.settling(&bump);
        link.each_named(&mut |kt| types.adopt(half.branch(), kt));
        read(Beside {
            link,
            half,
            types: &types,
        });
    }
}
