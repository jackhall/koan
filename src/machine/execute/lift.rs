use crate::machine::core::RegionBrand;
use crate::machine::model::Carried;

/// The workload's value-relocation hook: structurally copy a [`Carried`] into `dest`'s region so the
/// copy outlives the producer's dying frame. Only the top-level node is re-allocated into `dest`; the
/// composite spine shares its `Rc` payloads ([`KObject::deep_clone`](crate::machine::model::KObject::deep_clone)),
/// and a `KFunction` / first-class `Module` rides a *bare* borrow into its defining region —
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
    dest: RegionBrand<'b>,
) -> Carried<'b> {
    match value {
        Carried::Object(v) => Carried::Object(dest.alloc_object(v.deep_clone())),
        Carried::Type(t) => Carried::Type(dest.alloc_ktype(t.clone())),
    }
}

#[cfg(test)]
mod tests;
