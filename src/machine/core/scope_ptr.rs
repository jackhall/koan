//! The `Scope` lifetime-erasure plumbing every region-stored carrier shares — the families a scope
//! reference's substrate retype routes through, plus the one re-anchor helper a holder calls when it
//! stores a captured / defining / parent scope.
//!
//! A carrier holds a scope reference whose real lifetime the borrow checker can't track across the
//! region's `'static` storage. The reference is held **outright** as a plain `&'a Scope<'a>` (a thin
//! pointer, layout-invariant in `'a`) and re-anchored to the holder's `'a` as part of the holder's own
//! substrate retype when the holder is read out of its region — so the read site is a bare field read,
//! not a scope-specialized handle. The two pieces here are:
//!
//! - [`ScopeFamily`] / [`ScopeRefFamily`] — the [`Reattachable`](crate::witnessed::Reattachable)
//!   families a `Scope` / `&Scope` retype routes through (the region store, the carrier re-anchor).
//! - [`recouple_scope`] — the construction-time re-anchor a holder calls to couple a (possibly
//!   short-borrowed, or longer-content) scope reference to its storage lifetime, witnessed by the
//!   scope's own region. Routes the safe-signature [`reattach_ref_with`](crate::witnessed), so the
//!   scope path carries **no `unsafe`** of its own beyond the substrate's single retype.
//!
//! The frame's per-call child scope additionally rides a [`SealedExtern<ScopeRefFamily>`] carrier (a
//! `&'static Scope`), read through its rank-2 [`SealedExtern::open`] (the frame's `with_scope`). The
//! borrow-bounded [`SealedExtern::attach`] is now **callerless** — kept only for the `single-open-verb`
//! follow-up to delete.
//!
//! See [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
//! for the soundness argument the carriers' pinning supplies.

use super::scope::Scope;
use crate::scheduler::reattach_ref_with;
use crate::witnessed::{reattachable, SealedExtern, Witness};

/// `Reattachable` family for [`Scope`] — the family every scope-pointer re-attach (and the region's
/// scope-erasure storage) routes through the single audited lifetime-retype. Layout-invariant: a
/// `Scope<'r>` is generic only in `'r`.
pub struct ScopeFamily;

reattachable!(ScopeFamily => Scope<'r>);

/// `Reattachable` family for a **reference** to a [`Scope`] — `&'r Scope<'r>`. It lets a borrowed
/// scope erase to a `&'static Scope` through the safe
/// [`erase_to_static`](crate::witnessed::erase_to_static) (the frame's externally-witnessed scope
/// carrier and the `YokedChild` node carrier), so the erasure carries no `unsafe` cast.
/// Layout-invariant: `&'r Scope<'r>` is a thin pointer independent of `'r`. Recovery routes
/// [`ScopeFamily`] via [`reattach_ref_with`] (a `&Scope<'static>` → `&'w Scope<'b>`), the two
/// families sharing one `'static`-erased representation.
pub struct ScopeRefFamily;

reattachable!(ScopeRefFamily => &'r Scope<'r>);

/// Re-anchor a region-resident scope reference to a lifetime `'a` its own region pins — the single
/// scope re-anchor every holder routes when it stores a captured / defining / parent scope as a plain
/// `&'a Scope<'a>`. The input reference's borrow may be **shorter** than `'a` (a holder built from the
/// interior-mutable [`BodyCtx::scope`](crate::machine::core::kfunction::action) — a short reader borrow
/// of a long-content scope) or its content may be **longer** (a per-call child's longer-lived lexical
/// parent), and `recouple_scope` reconciles both: the scope's own [`region`](Scope::region) field — a
/// `&KoanRegion` that proves the region, hence the scope, is live for all of `'a` — witnesses the
/// re-anchor, so the output borrow is capped at `'a` and cannot out-claim the pointee.
///
/// Routes the substrate's audited retype through the safe-signature
/// [`reattach_ref_with`](crate::witnessed), so it carries **no `unsafe`** of its own: the scope is
/// held outright as a `&'a Scope<'a>` and re-coupled here, with no scope-specialized handle.
pub(crate) fn recouple_scope<'s, 'a>(scope: &Scope<'s>) -> &'a Scope<'a>
where
    's: 'a,
{
    reattach_ref_with::<ScopeFamily, _>(scope, scope.region)
}

/// The frame's per-call child scope rides the substrate's externally-witnessed
/// [`SealedExtern`] over [`ScopeRefFamily`] (a `&'static Scope`, erased once through the safe
/// `erase_to_static`), read through its rank-2 [`SealedExtern::open`] (the frame's `with_scope`).
/// [`Self::attach`] — the borrow-bounded re-anchor, the scope-pointer twin of
/// [`reattach_with`](crate::witnessed) — is now **callerless**: the frame-side reads and the
/// seed-side `it` / param binds all fold onto `open`. It hands back a free content `'b` the
/// `for<'b>`-branded `open` cannot, but no caller needs that now, so it survives only for the
/// `single-open-verb` follow-up to delete.
impl SealedExtern<ScopeRefFamily> {
    /// Re-attach the frame's child scope bounded by a held [`Witness`] borrow: the borrow `'w` is
    /// capped at the witness `&'w W` while the scope content `'b` is left free (`'b: 'w`), so the
    /// returned reference **cannot outlive `'w`** and a value alloc'd into its region rides the cart
    /// the witness pins. Routes the witness-bounded [`reattach_ref_with`] over the carrier's stored
    /// `&'static Scope`, so it carries **no `unsafe`**. **Callerless** — the frame's reads fold onto
    /// the rank-2 `open`; kept (and pinned by its own Miri test) for the `single-open-verb` follow-up
    /// to delete.
    pub fn attach<'w, 'b: 'w, W: Witness>(&'w self, witness: &'w W) -> &'w Scope<'b> {
        reattach_ref_with::<ScopeFamily, W>(self.static_carrier(), witness)
    }
}
