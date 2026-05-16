#![allow(dead_code)]

use std::io::Read;
use std::process::ExitCode;

use koan::machine::interpret;

/// CLI entry point: read source from a file (if a path is given as the first argument) or from
/// stdin, then parse, dispatch, and execute it via `interpret`.
fn main() -> ExitCode {
    let source = match std::env::args().nth(1) {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(s) => s,
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
            buf
        }
    };

    match interpret(&source) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // KError's Display formats `kind` followed by frames in `(expression) (function)`
            // form, one per line. The "error:" prefix matches the previous CLI shape.
            eprintln!("error: {}", e);
            ExitCode::FAILURE
        }
    }
}
