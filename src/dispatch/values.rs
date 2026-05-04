//! Runtime values: the universal `KObject` enum, dict-key wrapper `KKey`, and the
//! construction primitives for product (`struct_value`) and sum (`tagged_union`) types.
//! `named_pairs` is the shared parser used by both construction primitives and first-class
//! function calls.
//!
//! Construction primitives stay at module scope (`pub mod struct_value` / `pub mod
//! tagged_union`) because callers reach for the module's `apply` / `register` functions
//! rather than for a single re-exported item.

mod kkey;
mod kobject;
mod named_pairs;
pub mod struct_value;
pub mod tagged_union;

pub use kkey::KKey;
pub use kobject::KObject;
pub use named_pairs::parse_named_value_pairs;
