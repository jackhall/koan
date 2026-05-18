//! Runtime values: the universal [`KObject`] enum, dict-key wrapper [`KKey`], and the
//! [`Module`] / [`Signature`] carriers. [`named_pairs`] is the shared parser used by
//! struct-construction and first-class function calls (both consume `<name>: <value>`
//! lists).
//!
//! Construction-primitive builtins for product (`Struct`) and sum (`Tagged`) types live
//! one layer up in [`crate::builtins::struct_value`] and
//! [`crate::builtins::tagged_union`]; the dispatch entry point that routes a
//! resolved verb-object to the right `apply` lives in [`crate::builtins`] too.

mod kkey;
mod kobject;
mod module;
mod named_pairs;

pub use kkey::KKey;
pub use kobject::{KObject, NonWrappedRef};
pub use module::{Module, Signature};
pub use named_pairs::NamedPairs;
