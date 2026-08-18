//! Library facade for the koan interpreter, exposing the module graph to integration
//! tests. Canonical entry points: [`machine::interpret`] and
//! [`machine::interpret_with_writer`].

pub mod builtins;
/// Guard-fixture surface for the fold-provenance `compile_fail` tests, which compile as
/// external crates and so cannot name the `pub(crate)` fold machinery directly. Hidden from
/// docs; not part of koan's real API.
#[doc(hidden)]
pub mod fold_fixture;
pub mod machine;
pub mod parse;
pub mod source;
/// Guard-fixture surface for the step-brand `compile_fail` tests, which compile as external
/// crates and so cannot name the `pub(crate)` `StepCarried` directly. Hidden from docs; not part
/// of koan's real API.
#[doc(hidden)]
pub mod step_fixture;
/// The lifetime-erasure carrier substrate (`Witnessed`, `Reattachable`, `Erased`) and the
/// workload-generic DAG scheduler, re-exported from the `workgraph` crate so `machine` and
/// integration tests keep resolving `koan::witnessed::…` / `koan::scheduler::…` paths unchanged.
pub use workgraph::{scheduler, witnessed};

/// Crate-wide test scaffolding: installs the counting global allocator from
/// [`audit/counting_alloc.rs`](../audit/counting_alloc.rs) for the lib-test binary and exposes
/// the thread-local tally the relocation path's fixed-cost measurements read. Its only consumer
/// is `machine::execute::lift`'s aggregate suite, but a `#[global_allocator]` is a crate-level
/// declaration, so it lives at the crate root.
#[cfg(test)]
mod tests;
