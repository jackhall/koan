//! The witnessed-transfer copy hooks: the [`copy_carried`] relocate callback, the cell-level
//! [`copy_held_from_carried`] copy, the value-level escape
//! seam's [`seam_verb`] chooser, and the retention predicate [`seam_still_borrows`] the chosen verb
//! implies. The
//! cost decision itself ([`copy_or_pin`](crate::machine::model::copy_or_pin)), the per-value
//! relocation ([`relocate_object_into`](crate::machine::model::relocate_object_into)), the
//! total-rebuild verb ([`copy_object_into`](crate::machine::model::copy_object_into)), and the
//! stored release read ([`retains_home`](crate::machine::model::retains_home))
//! live in the value model, shared with the core binding seams. See
//! [design/value-substrates.md § Escape](../../../design/value-substrates.md#escape-pin-by-default).

use crate::machine::core::SubstrateDoor;
use crate::machine::core::{product_reaches_region, KoanRegion, KoanStorageProfile};
use crate::machine::model::{
    copy_or_pin, relocate_object_into, retains_home, Carried, Held, KObject, RegionEscape,
    TypeIdentifier,
};
use crate::machine::DeliveredCarried;
use crate::witnessed::RegionHandle;

/// The structural-copy callback a witnessed transfer's fold runs
/// ([`Delivered::transfer_into`](crate::witnessed::Delivered)): copy a [`Carried`] into `dest`'s
/// region at the fold brand. The per-value verb is
/// [`relocate_object_into`](crate::machine::model::relocate_object_into): under a `Copy` a value
/// keeping region storage behind
/// ([`needs_destination_door`](crate::machine::model::KObject::needs_destination_door) — a substrate
/// carrier, or a bare `KString` whose bytes live in the source bump) is **totally rebuilt** at
/// `dest`; every other value re-allocates only its top node
/// ([`deep_clone`](crate::machine::model::KObject::deep_clone)) — a scalar rebuilt owned, a
/// `KFunction` / first-class `Module` riding a bare borrow preserved verbatim — kept alive by the
/// reach set the transfer mints into the destination, so this hook owns only the copy, never a region
/// anchor. It is not a delivery channel: dep terminals cross to finishes as sealed carriers. `dest`
/// is a [`SubstrateDoor`], not a plain brand: every caller is a `transfer_into` fold closure, whose
/// enclosing combinator has already minted the value's reach into `dest`'s arena, so a bare-borrow
/// payload like `KFunction` is covered by the fold rather than an address-only audit that can't see
/// it — and the door carries the source envelope's coverage, the holder-rule proof a rebuilt cell's
/// stored reach is read under.
pub(in crate::machine::execute) fn copy_carried<'b>(
    value: Carried<'b>,
    verb: RegionEscape,
    dest: SubstrateDoor<'b, '_>,
) -> Carried<'b> {
    match value {
        Carried::Object(v) => {
            Carried::Object(dest.alloc_object_folded(relocate_object_into(v, verb, dest)))
        }
        Carried::Type(t) => Carried::Type(t),
        // Re-bump the name at the destination, the honest peer of the `KString` arm in
        // [`copy_object_into`]: the copy verb claims release of the source region, so the rebuilt
        // identifier must not keep borrowing bytes that region owns.
        Carried::UnresolvedType(ti) => {
            Carried::UnresolvedType(TypeIdentifier::leaf(dest.alloc_text(ti.as_str())))
        }
    }
}

/// Own a transferred [`Carried`] into an aggregate cell at `dest`, relocating what keeps region
/// storage behind — a substrate carrier, or a bare `KString` — into `dest`'s region
/// ([`relocate_object_into`]) so the cell is container-resident: the substrate-aware twin of
/// [`Held::from_carried`], for the literal fold's per-cell seam. The container cell always rebuilds
/// (Ruling 4: fresh containers stay self-contained), never pins.
pub(in crate::machine::execute) fn copy_held_from_carried<'b>(
    carried: Carried<'b>,
    dest: SubstrateDoor<'b, '_>,
) -> Held<'b> {
    match carried {
        Carried::Object(o) => Held::Object(relocate_object_into(
            o,
            RegionEscape::Copy { released: false },
            dest,
        )),
        Carried::Type(t) => Held::Type(t),
        // A cell always rebuilds at the door, so the name's bytes are re-bumped with it.
        Carried::UnresolvedType(ti) => {
            Held::UnresolvedType(TypeIdentifier::leaf(dest.alloc_text(ti.as_str())))
        }
    }
}

/// The [`RegionEscape`] for relocating `delivered` across a value-level escape seam. A top-level
/// substrate carrier (`Record` / `List` / `Dict` / `Tagged` / `Wrapped`) routes the cost chooser
/// ([`copy_or_pin`](crate::machine::model::copy_or_pin)); every other value copies its top node
/// unconditionally, claiming release off the same stored read the chooser's copying arms use
/// ([`retains_home`](crate::machine::model::retains_home)).
///
/// A **type-channel** carrier builds no object at all, so there is nothing to read a claim off and
/// it keeps every member — the conservatism a `None` product has always carried.
pub(in crate::machine::execute) fn seam_verb(delivered: &DeliveredCarried) -> RegionEscape {
    let opened = delivered.open_at();
    match opened.value() {
        // The crossing is priced against the region the value *lives in* — the host its own reach
        // description names, read off the carrier under the envelope's pins.
        Carried::Object(value) => opened.with_home_region(|host| match value {
            KObject::Record(substrate, _) => copy_or_pin(substrate, value, host),
            KObject::List(substrate, _) => copy_or_pin(substrate, value, host),
            KObject::Dict(substrate, _) => copy_or_pin(substrate, value, host),
            KObject::Tagged {
                value: substrate, ..
            } => copy_or_pin(substrate, value, host),
            KObject::Wrapped {
                inner: substrate, ..
            } => copy_or_pin(substrate, value, host),
            _ => RegionEscape::Copy {
                released: !retains_home(value, host),
            },
        }),
        _ => RegionEscape::Copy { released: false },
    }
}

/// The **retention claim** a value-level relocation of `delivered` hands its fold, given the verb
/// [`seam_verb`] chose (design § Escape). It reads the verb alone — the product is never walked:
///
/// - **Pin** — the value stays in the region it lived in and the relocation pointer-copies it, so
///   nothing is released: the producer transfers by hold.
/// - **Copy** — the relocation rebuilt the value at the destination, and `released` already states
///   whether any run of it still names the region it was copied out of. Only that region — the
///   value's own home — is releasable; every other member is kept, because a foreign member may be
///   reached transitively through a borrow leaf's own environment.
pub(in crate::machine::execute) fn seam_still_borrows<'e>(
    delivered: &'e DeliveredCarried,
    verb: RegionEscape,
) -> impl for<'b> FnMut(&Carried<'b>, &KoanRegion) -> bool + 'e {
    move |_product, region| match verb {
        RegionEscape::Pin => true,
        RegionEscape::Copy { released } => {
            !released
                || !delivered
                    .open_at()
                    .with_home_region(|home| std::ptr::eq(home, region))
        }
    }
}

/// The **retention claim** a relocation of `delivered` across the container-cell seam hands its
/// fold (whose relocate hook is [`copy_held_from_carried`]) — what the copy still reaches on the
/// source side (design § Escape). The cell always rebuilds, so the answer is exact and per-cell: it
/// reads the stored reach of the [`Held`] the fold just pushed, and a cell that no longer borrows
/// the region it lived in releases it, letting the retiring producer free at retention discharge
/// instead of riding the destination's reach.
///
/// It is also what keeps a birth-site mint from over-retaining: a fold door's mint names every
/// region the source bundle pins, and this pass is where a plain-data copy drops the ones it no
/// longer borrows, while a still-borrowing carrier keeps its pins.
pub(in crate::machine::execute) fn cell_still_borrows(
    delivered: &DeliveredCarried,
) -> impl for<'b> FnMut(&(RegionHandle<'b, KoanStorageProfile>, Vec<Held<'b>>), &KoanRegion) -> bool + '_
{
    move |product, region| {
        let cell = product.1.last().and_then(|held| match held {
            Held::Object(object) => Some(object),
            Held::Type(_) | Held::UnresolvedType(_) => None,
        });
        product_reaches_region(delivered, cell, region)
    }
}

#[cfg(test)]
mod tests;
