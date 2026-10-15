//! What a habitat half's header keeps for the build tree: the table of a build cell's
//! specializations by key, and the opaque token naming the build cell a function was born under.
//!
//! A [`SpecTable`] rests in a build cell's region beside its type branch, in its
//! [`HalfHeader`](super::HalfHeader): insert-only, each entry a key and a bare handle, renewed in
//! place when the child it named is rebuilt. A bare handle holds nothing alive, so a stale one is
//! a miss the drain rebuilds. A [`BuildRef`] is how code above the scheduler holds a build cell
//! without naming a handle: only `values` and `scheduler` open one. See
//! [build/README.md](../build/README.md#references-between-the-trees).

use std::cell::Cell;

use crate::memory::{DigestKeyed, DigestTable, LaysRuns, TreeHandle};

use super::ContentDigest;

/// What a parent's table finds a specialization by: the callee's segment and the argument digests
/// its invariant sites read, hashed into one digest.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SpecKey(pub ContentDigest);

/// One entry of a [`SpecTable`]: a key and the handle of the specialization it names, renewed in
/// place when that specialization is rebuilt.
pub struct SpecEntry {
    key: SpecKey,
    handle: Cell<TreeHandle>,
}

impl SpecEntry {
    /// The handle the entry names, which may be stale.
    pub fn handle(&self) -> TreeHandle {
        self.handle.get()
    }
}

impl DigestKeyed for SpecEntry {
    type Key = SpecKey;

    fn key(&self) -> SpecKey {
        self.key
    }

    fn word(key: SpecKey) -> u64 {
        key.0.bits() as u64
    }
}

/// A build cell's specializations by key. Insert-only: an evicted child's entry is renewed in
/// place, never removed.
pub type SpecTable<'cell> = DigestTable<'cell, SpecEntry>;

/// Record `handle` under `key` in `table`: renew the entry where the key has one, else lay a new
/// entry through `lay`.
pub(crate) fn enter_in<'cell>(
    table: &SpecTable<'cell>,
    lay: impl LaysRuns<'cell>,
    key: SpecKey,
    handle: TreeHandle,
) {
    match table.get(key) {
        Some(entry) => entry.handle.set(handle),
        None => {
            let entry = &lay.lay(1, |_| SpecEntry {
                key,
                handle: Cell::new(handle),
            })[0];
            table.insert_unique(lay, entry);
        }
    }
}

/// The build cell a function was born under: a specialization, or the build root. Opaque above
/// the scheduler, which alone turns one into a handle, so a step can hand the drain only a build
/// cell the scheduler named to it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BuildRef(Option<TreeHandle>);

impl BuildRef {
    /// The build root.
    pub const ROOT: BuildRef = BuildRef(None);

    /// The specialization `handle` names.
    pub(crate) fn of(handle: TreeHandle) -> Self {
        BuildRef(Some(handle))
    }

    /// The specialization this names, or `None` for the build root.
    pub(crate) fn opened(self) -> Option<TreeHandle> {
        self.0
    }
}
