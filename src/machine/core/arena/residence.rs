//! The three primitive reattach guards: the per-family [`AuditedStored`] impls whose audit is a
//! single `ptr::eq` against the region a value's own borrow names. A `KFunction` names its captured
//! scope, a `Scope` its own region, a `Module` its child scope — one pointer each, so residence is
//! answered by the value's own field rather than by a walk over its contents.
//!
//! Every other move-in is built at a fold brand ([`FoldingBrand`](super::FoldingBrand)), where the
//! rank-2 signature proves the value borrows nothing but the fold's declared operands, or reaches
//! its destination as a delivery envelope. See
//! [design/witness-hosting.md § Residence enforcement](../../../../design/witness-hosting.md#residence-enforcement).
//!
//! The region/brand substrate lives in the parent `arena` module.

use super::{KoanRegion, KoanStorageProfile};
use crate::machine::core::{KFunction, Scope};
use crate::machine::model::Module;
use crate::witnessed::AuditedStored;

// SAFETY: `audit` returns true only when `region` is the very region that owns the stored
// `KFunction`'s captured scope — the function borrows that scope, so a store elsewhere would
// lengthen the borrow's lifetime past its region.
unsafe impl AuditedStored<KoanStorageProfile> for KFunction<'static> {
    type AuditContext<'ctx> = ();
    fn audit(region: &KoanRegion, value: &KFunction<'_>, _context: ()) -> bool {
        std::ptr::eq(region, value.captured_scope().region())
    }
}

// SAFETY: `audit` returns true only when `region` is the region the stored `Scope` names as its
// own — every `Scope` borrows its parent, so a store into any other region would dangle.
unsafe impl AuditedStored<KoanStorageProfile> for Scope<'static> {
    type AuditContext<'ctx> = ();
    fn audit(region: &KoanRegion, value: &Scope<'_>, _context: ()) -> bool {
        std::ptr::eq(region, value.region())
    }
}

// SAFETY: `audit` returns true only when `region` is the region owning the stored `Module`'s child
// scope — the `Module` borrows that child scope, so a store into any other region would lengthen
// the borrow past its region. Exact: the child-scope reference is the `Module`'s only region borrow.
// The `type_members` / `slot_type_tags` maps and the `self_sig` cell need no walk — a `KType` owns
// its content and borrows no region data, so nothing installed through them can reach outside
// `region`. A `Module` re-tagging a *foreign* child scope has no route here: it is built at a fold
// brand instead ([`Scope::store_transparent_view`]).
unsafe impl AuditedStored<KoanStorageProfile> for Module<'static> {
    type AuditContext<'ctx> = ();
    fn audit(region: &KoanRegion, value: &Module<'_>, _context: ()) -> bool {
        std::ptr::eq(region, value.child_scope().region())
    }
}
