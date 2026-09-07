//! The cellgraph measurement harness: run every benchmark shape and print one CSV row per verb.
//!
//! Built as a `[[bin]]` behind the `perf` feature rather than as a `benches/` target, for two
//! reasons. The reading that gates is the allocation count, which is deterministic and needs no
//! statistics; and every measurement in this repo is off a debug build, which `cargo bench` is
//! not. The harness itself adds no dependency to the crate.
//!
//! ```text
//! cargo run -p cellgraph --features perf --bin perf [name-filter]
//! ```
//!
//! Stdout is the CSV and nothing else, so [tools/cellgraph_perf.py](../../tools/cellgraph_perf.py)
//! can parse it directly, compare it against the recorded dataframe at
//! [observe/perf.csv](../observe/perf.csv), and append to it.
//!
//! A shape is run in blocks rather than once. A process's first pass over a shape reads its cache
//! and heap first-touch as the verb's cost, and a row of a few calls is timer granularity, so each
//! shape's block is grown by doubling until its smallest row's block total clears
//! [`BLOCK_FLOOR_NANOS`] — the undersized blocks are the warm-up — and then [`BLOCKS`] blocks are
//! run and each row keeps its fastest. `nanos` is that block divided by its `runs`, so the column
//! stays one shape run's exclusive cost, and `runs` rides along so the block a row was read from is
//! on the row.

// The delegating counter koan's own allocation audit uses. One wrapper, four targets: this is the
// fourth.
#[path = "../../audit/counting_alloc.rs"]
mod counting_alloc;

mod meter;
mod shapes;

#[global_allocator]
static ALLOCATOR: counting_alloc::Counting<std::alloc::System> =
    counting_alloc::Counting(std::alloc::System);

/// The block total every row of a shape must reach before the shape is read: at 20 µs a
/// percentage of the row is work rather than the timer's granularity or the meter's own ~100 ns
/// per call.
const BLOCK_FLOOR_NANOS: u64 = 20_000;

/// Blocks run per shape once the block is sized; the reading is each row's fastest.
const BLOCKS: usize = 5;

/// Runs per block never grow past this, so a shape whose smallest row is a single bare call still
/// terminates — at this size even that row is ~1 ms of block.
const MAX_RUNS: usize = 1 << 14;

type Tallies = [meter::Tally; meter::Verb::ALL.len()];

/// One block: `runs` runs of the shape with the tallies summed. The gating figures have to agree
/// run to run — a shape is deterministic — and a disagreement is a harness bug, not a reading.
fn block(shape: &shapes::Shape, n: u32, runs: usize) -> Tallies {
    let mut total: Option<Tallies> = None;
    for _ in 0..runs {
        meter::reset();
        (shape.run)(n);
        let got = meter::take();
        match &mut total {
            None => total = Some(got),
            Some(sum) => {
                for (sum, got) in sum.iter_mut().zip(got) {
                    assert_eq!(
                        (sum.calls, sum.allocations, sum.bytes),
                        (got.calls, got.allocations, got.bytes),
                        "{}/{n}: the gating figures moved between runs of one shape",
                        shape.name,
                    );
                    sum.nanos += got.nanos;
                }
            }
        }
    }
    total.expect("a block is at least one run")
}

/// The block total of the smallest row the shape has, which is the row the floor is about.
fn smallest_row(tallies: &Tallies) -> u64 {
    tallies
        .iter()
        .filter(|tally| tally.calls > 0)
        .map(|tally| tally.nanos)
        .min()
        .unwrap_or(u64::MAX)
}

fn main() {
    // A substring filter on the benchmark name, for running one shape while working on it. The
    // sweep tool passes none, so the recorded set is always the whole set.
    let filter = std::env::args().nth(1);

    println!("benchmark,n,cap,verb,calls,allocations,bytes,nanos,runs");
    for shape in &shapes::SHAPES {
        let name = shape.name;
        if filter
            .as_deref()
            .is_some_and(|filter| !name.contains(filter))
        {
            continue;
        }
        for n in [shape.small, shape.large] {
            let mut runs = 1;
            while smallest_row(&block(shape, n, runs)) < BLOCK_FLOOR_NANOS && runs < MAX_RUNS {
                runs *= 2;
            }
            let mut best = block(shape, n, runs);
            for _ in 1..BLOCKS {
                for (best, got) in best.iter_mut().zip(block(shape, n, runs)) {
                    best.nanos = best.nanos.min(got.nanos);
                }
            }
            for (verb, tally) in meter::Verb::ALL.iter().zip(best) {
                if tally.calls == 0 {
                    continue;
                }
                let runs = runs as u64;
                println!(
                    "{name},{n},{cap},{verb},{calls},{allocations},{bytes},{nanos},{runs}",
                    cap = shapes::CAP,
                    verb = verb.name(),
                    calls = tally.calls,
                    allocations = tally.allocations,
                    bytes = tally.bytes,
                    nanos = (tally.nanos + runs / 2) / runs,
                );
            }
        }
    }
}
