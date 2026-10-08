//! The symbol vocabulary, the type lattice over it, and the bump tier both rest on.
//!
//! [`symbols`] is how every syntactic name travels; [`types`] is a closed algebra over interned
//! type nodes that names nothing but those symbols and the bump tier; [`bump`] is storage outside
//! any cell graph, which the lattice's registry and every scratch buffer are built over. The crate
//! depends on no koan code, so nothing koan's evaluator knows — a value, a cell, an AST node, a
//! scope — is nameable here.
//!
//! **Imports.** No koan file names `bumpalo`, `hashbrown`, `allocator_api2` or `blake3`: they are
//! this crate's dependencies, not koan's. Outside its test build the crate holds no `unsafe`.
//!
//! See [README.md](../README.md).

#![cfg_attr(not(test), forbid(unsafe_code))]

/// Storage outside the graph: the bump arena, the vector, table and set shapes built over it, and
/// the component walk staged in it.
pub mod bump;
/// Symbol identity: the classified newtypes every syntactic name travels as, the interner that
/// turns one back into text, and the `static_name!` declaration over them.
pub mod symbols;
/// Crate-wide test scaffolding: installs the counting global allocator from
/// [`audit/counting_alloc.rs`](../../audit/counting_alloc.rs) for this crate's test binary — the
/// lattice's heap contract — and the property-case share.
#[cfg(test)]
mod tests;
/// The type lattice: the node vocabulary, the interning registry, the identity recipe, the
/// relations between types and the unifier — a closed algebra over [`symbols`].
pub mod types;
