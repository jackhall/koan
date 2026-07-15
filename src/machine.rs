//! Machine — the runtime that maps a parsed `KExpression` to a value by selecting the
//! `KFunction` whose signature matches its parts and running its `Body`. Submodules:
//!
//! - `core` — `Scope`, `KoanRegion`, `KError`, scheduler glue, and the
//!   `kfunction` submodule (`KFunction`, `Body`).
//! - `model` — `KType`, `KObject`, `Module`, `ModuleSignature`, signature traits.
//! - `execute` — top-level interpret loop and scheduler driver.

pub(crate) mod core;
pub(crate) mod execute;
pub mod model;

pub use core::kfunction::{KFunction, NodeId};
pub(crate) use core::kfunction::Body;
pub use core::{
    run_root_storage, Bindings, DeliveredCarried, FrameStorage, KError, KErrorKind, Scope, ScopeId,
};
pub(crate) use core::{
    BindKind, BindingIndex, CallFrame, CarrierWitness, FrameSet, FunctionLookup, KoanRegion,
    LexicalFrame, MemberResolution, NameLookup, RegionBrand, RegionTypeFamily, TraceFrame,
};
pub use execute::{interpret, interpret_with_writer, interpret_with_writer_path, KoanRuntime};
pub(crate) use execute::{DispatchOutcome, NameOutcome};
