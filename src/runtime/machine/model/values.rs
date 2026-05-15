//! Runtime values: the universal [`KObject`] enum, dict-key wrapper [`KKey`], and the
//! [`Module`] / [`Signature`] carriers. [`named_pairs`] is the shared parser used by
//! struct-construction and first-class function calls (both consume `<name>: <value>`
//! lists).
//!
//! Construction-primitive builtins for product (`Struct`) and sum (`Tagged`) types live
//! one layer up in [`crate::runtime::builtins::struct_value`] and
//! [`crate::runtime::builtins::tagged_union`]; the dispatch entry point that routes a
//! resolved verb-object to the right `apply` lives in [`crate::runtime::builtins`] too.

mod kkey;
mod kobject;
mod module;
mod named_pairs;

pub use kkey::KKey;
pub use kobject::KObject;
pub use module::{Module, Signature};
pub(crate) use module::{resolve_module, resolve_signature};
pub use named_pairs::parse_named_value_pairs;
