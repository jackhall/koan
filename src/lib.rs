//! Library facade for the koan interpreter: the modules the binary and the tests reach the
//! interpreter through.

/// The language koan's programs run under: the builtin table, keyword selection over a use's
/// candidates, and the step every evaluation runs.
pub mod dispatch;
/// Type expressions elaborated into lattice handles where they are read, and a callable's type
/// read off its signature where it is born.
pub mod elaborate;
/// Functions, modules and circular data as values: the node that closes a value's knot-member
/// parameter, born by the tie of its component and copied by re-tying its knot.
pub mod knot;
/// The shape of things in storage: the cell tier over `cellgraph` — its names under Koan's
/// spelling and the slot array — and the bump tier outside the graph, where program storage lives.
pub mod memory;
pub mod parse;
/// A loaded program as one owning value: program storage, the symbol interner, the type registry
/// and the cell graph over them, reached through a closure per call.
pub mod program;
/// The deferred-work drain koan runs on: a unit of work is a `cellgraph` cell, and the module adds
/// the submission table, the work queue, the drain protocol and delivery over it.
pub mod scheduler;
/// Lexical environments over values and types: the shape a body resolves its names through, the
/// closure bindings a callable captures, and the activation a call reads and binds.
pub mod scope;
pub mod source;
/// Symbol identity: the classified newtypes every syntactic name travels as, the interner
/// that turns one back into text, and the `static_name!` declaration over them. A
/// leaf — it names nothing else in the crate — so [`parse`] and [`type_lattice`] both rest on it
/// without naming each other.
pub mod symbols;
/// Crate-wide test scaffolding: installs the counting global allocator from
/// [`audit/counting_alloc.rs`](../audit/counting_alloc.rs) for the lib-test binary and exposes
/// the thread-local tally an allocation-count bracket reads — the type lattice's heap contract. A
/// `#[global_allocator]` is a crate-level declaration, so it lives at the crate root.
#[cfg(test)]
mod tests;
/// The type lattice: the node vocabulary, the interning registry, the identity recipe, the
/// relations between types and the unifier — a closed algebra over [`symbols`] and `ScopeId`, with
/// no value, cell, AST or scope type reachable from it.
pub mod type_lattice;
/// Koan's data values and the per-dispatch expression form, laid down in a cell's region over
/// `memory`'s shapes, typed by memoized `type_lattice` handles.
pub mod values;
