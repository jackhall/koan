//! Runtime — everything that consumes a parsed `KExpression` to produce a value.
//! `model` is the value/type vocabulary; `machine` is the dispatcher, scheduler,
//! and executor; `builtins` is the K-language standard library implemented on top.

pub mod builtins;
pub mod model;
pub mod machine;
