//! What a habitat half's header keeps for the build tree: the table of a build cell's
//! settlements by key, and the opaque token naming the build cell a function was born under.
//!
//! A [`SpecBinding`] is the types a seek's keyed arguments carry, which the drain lays in a missed
//! settlement's branch before its step runs.
//!
//! A [`SpecTable`] rests in a build cell's region beside its type branch, in the region header
//! `scope` declares: a fixed run of entries laid with the cell and never grown, as long as the
//! cell's bound ([`SpecBounds`]). Each entry is a key, a bare handle and the
//! parent's birth count at the child's last birth. A bare handle holds nothing alive, so a stale
//! one is a miss the drain rebuilds in place. A miss with no empty or stale entry takes the entry
//! of the least recently born child, whose subtree the drain releases first. The table counts
//! every birth under its cell, so every stamp is distinct. A [`ArenaRef`] is how code above the
//! scheduler holds a build cell without naming a handle: only `values` and `scheduler` open one.
//! See [build/README.md](../build/README.md#references-between-the-trees).

use std::cell::Cell;
use std::num::NonZeroUsize;

use smallvec::SmallVec;

use crate::memory::{CellHandle, LaysRuns, TreeHandle};
use crate::type_lattice::{DeclaredType, KType, Parametric};

use super::{ContentDigest, Knotted, Value};

/// What a parent's table finds a settlement by: the callee's segment and the content digests
/// at the names its key lists, hashed into one digest.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SettlementKey(pub ContentDigest);

/// The types a settlement's binding carries, one entry per keyed name in the order its
/// key digests them: what the drain lays in a missed settlement's branch before its step
/// ([build/README.md](../build/README.md#references-between-the-trees)). A handle is a content
/// digest and holds nothing alive, so a binding outlives the values it was read from. One entry
/// sits inline, so a call keyed on one parameter allocates nothing, and a seek waiting on the
/// drain's ready stack keeps its entries small.
#[derive(Clone, Debug, Default)]
pub struct SpecBinding(SmallVec<[KeyedTypes; 1]>);

impl SpecBinding {
    pub fn push(&mut self, keyed: KeyedTypes) {
        self.0.push(keyed);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every handle the binding carries: each entry's own type, then the type it names, if any.
    pub fn handles(&self) -> impl Iterator<Item = DeclaredType<Parametric>> + '_ {
        (self.0.iter()).flat_map(|keyed| {
            std::iter::once(keyed.carried.into()).chain(keyed.named.map(Into::into))
        })
    }
}

/// What one keyed argument carries: its own type and, for a type value, the type it names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeyedTypes {
    pub carried: DeclaredType<KType>,
    pub named: Option<KType>,
}

impl KeyedTypes {
    pub fn of<X: Knotted>(argument: &Value<'_, X>) -> Self {
        KeyedTypes {
            carried: argument.ktype(),
            named: match argument {
                Value::Type(type_value) => Some(type_value.handle()),
                _ => None,
            },
        }
    }
}

/// The two bounds a load sets: how many live children the build root's table holds, and how many
/// every settlement's table holds. The root's table is shared by every top-level keyed
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
    key: Cell<SettlementKey>,
    handle: Cell<Option<TreeHandle>>,
    /// The parent's birth count at this settlement's last birth.
    born: Cell<u64>,
}

/// What a miss or a hit on a [`SpecTable`] comes to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    /// A live settlement holds the key; its entry is stamped.
    Hit(TreeHandle),
    /// The entry at this index is free for a new child: the key's own stale entry, an empty one,
    /// or another stale one.
    Fill(usize),
    /// Every entry is live: the entry at this index goes to the new child once the least recently
    /// born child it names is released.
    Evict(usize, TreeHandle),
}

/// A build cell's settlements: a fixed run of entries, as many as the cell's bound.
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
            key: Cell::new(SettlementKey(ContentDigest::NONE)),
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
    pub fn decide(&self, key: SettlementKey, is_live: impl Fn(TreeHandle) -> bool) -> Decision {
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
    pub fn fill(&self, index: usize, key: SettlementKey, handle: TreeHandle) {
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
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        (self.entries.iter())
            .filter(|entry| entry.handle.get().is_some())
            .count()
    }

    /// The length of a child's own table.
    pub fn nested(&self) -> NonZeroUsize {
        self.nested
    }
}

/// What a settlement's region header names: itself, and the build cell it is a child of, so a
/// hop under it can find its entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpecHome {
    pub me: TreeHandle,
    pub parent: CellHandle,
}

/// The build cell a function was born under: a settlement, or the build root. Opaque above
/// the scheduler, which alone turns one into a handle, so a step can hand the drain only a build
/// cell the scheduler named to it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ArenaRef(Option<TreeHandle>);

impl ArenaRef {
    /// The build root.
    pub const ROOT: ArenaRef = ArenaRef(None);

    /// The settlement `handle` names.
    pub(crate) fn of(handle: TreeHandle) -> Self {
        ArenaRef(Some(handle))
    }

    /// The settlement this names, or `None` for the build root.
    pub(crate) fn opened(self) -> Option<TreeHandle> {
        self.0
    }
}
