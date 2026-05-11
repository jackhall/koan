//! Dispatch — the runtime that maps a parsed `KExpression` to a value by selecting the
//! `KFunction` whose signature matches its parts and running its `Body`. Submodules:
//!
//! - `runtime` — `Scope`, `RuntimeArena`, `KError`, `KFuture`, scheduler glue.
//! - `types` — `KType` and signature shapes (`Argument`, `SignatureElement`, ...).
//! - `values` — `KObject` and the heap-side value representation.
//! - `kfunction` — `KFunction`, `Body`, `BodyResult`, `ArgumentBundle`, scheduler handle.
//!
//! The `pub use` block below is the curated public surface — the ~18 names that most
//! callers need. Reach into a submodule directly only for symbols not re-exported here.

pub(crate) mod kfunction;
pub(crate) mod runtime;
pub(crate) mod types;
pub(crate) mod values;

pub use kfunction::{
    ArgumentBundle, Body, BodyResult, CombineFinish, KFunction, NodeId, SchedulerHandle,
};
pub(crate) use kfunction::substitute_params;
pub use runtime::{CallArena, Frame, KError, KErrorKind, KFuture, Resolution, RuntimeArena, Scope};
// Resolution: 1 external import site
pub use types::{
    Argument, ExpressionSignature, KType, Parseable, Serializable, SignatureElement,
    UntypedElement, UntypedKey, is_keyword_token,
};
// Serializable: 3 external import sites
// UntypedElement: 1 external import site
// UntypedKey: 1 external import site
// is_keyword_token: 2 external import sites
pub use values::{KKey, KObject};
// KKey: 3 external import sites
