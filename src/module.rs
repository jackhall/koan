//! Modules as values: the views `:|` and `:!` build, the coercion that births a view's members,
//! and the binding a `USING … SCOPE` block enters on.
//!
//! A module is born in [`function`](crate::function), as a knot node holding its self-signature
//! and its members. This layer is everything that reads one by name. Its spine is **layout order**
//! — value members sorted by name, then type members sorted by name — which a body shape's slots,
//! a signature's tables and a `USING` block's parameters all agree on, so a member is reached by
//! index and never by a search through the value it sits in. [`layout`] owns that rule; nothing
//! else here spells it.
//!
//! A **view** narrows: [`ascribe`] checks the source satisfies the signature, keeps only what the
//! signature names, and lays a module node of its own down through the same door a body-born
//! module goes through. Under `:!` the view's types are the source's, so every member is carried
//! verbatim. Under `:|` each abstract member is minted afresh per application, and every member
//! is born **coerced** to the mint: data is re-tagged through the admission barrier, containers
//! are rebuilt cell by cell, a nested module is re-viewed, and a function is wrapped in a barrier
//! node a call will later go through.
//!
//! What this layer does *not* do is evaluate anything: `m :| Sig`, `m.f`, a `USING` expression and
//! a call through a barrier are all [modules](../roadmap/rewrite/modules.md)' work. Here are the
//! doors those will drive.
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::elaborate`,
//! `crate::function`, `crate::memory`, `crate::parse`, `crate::scope`, `crate::type_lattice` and
//! `crate::values`, and nothing else in the crate; `tests::boundary` reads the source to hold it
//! there. Nothing below names this module.
//!
//! See [module/README.md](module/README.md).

pub mod coerce;
pub mod layout;
pub mod surface;
pub mod view;

#[cfg(test)]
mod tests;
