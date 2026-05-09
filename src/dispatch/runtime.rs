//! Execution machinery: arenas that own per-run and per-call allocations, the `Scope` that
//! holds dispatch tables and resolves calls, and the structured `KError` that propagates
//! failures.

mod arena;
mod dispatcher;
mod kerror;
mod scope;

pub use arena::{CallArena, RuntimeArena};
pub use kerror::{Frame, KError, KErrorKind};
pub use scope::{KFuture, Resolution, Scope};
#[allow(unused_imports)]
pub use scope::ShapePick;
