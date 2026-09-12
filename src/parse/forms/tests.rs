//! Form-table tests: the table's own shape, what a parsed statement caches off it, and the kind
//! derivation its lazy slots are pinned through. The live builtin registration set every
//! table⟺registration question reads is derived once, in [`registration`].

mod binder;
mod lazy;
#[cfg(feature = "pending_rewrite")]
mod registration;
mod table;
