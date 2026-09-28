//! The interpreter: read a program from the file its first argument names, or from stdin, load it
//! under [`Koan`](koan::dispatch::Koan) and run it. `PRINT` writes to stdout; a refused load, or the
//! error value that ended the program, writes `error: <message>` to stderr and exits non-zero.

use std::io::{Read, Write};
use std::process::ExitCode;

use koan::dispatch::Koan;
use koan::program::{CellSubstrate, Outcome, Output};

// Allocator selection, over two axes. Miri can't call mimalloc's FFI (`mi_malloc_aligned`), so the
// binary falls back to the system allocator under Miri. `alloc-count` then *wraps* whichever of
// the two is in play in the delegating counter rather than replacing it, so the counted build and
// the shipped build allocate through the same allocator.
#[cfg(feature = "alloc-count")]
#[path = "../audit/counting_alloc.rs"]
mod counting_alloc;

// The dhat attribution profiler must own the global allocator outright (it wraps the system
// allocator and records a backtrace per allocation), so it cannot compose with the counter.
#[cfg(all(feature = "dhat", feature = "alloc-count"))]
compile_error!("`dhat` and `alloc-count` both claim the global allocator; enable exactly one");

#[cfg(feature = "dhat")]
#[global_allocator]
static GLOBAL: dhat::Alloc = dhat::Alloc;

#[cfg(all(not(feature = "alloc-count"), not(miri), not(feature = "dhat")))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(all(feature = "alloc-count", not(miri)))]
#[global_allocator]
static GLOBAL: counting_alloc::Counting<mimalloc::MiMalloc> =
    counting_alloc::Counting(mimalloc::MiMalloc);

#[cfg(all(feature = "alloc-count", miri))]
#[global_allocator]
static GLOBAL: counting_alloc::Counting<std::alloc::System> =
    counting_alloc::Counting(std::alloc::System);

/// The slab's cap: the whole width a graph's type fixes, so the cap is never what refuses a cell.
const CELLS: u32 = 64;

/// Read the source, load and run it, and exit on how it ended.
fn main() -> ExitCode {
    #[cfg(feature = "dhat")]
    let _profiler = dhat::Profiler::builder().trim_backtraces(None).build();
    let (source, path) = match std::env::args().nth(1) {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(source) => (source, path),
            Err(error) => {
                eprintln!("could not read {path}: {error}");
                return ExitCode::FAILURE;
            }
        },
        None => {
            let mut source = String::new();
            if let Err(error) = std::io::stdin().read_to_string(&mut source) {
                eprintln!("could not read stdin: {error}");
                return ExitCode::FAILURE;
            }
            (source, String::from("<input>"))
        }
    };
    let output = Output {
        print: |text| println!("{text}"),
        error: |text| {
            // Whatever was printed lands before the message that ended the program.
            let _ = std::io::stdout().flush();
            eprintln!("{text}");
        },
    };
    let mut substrate = match CellSubstrate::load::<Koan>(&source, &path, CELLS, output) {
        Ok(substrate) => substrate,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let outcome = substrate.with(|running| running.run());
    report_allocations();
    match outcome {
        Ok(Outcome::Completed) => ExitCode::SUCCESS,
        Ok(Outcome::Uncaught) => ExitCode::FAILURE,
        Err(stalled) => {
            eprintln!("error: the program stalled ({stalled:?})");
            ExitCode::FAILURE
        }
    }
}

/// Print the run's allocation and symbol-mint totals to stderr — the reader half of the
/// `alloc-count` feature. Both tallies are read before the first print, since the print allocates.
#[cfg(feature = "alloc-count")]
fn report_allocations() {
    let total = counting_alloc::allocations();
    let minted = koan::symbols::symbols_minted();
    eprintln!("allocations: {total}");
    eprintln!("symbols_minted: {minted}");
}

/// No-op when the counters are not compiled in, so the call site in `main` needs no cfg.
#[cfg(not(feature = "alloc-count"))]
fn report_allocations() {}
