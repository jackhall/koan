//! Runtime values: the universal [`KObject`] enum, dict-key wrapper [`KKey`],
//! the [`Module`] / [`Signature`] carriers, and the shared `<name>: <value>`
//! parser [`NamedPairs`] used by struct construction and first-class calls.
//!
//! Construction-primitive builtins for `Struct` and `Tagged` live one layer up
//! in [`crate::builtins::struct_value`] and [`crate::builtins::tagged_union`].

mod kkey;
mod kobject;
mod module;
mod named_pairs;

pub use kkey::KKey;
pub use kobject::{KObject, NonWrappedRef};
pub use module::{Module, Signature};
pub use named_pairs::NamedPairs;
