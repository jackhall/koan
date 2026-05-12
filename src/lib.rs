//! Library facade for the koan interpreter, exposing the module graph to integration
//! tests. Canonical entry points: [`runtime::machine::interpret`] and
//! [`runtime::machine::interpret_with_writer`].

#![allow(dead_code)]

pub mod ast;
pub mod parse;
pub mod runtime;
