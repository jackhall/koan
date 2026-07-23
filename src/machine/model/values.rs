//! Runtime values: the universal [`KObject`] enum, dict-key wrapper [`KKey`],
//! the [`Module`] carrier, and the shared `<name>: <value>` parser [`NamedPairs`] used by
//! struct construction and first-class calls.
//!
//! Construction dispatch for `Struct` and `Tagged` lives in
//! [`crate::machine::execute::dispatch::constructors`].

mod carried;
mod kkey;
mod kobject;
mod module;
mod container_substrate;
mod named_pairs;

pub use carried::{Carried, CarriedFamily, Held};
pub use kkey::KKey;
pub(crate) use kobject::{copy_object_into, record_seam_verb, record_still_borrows_host, SeamVerb};
pub use kobject::{KObject, ValueEqualityError, WrappedPayload};
pub use module::Module;
pub use container_substrate::{ContainerSubstrate, SubstrateMemos};
pub(crate) use container_substrate::RecordSubstrate;
pub use named_pairs::NamedPairs;
