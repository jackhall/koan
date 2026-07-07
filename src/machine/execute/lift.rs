//! The witnessed-transfer copy hook.

use crate::machine::core::RegionBrand;
use crate::machine::model::values::Carried;

/// The structural-copy callback a witnessed transfer's fold runs
/// ([`Delivered::transfer_into`](crate::witnessed::Delivered)): copy a [`Carried`] into `dest`'s
/// region at the fold brand. Only the top-level node is re-allocated; the composite spine shares
/// its `Rc` payloads ([`deep_clone`](crate::machine::model::KObject::deep_clone)), and a
/// `KFunction` / first-class `Module` rides a bare borrow preserved verbatim — kept alive by the
/// reach set the transfer mints into the destination, so this hook owns only the copy, never a
/// region anchor. It is not a delivery channel: dep terminals cross to finishes as sealed
/// carriers.
pub(in crate::machine::execute) fn copy_carried<'b>(
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
