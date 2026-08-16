//! Runtime values: the universal [`KObject`] enum, dict-key wrapper [`KKey`],
//! the [`Module`] carrier, and the shared `<name>: <value>` parser [`NamedPairs`] used by
//! struct construction and first-class calls.
//!
//! Construction dispatch for `Struct` and `Tagged` lives in
//! [`crate::machine::execute::decide::constructors`].

mod carried;
mod container_substrate;
mod kkey;
mod kobject;
mod module;
mod named_pairs;
mod rehomed;

pub use carried::{Carried, CarriedFamily, Held};
pub use container_substrate::{ContainerSubstrate, PartedCell};
pub(crate) use container_substrate::{
    DictSubstrate, ListSubstrate, PayloadSubstrate, RecordSubstrate,
};
pub use kkey::KKey;
pub use kobject::{KObject, Scalar, ValueEqualityError};
pub(crate) use kobject::{RegionEscape, copy_or_pin, relocate_object_into, retains_home};
pub use module::{Module, ModuleDraft};
pub use named_pairs::NamedPairs;
