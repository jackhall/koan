//! Library facade for the koan interpreter. The binary's `main.rs` previously declared the
//! whole module tree internally; this `lib.rs` re-exports the same modules so integration
//! tests under `tests/` can drive the interpreter without going through stdin/stdout. The
//! `main.rs` binary remains the authoritative entry point — `main.rs` re-imports through
//! this library, keeping the binary slim while letting `tests/` link against the same
//! module graph.
//!
//! Keep the surface minimal: `parse`, `dispatch`, `execute` are exposed as module groups
//! and the canonical entry points are `execute::interpret::interpret` and
//! `execute::interpret::interpret_with_writer`.

#![allow(dead_code)]

pub mod parse;
pub mod dispatch;
pub mod execute;
