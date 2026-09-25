//! BuiltinShape-table tests: the table's own shape, and what a parsed statement caches off it. The
//! live builtin registration set every table⟺registration question reads is derived once, in
//! [`registration`].

mod binder;
#[cfg(feature = "pending_rewrite")]
mod registration;
mod table;
