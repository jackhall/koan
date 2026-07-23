//! Koan's instantiation of the library's reference-only carrier witness
//! ([`crate::witnessed::Carrier`]) over `F = FrameStorage` (the per-call frame owner), and the
//! delivery envelope that carries a value's retained frame pin in transit. See
//! [design/witness-hosting.md § The carrier](../../../design/witness-hosting.md#the-carrier).

use std::rc::Rc;

use crate::machine::model::{Carried, CarriedFamily};
use crate::witnessed::{Erased, Witnessed};

use super::arena::FrameStorage;

/// Koan's value-carrier witness: the library [`Carrier`](crate::witnessed::Carrier) over koan's
/// frame owner — one `borrows_host` bit plus a reference to the value's hosted reach set. It pins
/// nothing; liveness is the scheduler's retention hold (walking) or the containing region
/// (resident). Every site that only *threads* this type as the `W` witness parameter of
/// `Witnessed<T, W>` / `Sealed<T, W>` is unaffected by this alias; a site that constructs or
/// inspects a carrier routes the library's `Carrier` surface directly.
pub type CarrierWitness = crate::witnessed::Carrier<FrameStorage>;

/// Koan's **delivery envelope**: the library [`Delivered`](crate::witnessed::Delivered) carrying a
/// [`CarrierWitness`]-witnessed value carrier paired with its retained [`FrameStorage`] owner. The
/// in-transit form of a value's liveness — from a scheduler pull (or a resident seal) to its
/// adoption — and the only surface that materializes a producer frame into a minted reach set
/// (`mint_reach` / `transfer_into`), so koan never holds a bare frame pin at a consumer site. Every
/// envelope-bearing mint routes through `Delivered::mint_reach` (koan's `Scope::envelope_reach_of`
/// funnel); the one envelope-free case — a value already resident in a region the caller's context
/// covers ambiently — routes through
/// [`Witnessed::mint_resident_reach`](crate::witnessed::Witnessed::mint_resident_reach) (koan's
/// `Scope::resident_reach_of`) instead.
pub type DeliveredCarried =
    crate::witnessed::Delivered<CarriedFamily, CarrierWitness, FrameStorage>;

/// The step-terminal seal's variant bit (design/value-substrates.md § Escape): force
/// `borrows_host = true` on `witnessed` when its carried value is a substrate carrier (`Record` /
/// `List` / `Dict` / `Tagged` / `Wrapped`) — see
/// [`KObject::embeds_substrate`](crate::machine::model::KObject::embeds_substrate).
///
/// Every fold engine that builds `witnessed` (`map_pinned_placing`, `merge_pinned_placing`,
/// `transfer_into_placing`) composes its witness from the fold's *other* operands alone — it is
/// structurally blind to the value the closure just built — so a freshly-born substrate carrier's
/// own self-borrow into its birth region is otherwise under-reported: a later `Residence::Copied`
/// crossing would read `borrows_host = false` and release the producer while the copy (still a
/// pointer, per Ruling 4) keeps pointing into it. Rebuilding with an empty reach loses nothing:
/// every current birth site's non-substrate fold operand is reach-free (a bare type-channel
/// handle), so the composed reach a correctly-derived witness would have carried was already
/// empty in every case this forces. `pin` is the frame the value was just built into (its
/// producer's own retained owner).
pub(crate) fn force_substrate_borrows_host(
    witnessed: Witnessed<CarriedFamily, CarrierWitness>,
    pin: &Rc<FrameStorage>,
) -> Witnessed<CarriedFamily, CarrierWitness> {
    let forced = witnessed.with_pinned(pin, |carried: &Carried<'_>| match carried {
        Carried::Object(o) if o.embeds_substrate() => Some(Erased::erase(*carried)),
        _ => None,
    });
    match forced {
        Some(erased) => Witnessed::from_erased(erased, CarrierWitness::new(true, None)),
        None => witnessed,
    }
}
