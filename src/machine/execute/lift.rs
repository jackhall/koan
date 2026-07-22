//! The witnessed-transfer copy hooks: the [`copy_carried`] relocate callback, the value-level
//! [`relocate_object_into`] / cell-level [`copy_held_from_carried`] copies, and the per-seam
//! [`copied_seam_mode`] selection. The total-rebuild verb itself
//! ([`copy_object_into`](crate::machine::model::copy_object_into)) and its host-release probe
//! ([`record_still_borrows_host`](crate::machine::model::record_still_borrows_host)) live in the
//! value model, shared with the core binding seams. See
//! [design/value-substrates.md § Escape](../../../design/value-substrates.md#escape-pin-by-default).

use crate::machine::core::FoldingBrand;
use crate::machine::model::{copy_object_into, record_still_borrows_host, Carried, Held, KObject};
use crate::machine::DeliveredCarried;
use crate::witnessed::Residence;

/// The structural-copy callback a witnessed transfer's fold runs
/// ([`Delivered::transfer_into`](crate::witnessed::Delivered)): copy a [`Carried`] into `dest`'s
/// region at the fold brand. A top-level `Record` is **totally rebuilt**
/// ([`copy_object_into`](crate::machine::model::copy_object_into)) so its region-resident substrate
/// lands at `dest`; every other value re-allocates only its top node while its still-`Rc` composite
/// spine shares its payloads ([`deep_clone`](crate::machine::model::KObject::deep_clone)), a
/// `KFunction` / first-class `Module` riding a bare borrow preserved verbatim — kept alive by the
/// reach set the transfer mints into the destination, so this hook owns only the copy, never a region
/// anchor. It is not a delivery channel: dep terminals cross to finishes as sealed carriers. `dest`
/// is a [`FoldingBrand`], not a plain brand: every caller is a `transfer_into` fold closure, whose
/// enclosing combinator has already minted the value's reach into `dest`'s arena, so a bare-borrow
/// payload like `KFunction` is covered by the fold rather than an address-only audit that can't see
/// it.
pub(in crate::machine::execute) fn copy_carried<'b>(
    value: Carried<'b>,
    dest: FoldingBrand<'b>,
) -> Carried<'b> {
    match value {
        Carried::Object(v) => {
            Carried::Object(dest.alloc_object_folded(relocate_object_into(v, dest)))
        }
        Carried::Type(t) => Carried::Type(t),
        Carried::UnresolvedType(ti) => {
            Carried::UnresolvedType(dest.alloc_type_identifier(ti.clone()))
        }
    }
}

/// Relocate one value into `dest` at a `Residence::Copied` / `Residence::Released` seam: a top-level
/// `Record` is totally rebuilt at the door
/// ([`copy_object_into`](crate::machine::model::copy_object_into)) so its substrate lands in `dest`;
/// every other value keeps the pointer-copy `deep_clone` (its still-`Rc` spine rides, and a record
/// nested under that spine stays conservatively pinned via the seal bit until its own container
/// converts). Shared by the seam hooks ([`copy_carried`], the return-contract relocation).
pub(in crate::machine::execute) fn relocate_object_into<'b>(
    value: &KObject<'b>,
    dest: FoldingBrand<'b>,
) -> KObject<'b> {
    match value {
        KObject::Record(..) => copy_object_into(value, dest),
        _ => value.deep_clone(),
    }
}

/// Own a transferred [`Carried`] into an aggregate cell at `dest`, relocating a top-level record
/// into `dest`'s region ([`relocate_object_into`]) so its substrate is container-resident — the
/// record-aware twin of [`Held::from_carried`], for the literal fold's per-cell seam.
pub(in crate::machine::execute) fn copy_held_from_carried<'b>(
    carried: Carried<'b>,
    dest: FoldingBrand<'b>,
) -> Held<'b> {
    match carried {
        Carried::Object(o) => Held::Object(relocate_object_into(o, dest)),
        Carried::Type(t) => Held::Type(t),
        Carried::UnresolvedType(ti) => Held::UnresolvedType(ti.clone()),
    }
}

/// The [`Residence`] mode for relocating `delivered` across a copy seam whose relocate hook is
/// [`copy_carried`]. A top-level record whose total copy no longer borrows its producer host is
/// [`Residence::Released`] — the retiring producer frees at retention discharge rather than riding
/// the destination's reach; a record that genuinely still borrows the host, or any non-record value,
/// keeps [`Residence::Copied`] (the seal bit's conservative pin then materializes the host). This is
/// the exact answer that reconciles with `force_record_borrows_host`'s conservative seal bit: at a
/// copy seam a still-borrowing record stays `Copied` + pinned, and a plain-data record is `Released`,
/// its bit overridden by the copy pass's exact release.
pub(in crate::machine::execute) fn copied_seam_mode(delivered: &DeliveredCarried) -> Residence {
    let host = delivered.host().region();
    delivered.open(|carried| match carried {
        Carried::Object(value) if matches!(value, KObject::Record(..)) => {
            if record_still_borrows_host(value, host) {
                Residence::Copied
            } else {
                Residence::Released
            }
        }
        _ => Residence::Copied,
    })
}

#[cfg(test)]
mod tests;
