//! Koan's instantiation of the library's reference-only carrier witness over `F = FrameStorage`
//! (the per-call frame owner), and the carrier aliases each value family rests, travels and is read
//! through. See
//! [workgraph/design/reach.md § The carrier states](../../workgraph/design/reach.md#the-carrier-states).

use super::cell::{Carried, CarriedFamily};
use super::region::{FrameStorage, KoanRegion};
use super::substrate::{Carrier, Delivered, Opened, Sealed};
use crate::machine::core::KFunctionFamily;
use crate::machine::model::{KObject, OperatorGroupFamily, retains_home};

/// Koan's value-carrier witness: the library [`Carrier`] over koan's
/// frame owner — a reference to the value's hosted reach description and nothing else. The
/// description carries both of the value's region facts: its *host* is the region the value lives
/// in, its *members* are the regions the value's borrows reach, home among them exactly when the
/// value genuinely borrows into its own region. The carrier pins nothing; liveness is always the
/// containing region — the producer's own while the delivery walk carries the terminal, the
/// destination's the moment the walk adopts it in. Every site that only
/// *threads* this type as the `W` witness parameter of `Witnessed<T, W>` / `Sealed<T, W>` is
/// unaffected by this alias; a site that constructs or inspects a carrier routes the library's
/// `Carrier` surface directly.
pub type CarrierWitness = Carrier<FrameStorage>;

/// Koan's **delivery envelope**: the library [`Delivered`] carrying a
/// [`CarrierWitness`]-witnessed value carrier paired with its retained [`FrameStorage`] owner. The
/// in-transit form of a value's liveness — from a scheduler pull (or a resident seal) to its
/// adoption. The retained frame is private to the envelope and materializes into a minted reach set
/// only through the envelope's own verbs (`adopt_into` / `open_adopted` / `transfer_into`), so koan
/// never holds a bare frame pin at a consumer site. The
/// envelope's member set pins the value's home region alongside everything else it reaches, and the
/// residence itself is the host of the description the carrier references — so a site that needs the
/// home back reads it off the value's own record rather than off a side channel on the envelope, and
/// a relocation derives what it still reaches from the product it built ([`product_reaches_region`])
/// rather than choosing a bundle up front.
pub type DeliveredCarried = Delivered<CarriedFamily>;

/// A callable **in transit from its birth**: the merge-born `KFunction` carrier paired with the home
/// pin its birth composed. What [`KFunction::alloc_captured`](crate::machine::core::KFunction::alloc_captured) hands back and what every registration
/// door composes from — the seal ([`OverloadSeal::of_delivered`]) rests it, the `KObject` wrapper
/// ([`Scope::store_function_cell`](crate::machine::core::Scope)) merges it — so no door re-states the
/// callable's reach on its own authority.
pub type DeliveredFunction = Delivered<KFunctionFamily>;

/// A resolved sub-result **at rest** inside a working expression: the producer's sealed value
/// carrier alone, `Copy` and `Drop`-free, with the pins that keep its backing alive lodged one level
/// down in the region the cell was rested into
/// ([`Delivered::rest_in`](super::substrate::Delivered::rest_in), reached through
/// [`Scope::rest_delivered`](Scope::rest_delivered)). The resting form of a
/// [`DeliveredCarried`]: same carrier, ownership relocated — which is what lets an
/// [`WorkingPart`](crate::machine::model::WorkingPart) hold one without becoming heap-shaped.
///
/// Reading one names its coverage, as every reference-only carrier does: the reach-carrying route is
/// [`Scope::lift_spliced`](Scope::lift_spliced), back to an envelope for an
/// adoption; a verdict-only reader opens the cell at its own brand through [`read_resting`].
pub type SplicedCell<'home> = Sealed<'home, CarriedFamily>;

/// Read a resting splice cell at a site with **no pin vocabulary** — the registry-free renderers
/// ([`WorkingPart`](crate::machine::model::WorkingPart)'s `Debug` / `summarize`) and the slot
/// classifier `KType::accepts_cell`). Each is a pure
/// probe over a part the caller already holds, reached from signatures that carry no scope and (for
/// `Debug::fmt`) could not be given one.
///
/// The coverage is the step's, not the reader's: a probe runs synchronously inside the step holding
/// the expression, and a cell rests in that step's own cart — the splice and every read of it happen
/// on one side of a tail hop, never across one. So the pointee outlives the read for a reason
/// outside it, which is exactly what [`NoPins`] names. Stated once here so the
/// assertion has one home rather than one per call site. A reader that holds a scope names a pin
/// instead: [`Scope::read_spliced`](Scope::read_spliced) for another verdict,
/// [`Scope::lift_spliced`](Scope::lift_spliced) when it goes on to *adopt* the
/// value, which owns the reach rather than merely naming it.
pub(crate) fn read_resting<R>(
    cell: &SplicedCell<'_>,
    read: impl for<'b> FnOnce(Carried<'b>) -> R,
) -> R {
    cell.open(read)
}

/// A callable's **dormant** carrier: the `KFunction` fused to the exact reach description its birth
/// composed for it, over the [`KFunctionFamily`] the library dispatches on. This is what a `functions` dispatch
/// bucket stores and what a [`ReturnContract`](crate::machine::core::ReturnContract) carries across
/// a tail chain: the seal fuses the callable with its reach claim, where a bare `&KFunction` would
/// state no reach at all.
pub type SealedFunction<'home> = Sealed<'home, KFunctionFamily>;

/// An operator group's **dormant** carrier: the region-hosted [`OperatorGroup`](crate::machine::model::OperatorGroup) record fused to the
/// reach description its yoked birth composed for it, over the [`OperatorGroupFamily`]. This is what an `operators`
/// registry entry stores — the same entry shape the `data` and `functions` tables use, so a
/// [`Bindings`](crate::machine::core::Bindings) table stays lifetime-free. Every powerset key of one
/// `GROUP` declaration holds a duplicate of the same seal over the same pointee, so sharing is
/// address identity.
pub type SealedOperatorGroup<'home> = Sealed<'home, OperatorGroupFamily>;

/// An operator group **in transit**: [`SealedOperatorGroup`] lifted at its declaring scope, so the
/// envelope's coverage owns the region hosting the record — which is what lets a chain resolve a
/// group declared in an ancestor scope and read it under pins of its own.
pub type DeliveredOperatorGroup = Delivered<OperatorGroupFamily>;

/// A callable **in use**: re-anchored at a region's own lifetime, paired with the reach witness it
/// was opened under. Dispatch resolves on one of these and carries it across argument evaluation
/// (`Resolved<'step>`); the escape into the call chain
/// [`reseal`](super::substrate::Opened::reseal)s it back to a [`SealedFunction`].
pub type OpenedFunction<'a> = Opened<'a, KFunctionFamily>;

/// Koan's **retention claim** for a copying relocation of `envelope`
/// ([`Delivered::transfer_into`](super::substrate::Delivered::transfer_into),
/// design/witness-hosting.md § Escape): whether `product` — what the fold just built at the
/// destination — still borrows `region`, one of the regions the envelope pins. Answered by
/// [`retains_home`], a read over `product`'s stored reach; no probe walks its shape.
///
/// A copy releases only the value's own home region
/// ([value-substrates.md § Sectioned reach](../../design/value-substrates.md#sectioned-reach)).
/// `region` is home exactly when the value's own reach description names it as host — read off the
/// carrier through the envelope's open, so residence is answered by identity against the value's own
/// record rather than a side channel on the envelope. A non-home member is kept because it may be
/// reached through structure the product's stored reach does not cover (a `KFunction`'s captured
/// environment reaches on transitively), so releasing it would dangle.
///
/// A `product` of `None` (the fold built no object — a type-channel cell) keeps every member.
/// Releasing home is what frees a tail loop's retiring region once its delivered carrier drops,
/// instead of chaining it into every successor region's arena.
pub(crate) fn product_reaches_region(
    envelope: &DeliveredCarried,
    product: Option<&KObject<'_>>,
    region: &KoanRegion,
) -> bool {
    let is_home = envelope
        .open_at()
        .with_home_region(|home| std::ptr::eq(home, region));
    !is_home || product.is_none_or(|value| retains_home(value, region))
}
