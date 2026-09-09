//! `examples` pins the rulings a reader consults to learn the layout rules; `properties` pins
//! the invariants every input must satisfy: layout never changes structure, every span slices
//! back to its own text, and an unbalanced edit is always rejected.

mod examples;
mod properties;

use crate::{Error, read, render};

/// Parse and render, or the error's message.
fn tree(source: &str) -> Result<String, String> {
    read(source)
        .map(|items| render(&items))
        .map_err(|e| e.to_string())
}

fn ok(source: &str) -> String {
    tree(source).unwrap_or_else(|e| panic!("{source:?} failed to read: {e}"))
}

fn err(source: &str) -> Error {
    match read(source) {
        Ok(items) => panic!("{source:?} read as {}", render(&items)),
        Err(e) => e,
    }
}
