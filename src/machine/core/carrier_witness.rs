//! Koan's instantiation of the library's reference-only carrier witness
//! ([`crate::witnessed::Carrier`]) over `F = FrameStorage` (the per-call frame owner), and the
//! delivery envelope that carries a value's retained frame pin in transit. See
//! [workgraph/design/reach.md § The carrier states](../../../workgraph/design/reach.md#the-carrier-states).

use crate::machine::model::{
    retains_home, Carried, CarriedFamily, DispatchToken, KObject, OperatorGroupFamily, UntypedKey,
};

use super::arena::{FrameStorage, KoanRegion};
use super::kfunction::{KFunction, KFunctionFamily};
use super::scope::Scope;

/// Koan's value-carrier witness: the library [`Carrier`](crate::witnessed::Carrier) over koan's
/// frame owner — a reference to the value's hosted reach description and nothing else. The
/// description carries both of the value's region facts: its *host* is the region the value lives
/// in, its *members* are the regions the value's borrows reach, home among them exactly when the
/// value genuinely borrows into its own region. The carrier pins nothing; liveness is the
/// scheduler's retention hold (walking) or the containing region (resident). Every site that only
/// *threads* this type as the `W` witness parameter of `Witnessed<T, W>` / `Sealed<T, W>` is
/// unaffected by this alias; a site that constructs or inspects a carrier routes the library's
/// `Carrier` surface directly.
pub type CarrierWitness = crate::witnessed::Carrier<FrameStorage>;

/// Koan's **delivery envelope**: the library [`Delivered`](crate::witnessed::Delivered) carrying a
/// [`CarrierWitness`]-witnessed value carrier paired with its retained [`FrameStorage`] owner. The
/// in-transit form of a value's liveness — from a scheduler pull (or a resident seal) to its
/// adoption — and the only surface that materializes a producer frame into a minted reach set
/// (`adopt_into` / `open_adopted` / `transfer_into`), so koan never holds a bare frame pin at a
/// consumer site. The
/// envelope's member set pins the value's home region alongside everything else it reaches, and the
/// residence itself is the host of the description the carrier references — so a site that needs the
/// home back reads it off the value's own record rather than off a side channel on the envelope, and
/// a relocation derives what it still reaches from the product it built ([`product_reaches_region`])
/// rather than choosing a bundle up front.
pub type DeliveredCarried =
    crate::witnessed::Delivered<CarriedFamily, CarrierWitness, FrameStorage>;

/// A resolved sub-result **at rest** inside a working expression: the producer's sealed value
/// carrier alone, `Copy` and `Drop`-free, with the pins that keep its backing alive lodged one level
/// down in the region the cell was rested into
/// ([`Delivered::rest_in`](crate::witnessed::Delivered::rest_in), reached through
/// [`Scope::rest_delivered`](crate::machine::core::Scope::rest_delivered)). The resting form of a
/// [`DeliveredCarried`]: same carrier, ownership relocated — which is what lets an
/// [`WorkingPart`](crate::machine::model::WorkingPart) hold one without becoming heap-shaped.
///
/// Reading one names its coverage, as every reference-only carrier does: the reach-carrying route is
/// [`Scope::lift_spliced`](crate::machine::core::Scope::lift_spliced), back to an envelope for an
/// adoption; a verdict-only reader opens the cell at its own brand through [`read_resting`].
pub type SplicedCell = crate::witnessed::Sealed<CarriedFamily, CarrierWitness>;

/// Read a resting splice cell at a site with **no pin vocabulary** — the registry-free renderers
/// ([`WorkingPart`](crate::machine::model::WorkingPart)'s `Debug` / `summarize`) and the slot
/// classifier `KType::accepts_cell`). Each is a pure
/// probe over a part the caller already holds, reached from signatures that carry no scope and (for
/// `Debug::fmt`) could not be given one.
///
/// The coverage is the step's, not the reader's: a probe runs synchronously inside the step holding
/// the expression, and that step holds the region the cell was rested into — its own cart for a
/// decide, the run loop's TCO handoff hold across a framed tail hop. So the pointee outlives the read
/// for a reason outside it, which is exactly what [`NoPins`] names. Stated once here so the
/// assertion has one home rather than one per call site; a reader that goes on to *adopt* the value
/// takes [`Scope::lift_spliced`](crate::machine::core::Scope::lift_spliced) instead, which owns the
/// reach rather than merely naming it.
pub(crate) fn read_resting<R>(
    cell: &SplicedCell,
    read: impl for<'b> FnOnce(Carried<'b>) -> R,
) -> R {
    cell.open_with(&crate::witnessed::NoPins, read)
}

/// A callable's **dormant** carrier: the `KFunction` fused to the exact reach description minted for
/// it, over the [`KFunctionFamily`] the library dispatches on. This is what a `functions` dispatch
/// bucket stores and what a [`ReturnContract`](crate::machine::core::ReturnContract) carries across
/// a tail chain: the seal fuses the callable with its reach claim, where a bare `&KFunction` would
/// state no reach at all.
pub type SealedFunction = crate::witnessed::Sealed<KFunctionFamily, CarrierWitness>;

/// An operator group's **dormant** carrier: the region-hosted [`OperatorGroup`] record fused to the
/// reach description minted for it, over the [`OperatorGroupFamily`]. This is what an `operators`
/// registry entry stores — the same entry shape the `data` and `functions` tables use, so a
/// [`Bindings`](crate::machine::core::Bindings) table stays lifetime-free. Every powerset key of one
/// `GROUP` declaration holds a duplicate of the same seal over the same pointee, so sharing is
/// address identity.
pub type SealedOperatorGroup = crate::witnessed::Sealed<OperatorGroupFamily, CarrierWitness>;

/// An operator group **in transit**: [`SealedOperatorGroup`] lifted at its declaring scope, so the
/// envelope's coverage owns the region hosting the record — which is what lets a chain resolve a
/// group declared in an ancestor scope and read it under pins of its own.
pub type DeliveredOperatorGroup =
    crate::witnessed::Delivered<OperatorGroupFamily, CarrierWitness, FrameStorage>;

/// A callable **in use**: re-anchored at a region's own lifetime, paired with the reach witness it
/// was opened under. Dispatch resolves on one of these and carries it across argument evaluation
/// (`Resolved<'step>`); the escape into the call chain
/// [`reseal`](crate::witnessed::Opened::reseal)s it back to a [`SealedFunction`].
pub type OpenedFunction<'a> = crate::witnessed::Opened<'a, KFunctionFamily, CarrierWitness>;

/// Everything a dispatch-bucket registration needs about a callable, computed at seal time — the
/// one moment the callable is open under its home pin — so no write verb ever opens a carrier.
/// `sealed` is what the `functions` bucket stores; the rest is plain data with no region lifetime.
///
/// A keyworded expression becomes dispatchable **only** through one of these — the `FN` / `OP`
/// registration doors and the builtin seeds. Binding a function *value* (`LET g = (f)`) publishes
/// nothing here: a value binding is callable by name alone.
pub(crate) struct OverloadSeal {
    /// The dormant callable carrier the dispatch bucket stores.
    pub sealed: SealedFunction,
    /// `signature.untyped_key()` — the bucket this callable belongs in.
    pub key: UntypedKey,
    /// `signature.dispatch_token()` — the stored form of the duplicate-overload predicate.
    pub token: DispatchToken,
    /// `KFunction::summarize()`, rendered here so the `DuplicateOverload` diagnostic can name the
    /// colliding overload without re-opening it.
    pub summary: String,
}

impl OverloadSeal {
    /// The bundle for a callable **resident in `scope`'s own region** — the `FN` / `OP`
    /// registration doors. The description is hosted in `scope`'s own region with no members: `FN`
    /// allocates the callable into the very scope it captures, so it reaches nothing beyond the
    /// region it lives in, which every read of it already pins. The callable is held live here, so
    /// everything the bucket write keys on is read straight off the reference and travels as plain
    /// data.
    pub(crate) fn of_resident<'a>(scope: &Scope<'a>, f: &'a KFunction<'a>) -> Self {
        let sealed = scope.seal_resident::<KFunctionFamily>(f);
        OverloadSeal {
            sealed,
            key: f.signature.untyped_key(),
            token: f.signature.dispatch_token(),
            summary: f.summarize(),
        }
    }
}

/// Koan's **retention claim** for a copying relocation of `envelope`
/// ([`Delivered::transfer_into`](crate::witnessed::Delivered::transfer_into),
/// design/witness-hosting.md § Escape): whether `product` — what the fold just built at the
/// destination — still borrows `region`, one of the regions the envelope pins. Answered by
/// [`retains_home`], a read over `product`'s stored reach; no probe walks its shape.
///
/// Only the value's **home** region is ever released, and `region` is home exactly when it is the
/// region the value's own reach description names as its host — read off the carrier through the
/// envelope's open, so residence is answered per region by identity against the value's own record
/// rather than a side channel on the envelope.
/// Every other member is kept: a foreign member may be reached through structure the product's own
/// stored reach does not cover (a `KFunction`'s captured environment reaches on transitively), so
/// releasing it here would dangle. Home is exact — the question is whether the copy left a leaf
/// behind in the region it was copied out of, which is precisely what a run naming that region says.
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
