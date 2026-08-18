use std::io::Read;
use std::process::ExitCode;

use koan::machine::interpret_with_writer_path;

// Allocator selection, over two axes. Miri can't call mimalloc's FFI
// (`mi_malloc_aligned`), so the bin target falls back to the system allocator under miri
// to stay in the audit slate. `alloc-count` then *wraps* whichever of the two is in play
// in the delegating counter rather than replacing it, so the counted build and the
// shipped build allocate through the same allocator and a wall-clock reading off the
// counted one is still comparable.
#[cfg(feature = "alloc-count")]
#[path = "../audit/counting_alloc.rs"]
mod counting_alloc;

#[cfg(all(not(feature = "alloc-count"), not(miri)))]
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

/// CLI entry point: read source from a file (if a path is given as the first argument) or from
/// stdin, then parse, dispatch, and execute it via `interpret_with_writer_path` so error
/// frames can render real `path:line:col` locations.
fn main() -> ExitCode {
    let (source, path): (String, Option<String>) = match std::env::args().nth(1) {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(s) => (s, Some(path)),
            Err(e) => {
                eprintln!("could not read {}: {}", path, e);
                return ExitCode::FAILURE;
            }
        },
        None => {
            let mut buf = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
                eprintln!("could not read stdin: {}", e);
                return ExitCode::FAILURE;
            }
            (buf, None)
        }
    };

    let out: Box<dyn std::io::Write> = Box::new(std::io::stdout());
    let outcome = interpret_with_writer_path(&source, path.as_deref(), out);
    // Before the region audits: their pin-ring walk is debug scaffolding that allocates in
    // proportion to what the run pinned, so counting it would fold the reader's own cost
    // into the program's.
    report_allocations();
    report_region_audits();
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {}", e);
            ExitCode::FAILURE
        }
    }
}

/// Print the debug region audits' findings to stderr after the run — the reader half of the two
/// audits, without which turning them on surfaces nothing. Each arm compiles in with what it
/// reports: the pin-ring log is a debug-build surface, the tightness log the `region-audit`
/// feature's. The feature is named rather than `cfg(test)`-ored because this binary is its own
/// crate: the library it links against is built without `--test`, so only the feature turns the
/// tightness surface on here.
fn report_region_audits() {
    #[cfg(debug_assertions)]
    for ring in koan::witnessed::pin_cycle_reports() {
        eprintln!(
            "region audit: pin ring retained by {:#x} along {:x?}",
            ring.retainer, ring.path
        );
    }
    #[cfg(feature = "region-audit")]
    for flag in koan::machine::reach_audit::tightness_flags() {
        eprintln!(
            "region audit: over-fold at {} — member {:#x} unjustified; deps {:?} contributed nothing",
            flag.site, flag.member, flag.non_contributing
        );
    }
}

/// Print the run's allocation total to stderr — the reader half of the `alloc-count`
/// feature, without which wrapping the allocator surfaces nothing. The tally is read
/// before the print, since the print itself allocates.
#[cfg(feature = "alloc-count")]
fn report_allocations() {
    let total = counting_alloc::allocations();
    eprintln!("allocations: {total}");
}

/// No-op when the counter is not compiled in, so the call site in `main` needs no cfg.
#[cfg(not(feature = "alloc-count"))]
fn report_allocations() {}
