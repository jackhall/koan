//! Library facade for the koan interpreter, exposing the module graph to integration
//! tests. Canonical entry points: [`machine::interpret`] and
//! [`machine::interpret_with_writer`].

#![allow(dead_code)]

pub mod builtins;
pub mod machine;
pub mod parse;
