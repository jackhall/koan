//! Library facade for the koan interpreter, exposing the module graph to integration
//! tests. Canonical entry points: [`machine::interpret`] and
//! [`machine::interpret_with_writer`].

pub mod builtins;
pub mod machine;
pub mod parse;
pub mod source;
/// The lifetime-erasure carrier substrate (`Witnessed`, `Reattachable`, `Erased`) and the
/// workload-generic DAG scheduler, re-exported from the `workgraph` crate so `machine` and
/// integration tests keep resolving `koan::witnessed::…` / `koan::scheduler::…` paths unchanged.
pub use workgraph::{scheduler, witnessed};
