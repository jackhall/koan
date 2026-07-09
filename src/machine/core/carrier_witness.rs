//! Koan's instantiation of the library's reference-only carrier witness
//! ([`crate::witnessed::Carrier`]) over `F = FrameStorage` (the per-call frame owner), and the
//! delivery envelope that carries a value's retained frame pin in transit. See
//! [design/witness-hosting.md § The carrier](../../../design/witness-hosting.md#the-carrier).

use crate::machine::model::values::CarriedFamily;

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
