//! The symbol vocabulary and the type lattice over it.
//!
//! [`symbols`] is how every syntactic name travels; [`types`] is a closed algebra over interned
//! type nodes that names nothing but those symbols and the bump tier. The lattice's root and every
//! scratch buffer rest on `bumpstorage`, the bump tier: storage outside any cell graph. The crate
//! depends on no koan code, so nothing koan's evaluator knows — a value, a cell, an AST node, a
//! scope — is nameable here.
//!
//! **Imports.** No koan file names `blake3`: it is this crate's dependency, not koan's. `bumpalo`,
//! `hashbrown` and `allocator_api2` are `bumpstorage`'s, and no file here names them either. Outside its test build the crate's one `unsafe` is the read behind a sealed component's member
//! table ([`types`]' `component` module), which ties the region's one cycle.
//!
//! See [README.md](../README.md).

#![cfg_attr(not(test), deny(unsafe_code))]

/// Symbol identity: the classified newtypes every syntactic name travels as, the interner that
/// turns one back into text, and the `static_name!` declaration over them.
pub mod symbols;
/// Crate-wide test scaffolding: installs the counting global allocator from
/// [`audit/counting_alloc.rs`](../../audit/counting_alloc.rs) for this crate's test binary — the
/// lattice's heap contract — and the property-case share.
#[cfg(test)]
mod tests;
/// The type lattice: the node vocabulary, the tiers types are interned in, the identity recipe, the
/// relations between types and the unifier — a closed algebra over [`symbols`].
pub mod types;
