//! Runtime values: the universal [`KObject`] enum, dict-key wrapper [`KKey`],
//! the [`Module`] / [`Signature`] carriers, and the shared `<name>: <value>`
//! parser [`NamedPairs`] used by struct construction and first-class calls.
//!
//! Construction dispatch for `Struct` and `Tagged` lives in
//! [`crate::machine::execute::dispatch::constructors`].

mod carried;
mod kkey;
mod kobject;
mod module;
mod named_pairs;

pub use carried::Carried;
pub use kkey::KKey;
pub use kobject::{KObject, NonWrappedRef};
pub use module::{Module, Signature};
pub use named_pairs::NamedPairs;
