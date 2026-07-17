//! Runtime values: the universal [`KObject`] enum, dict-key wrapper [`KKey`],
//! the [`Module`] / [`ModuleSignature`] carriers, and the shared `<name>: <value>`
//! parser [`NamedPairs`] used by struct construction and first-class calls.
//!
//! Construction dispatch for `Struct` and `Tagged` lives in
//! [`crate::machine::execute::dispatch::constructors`].

mod carried;
mod kkey;
mod kobject;
mod module;
mod named_pairs;

pub use carried::{Carried, CarriedFamily, Held};
pub use kkey::KKey;
pub(crate) use kobject::expression_equal;
pub use kobject::{KObject, ValueEqualityError, WrappedPayload};
pub use module::{Module, ModuleSignature};
pub use named_pairs::NamedPairs;
