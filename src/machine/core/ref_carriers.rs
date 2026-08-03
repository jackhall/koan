//! The [`Reattachable`](crate::witnessed::Reattachable) families a region-stored **reference**
//! carrier erases through — a `&Scope` and a `&Module`.
//!
//! A carrier holds a `&'a T<'a>` whose real lifetime the borrow checker can't track across the
//! region's `'static` storage. The reference is held outright as a thin pointer (layout-invariant in
//! `'a`) and re-anchored to the holder's `'a` as part of the holder's own substrate retype on read.
//!
//! See [memory-model.md § Region lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
//! for the soundness argument the carriers' pinning supplies.

use super::bindings::Bindings;
use super::scope::Scope;
use crate::machine::model::Module;
use crate::witnessed::reattachable;

/// `Reattachable` family for a **reference** to a [`Scope`] — `&'r Scope<'r>`. Layout-invariant:
/// `&'r Scope<'r>` is a thin pointer independent of `'r`, so a borrowed scope erases to `&'static`
/// through the safe [`erase_to_static`](crate::witnessed::erase_to_static) / [`SealedExtern::erase`]
/// with no `unsafe` cast. Recovery routes the rank-2 [`SealedExtern::open`], re-anchoring the erased
/// reference to a fresh existential `'b` the caller cannot leak.
pub struct ScopeRefFamily;

/// `Reattachable` family for a **reference** to a [`Module`] — `&'r Module<'r>`. Layout-invariant by
/// the same argument as [`ScopeRefFamily`]: the reference is a thin pointer independent of `'r`.
/// It is the source-operand family of the module store fold
/// ([`Scope::store_module_object`](crate::machine::core::Scope)), which merges an already-resident
/// module reference into the storing scope's region so the composition — not a runtime walk — is
/// what covers the module's own home.
pub struct ModuleRefFamily;

/// `Reattachable` family for a **reference** to a [`Bindings`] table — `&'r Bindings`. The pointee is
/// lifetime-free, so `'r` names only the borrow; the family exists so a transparent `USING … SCOPE`
/// window can cross a construction brand alongside its parent scope
/// ([`Scope::alloc_child_transparent`]), which a bare `&'a Bindings` cannot — an ambient borrow has
/// no outlives relation to a `for<'b>` brand.
pub struct BindingsReferenceFamily;

reattachable!(
    ScopeRefFamily => &'r Scope<'r>,
    ModuleRefFamily => &'r Module<'r>,
    BindingsReferenceFamily => &'r Bindings,
);
