//! The witnessed-transfer copy hooks: the [`copy_carried`] relocate callback, the cell-level
//! [`copy_held_from_carried`] copy, and the fused value-level escape seam ([`relocate_seam`]) —
//! the verb choice ([`seam_verb`]) paired with the retention claim it implies
//! ([`seam_still_borrows`]). The cost decision, the per-value relocation verbs, and the stored
//! release read live in [`crate::machine::model`], shared with the core binding seams. See
//! [design/value-substrates.md § Escape](../../../design/value-substrates.md#escape-pin-by-default).

use crate::machine::core::{FoldingBrand, SubstrateDoor};
use crate::machine::core::{KoanRegion, KoanStorageProfile, product_reaches_region};
use crate::machine::model::{
    Carried, CarriedFamily, Held, KObject, RegionEscape, copy_or_pin, copy_or_pin_callable,
    relocate_object_into,
};
use crate::machine::{CarrierWitness, DeliveredCarried, FrameStorage};
use crate::witnessed::{Delivered, RegionHandleFamily, reattachable};

/// The structural-copy callback a witnessed transfer's fold runs
/// ([`Delivered::transfer_into`](crate::witnessed::Delivered)): copy a [`Carried`] into `dest`'s
/// region at the fold brand, per value under [`relocate_object_into`]. The copy is all this hook
/// owns — never a region anchor: what a preserved bare borrow points at is kept alive by the reach
/// the transfer mints into the destination.
///
/// `dest` is a [`SubstrateDoor`], not a plain brand, because it carries the source envelope's
/// coverage — the holder-rule proof a rebuilt cell's stored reach is read under.
pub(in crate::machine::execute) fn copy_carried<'b>(
    value: Carried<'b>,
    verb: RegionEscape,
    dest: SubstrateDoor<'b, '_>,
) -> Carried<'b> {
    match value {
        Carried::Object(v) => {
            Carried::Object(dest.alloc_object_folded(relocate_object_into(v, verb, dest)))
        }
        // Both type-channel arms are lifetime-free `Copy` handles, so the copy verb's release of
        // the source region leaves them untouched — there is nothing to rebuild.
        Carried::Type(t) => Carried::Type(t),
        Carried::UnresolvedType(name) => Carried::UnresolvedType(name),
    }
}

/// Own a transferred [`Carried`] into an aggregate cell at `dest`, relocating what keeps region
/// storage behind into `dest`'s region ([`relocate_object_into`]) so the cell is container-resident.
/// A container cell always rebuilds — fresh containers stay self-contained — and never pins.
pub(in crate::machine::execute) fn copy_held_from_carried<'b>(
    carried: Carried<'b>,
    dest: SubstrateDoor<'b, '_>,
) -> Held<'b> {
    match carried {
        Carried::Object(o) => Held::Object(relocate_object_into(o, RegionEscape::Copy, dest)),
        Carried::Type(t) => Held::Type(t),
        Carried::UnresolvedType(name) => Held::UnresolvedType(name),
    }
}

/// The [`RegionEscape`] for relocating `delivered` across a value-level escape seam: a top-level
/// substrate carrier routes the cost chooser ([`copy_or_pin`]), a top-level callable the
/// environment chooser ([`copy_or_pin_callable`]), and everything else copies its top node
/// unconditionally. The verb names only the act; what the relocation still reaches is
/// [`seam_still_borrows`]'s question, answered off the rebuilt product.
fn seam_verb(delivered: &DeliveredCarried) -> RegionEscape {
    let opened = delivered.open_at();
    match opened.value() {
        // Priced against the region the value *lives in* — the host its own reach description
        // names, read off the carrier under the envelope's pins.
        Carried::Object(value) => opened.with_home_region(|host| match value {
            KObject::Record(substrate, _) => copy_or_pin(substrate, host),
            KObject::List(substrate, _) => copy_or_pin(substrate, host),
            KObject::Dict(substrate, _) => copy_or_pin(substrate, host),
            KObject::Wrapped {
                inner: substrate, ..
            } => copy_or_pin(substrate, host),
            // A top-level callable prices its captured environment instead of a substrate: the
            // chain the closure holds is what a pin would retain, and consolidating it is what
            // frees the producer.
            KObject::KFunction(function) => copy_or_pin_callable(function.captured_scope(), host),
            _ => RegionEscape::Copy,
        }),
        _ => RegionEscape::Copy,
    }
}

/// The **retention claim** a value-level relocation of `delivered` hands its fold, given the verb
/// [`seam_verb`] chose. Derived from the **product** the fold just built
/// ([`product_reaches_region`]) so the verdict and the act cannot disagree: a pin releases nothing,
/// and a copy releases only the value's own home region. Every other member is kept, because a
/// foreign member may be reached transitively through a borrow leaf's own environment.
fn seam_still_borrows<'e>(
    delivered: &'e DeliveredCarried,
    verb: RegionEscape,
) -> impl for<'b> FnMut(&Carried<'b>, &KoanRegion) -> bool + 'e {
    move |product, region| {
        !verb.rebuilds() || product_reaches_region(delivered, product.as_object(), region)
    }
}

/// Relocate `delivered` across the value-level escape seam into `dest`, fusing the verb choice
/// ([`seam_verb`]) with the product-derived retention claim ([`seam_still_borrows`]) and the
/// relocate hook ([`copy_carried`]) so the three cannot be re-paired at a call site.
///
/// `dest` is a bare region handle sealed into a delivery envelope ([`Delivered::destination`]), so
/// the transfer composes the producer's reach alone and homes the product in the destination's own
/// frame.
pub(in crate::machine::execute) fn relocate_seam(
    delivered: &DeliveredCarried,
    dest: Delivered<RegionHandleFamily<KoanStorageProfile>, CarrierWitness, FrameStorage>,
) -> DeliveredCarried {
    let verb = seam_verb(delivered);
    // Captured before the fold, which cannot reach its operand's pins: the source envelope's
    // coverage is the holder-rule proof the relocation's cells read their stored reach under.
    let holder = delivered.coverage().clone();
    delivered
        .transfer_into::<RegionHandleFamily<KoanStorageProfile>, CarriedFamily, KoanStorageProfile>(
            dest,
            seam_still_borrows(delivered, verb),
            |value, _region, placement| {
                copy_carried(
                    value,
                    verb,
                    FoldingBrand::in_fold_closure(placement).with_holder(&holder),
                )
            },
        )
}

/// The cell family an aggregate relocation's product run is made of. Layout-invariant in `'r`:
/// [`Held`] is one type up to its lifetime.
pub(in crate::machine::execute) struct HeldFamily;
reattachable!(HeldFamily => Held<'r>);

/// The **retention claim** a relocation of a cell run across the container-cell seam hands its
/// door. Every cell rebuilds, so the answer is exact and per-cell: a cell that no longer borrows
/// the region it lived in releases it, letting the retiring producer free at retention discharge
/// instead of riding the destination's reach. That is also what keeps a birth-site mint from
/// over-retaining — a fold door's mint names every region the source bundle pins, and this pass is
/// where a plain-data copy drops the ones it no longer borrows.
///
/// [`Delivered::transfer_all_into`] hands each source together with **its own** product cell, so
/// there is no index into a run to trust: the claim is always the one belonging to the cell it is
/// looking at.
pub(in crate::machine::execute) fn relocated_cell_still_borrows(
    source: &DeliveredCarried,
    cell: &Held<'_>,
    region: &KoanRegion,
) -> bool {
    let object = match cell {
        Held::Object(object) => Some(object),
        Held::Type(_) | Held::UnresolvedType(_) | Held::Name(_) | Held::RecordType(_) => None,
    };
    product_reaches_region(source, object, region)
}

#[cfg(test)]
mod tests;
