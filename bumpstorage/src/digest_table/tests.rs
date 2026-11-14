use std::cell::{Cell, RefCell};

use super::{DigestTable, Keyed, LaysRuns};
use crate::{Bump, FrozenTable};

/// A test entry that counts its key reads.
struct Entry<'c> {
    key: u128,
    reads: &'c Cell<usize>,
}

impl Keyed for Entry<'_> {
    type Key = u128;

    fn key(&self) -> u128 {
        self.reads.set(self.reads.get() + 1);
        self.key
    }

    fn word(key: u128) -> u64 {
        key as u64
    }
}

struct SplitMix(u64);

impl SplitMix {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn keys(&mut self, n: usize) -> Vec<u128> {
        (0..n)
            .map(|_| (u128::from(self.next_u64()) << 64) | u128::from(self.next_u64()))
            .collect()
    }
}

/// A layer that records the length of every run it lays.
#[derive(Clone, Copy)]
struct Recording<'b> {
    bump: &'b Bump,
    runs: &'b RefCell<Vec<usize>>,
}

impl<'b> LaysRuns<'b> for Recording<'b> {
    fn lay<T>(self, len: usize, fill: impl FnMut(usize) -> T) -> &'b [T] {
        self.runs.borrow_mut().push(len);
        self.bump.lay(len, fill)
    }

    fn text(self, text: &str) -> &'b str {
        self.bump.text(text)
    }
}

/// Lay one entry per key in `bump`.
fn entries<'b>(bump: &'b Bump, keys: &[u128], reads: &'b Cell<usize>) -> Vec<&'b Entry<'b>> {
    keys.iter()
        .map(|&key| &*bump.alloc(Entry { key, reads }))
        .collect()
}

#[test]
fn an_empty_table_misses_everything() {
    let bump = Bump::new();
    let table = DigestTable::<Entry>::new(&bump);
    assert_eq!(table.len(), 0);
    assert!(table.is_empty());
    assert_eq!(table.slots(), 4);
    for key in SplitMix(1).keys(16) {
        assert!(table.get(key).is_none());
    }
}

#[test]
fn every_inserted_entry_is_found_and_absent_keys_miss() {
    let bump = Bump::new();
    let reads = Cell::new(0);
    let laid = entries(&bump, &SplitMix(2).keys(1000), &reads);
    let absent = SplitMix(3).keys(1000);
    for table in [
        DigestTable::new(&bump),
        DigestTable::with_capacity(&bump, 1000),
    ] {
        for &entry in &laid {
            table.insert_unique(&bump, entry);
        }
        assert_eq!(table.len(), 1000);
        for &entry in &laid {
            let found = table.get(entry.key).expect("an inserted key is found");
            assert!(std::ptr::eq(found, entry));
        }
        for key in &absent {
            assert!(table.get(*key).is_none());
        }
    }
}

#[test]
fn each_visits_every_entry_once() {
    let bump = Bump::new();
    let reads = Cell::new(0);
    let laid = entries(&bump, &SplitMix(4).keys(100), &reads);
    let table = DigestTable::new(&bump);
    for &entry in &laid {
        table.insert_unique(&bump, entry);
    }
    let mut seen = Vec::new();
    table.each(|entry| seen.push(entry.key));
    seen.sort_unstable();
    let mut keys: Vec<u128> = laid.iter().map(|entry| entry.key).collect();
    keys.sort_unstable();
    assert_eq!(seen, keys);
}

#[test]
fn a_shared_low_word_is_told_apart_by_the_whole_key() {
    let bump = Bump::new();
    let reads = Cell::new(0);
    // 15 has 7's home in both the 4-slot and the 8-slot run; the third insert grows the table,
    // and the probe from 7's home wraps past the run's end.
    let keys = [(1 << 64) | 7, (2 << 64) | 7, 15];
    let laid = entries(&bump, &keys, &reads);
    let table = DigestTable::new(&bump);
    for &entry in &laid {
        table.insert_unique(&bump, entry);
    }
    assert_eq!(table.slots(), 8);
    for &entry in &laid {
        assert!(std::ptr::eq(table.get(entry.key).expect("found"), entry));
    }
    assert!(table.get((3 << 64) | 7).is_none());
}

#[test]
fn growth_lays_doubled_runs_and_keeps_load_at_most_half() {
    let bump = Bump::new();
    let reads = Cell::new(0);
    let laid = entries(&bump, &SplitMix(4).keys(256), &reads);

    let runs = RefCell::new(Vec::new());
    let lay = Recording {
        bump: &bump,
        runs: &runs,
    };
    let table = DigestTable::new(lay);
    for &entry in &laid {
        table.insert_unique(lay, entry);
        assert!(table.len() * 2 <= table.slots());
    }
    assert_eq!(*runs.borrow(), [4, 8, 16, 32, 64, 128, 256, 512]);

    let runs = RefCell::new(Vec::new());
    let lay = Recording {
        bump: &bump,
        runs: &runs,
    };
    let table = DigestTable::with_capacity(lay, 256);
    for &entry in &laid {
        table.insert_unique(lay, entry);
    }
    assert_eq!(*runs.borrow(), [512]);
}

#[test]
fn growth_reads_no_entry() {
    let n = 1000;
    let keys = SplitMix(5).keys(n);
    for presized in [false, true] {
        let bump = Bump::new();
        let reads = Cell::new(0);
        let laid = entries(&bump, &keys, &reads);
        let table = match presized {
            true => DigestTable::with_capacity(&bump, n),
            false => DigestTable::new(&bump),
        };
        for &entry in &laid {
            table.insert_unique(&bump, entry);
        }
        // The debug absence check reads no entry either: random keys share no low word.
        assert_eq!(reads.get(), n, "presized: {presized}");
    }
}

#[test]
fn a_frozen_table_finds_every_entry_and_misses_the_rest() {
    let bump = Bump::new();
    let reads = Cell::new(0);
    let laid = entries(&bump, &SplitMix(6).keys(1000), &reads);
    let table = DigestTable::new(&bump);
    for &entry in &laid {
        table.insert_unique(&bump, entry);
    }
    let frozen = FrozenTable::placed(&bump, table.len(), table.entries());
    assert_eq!(frozen.len(), table.len());
    for &entry in &laid {
        assert!(std::ptr::eq(frozen.get(entry.key).expect("found"), entry));
    }
    for key in SplitMix(7).keys(1000) {
        assert!(frozen.get(key).is_none());
    }
    let mut seen = 0;
    frozen.each(|_| seen += 1);
    assert_eq!(seen, 1000);
}

/// An entry found by a word its caller computes and a test over a slice it holds, as a key built
/// for one lookup is: no `Keyed` impl.
struct Run<'b> {
    words: &'b [u64],
}

/// A word every `Run` of one length shares, so only the test tells two apart.
fn length_word(words: &[u64]) -> u64 {
    words.len() as u64
}

#[test]
fn find_tells_entries_sharing_a_word_apart_by_the_test() {
    let bump = Bump::new();
    let table = DigestTable::new(&bump);
    let a = &*bump.alloc(Run {
        words: bump.alloc_slice_copy(&[1, 2]),
    });
    let b = &*bump.alloc(Run {
        words: bump.alloc_slice_copy(&[3, 4]),
    });
    table.insert_at(&bump, length_word(a.words), a);
    table.insert_at(&bump, length_word(b.words), b);
    for (key, entry) in [([1, 2], a), ([3, 4], b)] {
        let found = table.find(length_word(&key), |run: &Run| run.words == key);
        assert!(std::ptr::eq(found.expect("found"), entry));
    }
    assert!(table.find(2, |run| run.words == [5, 6]).is_none());
}

#[test]
fn insert_at_grows_and_a_frozen_twin_finds_the_same() {
    let bump = Bump::new();
    let table = DigestTable::new(&bump);
    let mut random = SplitMix(8);
    let keys: Vec<[u64; 2]> = (0..300)
        .map(|_| [random.next_u64(), random.next_u64()])
        .collect();
    let word = |key: &[u64]| key[0] ^ key[1].rotate_left(17);
    for key in &keys {
        let entry = &*bump.alloc(Run {
            words: bump.alloc_slice_copy(key),
        });
        table.insert_at(&bump, word(key), entry);
    }
    assert_eq!(table.len(), 300);
    assert_eq!(table.slots(), 1024);
    let frozen = FrozenTable::placed_at(&bump, table.len(), table.words());
    for key in &keys {
        let growing = table.find(word(key), |run| run.words == key);
        let twin = frozen.find(word(key), |run| run.words == key);
        assert!(std::ptr::eq(growing.expect("found"), twin.expect("found")));
    }
    assert!(
        frozen
            .find(word(&[0, 0]), |run| run.words == [0, 0])
            .is_none()
    );
    assert_eq!(frozen.words().count(), 300);
}
