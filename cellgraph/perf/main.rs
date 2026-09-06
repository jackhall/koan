//! The cellgraph measurement harness: run every benchmark shape and print one CSV row per verb.
//!
//! Built as a `[[bin]]` behind the `perf` feature rather than as a `benches/` target, for two
//! reasons. The reading that gates is the allocation count, which is deterministic and needs no
//! statistics; and every measurement in this repo is off a debug build, which `cargo bench` is
//! not. The crate's dependency list stays at `bumpalo`.
//!
//! ```text
//! cargo run -p cellgraph --features perf --bin perf [name-filter]
//! ```
//!
//! Stdout is the CSV and nothing else, so [tools/cellgraph_perf.py](../../tools/cellgraph_perf.py)
//! can parse it directly, compare it against the recorded dataframe at
//! [observe/perf.csv](../observe/perf.csv), and append to it.

// The delegating counter koan's own allocation audit uses. One wrapper, four targets: this is the
// fourth.
#[path = "../../audit/counting_alloc.rs"]
mod counting_alloc;

mod meter;
mod shapes;

#[global_allocator]
static ALLOCATOR: counting_alloc::Counting<std::alloc::System> =
    counting_alloc::Counting(std::alloc::System);

fn main() {
    // A substring filter on the benchmark name, for running one shape while working on it. The
    // sweep tool passes none, so the recorded set is always the whole set.
    let filter = std::env::args().nth(1);

    println!("benchmark,n,cap,verb,calls,allocations,bytes,nanos");
    for shape in shapes::SHAPES {
        let name = shape.name;
        if filter
            .as_deref()
            .is_some_and(|filter| !name.contains(filter))
        {
            continue;
        }
        for n in [shape.small, shape.large] {
            meter::reset();
            (shape.run)(n);
            for (verb, tally) in meter::Verb::ALL.iter().zip(meter::take()) {
                if tally.calls == 0 {
                    continue;
                }
                println!(
                    "{name},{n},{cap},{verb},{calls},{allocations},{bytes},{nanos}",
                    cap = shapes::CAP,
                    verb = verb.name(),
                    calls = tally.calls,
                    allocations = tally.allocations,
                    bytes = tally.bytes,
                    nanos = tally.nanos,
                );
            }
        }
    }
}
