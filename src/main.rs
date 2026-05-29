#![allow(dead_code)]

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
    match interpret_with_writer_path(&source, path.as_deref(), out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {}", e);
            ExitCode::FAILURE
        }
    }
}
