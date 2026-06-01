//! Tests for the `FN` builtin, split by theme:
//!
//! - [`basic`] — registration, dispatch routing, param binding, infix shapes.
//! - [`arena`] — run-root and scheduler-slot reclamation invariants.
//! - [`body_block`] — multi-statement body split, sibling visibility, TCO on last.
//! - [`body_routing`] — selection of the body to evaluate per call.
//! - [`return_type`] — parsing the `-> Type` slot and runtime return-type checks.
//! - [`param_type`] — typed-parameter dispatch, overload routing, shape errors.
//! - [`container_types`] — `List<T>`, `Dict<K,V>`, `Function<…>`, specificity.
//! - [`functor`] — FN as a functor over module-typed parameters.

mod arena;
mod basic;
mod body_block;
mod body_routing;
mod container_types;
mod functor;
mod param_type;
mod return_type;

use crate::builtins::test_support::{run, run_root_with_buf};
use crate::machine::RuntimeArena;

pub(super) fn capture_program_output(source: &str) -> Vec<u8> {
    let arena = RuntimeArena::new();
    let (scope, captured) = run_root_with_buf(&arena);
    run(scope, source);
    let bytes = captured.borrow().clone();
    bytes
}
