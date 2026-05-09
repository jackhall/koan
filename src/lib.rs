//! Library facade for the koan interpreter, exposing the module graph to integration
//! tests. Canonical entry points: [`execute::interpret::interpret`] and
//! [`execute::interpret::interpret_with_writer`].

#![allow(dead_code)]

pub mod parse;
pub mod dispatch;
pub mod execute;
