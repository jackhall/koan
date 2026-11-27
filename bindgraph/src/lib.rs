//! A graph of bind sites over a description of a program's text, generic over a fact domain.
//!
//! Three layers, each over the last:
//!
//! 1. **Text** ([`Text`]): the embedder describes bodies and how they nest, the site names bound
//!    at each site and the answered names an answer types, the questions each site asks with the
//!    names each reads, the asks each answer may make, the arms each answer can take, the calls
//!    (`PassesName` edges on an arm or on a site), the `AnswersName` edges, and the `OpenCaller`
//!    and staged marks.
//! 2. **Graph** ([`Graph`]): laying the text computes each name's base, orders the sites, picks
//!    each question's and call's owner (no earlier than a staged body's entry around it), binds
//!    each answered name, fixes each node's handle width, checks the description, refuses a
//!    recursion that grows and finds where a knot can form. It reads no binding, so an owner is
//!    the same under every binding.
//! 3. **Bound** ([`Bound`]): bindings are read against a borrowed graph. An evaluation of one
//!    node under one view answers its questions through the embedder's [`Answers`], types its
//!    answered names and follows its calls, ending in a [`Handle`]: the instance's admission,
//!    which names no storage. Handles are filed by view. The answers behind them rest in bumps the
//!    embedder lends ([`Frozen`], [`Resting`]), and may be dropped and built again.
//!
//! The crate names nothing of its embedders; the dependency runs embedder → `bindgraph`, so a
//! path into koan does not compile:
//!
//! ```compile_fail
//! use koan as _;
//! ```
//!
//! The crate's design is [README.md](../README.md); koan's reading of it is
//! [the binding graph](../../design/binding-graph.md).

#![forbid(unsafe_code)]

/// Handles, evaluation, views filed by digest, and the answers resting in lent bumps.
mod bound;
/// The facts and the domain, the answer function, bindings and the doors an answer asks through.
mod domain;
/// The laid graph: bases, site order, ownership, edges, the refusal of a growing recursion, and
/// where a knot can form.
mod graph;
/// The id newtypes and [`Site`].
mod id;
/// The toy domain, generated texts, hand-written cases and the crate's laws.
#[cfg(test)]
mod tests;
/// The description of the text, and what makes one malformed.
mod text;

#[cfg(feature = "perf")]
pub use bound::Counts;
pub use bound::{Bound, Frozen, Handle, Resting};
pub use domain::{Answering, Answers, Binding, Domain, Doors, Facts, Kept, Nesting};
pub use graph::{
    AnswerBranch, AnswersName, Ask, BindingNode, BindsAfter, Edge, Graph, LayError, On, OpenCaller,
    PassesName, SiteQuestion, Source,
};
pub use id::{AnswersId, BodyId, ComponentId, EdgeId, NameId, NodeId, QuestionId, Site};
pub use text::{Malformed, Text};
