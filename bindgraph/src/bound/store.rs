//! Where a `Bound`'s answers and views rest: entries laid in bumps the embedder lends, in tables
//! found by a content word.
//!
//! An **answer entry** is an instance's answers under its node and handle words; a **view entry**
//! is a handle's words under its node and its view's digests. Each is laid with its slices in a
//! bump, and one representation serves the load, a run and the frozen part. A table is a
//! `bumpstorage` digest table or its frozen twin, homed by a key's content word ([`fold`]), which
//! is fixed and never seeded, so an image may write one.
//!
//! [`Resting`] holds a run's answers in one bump the embedder lends and resets whole. It lays
//! through [`Counted`], which adds each run's bytes plus its alignment's slack, so a bump made
//! with a capacity of the budget never grows a second chunk; past the budget it lays nothing more.
//! A [`ViewTable`] holds one node's views that do not last, a newer one overwriting an older at
//! its home slot, each slot holding its digests in full.
//!
//! Every type here is drop-free: the bumps run no destructor.
//!
//! See [the crate's README](../../README.md#instances).

use std::cell::Cell;
use std::mem::{align_of, size_of};

use bumpstorage::{Bump, BumpAllocator, DigestTable, LaysRuns, Slot};

use crate::domain::Kept;
use crate::id::NodeId;

/// A view's key: per name the node can read, its content's digest, `None` where the binding
/// leaves it open.
pub(super) type ViewKey = Vec<Option<u128>>;

/// splitmix64's finalizer: spreads a word so its low bits are uniform.
fn spread(mut z: u64) -> u64 {
    z ^= z >> 30;
    z = z.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z ^= z >> 27;
    z = z.wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A key's content word: its words folded in order. Fixed, never seeded, so an image may write it.
fn fold(words: impl IntoIterator<Item = u64>) -> u64 {
    let mut folded = 0x9e37_79b9_7f4a_7c15;
    for word in words {
        folded = spread(folded ^ word);
    }
    folded
}

/// The content word of `node`'s handle `words`.
pub(super) fn answer_word(node: NodeId, words: &[u64]) -> u64 {
    fold(std::iter::once(node.index() as u64).chain(words.iter().copied()))
}

/// The content word of `node`'s view `key`.
pub(super) fn view_word(node: NodeId, key: &[Option<u128>]) -> u64 {
    let digests = key.iter().flat_map(|digest| match *digest {
        None => [0, 0, 0],
        Some(digest) => [1, digest as u64, (digest >> 64) as u64],
    });
    fold(std::iter::once(node.index() as u64).chain(digests))
}

/// An instance's answers at rest, under its node and handle words.
pub(super) struct AnswerEntry<'r, K: Kept> {
    node: NodeId,
    words: &'r [u64],
    answers: &'r [K::SiteAnswer],
}

// Manual impls: `#[derive(Clone, Copy)]` would demand `F: Copy`.
impl<K: Kept> Clone for AnswerEntry<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: Kept> Copy for AnswerEntry<'_, K> {}

impl<'r, K: Kept> AnswerEntry<'r, K> {
    /// Whether this is the entry of `node`'s handle `words`.
    pub(super) fn is(&self, node: NodeId, words: &[u64]) -> bool {
        self.node == node && self.words == words
    }

    /// One answer per question the node owns.
    pub(super) fn answers(&self) -> &'r [K::SiteAnswer] {
        self.answers
    }

    #[cfg(test)]
    pub(super) fn node(&self) -> NodeId {
        self.node
    }
}

/// A filed view: a handle's words under its node and its view's digests.
#[derive(Copy, Clone)]
pub(super) struct ViewEntry<'r> {
    node: NodeId,
    digests: &'r [Option<u128>],
    words: &'r [u64],
}

impl<'r> ViewEntry<'r> {
    /// Whether this is the entry of `node`'s view `key`.
    pub(super) fn is(&self, node: NodeId, key: &[Option<u128>]) -> bool {
        self.node == node && self.digests == key
    }

    /// The handle's words.
    pub(super) fn words(&self) -> &'r [u64] {
        self.words
    }

    /// The entry of `node`'s view `key` and handle `words`, laid with its slices through `lay`.
    pub(super) fn laid(
        lay: impl LaysRuns<'r>,
        node: NodeId,
        key: &[Option<u128>],
        words: &[u64],
    ) -> &'r Self {
        let digests = lay.lay(key.len(), |index| key[index]);
        let words = lay.lay(words.len(), |index| words[index]);
        let entry = ViewEntry {
            node,
            digests,
            words,
        };
        &lay.lay(1, |_| entry)[0]
    }
}

/// The bytes a run of `len` values of `T` may take in a bump: their size, and the padding that
/// aligning the run may cost.
fn cost<T>(len: usize) -> usize {
    len * size_of::<T>() + align_of::<T>() - 1
}

/// A door over a bump that adds what each run may take to a tally.
#[derive(Copy, Clone)]
struct Counted<'b> {
    bump: BumpAllocator<'b>,
    laid: &'b Cell<usize>,
}

impl<'b> LaysRuns<'b> for Counted<'b> {
    fn lay<T>(self, len: usize, fill: impl FnMut(usize) -> T) -> &'b [T] {
        self.laid.set(self.laid.get() + cost::<T>(len));
        self.bump.lay(len, fill)
    }

    fn text(self, text: &str) -> &'b str {
        self.laid.set(self.laid.get() + cost::<u8>(text.len()));
        self.bump.text(text)
    }
}

/// The answers a run lays, in a bump the embedder lends and resets whole. Past its budget it lays
/// nothing more; a read through a handle whose answers are not here replays. Laid in its own bump
/// and written through shared borrows, so a `Bound` names none of it, and the embedder may drop it
/// and reset the bump between any two calls.
pub struct Resting<'b, K: Kept> {
    lay: Counted<'b>,
    table: DigestTable<'b, AnswerEntry<'b, K>>,
    budget: Option<usize>,
    passed: Cell<bool>,
}

impl<'b, K: Kept> Resting<'b, K> {
    /// A store over `bump`, laid there and counting itself against `budget` bytes; `None` sets no
    /// budget. A bump made with a capacity of the budget never grows a second chunk for it.
    pub fn new(bump: &'b Bump, budget: Option<usize>) -> &'b Self {
        let laid = &*bump.alloc(Cell::new(cost::<Cell<usize>>(1)));
        let lay = Counted { bump, laid };
        let table = DigestTable::new(lay);
        let mut resting = Some(Resting {
            lay,
            table,
            budget,
            passed: Cell::new(false),
        });
        let resting = &lay.lay(1, |_| resting.take().expect("one store"))[0];
        resting
            .passed
            .set(budget.is_some_and(|budget| laid.get() > budget));
        resting
    }

    /// Whether a lay was refused for the budget: the embedder may now drop the store, reset its
    /// bump and lay a new one.
    pub fn passed(&self) -> bool {
        self.passed.get()
    }

    /// The bytes laid, counting each run's alignment slack.
    pub fn laid(&self) -> usize {
        self.lay.laid.get()
    }

    /// How many instances' answers rest here.
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// Whether no instance's answers rest here.
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// The entry of `node`'s handle `words`, homed at `word`, if it rests here.
    pub(super) fn find(
        &self,
        word: u64,
        node: NodeId,
        words: &[u64],
    ) -> Option<&'b AnswerEntry<'b, K>> {
        self.table.find(word, |entry| entry.is(node, words))
    }

    /// Every entry beside its word.
    pub(super) fn entries(&self) -> impl Iterator<Item = (u64, &'b AnswerEntry<'b, K>)> {
        self.table.words()
    }

    /// Lays `node`'s answers under its handle `words`, homed at `word`, where they fit the budget;
    /// otherwise marks the store passed and lays nothing. The caller guarantees they are absent.
    pub(super) fn rest(&self, word: u64, node: NodeId, words: &[u64], answers: &[K::SiteAnswer]) {
        if self.passed.get() {
            return;
        }
        let mut needs = cost::<AnswerEntry<'b, K>>(1)
            + cost::<u64>(words.len())
            + cost::<K::SiteAnswer>(answers.len());
        if (self.table.len() + 1) * 2 > self.table.slots() {
            needs += cost::<Cell<Slot<'b, AnswerEntry<'b, K>>>>(self.table.slots() * 2);
        }
        if self
            .budget
            .is_some_and(|budget| self.laid() + needs > budget)
        {
            self.passed.set(true);
            return;
        }
        let words = self.lay.lay(words.len(), |index| words[index]);
        let answers = self.lay.lay(answers.len(), |index| answers[index]);
        let entry = &self.lay.lay(1, |_| AnswerEntry {
            node,
            words,
            answers,
        })[0];
        self.table.insert_at(self.lay, word, entry);
        debug_assert!(self.budget.is_none_or(|budget| self.laid() <= budget));
    }

    /// The node of each instance resting here.
    #[cfg(test)]
    pub(crate) fn nodes(&self) -> impl Iterator<Item = NodeId> {
        self.table.entries().map(|entry| entry.node())
    }
}

/// One node's views that do not last: `slots` slots, each holding a view's digests in full and its
/// handle's words, laid once. A newer view overwrites the one at its home slot.
#[derive(Copy, Clone)]
pub(super) struct ViewTable<'a> {
    relevant: usize,
    width: usize,
    digests: &'a [Cell<Option<u128>>],
    words: &'a [Cell<u64>],
    occupied: &'a [Cell<bool>],
}

impl<'a> ViewTable<'a> {
    /// An empty table of `slots` slots, for views of `relevant` digests and handles of `width`
    /// words, laid in `lay`.
    pub(super) fn laid(
        lay: BumpAllocator<'a>,
        slots: usize,
        relevant: usize,
        width: usize,
    ) -> &'a Self {
        let table = ViewTable {
            relevant,
            width,
            digests: lay.lay(slots * relevant, |_| Cell::new(None)),
            words: lay.lay(slots * width, |_| Cell::new(0)),
            occupied: lay.lay(slots, |_| Cell::new(false)),
        };
        &lay.lay(1, |_| table)[0]
    }

    fn home(&self, word: u64) -> usize {
        (word % self.occupied.len() as u64) as usize
    }

    /// Copies the words filed under view `key`, homed at `word`, into `into`; whether it is held.
    pub(super) fn get(&self, word: u64, key: &[Option<u128>], into: &mut [u64]) -> bool {
        let slot = self.home(word);
        let digests = &self.digests[slot * self.relevant..][..self.relevant];
        let held = self.occupied[slot].get()
            && (digests.iter().zip(key)).all(|(held, digest)| held.get() == *digest);
        if held {
            let words = &self.words[slot * self.width..][..self.width];
            for (into, word) in into.iter_mut().zip(words) {
                *into = word.get();
            }
        }
        held
    }

    /// Files `words` under view `key`, homed at `word`, over whatever its slot held.
    pub(super) fn put(&self, word: u64, key: &[Option<u128>], words: &[u64]) {
        let slot = self.home(word);
        let digests = &self.digests[slot * self.relevant..][..self.relevant];
        for (held, digest) in digests.iter().zip(key) {
            held.set(*digest);
        }
        let held = &self.words[slot * self.width..][..self.width];
        for (held, word) in held.iter().zip(words) {
            held.set(*word);
        }
        self.occupied[slot].set(true);
    }

    /// Each held view, laid as an entry through `lay`, beside its word.
    pub(super) fn entries(
        &self,
        node: NodeId,
        lay: BumpAllocator<'a>,
    ) -> impl Iterator<Item = (u64, &'a ViewEntry<'a>)> {
        (0..self.occupied.len())
            .filter(|&slot| self.occupied[slot].get())
            .map(move |slot| {
                let key: ViewKey = self.digests[slot * self.relevant..][..self.relevant]
                    .iter()
                    .map(Cell::get)
                    .collect();
                let words: Vec<u64> = self.words[slot * self.width..][..self.width]
                    .iter()
                    .map(Cell::get)
                    .collect();
                (
                    view_word(node, &key),
                    ViewEntry::laid(lay, node, &key, &words),
                )
            })
    }
}
