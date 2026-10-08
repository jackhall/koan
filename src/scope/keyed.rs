//! A **keyed table**: a run of entries sorted by the key each carries, and the one search over it.
//! A table sparse over its key — each one keyed by a part's [`Site`](super::Site) — is held as one
//! and read through [`Keyed::get`]; a table dense over its key is a slice indexed directly.

use std::fmt;
use std::ops::Deref;

use crate::memory::{Writer, collect};

/// An entry of a [`Keyed`] table: the key it is found by.
pub trait KeyedEntry {
    type Key: Ord + Copy;

    fn key(&self) -> Self::Key;
}

impl<K: Ord + Copy, V> KeyedEntry for (K, V) {
    type Key = K;

    fn key(&self) -> K {
        self.0
    }
}

/// Entries sorted by key. A key may repeat — a load pass may record one site twice — and then
/// [`get`](Self::get) answers with whichever entry the binary search lands on.
pub struct Keyed<'a, T>(&'a [T]);

impl<T> Clone for Keyed<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Keyed<'_, T> {}

impl<T: fmt::Debug> fmt::Debug for Keyed<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0).finish()
    }
}

impl<T> Deref for Keyed<'_, T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.0
    }
}

impl<'a, T: KeyedEntry> Keyed<'a, T> {
    /// `entries`, already sorted by key.
    pub(crate) fn from_sorted(entries: &'a [T]) -> Self {
        debug_assert!(entries.is_sorted_by_key(KeyedEntry::key));
        Keyed(entries)
    }

    /// `staged` sorted by key and laid down in `writer`'s region.
    pub(crate) fn sorted(writer: Writer<'a>, staged: &mut [T]) -> Self
    where
        T: Copy,
    {
        staged.sort_unstable_by_key(KeyedEntry::key);
        Keyed(collect(writer, staged.iter().copied()))
    }

    /// Every entry, sorted by key.
    pub fn entries(self) -> &'a [T] {
        self.0
    }

    /// Where the entry at `key` sits.
    pub fn position(self, key: T::Key) -> Option<usize> {
        self.0.binary_search_by_key(&key, KeyedEntry::key).ok()
    }

    /// The entry at `key`.
    pub fn get(self, key: T::Key) -> Option<&'a T> {
        self.position(key).map(|index| &self.0[index])
    }
}
