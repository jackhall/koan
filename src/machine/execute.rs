//! Execute — drives parsed `KExpression`s through a work-stealing scheduler to final
//! `KObject`s. Top-level expressions enter as `Dispatch` nodes against a run-root scope;
//! producer/consumer slots park on each other via `pending_deps` and wake on terminal
//! writes. Submodules:
//!
//! - `interpret` — `interpret` / `interpret_with_writer` entry points used by `main.rs`.
//! - `scheduler` — the `Scheduler` work-stealing dataflow engine.
//! - `lift`, `nodes` — internal scheduler machinery.
//!
//! The `pub use` block below is the entire public surface: the [`Scheduler`], the
//! [`interpret`]/[`interpret_with_writer`] entry points, and the test-only
//! `lift_kobject_for_test`. Submodules are private — all callers go through this surface.
//!
//! See [design/execution-model.md](../../design/execution-model.md) and
//! [design/memory-model.md](../../design/memory-model.md).

mod interpret;
mod lift;
mod nodes;
mod scheduler;

pub use interpret::{interpret, interpret_with_writer};
pub use scheduler::Scheduler;

#[cfg(test)]
pub use lift::lift_kobject_for_test;
