//! Library facade for the koan interpreter, exposing the module graph to integration
//! tests. Canonical entry points: [`machine::interpret`] and
//! [`machine::interpret_with_writer`].

pub mod builtins;
pub mod machine;
pub mod parse;
pub mod scheduler;
pub mod source;
/// The lifetime-erasure carrier substrate (`Witnessed`, `Reattachable`, `Erased`), re-exported
/// from the `workgraph` crate so `machine`, `scheduler`, and integration tests keep resolving
/// `koan::witnessed::…` paths unchanged.
pub use workgraph::witnessed;
