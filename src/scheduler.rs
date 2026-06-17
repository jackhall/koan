//! The workload-independent DAG scheduler — a dynamic graph of dependency-linked nodes
//! with per-node memory frames, parameterized over a [`Workload`] and naming no Koan value,
//! error, scope, memory, or AST type.
//!
//! The Koan interpreter ([`crate::machine`]) is the sole workload: it instantiates the scheduler
//! and drives it through the inherent-method contract. See design/execution-model.md.

mod node_id;

pub use node_id::NodeId;
