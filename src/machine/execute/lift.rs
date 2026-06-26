use crate::machine::core::{FrameStorage, KoanRegion};
use crate::machine::model::types::KType;
use crate::machine::model::{Carried, KObject};
use std::rc::Rc;

/// The workload's value-relocation hook: structurally copy a [`Carried`] into `dest`'s region so the
/// copy outlives the producer's dying frame. Only the top-level node is re-allocated into `dest`; the
/// composite spine shares its `Rc` payloads ([`KObject::deep_clone`](crate::machine::model::KObject::deep_clone)),
/// and a `KFunction` / `KFuture` / first-class `Module` rides a *bare* borrow into its defining region —
/// preserved verbatim, never deep-copied (a closure may reference anything reachable from its captured
/// scope). Those surviving borrows are kept alive by the carrier's witness set
/// ([`FrameSet`](crate::machine::FrameSet)), which [`Sealed::transfer_into`](crate::witnessed::Sealed::transfer_into)
/// assembles as the set union of the producer's reached regions and `dest` — so this hook owns only
/// the copy, never a region anchor.
///
/// Runs at the destination brand `'b` that `transfer_into` opens, so the copy allocs into `dest`
/// natively: no fabricated lifetime, no caller `unsafe`. The peer Done-boundary hook is
/// [`finalize_terminal`](super::finalize::NodeFinalize::finalize_terminal) (the contract check); the
/// two stay separate.
pub(in crate::machine::execute) fn relocate_carried<'b>(
    value: Carried<'b>,
    dest: &'b KoanRegion,
) -> Carried<'b> {
    match value {
        Carried::Object(v) => Carried::Object(dest.alloc_object(v.deep_clone())),
        Carried::Type(t) => Carried::Type(dest.alloc_ktype(t.clone())),
    }
}

/// The defining frame a region-referencing value still borrows into, recovered from the value's
/// scope `region_owner`: a closure (`KFunction` / `KFuture`, via its captured scope) or a first-class
/// `Module` (via its child scope) rides a *bare* `&` into a per-call region whose `Rc` lives only on
/// the producer's scheduler nodes. When such a value is relocated into a consumer's region — spliced
/// into a working expr, bound into a scope, or forwarded — the consumer must retain this frame so the
/// borrow outlives the producer's frame; it is the witness member the per-value anchor used to carry
/// in-band. `None` for a region-pure value (no borrow to keep alive) or a frameless test scope.
pub(crate) fn reached_frame(value: Carried<'_>) -> Option<Rc<FrameStorage>> {
    let scope = match value {
        Carried::Object(KObject::KFunction(f)) => f.captured_scope(),
        Carried::Object(KObject::KFuture(fut)) => fut.function.captured_scope(),
        Carried::Type(KType::Module { module }) => module.child_scope(),
        _ => return None,
    };
    scope.region_owner().upgrade()
}

#[cfg(test)]
mod tests;
