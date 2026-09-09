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
/// Koan's instantiation of the region substrate, and every substrate name Koan spells — the
/// storage profile, the allocation brands, the per-call frame, program storage, the carrier
/// aliases, the value-channel cells and the container substrates.
pub mod memory;
pub mod parse;
pub mod source;
/// Guard-fixture surface for the step-brand `compile_fail` tests, which compile as external
/// crates and so cannot name the `pub(crate)` `StepCarried` directly. Hidden from docs; not part
/// of koan's real API.
#[doc(hidden)]
pub mod step_fixture;
/// The workload-generic DAG scheduler, re-exported from the `workgraph` crate so `machine` and
/// integration tests keep resolving `koan::scheduler::…` paths unchanged. The carrier substrate
/// beside it reaches Koan through [`memory`], never from here.
pub use workgraph::scheduler;

/// Crate-wide test scaffolding: installs the counting global allocator from
/// [`audit/counting_alloc.rs`](../audit/counting_alloc.rs) for the lib-test binary and exposes
/// the thread-local tally the relocation path's fixed-cost measurements read. Its only consumer
/// is `machine::execute::lift`'s aggregate suite, but a `#[global_allocator]` is a crate-level
/// declaration, so it lives at the crate root.
#[cfg(test)]
mod tests;
