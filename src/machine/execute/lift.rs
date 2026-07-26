//! The witnessed-transfer copy hooks: the [`copy_carried`] relocate callback, the value-level
//! [`relocate_object_into`] / cell-level [`copy_held_from_carried`] copies, the value-level escape
//! seam's [`seam_verb`] chooser, and the retention predicate [`seam_still_borrows`] the chosen verb
//! implies. The
//! cost decision itself ([`copy_or_pin`](crate::machine::model::copy_or_pin)), the
//! total-rebuild verb ([`copy_object_into`](crate::machine::model::copy_object_into)), and the
//! host-release probe ([`still_borrows_host`](crate::machine::model::still_borrows_host))
//! live in the value model, shared with the core binding seams. See
//! [design/value-substrates.md § Escape](../../../design/value-substrates.md#escape-pin-by-default).

use crate::machine::core::FoldingBrand;
use crate::machine::core::{product_still_borrows, KoanRegion, KoanStorageProfile};
use crate::machine::model::{copy_object_into, copy_or_pin, Carried, Held, KObject, RegionEscape};
use crate::machine::DeliveredCarried;
use crate::witnessed::RegionHandle;

/// The structural-copy callback a witnessed transfer's fold runs
/// ([`Delivered::transfer_into`](crate::witnessed::Delivered)): copy a [`Carried`] into `dest`'s
/// region at the fold brand. A top-level substrate carrier (`Record` / `List` / `Dict` / `Tagged` / `Wrapped`) is **totally rebuilt**
/// ([`copy_object_into`](crate::machine::model::copy_object_into)) so its region-resident substrate
/// lands at `dest`; every other value re-allocates only its top node
/// ([`deep_clone`](crate::machine::model::KObject::deep_clone)) — a scalar rebuilt owned, a
/// `KFunction` / first-class `Module` riding a bare borrow preserved verbatim — kept alive by the
/// reach set the transfer mints into the destination, so this hook owns only the copy, never a region
/// anchor. It is not a delivery channel: dep terminals cross to finishes as sealed carriers. `dest`
/// is a [`FoldingBrand`], not a plain brand: every caller is a `transfer_into` fold closure, whose
/// enclosing combinator has already minted the value's reach into `dest`'s arena, so a bare-borrow
/// payload like `KFunction` is covered by the fold rather than an address-only audit that can't see
/// it.
pub(in crate::machine::execute) fn copy_carried<'b>(
    value: Carried<'b>,
    verb: RegionEscape,
    dest: FoldingBrand<'b>,
) -> Carried<'b> {
    match value {
        Carried::Object(v) => {
            Carried::Object(dest.alloc_object_folded(relocate_object_into(v, verb, dest)))
        }
        Carried::Type(t) => Carried::Type(t),
        Carried::UnresolvedType(ti) => {
            Carried::UnresolvedType(dest.alloc_type_identifier(ti.clone()))
        }
    }
}

/// Relocate one value into `dest` under the chosen [`RegionEscape`]: a top-level substrate carrier
/// (`Record` / `List` / `Dict` / `Tagged` / `Wrapped`) under a `Copy` verb is totally rebuilt at the door
/// ([`copy_object_into`](crate::machine::model::copy_object_into)) so its substrate lands in `dest`,
/// while under `Pin` it pointer-copies (its region-resident substrate borrow rides, covered by the
/// Kept-minted producer reach at the enclosing transfer). Every other value keeps the pointer-copy
/// `deep_clone` — a scalar or a `KFunction` / `Module` leaf, owning or borrowing verbatim with no
/// nested substrate to relocate. Shared by the seam hooks ([`copy_carried`], the return-contract
/// relocation).
pub(in crate::machine::execute) fn relocate_object_into<'b>(
    value: &KObject<'b>,
    verb: RegionEscape,
    dest: FoldingBrand<'b>,
) -> KObject<'b> {
    match value {
        KObject::Record(..)
        | KObject::List(..)
        | KObject::Dict(..)
        | KObject::Tagged { .. }
        | KObject::Wrapped { .. } => match verb {
            // Pin: pointer-copy the substrate carrier — its region-resident substrate borrow rides,
            // covered by the Kept-minted producer reach at the enclosing transfer.
            RegionEscape::Pin => value.deep_clone(),
            // Copy: total rebuild at the door so the substrate lands in `dest`.
            RegionEscape::Copy { .. } => copy_object_into(value, dest),
        },
        _ => value.deep_clone(),
    }
}

/// Own a transferred [`Carried`] into an aggregate cell at `dest`, relocating a top-level substrate
/// carrier (`Record` / `List` / `Dict` / `Tagged` / `Wrapped`) into `dest`'s region ([`relocate_object_into`]) so its substrate is
/// container-resident — the substrate-aware twin of [`Held::from_carried`], for the literal fold's
/// per-cell seam. The container cell always rebuilds a substrate carrier (Ruling 4: fresh containers
/// stay self-contained), never pins.
pub(in crate::machine::execute) fn copy_held_from_carried<'b>(
    carried: Carried<'b>,
    dest: FoldingBrand<'b>,
) -> Held<'b> {
    match carried {
        Carried::Object(o) => Held::Object(relocate_object_into(
            o,
            RegionEscape::Copy { released: false },
            dest,
        )),
        Carried::Type(t) => Held::Type(t),
        Carried::UnresolvedType(ti) => Held::UnresolvedType(ti.clone()),
    }
}

/// The [`RegionEscape`] for relocating `delivered` across a value-level escape seam. A top-level
/// substrate carrier (`Record` / `List` / `Dict` / `Tagged` / `Wrapped`) routes the cost chooser
/// ([`copy_or_pin`](crate::machine::model::copy_or_pin)); every other value copies unconditionally
/// (`Copy { released: false }` → `Residence::Copied`, the behavior for non-substrate carriers).
pub(in crate::machine::execute) fn seam_verb(delivered: &DeliveredCarried) -> RegionEscape {
    delivered.open(|carried| match carried {
        // The crossing is priced against the region the value *lives in* — the residence the
        // envelope's container supplied ([`Delivered::with_home_region`]).
        Carried::Object(value) => delivered.with_home_region(|host| match value {
            KObject::Record(substrate, _) => copy_or_pin(substrate, value, host),
            KObject::List(substrate, _) => copy_or_pin(substrate, value, host),
            KObject::Dict(substrate, _) => copy_or_pin(substrate, value, host),
            KObject::Tagged {
                value: substrate, ..
            } => copy_or_pin(substrate, value, host),
            KObject::Wrapped {
                inner: substrate, ..
            } => copy_or_pin(substrate, value, host),
            _ => RegionEscape::Copy { released: false },
        }),
        _ => RegionEscape::Copy { released: false },
    })
}

/// The **retention predicate** a value-level relocation of `delivered` hands its fold, given the
/// verb [`seam_verb`] chose (design § Escape):
///
/// - **Pin** — the record stays in the region it lived in and the relocation pointer-copies it, so
///   nothing is released: the producer transfers by hold. A pinned substrate carrier's own
///   region-resident substrate is not a *cell* borrow, so a walk over the product would not see it;
///   the verb answers for it.
/// - **Copy** — the relocation totally rebuilt the value at the destination, so the walk over the
///   product ([`product_still_borrows`]) is the exact answer: a rebuild that left nothing pointing
///   back drops the producer region and the retiring frame frees at retention discharge.
pub(in crate::machine::execute) fn seam_still_borrows<'e>(
    delivered: &'e DeliveredCarried,
    verb: RegionEscape,
) -> impl for<'b> FnMut(&Carried<'b>, &KoanRegion) -> bool + 'e {
    move |product, region| match verb {
        RegionEscape::Pin => true,
        RegionEscape::Copy { .. } => product_still_borrows(delivered, product.as_object(), region),
    }
}

/// The **retention predicate** a relocation of `delivered` across the container-cell seam hands its
/// fold (whose relocate hook is [`copy_held_from_carried`]) — what the copy still reaches on the
/// source side (design § Escape). The cell always rebuilds, so the answer is exact: the walk runs
/// over the [`Held`] the fold just pushed, and a cell that no longer borrows the region it lived in
/// releases it, letting the retiring producer free at retention discharge instead of riding the
/// destination's reach.
///
/// This is what reconciles with `force_substrate_borrows_host`'s conservative seal bit: at a copy
/// seam a still-borrowing carrier keeps its pins, and a plain-data carrier releases, its bit
/// overridden by the copy pass's exact answer.
pub(in crate::machine::execute) fn cell_still_borrows(
    delivered: &DeliveredCarried,
) -> impl for<'b> FnMut(&(RegionHandle<'b, KoanStorageProfile>, Vec<Held<'b>>), &KoanRegion) -> bool + '_
{
    move |product, region| {
        let cell = product.1.last().and_then(|held| match held {
            Held::Object(object) => Some(object),
            Held::Type(_) | Held::UnresolvedType(_) => None,
        });
        product_still_borrows(delivered, cell, region)
    }
}

#[cfg(test)]
mod tests;
