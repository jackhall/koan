//! [`Region`] — the per-cell bump: the only place a value with reach may rest, in pointer-stable
//! chunks that a sealing cell's storage detaches with unmoved. See
//! [design/cellgraph.md](../design/cellgraph.md) § The cell.
//!
//! The bump is lifetime-free, so a region borrow `'r` enters only at the allocating call. That is
//! what lets a value written here hold an `&'r` back into the very region it lives in with no
//! residence check: a lifetime-*typed* slot would have to name a lifetime a region has no
//! parameter for.
//!
//! Nothing stored in a region is ever dropped — a bump releases its chunks whole — which is why
//! every family a region hosts is [`DropFree`](crate::DropFree). The write surface is
//! [`Writer`], a `Copy` handle a step receives inside a build closure's brand and cannot widen.

use bumpalo::Bump;

/// One cell's storage. Minted lazily at the cell's first allocation, so a cell that never
/// allocates costs no chunk.
pub(crate) struct Region {
    bump: Bump,
}

impl Region {
    pub(crate) fn new() -> Self {
        Region { bump: Bump::new() }
    }

    pub(crate) fn writer(&self) -> Writer<'_> {
        Writer(&self.bump)
    }

    /// Bytes the chunks occupy, whether or not a value still uses them — a bump never reclaims
    /// within a chunk, so this is what the region costs while anything holds it.
    pub(crate) fn allocated_bytes(&self) -> usize {
        self.bump.allocated_bytes()
    }
}

/// The write surface into a region's bytes, handed to a build closure at the closure's own brand.
///
/// `Copy` with a private field, so a writer exists only where the table hands one out, and every
/// verb returns a shared `&'r` rather than the `&mut` the bump itself yields: a written value is
/// region state its holder names, never one it owns. `T: Copy` on the value verbs is what stands
/// in for the missing destructor — a bump never runs one.
#[derive(Clone, Copy)]
pub struct Writer<'r>(&'r Bump);

impl<'r> Writer<'r> {
    /// Write one value and hand back the borrow of it that lives in the region.
    pub fn value<T: Copy>(self, value: T) -> &'r T {
        self.0.alloc(value)
    }

    /// Write a run of values contiguously.
    pub fn slice<T: Copy>(self, items: &[T]) -> &'r [T] {
        self.0.alloc_slice_copy(items)
    }

    /// Write text.
    pub fn text(self, text: &str) -> &'r str {
        self.0.alloc_str(text)
    }
}
