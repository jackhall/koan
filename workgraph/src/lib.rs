//! The witnessed carrier substrate and the workload-generic DAG scheduler.
//!
//! Koan-agnostic by construction: this crate depends on nothing above it,
//! so it cannot name an embedder type.

pub mod witnessed;
