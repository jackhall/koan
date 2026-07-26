//! Koan's instantiation of the library's reference-only carrier witness
//! ([`crate::witnessed::Carrier`]) over `F = FrameStorage` (the per-call frame owner), and the
//! delivery envelope that carries a value's retained frame pin in transit. See
//! [design/witness-hosting.md § The carrier states](../../../design/witness-hosting.md#the-carrier-states).

use std::rc::Rc;

use crate::machine::core::FramePins;
use crate::machine::model::{Carried, CarriedFamily, KObject};
use crate::witnessed::{Erased, Witnessed};

use super::arena::{FrameStorage, KoanRegion, KoanRegionExt};

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
/// (`mint_reach` / `transfer_into`), so koan never holds a bare frame pin at a consumer site. The
/// envelope's member set names the value's home region as an ordinary member — there is no
/// distinguished host field — so a site that needs the home back locates it by residence
/// ([`with_home_region`]), and a relocation's only choice is *which bundle* it hands the fold.
pub type DeliveredCarried =
    crate::witnessed::Delivered<CarriedFamily, CarrierWitness, FrameStorage>;

/// Run `f` against the region that **hosts** `object` among `envelope`'s pinned members — the
/// value's own home region. Home is an ordinary member of the envelope's flat member set with no
/// distinguished field, so it is recovered here the way residence is defined: by *where the value
/// lives*, i.e. the member whose address side table recorded the value's top node
/// ([`KoanRegionExt::owns_object`]). No member reference escapes — the probe runs inside
/// [`PinBundle::any_member_region`](crate::witnessed::PinBundle::any_member_region).
///
/// The escape-seam probes are the callers: [`copy_or_pin`](crate::machine::model::copy_or_pin) and
/// [`still_borrows_host`](crate::machine::model::still_borrows_host) each price a crossing *out of
/// the region the value lives in*, which is exactly that member.
///
/// `None` when no pinned member owns the top node — a value whose home was subsumed out of the
/// antichain by an outer member, or one built into a region the envelope does not pin. Every
/// caller reads `None` conservatively: copy the value and keep its source pinned.
pub(crate) fn with_home_region<R>(
    envelope: &DeliveredCarried,
    object: &KObject<'_>,
    f: impl FnOnce(&KoanRegion) -> R,
) -> Option<R> {
    let ptr = object as *const KObject<'_>;
    let mut found = None;
    let mut f = Some(f);
    envelope.pins().any_member_region(|region| {
        if region.owns_object(ptr) {
            found = f.take().map(|f| f(region));
            true
        } else {
            false
        }
    });
    found
}

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

/// The **source claim** for a copy that leaves nothing pointing back at the region the value lived
/// in: `envelope`'s own members minus that home region. Every other member survives — a copy can
/// drop its producer and still borrow elsewhere. Falls back to the whole set when the home is not
/// locatable among the members ([`with_home_region`]), which over-retains rather than dangles.
///
/// Dropping the producer from the claim is what frees a tail loop's retiring region once its
/// delivered carrier drops, instead of chaining it into every successor region's arena.
pub(crate) fn source_pins_releasing_home(envelope: &DeliveredCarried) -> FramePins {
    envelope
        .open(|live| {
            live.as_object().and_then(|object| {
                with_home_region(envelope, object, |home| {
                    envelope.pins().without_region(home)
                })
            })
        })
        .unwrap_or_else(|| envelope.pins().clone())
}

/// The **source claim** for a value copied out of `envelope` with no per-value release probe run:
/// [`source_pins_releasing_home`] when the carrier reports its borrows do not reach its own home
/// (the borrows-home bit, the pin-free form of that membership query), else the whole member set.
/// The copy-bind doors and the non-substrate copy seams take this; a seam that runs the exact
/// `still_borrows_host` probe overrides the bit with its own answer.
pub(crate) fn copied_source_pins(envelope: &DeliveredCarried) -> FramePins {
    if envelope.witness().borrows_host() {
        return envelope.pins().clone();
    }
    source_pins_releasing_home(envelope)
}
