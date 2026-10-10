//! The node-table benchmark: the lattice's [`DigestTable`], which a type branch lays in its
//! region, against the hashbrown table the registry's root keeps, over 48 B stand-in nodes that
//! carry their 128-bit digest and name a child by reference, laid in the same bump as the table.
//!
//! Sections, each a markdown table on stdout:
//!
//! - `tables`: per size, build, a dependent hit (latency: each key is read from the node the last
//!   lookup found), independent hits, misses and a 90/10 entry mix (throughput), live table bytes
//!   per entry and dead bytes, for each table grown from its smallest size and presized.
//! - `branch`: a small branch alone in its bump: table bytes laid and live, the dead share of
//!   table and node bytes, a dependent hit and a miss.
//! - `two-tier`: a root and a branch of 256 over one random stream of keys, walked dependently, in
//!   each probe order (branch first, root first, an empty branch skipped), for the shipped pairing
//!   (a hashbrown root, a digest-table branch) and each table in both tiers.
//! - `cold`: hits and misses at 60k after flushing the caches before each batch of lookups.
//!
//! Run `cargo bench -p lattice --bench node_table [section…]`; no section runs them all, and
//! `NODE_TABLE_RUNS` overrides how many timings each number is the best of. `cargo bench` passes
//! `--bench` to a `harness = false` target, so arguments starting with `-` are skipped.
//!
//! The results and each tier's table are in
//! [identity.md § Storage](../src/types/identity.md#storage-a-root-and-its-branches).

#![forbid(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::hint::black_box;
use std::time::{Duration, Instant};

use lattice::bump::{Bump, BumpHashTable, DigestTable, Keyed, LaysRuns, bump_hash_table};

/// Best of this many timings, unless `NODE_TABLE_RUNS` says otherwise.
const RUNS: usize = 5;
/// Best of this many for two tiers, whose rows differ by a nanosecond or two.
const TWO_TIER_RUNS: usize = 21;
const SIZES: [usize; 6] = [1_000, 12_000, 16_000, 24_000, 48_000, 60_000];
const BRANCH: usize = 256;
/// A table's header: a reference to its run (pointer and length) and a count.
const HEADER: usize = 24;
const NODE_BYTES: usize = 48;
/// Lookups per small-branch timing loop.
const SMALL_LOOKUPS: usize = 1 << 16;
/// Cold batch: lookups between cache flushes.
const COLD_BATCH: usize = 512;
/// Cache-flush buffer: twice the dev machine's 16 MiB L3.
const FLUSH_BYTES: usize = 32 << 20;

fn runs(default: usize) -> usize {
    std::env::var("NODE_TABLE_RUNS")
        .ok()
        .and_then(|runs| runs.parse().ok())
        .unwrap_or(default)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Digest(u128);

impl Digest {
    /// The word both tables hash by: the digest's low word.
    fn word(self) -> u64 {
        self.0 as u64
    }
}

/// A stored node's stand-in: 16 B digest and 32 B content. The content holds a child reference
/// (an earlier node, as a real forest's would), the key of the next node in the dependent chain,
/// and its own key's word again as `pad`, a dependency no digest compare lets the compiler fold.
#[repr(C)]
struct Node<'b> {
    digest: Digest,
    next: Digest,
    child: Option<&'b Node<'b>>,
    pad: u64,
}

const _: () = assert!(std::mem::size_of::<Node<'static>>() == NODE_BYTES);
const _: () = assert!(std::mem::align_of::<Node<'static>>() == 16);

impl Keyed for Node<'_> {
    fn key(&self) -> u128 {
        self.digest.0
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

    fn key(&mut self) -> Digest {
        Digest((u128::from(self.next_u64()) << 64) | u128::from(self.next_u64()))
    }

    fn keys(&mut self, n: usize) -> Vec<Digest> {
        (0..n).map(|_| self.key()).collect()
    }
}

fn shuffle<T>(items: &mut [T], stream: &mut SplitMix) {
    for i in (1..items.len()).rev() {
        let j = (stream.next_u64() as usize) % (i + 1);
        items.swap(i, j);
    }
}

/// A bump that tallies every run the digest table lays through it. Nodes are laid through it
/// untallied; hashbrown's buckets bypass it.
#[derive(Clone, Copy)]
struct Counting<'b> {
    bump: &'b Bump,
    table_bytes: &'b Cell<usize>,
}

impl<'b> LaysRuns<'b> for Counting<'b> {
    fn lay<T>(self, len: usize, fill: impl FnMut(usize) -> T) -> &'b [T] {
        self.table_bytes
            .set(self.table_bytes.get() + len * std::mem::size_of::<T>());
        self.bump.lay(len, fill)
    }
}

impl<'b> Counting<'b> {
    fn node(self, node: Node<'b>) -> &'b Node<'b> {
        self.bump.alloc(node)
    }
}

/// A table under test.
trait Table<'b> {
    fn get(&self, key: Digest) -> Option<&'b Node<'b>>;
    fn insert_absent(&self, storage: Counting<'b>, node: &'b Node<'b>);
    fn len(&self) -> usize;
    /// Bytes the table holds live, header included.
    fn live_bytes(&self) -> usize;
}

impl<'b> Table<'b> for DigestTable<'b, Node<'b>> {
    #[inline]
    fn get(&self, key: Digest) -> Option<&'b Node<'b>> {
        DigestTable::get(self, key.0)
    }

    fn insert_absent(&self, storage: Counting<'b>, node: &'b Node<'b>) {
        self.insert_unique(storage, node);
    }

    fn len(&self) -> usize {
        DigestTable::len(self)
    }

    fn live_bytes(&self) -> usize {
        self.slots() * 16 + HEADER
    }
}

/// Hashbrown behind the `RefCell` the registry keeps it in, hashed by the digest's low word.
impl<'b> Table<'b> for RefCell<BumpHashTable<'b, &'b Node<'b>>> {
    #[inline]
    fn get(&self, key: Digest) -> Option<&'b Node<'b>> {
        self.borrow()
            .find(key.word(), |node| node.digest == key)
            .copied()
    }

    fn insert_absent(&self, _: Counting<'b>, node: &'b Node<'b>) {
        self.borrow_mut()
            .insert_unique(node.digest.word(), node, |node| node.digest.word());
    }

    fn len(&self) -> usize {
        self.borrow().len()
    }

    /// The buckets as the bump lays them, padded to the nodes' 16 B alignment.
    fn live_bytes(&self) -> usize {
        self.borrow().allocation_size().next_multiple_of(16) + HEADER
    }
}

/// An arm: which table, and whether it opens presized.
trait Spec {
    type Table<'b>: Table<'b>;
    fn kind(&self) -> &'static str;
    fn presized(&self) -> bool;
    /// A table ready for `n` entries if presized, else the smallest.
    fn open<'b>(&self, storage: Counting<'b>, n: usize) -> Self::Table<'b>;

    fn name(&self) -> String {
        let how = if self.presized() { "presized" } else { "grown" };
        format!("{} {how}", self.kind())
    }
}

struct DigestSpec {
    presized: bool,
}

impl Spec for DigestSpec {
    type Table<'b> = DigestTable<'b, Node<'b>>;

    fn kind(&self) -> &'static str {
        "digest table"
    }

    fn presized(&self) -> bool {
        self.presized
    }

    fn open<'b>(&self, storage: Counting<'b>, n: usize) -> Self::Table<'b> {
        match self.presized {
            true => DigestTable::with_capacity(storage, n),
            false => DigestTable::new(storage),
        }
    }
}

struct BrownSpec {
    presized: bool,
}

impl Spec for BrownSpec {
    type Table<'b> = RefCell<BumpHashTable<'b, &'b Node<'b>>>;

    fn kind(&self) -> &'static str {
        "hashbrown"
    }

    fn presized(&self) -> bool {
        self.presized
    }

    fn open<'b>(&self, storage: Counting<'b>, n: usize) -> Self::Table<'b> {
        RefCell::new(match self.presized {
            true => BumpHashTable::with_capacity_in(n, storage.bump),
            false => bump_hash_table(storage.bump),
        })
    }
}

/// The keys and lookup sequences every arm is measured over.
struct Workload {
    keys: Vec<Digest>,
    /// Node `i`'s child: an earlier node's index (node 0 has none).
    children: Vec<usize>,
    /// Node `i`'s next key in the dependent chain: one random cycle through every key.
    next: Vec<Digest>,
    shuffled: Vec<Digest>,
    absent: Vec<Digest>,
    /// 90% present, 10% absent, random order.
    mix: Vec<Digest>,
}

impl Workload {
    fn new(n: usize) -> Workload {
        let mut stream = SplitMix(n as u64);
        let keys = stream.keys(n);
        let children = (0..n)
            .map(|i| {
                if i == 0 {
                    0
                } else {
                    stream.next_u64() as usize % i
                }
            })
            .collect();
        let mut order: Vec<usize> = (0..n).collect();
        shuffle(&mut order, &mut stream);
        let mut next = vec![Digest(0); n];
        for (k, &i) in order.iter().enumerate() {
            next[i] = keys[order[(k + 1) % n]];
        }
        let mut shuffled = keys.clone();
        shuffle(&mut shuffled, &mut stream);
        let absent = SplitMix(!(n as u64)).keys(n);
        let mix = (0..n)
            .map(|i| match stream.next_u64() % 10 {
                0 => absent[i],
                _ => shuffled[i],
            })
            .collect();
        Workload {
            keys,
            children,
            next,
            shuffled,
            absent,
            mix,
        }
    }
}

/// Lay the forest and enter each node: the build every arm pays.
fn fill<'b, S: Spec>(spec: &S, storage: Counting<'b>, work: &Workload) -> S::Table<'b> {
    let n = work.keys.len();
    let table = spec.open(storage, n);
    let mut laid: Vec<&'b Node<'b>> = Vec::with_capacity(n);
    for i in 0..n {
        let child = if i == 0 {
            None
        } else {
            Some(laid[work.children[i]])
        };
        let node = storage.node(Node {
            digest: work.keys[i],
            child,
            next: work.next[i],
            pad: work.keys[i].word(),
        });
        laid.push(node);
        table.insert_absent(storage, node);
    }
    table
}

/// A cycle of keys, each node naming the next, laid as a branch.
fn fill_branch<'b, S: Spec>(spec: &S, storage: Counting<'b>, keys: &[Digest]) -> S::Table<'b> {
    let table = spec.open(storage, keys.len());
    for (i, key) in keys.iter().enumerate() {
        let node = storage.node(Node {
            digest: *key,
            child: None,
            next: keys[(i + 1) % keys.len()],
            pad: key.word(),
        });
        table.insert_absent(storage, node);
    }
    table
}

/// Best of `runs` timings of `work`, in ns per operation over `ops` operations.
fn best_ns(runs: usize, ops: usize, mut work: impl FnMut()) -> f64 {
    (0..runs)
        .map(|_| {
            let start = Instant::now();
            work();
            start.elapsed().as_nanos() as f64 / ops as f64
        })
        .fold(f64::INFINITY, f64::min)
}

/// Independent lookups: throughput, since the CPU overlaps them.
fn lookups<'b, T: Table<'b>>(table: &T, keys: &[Digest]) -> f64 {
    best_ns(runs(RUNS), keys.len(), || {
        for key in keys {
            black_box(table.get(*key));
        }
    })
}

/// Dependent lookups through `find`, each key read from the node the last one found: latency.
fn chain_ns<'b>(find: impl Fn(Digest) -> Option<&'b Node<'b>>, start: Digest, steps: usize) -> f64 {
    best_ns(runs(RUNS), steps, || {
        let mut at = start;
        for _ in 0..steps {
            at = find(at).expect("the chain is present").next;
        }
        black_box(at);
    })
}

fn bump_used(bump: &mut Bump) -> usize {
    bump.iter_allocated_chunks().map(|chunk| chunk.len()).sum()
}

/// Table bytes laid over the table's life and those of them dead, headers excluded. The digest
/// table lays every run through the tally; hashbrown's buckets bypass it, so its laid bytes are
/// the bump's use less the nodes.
fn laid_and_dead(tallied: usize, used: usize, nodes: usize, live: usize) -> (usize, usize) {
    let laid = match tallied {
        0 => used - nodes * NODE_BYTES,
        laid => laid,
    };
    (laid, laid - (live - HEADER))
}

/// Every key found, every absent key missed, and the chain one cycle.
fn check<S: Spec>(spec: &S, work: &Workload) {
    let bump = Bump::new();
    let bytes = Cell::new(0);
    let storage = Counting {
        bump: &bump,
        table_bytes: &bytes,
    };
    let table = fill(spec, storage, work);
    assert_eq!(table.len(), work.keys.len(), "{}", spec.name());
    for key in &work.shuffled {
        assert_eq!(table.get(*key).map(|node| node.digest), Some(*key));
    }
    for key in &work.absent {
        assert!(table.get(*key).is_none(), "{}", spec.name());
    }
    let mut at = work.keys[0];
    for _ in 0..work.keys.len() {
        at = table.get(at).expect("chain").next;
    }
    assert_eq!(at, work.keys[0], "the chain is one cycle");
}

/// Build, lookups and bytes for one arm at one size.
fn measure<S: Spec>(spec: &S, work: &Workload) -> String {
    let n = work.keys.len();
    let build = best_ns(runs(RUNS), n, || {
        let bump = Bump::new();
        let bytes = Cell::new(0);
        let storage = Counting {
            bump: &bump,
            table_bytes: &bytes,
        };
        black_box(&fill(spec, storage, work));
    });
    let mut bump = Bump::new();
    let bytes = Cell::new(0);
    let (dependent, hit, miss, mix, live) = {
        let storage = Counting {
            bump: &bump,
            table_bytes: &bytes,
        };
        let table = fill(spec, storage, work);
        let dependent = chain_ns(|key| table.get(key), work.keys[0], n);
        let hit = lookups(&table, &work.shuffled);
        let miss = lookups(&table, &work.absent);
        let mix = lookups(&table, &work.mix);
        let live = table.live_bytes();
        // Hashbrown's drop would hand its buckets back to the bump before the bump is read.
        std::mem::forget(table);
        (dependent, hit, miss, mix, live)
    };
    let (_, dead) = laid_and_dead(bytes.get(), bump_used(&mut bump), n, live);
    format!(
        "| {} | {build:.1} | {dependent:.1} | {hit:.1} | {miss:.1} | {mix:.1} | {:.1} | {dead} |",
        spec.name(),
        live as f64 / n as f64,
    )
}

/// A branch of `m` entries alone in its own bump. Hits chain (latency); misses cannot, and are
/// throughput.
fn small_branch<S: Spec>(spec: &S, m: usize) -> String {
    let mut bump = Bump::new();
    let bytes = Cell::new(0);
    let mut stream = SplitMix(0x5a11 ^ m as u64);
    let keys = stream.keys(m);
    let absent = stream.keys(BRANCH);
    let (hit, miss, live) = {
        let storage = Counting {
            bump: &bump,
            table_bytes: &bytes,
        };
        let table = fill_branch(spec, storage, &keys);
        let hit = chain_ns(|key| table.get(key), keys[0], SMALL_LOOKUPS);
        let miss = best_ns(runs(RUNS), SMALL_LOOKUPS, || {
            for key in absent.iter().cycle().take(SMALL_LOOKUPS) {
                black_box(table.get(*key));
            }
        });
        let live = table.live_bytes();
        std::mem::forget(table);
        (hit, miss, live)
    };
    let (laid, dead) = laid_and_dead(bytes.get(), bump_used(&mut bump), m, live);
    let share = 100.0 * dead as f64 / (laid + m * NODE_BYTES) as f64;
    format!(
        "| {} | {m} | {} | {live} | {share:.0}% | {hit:.1} | {miss:.1} |",
        spec.name(),
        laid + HEADER,
    )
}

#[derive(Clone, Copy)]
enum Order {
    BranchFirst,
    RootFirst,
    EmptyBranchSkipped,
}

/// A root over `work` and a branch of [`BRANCH`] in one bump, over one random stream mixing root
/// keys with branch keys (`share` per cent branch), walked dependently: the next index waits on a
/// word of the node found, so nothing is periodic and the predictor learns no probe length. ns
/// per lookup.
fn two_tier_mixed<R: Spec, B: Spec>(
    root: &R,
    branch: &B,
    work: &Workload,
    order: Order,
    share: u64,
) -> f64 {
    let n = work.keys.len();
    let bump = Bump::new();
    let bytes = Cell::new(0);
    let storage = Counting {
        bump: &bump,
        table_bytes: &bytes,
    };
    let root = fill(root, storage, work);
    let branch_keys = SplitMix(0xb4a9c4).keys(BRANCH);
    let branch = match order {
        Order::EmptyBranchSkipped => branch.open(storage, 0),
        _ => fill_branch(branch, storage, &branch_keys),
    };
    let find = |key: Digest| match order {
        Order::BranchFirst => branch.get(key).or_else(|| root.get(key)),
        Order::RootFirst => root.get(key).or_else(|| branch.get(key)),
        Order::EmptyBranchSkipped => match branch.len() {
            0 => root.get(key),
            _ => branch.get(key).or_else(|| root.get(key)),
        },
    };
    let mut stream = SplitMix(0x7a1e ^ share);
    let keys: Vec<Digest> = (0..n)
        .map(|i| match stream.next_u64() % 100 < share {
            true => branch_keys[stream.next_u64() as usize % BRANCH],
            false => work.shuffled[i],
        })
        .collect();
    let words: Vec<u64> = keys.iter().map(|key| key.word()).collect();
    best_ns(runs(TWO_TIER_RUNS), n, || {
        let mut i = 0usize;
        while i < n {
            let node = find(keys[i]).expect("present");
            // Zero, but only the node's pad word says so (a compare on the digest would let the
            // compiler substitute the key): the next index waits on the node line.
            i += 1 + (node.pad ^ words[i]) as usize;
        }
        black_box(i);
    })
}

fn two_tier_row<R: Spec, B: Spec>(root: &R, branch: &B, work: &Workload, order: Order) -> String {
    let name = match order {
        Order::BranchFirst => "branch, then root",
        Order::RootFirst => "root, then branch",
        Order::EmptyBranchSkipped => "empty branch skipped",
    };
    let cells: Vec<String> = [0, 10, 50, 100]
        .into_iter()
        .map(|share| match (order, share) {
            (Order::EmptyBranchSkipped, 1..) => String::from("–"),
            _ => format!("{:.1}", two_tier_mixed(root, branch, work, order, share)),
        })
        .collect();
    format!(
        "| {} root, {} branch | {name} | {} |",
        root.kind(),
        branch.kind(),
        cells.join(" | ")
    )
}

fn flush(buffer: &mut [u8], salt: u8) {
    for i in (0..buffer.len()).step_by(64) {
        buffer[i] = buffer[i].wrapping_add(salt);
    }
    black_box(&buffer);
}

/// Independent lookups in batches of [`COLD_BATCH`] after a cache flush before each batch; only
/// the batches are timed.
fn cold_ns<'b, T: Table<'b>>(table: &T, keys: &[Digest], buffer: &mut [u8]) -> f64 {
    let mut best = f64::INFINITY;
    for run in 0..runs(RUNS) {
        let mut total = Duration::ZERO;
        for batch in keys.chunks(COLD_BATCH) {
            flush(buffer, run as u8 + 1);
            let start = Instant::now();
            for key in batch {
                black_box(table.get(*key));
            }
            total += start.elapsed();
        }
        best = best.min(total.as_nanos() as f64 / keys.len() as f64);
    }
    best
}

/// Dependent lookups in batches of [`COLD_BATCH`] after a cache flush before each batch.
fn cold_chain_ns<'b, T: Table<'b>>(
    table: &T,
    start: Digest,
    steps: usize,
    buffer: &mut [u8],
) -> f64 {
    let batches = steps / COLD_BATCH;
    let mut best = f64::INFINITY;
    for run in 0..runs(RUNS) {
        let mut total = Duration::ZERO;
        let mut at = start;
        for _ in 0..batches {
            flush(buffer, run as u8 + 1);
            let begin = Instant::now();
            for _ in 0..COLD_BATCH {
                at = table.get(at).expect("present").next;
            }
            total += begin.elapsed();
        }
        black_box(at);
        best = best.min(total.as_nanos() as f64 / (batches * COLD_BATCH) as f64);
    }
    best
}

fn cold<S: Spec>(spec: &S, work: &Workload, buffer: &mut [u8]) -> String {
    let n = work.keys.len();
    let bump = Bump::new();
    let bytes = Cell::new(0);
    let storage = Counting {
        bump: &bump,
        table_bytes: &bytes,
    };
    let table = fill(spec, storage, work);
    let warm_dependent = chain_ns(|key| table.get(key), work.keys[0], n);
    let cold_dependent = cold_chain_ns(&table, work.keys[0], n, buffer);
    let warm = lookups(&table, &work.shuffled);
    let hit = cold_ns(&table, &work.shuffled, buffer);
    let miss = cold_ns(&table, &work.absent, buffer);
    format!(
        "| {} | {warm_dependent:.1} | {cold_dependent:.1} | {warm:.1} | {hit:.1} | {miss:.1} |",
        spec.name()
    )
}

/// Run `$f` over every arm in table order, printing a row each.
macro_rules! each_arm {
    ($f:ident($($arg:expr),*)) => {
        for presized in [false, true] {
            println!("{}", $f(&DigestSpec { presized }, $($arg),*));
        }
        for presized in [false, true] {
            println!("{}", $f(&BrownSpec { presized }, $($arg),*));
        }
    };
}

fn tables() {
    for n in SIZES {
        let work = Workload::new(n);
        for presized in [false, true] {
            check(&DigestSpec { presized }, &work);
            check(&BrownSpec { presized }, &work);
        }
        println!("\n### n = {n}\n");
        println!(
            "| arm | build ns | dependent hit ns | hit ns | miss ns | mix 90/10 ns | live B/entry | dead B |"
        );
        println!("|---|---|---|---|---|---|---|---|");
        each_arm!(measure(&work));
    }
}

fn branch() {
    println!(
        "\n### Small branch (alone in its bump; bytes are the table's, header included; dead share of table and node bytes)\n"
    );
    println!("| arm | entries | bytes laid | live B | dead share | dependent hit ns | miss ns |");
    println!("|---|---|---|---|---|---|---|");
    for m in [1, 3, 8, 32, 256] {
        each_arm!(small_branch(m));
    }
}

fn two_tier() {
    let digest = DigestSpec { presized: false };
    let brown = BrownSpec { presized: false };
    for n in [16_000, 60_000] {
        let work = Workload::new(n);
        println!(
            "\n### Two tiers (root of {n}, branch of {BRANCH}, both grown, dependent; ns per lookup by branch share)\n"
        );
        println!("| tiers | order | 0% | 10% | 50% | 100% |");
        println!("|---|---|---|---|---|---|");
        for order in [
            Order::BranchFirst,
            Order::RootFirst,
            Order::EmptyBranchSkipped,
        ] {
            println!("{}", two_tier_row(&brown, &digest, &work, order));
            println!("{}", two_tier_row(&digest, &digest, &work, order));
            println!("{}", two_tier_row(&brown, &brown, &work, order));
        }
    }
}

fn cold_section() {
    let n = 60_000;
    let work = Workload::new(n);
    let mut buffer = vec![0u8; FLUSH_BYTES];
    println!(
        "\n### Cold (n = {n}, grown; flush of {} MiB before each batch of {COLD_BATCH})\n",
        FLUSH_BYTES >> 20
    );
    println!(
        "| arm | warm dependent hit ns | cold dependent hit ns | warm hit ns | cold hit ns | cold miss ns |"
    );
    println!("|---|---|---|---|---|---|");
    println!(
        "{}",
        cold(&DigestSpec { presized: false }, &work, &mut buffer)
    );
    println!(
        "{}",
        cold(&BrownSpec { presized: false }, &work, &mut buffer)
    );
}

fn main() {
    let sections: Vec<String> = std::env::args()
        .skip(1)
        .filter(|argument| !argument.starts_with('-'))
        .collect();
    let runs_section = |name: &str| sections.is_empty() || sections.iter().any(|s| s == name);
    // A warm-up pass, discarded.
    black_box(measure(
        &DigestSpec { presized: false },
        &Workload::new(16_000),
    ));
    if runs_section("tables") {
        tables();
    }
    if runs_section("branch") {
        branch();
    }
    if runs_section("two-tier") {
        two_tier();
    }
    if runs_section("cold") {
        cold_section();
    }
}
