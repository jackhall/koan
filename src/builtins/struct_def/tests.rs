//! Tests for the STRUCT builtin, split by surface:
//!
//! - [`basics`] — binder_name extraction, type registration, field ordering, schema errors.
//! - [`recursion`] — self-recursive and mutually-recursive struct elaboration.
//! - [`dispatch`] — per-declaration dispatch separation, wildcard slot admission,
//!   finalize idempotency, scope-id sharing.

mod basics;
mod dispatch;
mod recursion;
