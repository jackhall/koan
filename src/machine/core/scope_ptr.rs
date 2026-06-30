//! The `Scope` lifetime-erasure plumbing every region-stored carrier shares — the family a scope
//! reference's substrate retype routes through.
//!
//! A carrier holds a scope reference whose real lifetime the borrow checker can't track across the
//! region's `'static` storage. The reference is held **outright** as a plain `&'a Scope<'a>` (a thin
//! pointer, layout-invariant in `'a`) and re-anchored to the holder's `'a` as part of the holder's own
//! substrate retype when the holder is read out of its region — so the read site is a bare field read,
//! not a scope-specialized handle.
//!
//! - [`ScopeRefFamily`] — the [`Reattachable`](crate::witnessed::Reattachable) family a `&Scope`
//!   erases through (the region store, the externally-witnessed carrier).
//!
//! The frame's per-call child scope rides a [`SealedExtern<ScopeRefFamily>`] carrier (a `&'static
//! Scope`), born through the externally-witnessed construction door
//! (`build_frame_child_witnessed`) and read through its rank-2 [`SealedExtern::open`] (the frame's
//! `with_scope`) — the single access verb. There is no scope-specialized re-anchor verb: construction
//! and reads alike route the substrate's brand.
//!
//! See [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
//! for the soundness argument the carriers' pinning supplies.

use super::scope::Scope;
use crate::witnessed::reattachable;

/// `Reattachable` family for a **reference** to a [`Scope`] — `&'r Scope<'r>`. It lets a borrowed
/// scope erase to a `&'static Scope` through the safe
/// [`erase_to_static`](crate::witnessed::erase_to_static) / [`SealedExtern::erase`] (the frame's
/// externally-witnessed scope carrier and the `YokedChild` node carrier), so the erasure carries no
/// `unsafe` cast. Layout-invariant: `&'r Scope<'r>` is a thin pointer independent of `'r`. Recovery
/// routes the rank-2 [`SealedExtern::open`] — the `&'static`-erased reference re-anchored to a fresh
/// existential `'b` the caller cannot leak — the same brand the run-loop step opens every scope at.
pub struct ScopeRefFamily;

reattachable!(ScopeRefFamily => &'r Scope<'r>);
