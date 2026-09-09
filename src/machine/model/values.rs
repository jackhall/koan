//! Runtime values: the value-channel cells [`Carried`] / [`Held`] and the carrier states they
//! travel in, the region-resident [`ContainerSubstrate`] storage those cells sit in and its
//! [`Rehomed`] door, the universal [`KObject`] enum, dict-key wrapper [`KKey`],
//! the [`Module`] carrier, and the shared `<name>: <value>` parser [`NamedPairs`] used by
//! struct construction and first-class calls.
//!
//! Construction dispatch for `Struct` and `Wrapped` lives in
//! [`crate::machine::execute::decide::constructors`].

mod cell;
mod coerce;
mod container_substrate;
mod kkey;
mod kobject;
mod module;
mod named_pairs;
mod rehomed;

pub(crate) use cell::read_resting;
pub use cell::{Carried, CarriedFamily, DeliveredCarried, Held, SplicedCell};
pub use coerce::coerce_object_into;
pub(crate) use coerce::{DeclaredSlots, coerce_function_cell, declared_return};
pub use container_substrate::{ContainerSubstrate, PartedCell};
pub(crate) use container_substrate::{
    DictSubstrate, ListSubstrate, PayloadSubstrate, RecordSubstrate, object_copy_cost,
};
pub use kkey::KKey;
pub use kobject::{KObject, Scalar, ValueEqualityError};
pub(crate) use kobject::{
    RegionEscape, copy_or_pin, copy_or_pin_callable, product_reaches_region, relocate_object_into,
};
pub use module::{Module, ModuleDraft, ModuleRefFamily};
pub use named_pairs::NamedPairs;
pub(crate) use rehomed::Rehomed;
