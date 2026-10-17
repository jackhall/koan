//! What a habitat half's header keeps for the build tree: the table of a build cell's
//! specializations by key, and the opaque token naming the build cell a function was born under.
//!
//! A [`SpecTable`] rests in a build cell's region beside its type branch, in its
//! [`HalfHeader`](super::HalfHeader): a fixed run of entries laid with the cell and never grown,
//! as long as the cell's bound ([`SpecBounds`]). Each entry is a key, a bare handle and the
//! parent's birth count at the child's last birth. A bare handle holds nothing alive, so a stale
//! one is a miss the drain rebuilds in place. A miss with no empty or stale entry takes the entry
//! of the least recently born child, whose subtree the drain releases first. The table counts
//! every birth under its cell, so every stamp is distinct. A [`BuildRef`] is how code above the
//! scheduler holds a build cell without naming a handle: only `values` and `scheduler` open one.
//! See [build/README.md](../build/README.md#references-between-the-trees).

use std::cell::Cell;
use std::num::NonZeroUsize;

use crate::memory::{CellHandle, LaysRuns, TreeHandle};

use super::ContentDigest;

/// What a parent's table finds a specialization by: the callee's segment and the argument digests
/// its invariant sites read, hashed into one digest.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SpecKey(pub ContentDigest);

/// The two bounds a load sets: how many live children the build root's table holds, and how many
/// every specialization's table holds. The root's table is shared by every top-level keyed
/// callee, so it is wider; the nested bound is a branching factor that nesting multiplies.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpecBounds {
    pub root: NonZeroUsize,
    pub nested: NonZeroUsize,
}

impl Default for SpecBounds {
    fn default() -> Self {
        SpecBounds {
            root: NonZeroUsize::new(16).expect("16 is not zero"),
            nested: NonZeroUsize::new(4).expect("4 is not zero"),
        }
    }
}

/// One entry of a [`SpecTable`]: empty while it names no handle, its key meaningless then.
pub struct SpecEntry {
    key: Cell<SpecKey>,
    handle: Cell<Option<TreeHandle>>,
    /// The parent's birth count at this specialization's last birth.
    born: Cell<u64>,
}

/// What a miss or a hit on a [`SpecTable`] comes to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    /// A live specialization holds the key; its entry is stamped.
    Hit(TreeHandle),
    /// The entry at this index is free for a new child: the key's own stale entry, an empty one,
    /// or another stale one.
    Fill(usize),
    /// Every entry is live: the entry at this index goes to the new child once the least recently
    /// born child it names is released.
    Evict(usize, TreeHandle),
}

/// A build cell's specializations: a fixed run of entries, as many as the cell's bound.
pub struct SpecTable<'cell> {
    entries: &'cell [SpecEntry],
    /// Every birth under this cell so far: misses, seek hits and hops.
    births: Cell<u64>,
    /// The length of a child's own table.
    nested: NonZeroUsize,
}

impl<'cell> SpecTable<'cell> {
    /// Lay a table of `length` empty entries through `lay`, whose children lay tables of `nested`.
    pub fn lay(
        lay: impl LaysRuns<'cell>,
        length: NonZeroUsize,
        nested: NonZeroUsize,
    ) -> &'cell SpecTable<'cell> {
        let entries = lay.lay(length.get(), |_| SpecEntry {
            key: Cell::new(SpecKey(ContentDigest::NONE)),
            handle: Cell::new(None),
            born: Cell::new(0),
        });
        let mut table = Some(SpecTable {
            entries,
            births: Cell::new(0),
            nested,
        });
        &lay.lay(1, |_| table.take().expect("a run of one fills once"))[0]
    }

    /// Count one birth under this cell, and return the new count.
    fn tick(&self) -> u64 {
        let births = self.births.get() + 1;
        self.births.set(births);
        births
    }

    /// What a seek of `key` comes to, a handle live where `is_live` says so: a live hit, stamped
    /// here; else the key's own stale entry, an empty entry, or another stale one, to fill; else
    /// the least recently born child, to evict.
    pub fn decide(&self, key: SpecKey, is_live: impl Fn(TreeHandle) -> bool) -> Decision {
        let held = |entry: &SpecEntry| entry.handle.get().filter(|_| entry.key.get() == key);
        if let Some((index, handle)) =
            (self.entries.iter().enumerate()).find_map(|(index, entry)| Some((index, held(entry)?)))
        {
            if is_live(handle) {
                self.entries[index].born.set(self.tick());
                return Decision::Hit(handle);
            }
            return Decision::Fill(index);
        }
        let empty = (self.entries.iter()).position(|entry| entry.handle.get().is_none());
        let stale = || {
            (self.entries.iter())
                .position(|entry| entry.handle.get().is_some_and(|handle| !is_live(handle)))
        };
        if let Some(index) = empty.or_else(stale) {
            return Decision::Fill(index);
        }
        let (index, entry) = (self.entries.iter().enumerate())
            .min_by_key(|(_, entry)| entry.born.get())
            .expect("a table has at least one entry");
        Decision::Evict(index, entry.handle.get().expect("every entry is occupied"))
    }

    /// Name `handle` under `key` at `index`, stamped with a new birth.
    pub fn fill(&self, index: usize, key: SpecKey, handle: TreeHandle) {
        let entry = &self.entries[index];
        entry.key.set(key);
        entry.handle.set(Some(handle));
        entry.born.set(self.tick());
    }

    /// Stamp the entry naming `handle` with a new birth, as a hop under it is one: `false` where
    /// no entry names it.
    pub fn stamp(&self, handle: TreeHandle) -> bool {
        match (self.entries.iter()).find(|entry| entry.handle.get() == Some(handle)) {
            Some(entry) => {
                entry.born.set(self.tick());
                true
            }
            None => false,
        }
    }

    /// Every child the table names that `is_live` says is live.
    pub fn live(&self, is_live: impl Fn(TreeHandle) -> bool) -> impl Iterator<Item = TreeHandle> {
        (self.entries.iter())
            .filter_map(|entry| entry.handle.get())
            .filter(move |handle| is_live(*handle))
    }

    /// How many entries name a child, live or stale.
    pub fn len(&self) -> usize {
        (self.entries.iter())
            .filter(|entry| entry.handle.get().is_some())
            .count()
    }

    /// Whether no entry names a child.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The length of a child's own table.
    pub fn nested(&self) -> NonZeroUsize {
        self.nested
    }
}

/// What a specialization's region header names: itself, and the build cell it is a child of, so a
/// hop under it can find its entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpecHome {
    pub me: TreeHandle,
    pub parent: CellHandle,
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
