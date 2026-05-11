//! Tests for the `FN` builtin, split by theme:
//!
//! - [`basic`] — registration, dispatch routing, param substitution, infix shapes.
//! - [`arena`] — run-root and scheduler-slot reclamation invariants.
//! - [`return_type`] — parsing the `-> Type` slot and runtime return-type checks.
//! - [`param_type`] — typed-parameter dispatch, overload routing, shape errors.
//! - [`container_types`] — `List<T>`, `Dict<K,V>`, `Function<…>`, specificity.
//! - [`module_stage2`] — `ScopeResolver`, signature-bound params, functor lifting.

mod arena;
mod basic;
mod container_types;
mod module_stage2;
mod param_type;
mod return_type;

use crate::dispatch::builtins::test_support::{run, run_root_with_buf};
use crate::dispatch::runtime::RuntimeArena;

/// Run `source` in a fresh scope whose root output buffer is captured, and return the
/// captured bytes. Shared across the FN test clusters.
pub(super) fn capture_program_output(source: &str) -> Vec<u8> {
    let arena = RuntimeArena::new();
    let (scope, captured) = run_root_with_buf(&arena);
    run(scope, source);
    let bytes = captured.borrow().clone();
    bytes
}
