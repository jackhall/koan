//! Library facade for the koan interpreter, exposing the module graph to integration
//! tests. Canonical entry points: [`machine::interpret`] and
//! [`machine::interpret_with_writer`].

pub mod builtins;
pub mod machine;
pub mod parse;
pub mod scheduler;
pub mod source;
/// The lifetime-erasure carrier substrate (`Witnessed`, `Reattachable`, `Erased`) shared by
/// `machine` and `scheduler`; sits below both and names no workload type. `pub` only so the
/// rank-2 brand's `compile_fail` doctests can reach the carrier as an external crate. See the
/// module docs.
pub mod witnessed;
