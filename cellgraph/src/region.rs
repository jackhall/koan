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
//!
//! A region is a **bundle** of bumps: the one it writes into, plus the bumps of every region
//! absorbed into it. Absorption is how a merge splices storage
//! ([liveness-matrix.md § Locality tactics](../design/liveness-matrix.md#locality-tactics)) —
//! a `Bump` moves without moving a chunk byte, so the pointer stability a detached seal already
//! relies on carries a borrow across the merge unchanged.

use bumpalo::Bump;

/// One cell's storage: the bump it writes into, plus the bumps it has absorbed. Minted lazily at
/// the cell's first allocation, so a cell that never allocates costs no chunk.
pub(crate) struct Region {
    bump: Bump,
    /// Bumps merged in from regions this one absorbed. Read-only from here on — nothing is ever
    /// allocated into an absorbed bump again — but their chunks stay at their addresses, which is
    /// what the borrows minted before the merge still name.
    absorbed: Vec<Bump>,
    /// Chunk bytes this bundle has taken in from other regions, over its whole life. Monotone: a
    /// bundle never gives storage back, so the difference between two readings is what it absorbed
    /// between them — the accretion a loop cart is priced on
    /// ([liveness-matrix.md § Locality tactics](../design/liveness-matrix.md#locality-tactics)).
    absorbed_bytes: usize,
}

impl Region {
    pub(crate) fn new() -> Self {
        Region {
            bump: Bump::new(),
            absorbed: Vec::new(),
            absorbed_bytes: 0,
        }
    }

    pub(crate) fn writer(&self) -> Writer<'_> {
        Writer(&self.bump)
    }

    /// Take `other`'s chunks into this bundle. The bumps move; the chunks do not.
    fn absorb(&mut self, other: Region) {
        self.absorbed_bytes += other.allocated_bytes();
        self.absorbed.extend(other.absorbed);
        self.absorbed.push(other.bump);
    }

    /// Splice one optional region into another — the storage half of every merge. A source with no
    /// region contributes nothing; a target with none takes the source whole.
    pub(crate) fn splice(into: &mut Option<Region>, from: Option<Region>) {
        let Some(mut from) = from else {
            return;
        };
        match into {
            Some(target) => target.absorb(from),
            // A target with no region of its own takes the source whole, so every byte of the
            // bundle it ends up with is storage that came in from elsewhere.
            None => {
                from.absorbed_bytes = from.allocated_bytes();
                *into = Some(from);
            }
        }
    }

    /// Bytes the chunks occupy, whether or not a value still uses them — a bump never reclaims
    /// within a chunk, so this is what the region costs while anything holds it. Absorbed bumps
    /// count: the bundle is answerable for every chunk it took in.
    pub(crate) fn allocated_bytes(&self) -> usize {
        self.bump.allocated_bytes()
            + self
                .absorbed
                .iter()
                .map(Bump::allocated_bytes)
                .sum::<usize>()
    }

    /// Of those bytes, the ones that arrived by absorbing another region. Never decreases, so an
    /// embedder reads a cart's accretion as the difference against an earlier reading rather than
    /// by scanning what the chunks still hold.
    pub(crate) fn absorbed_bytes(&self) -> usize {
        self.absorbed_bytes
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
