//! The [`Reattachable`](crate::witnessed::Reattachable) family a region-stored `&Scope` carrier
//! erases through.
//!
//! A carrier holds a `&'a Scope<'a>` whose real lifetime the borrow checker can't track across the
//! region's `'static` storage. The reference is held outright as a thin pointer (layout-invariant in
//! `'a`) and re-anchored to the holder's `'a` as part of the holder's own substrate retype on read.
//!
//! See [memory-model.md § Region lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
//! for the soundness argument the carriers' pinning supplies.

use super::scope::Scope;
use crate::witnessed::reattachable;

/// `Reattachable` family for a **reference** to a [`Scope`] — `&'r Scope<'r>`. Layout-invariant:
/// `&'r Scope<'r>` is a thin pointer independent of `'r`, so a borrowed scope erases to `&'static`
/// through the safe [`erase_to_static`](crate::witnessed::erase_to_static) / [`SealedExtern::erase`]
/// with no `unsafe` cast. Recovery routes the rank-2 [`SealedExtern::open`], re-anchoring the erased
/// reference to a fresh existential `'b` the caller cannot leak.
pub struct ScopeRefFamily;

reattachable!(ScopeRefFamily => &'r Scope<'r>);
