//! The interpreter: read a program from the file its first argument names, or from stdin, load it
//! under [`Koan`](koan::dispatch::Koan) and run it. `PRINT` writes to stdout; a refused load, or the
//! error value that ended the program, writes `error: <message>` to stderr and exits non-zero.
//! The load and the run happen on a thread of [`STACK_BYTES`], the stack the language sizes its
//! syntax depth limit against.

use std::io::{Read, Write};
use std::process::ExitCode;

use koan::dispatch::Koan;
use koan::program::{CellSubstrate, Outcome, Output, STACK_BYTES};

// The dhat attribution profiler owns the global allocator outright: it wraps the system
// allocator and records a backtrace per allocation. Otherwise the binary runs on the system
// allocator.
#[cfg(feature = "dhat")]
#[global_allocator]
static GLOBAL: dhat::Alloc = dhat::Alloc;

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
    let running = std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(move || load_and_run(&source, &path));
    match running.map(|thread| thread.join()) {
        Ok(Ok(code)) => code,
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(error) => {
            eprintln!("could not start the interpreter's thread: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Load `source` and run it, reporting how it ended.
fn load_and_run(source: &str, path: &str) -> ExitCode {
    let output = Output {
        print: |text| println!("{text}"),
        error: |text| {
            // Whatever was printed lands before the message that ended the program.
            let _ = std::io::stdout().flush();
            eprintln!("{text}");
        },
    };
    let mut substrate = match CellSubstrate::load::<Koan>(source, path, CELLS, output) {
        Ok(substrate) => substrate,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let outcome = substrate.with(|running| running.run());
    match outcome {
        Ok(Outcome::Completed) => ExitCode::SUCCESS,
        Ok(Outcome::Uncaught) => ExitCode::FAILURE,
        Err(stalled) => {
            eprintln!("error: the program stalled ({stalled:?})");
            ExitCode::FAILURE
        }
    }
}
