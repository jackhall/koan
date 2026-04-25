#![allow(dead_code)]

mod parse;
mod dispatch;

use std::io::Read;
use std::process::ExitCode;

use crate::parse::expression_tree::parse;

/// CLI entry point: read source from a file (if a path is given as the first argument) or from
/// stdin, parse it, and pretty-print the resulting `KExpression` tree.
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

    match parse(&source) {
        Ok(exprs) => {
            println!("{:#?}", exprs);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("parse error: {}", e);
            ExitCode::FAILURE
        }
    }
}
