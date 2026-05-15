//! Runtime — everything that consumes a parsed `KExpression` to produce a value.
//! `machine` owns the dispatcher, scheduler, executor, and the value/type vocabulary
//! they operate on (`machine::model`); `builtins` is the Koan standard library
//! implemented on top.

pub mod builtins;
pub mod machine;
