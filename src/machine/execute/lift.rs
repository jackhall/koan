use crate::machine::core::{FrameStorage, KoanRegion};
use crate::machine::model::types::KType;
use crate::machine::model::Carried;
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

/// The defining frame a first-class `Module` still borrows into, recovered from its child scope's
/// `region_owner`: a module rides a *bare* `&` into a per-call region whose `Rc` lives only on the
/// producer's scheduler nodes, so when its identity is relocated into a consumer's region the consumer
/// must retain this frame for the borrow to outlive the producer's frame. The object channel
/// (`KFunction` / `KFuture` closures) instead carries its reach on the delivered carrier — folded at
/// the embedding site (let / attr / FROM / user-fn arg) and read off the carrier's witness set — so
/// only the not-yet-witnessed type-channel module remains on this reconstruction, which
/// [`alloc_ktype`](../../../roadmap/per-node-memory/alloc-ktype-witnessed.md) deletes when it
/// inverts the type family. `None` for every other value (no module borrow to keep alive).
pub(crate) fn reached_frame(value: Carried<'_>) -> Option<Rc<FrameStorage>> {
    let scope = match value {
        Carried::Type(KType::Module { module }) => module.child_scope(),
        _ => return None,
    };
    scope.region_owner().upgrade()
}

#[cfg(test)]
mod tests;
