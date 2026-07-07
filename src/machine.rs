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

pub use core::kfunction::{Body, KFunction, NodeId};
pub use core::{
    run_root_storage, BindKind, BindingIndex, Bindings, CallFrame, CarrierWitness,
    DeliveredCarried, FrameSet, FrameStorage, FunctionLookup, KError, KErrorKind, KoanRegion,
    LexicalFrame, MemberResolution, NameLookup, RegionBrand, RegionTypeFamily, Scope, ScopeId,
    ScopeKind, TraceFrame, TypeHit, ValueHit,
};
pub use execute::{
    interpret, interpret_with_writer, interpret_with_writer_path, DispatchOutcome, KoanRuntime,
    NameOutcome, Resolved,
};
