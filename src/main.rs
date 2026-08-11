use std::io::Read;
use std::process::ExitCode;

use koan::machine::interpret_with_writer_path;

// Miri can't call mimalloc's FFI (`mi_malloc_aligned`); fall back to the
// system allocator under miri so the bin target stays in the audit slate.
#[cfg(not(miri))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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
