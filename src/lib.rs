//! Library facade for the koan interpreter, exposing the module graph to integration
//! tests.
//!
//! The runtime is being rewritten from the ground up over the modules the rewrite keeps — every
//! module below not gated on `pending_rewrite`, and the embedded crates. Everything above them —
//! the machine, the builtins, the interpreter binary, the guard fixtures and the
//! integration tests — is the old runtime, compiled only under the `pending_rewrite` feature, so
//! the default build and test slate spend nothing on code slated for replacement. Under that
//! feature the canonical entry points are `machine::interpret` and `machine::interpret_with_writer`.

#[cfg(feature = "pending_rewrite")]
pub mod builtins;
/// Type expressions elaborated into lattice handles where they are read, and a callable's type
/// read off its signature where it is born.
pub mod elaborate;
/// Guard-fixture surface for the fold-provenance `compile_fail` tests, which compile as
/// external crates and so cannot name the `pub(crate)` fold machinery directly. Hidden from
/// docs; not part of koan's real API.
#[cfg(feature = "pending_rewrite")]
#[doc(hidden)]
pub mod fold_fixture;
/// Functions as values: the callable a value's parameter closes over, a knot node born by the tie
/// of its component and copied by re-tying its knot.
pub mod function;
#[cfg(feature = "pending_rewrite")]
pub mod machine;
/// The shape of things in storage: the cell tier over `cellgraph` — its names under Koan's
/// spelling and the slot array — and the bump tier outside the graph, where program storage lives.
pub mod memory;
pub mod module;
pub mod parse;
/// The deferred-work drain koan runs on: a unit of work is a `cellgraph` cell, and the module adds
/// the submission table, the work queue, the drain protocol and delivery over it.
pub mod scheduler;
/// Lexical environments over values and types: the shape a body resolves its names through, the
/// closure bindings a callable captures, and the activation a call reads and binds.
pub mod scope;
pub mod source;
/// Guard-fixture surface for the step-brand `compile_fail` tests, which compile as external
/// crates and so cannot name the `pub(crate)` `StepCarried` directly. Hidden from docs; not part
/// of koan's real API.
#[cfg(feature = "pending_rewrite")]
#[doc(hidden)]
pub mod step_fixture;
/// Crate-wide test scaffolding: installs the counting global allocator from
/// [`audit/counting_alloc.rs`](../audit/counting_alloc.rs) for the lib-test binary and exposes
/// the thread-local tally an allocation-count bracket reads — the type lattice's heap contract,
/// and the relocation path's fixed-cost measurements under `pending_rewrite`. A
/// `#[global_allocator]` is a crate-level declaration, so it lives at the crate root.
#[cfg(test)]
mod tests;
/// The type lattice: the node vocabulary, the interning registry, the identity recipe, the
/// relations between types and the unifier — a closed algebra over labels and `ScopeId`, with no
/// value, cell, AST or scope type reachable from it.
pub mod type_lattice;
/// Koan's data values and the per-dispatch expression form, laid down in a cell's region over
/// `memory`'s shapes, typed by memoized `type_lattice` handles.
pub mod values;
