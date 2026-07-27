//! Koan's instantiation of the library's reference-only carrier witness
//! ([`crate::witnessed::Carrier`]) over `F = FrameStorage` (the per-call frame owner), and the
//! delivery envelope that carries a value's retained frame pin in transit. See
//! [design/witness-hosting.md § The carrier states](../../../design/witness-hosting.md#the-carrier-states).

use std::rc::Rc;

use crate::machine::model::{
    still_borrows_host, Carried, CarriedFamily, DispatchToken, KObject, UntypedKey,
};
use crate::witnessed::{Erased, Witnessed};

use super::arena::{FrameStorage, KoanRegion};
use super::kfunction::{KFunction, KFunctionFamily};

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
/// envelope's member set names the value's home region as an ordinary member, and the envelope
/// records the residence owner its container supplied — so a site that needs the home back reads it
/// ([`Delivered::with_home_region`](crate::witnessed::Delivered::with_home_region)) rather than
/// searching the member set, and a relocation derives what it still reaches from the product it
/// built ([`product_still_borrows`]) rather than choosing a bundle up front.
pub type DeliveredCarried =
    crate::witnessed::Delivered<CarriedFamily, CarrierWitness, FrameStorage>;

/// A callable's **dormant** carrier: the `KFunction` fused to the exact reach description minted for
/// it, over the [`KFunctionFamily`] the library dispatches on. This is what a `functions` dispatch
/// bucket stores and what a [`ReturnContract`](crate::machine::core::ReturnContract) carries across
/// a tail chain: the seal states the same claim its mirrored `data` entry does, where a bare
/// `&KFunction` would state no reach at all.
pub type SealedFunction = crate::witnessed::Sealed<KFunctionFamily, CarrierWitness>;

/// A callable **in use**: re-anchored at a region's own lifetime, paired with the reach witness it
/// was opened under. Dispatch resolves on one of these and carries it across argument evaluation
/// (`Resolved<'step>`); the escape into the call chain
/// [`reseal`](crate::witnessed::Opened::reseal)s it back to a [`SealedFunction`].
pub type OpenedFunction<'a> = crate::witnessed::Opened<'a, KFunctionFamily, CarrierWitness>;

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

/// Address identity of a callable, for the write path's intentional-alias short-circuit
/// (`LET g = (f)` registers the same callable twice; the bucket keeps its first entry). Captured
/// from the `&KFunction` at seal time — the one moment it is open — and thereafter **compared,
/// never dereferenced**, and never used for ordering or hashing.
///
/// Sound because identities are only ever compared against other identities captured from
/// callables in the same live region set: exactly what a `ptr::eq` over two re-anchored references
/// compares, minus the re-anchor.
#[derive(Copy, Clone, PartialEq)]
pub(crate) struct CallableIdentity(*const ());

impl CallableIdentity {
    fn of(f: &KFunction<'_>) -> Self {
        CallableIdentity(f as *const KFunction<'_> as *const ())
    }
}

/// Everything a table write path needs about a callable, computed at mirror-seal time — the one
/// moment the callable is open under its home pin — so no write verb ever opens a carrier.
/// `sealed` is what the `functions` bucket stores; the rest is plain data with no region lifetime.
pub(crate) struct FunctionMirror {
    /// The dormant callable carrier the dispatch bucket stores.
    pub sealed: SealedFunction,
    /// `signature.untyped_key()` — the bucket this callable belongs in.
    pub key: UntypedKey,
    /// `signature.dispatch_token()` — the stored form of the duplicate-overload predicate.
    pub token: DispatchToken,
    /// The callable's address identity, for the intentional-alias short-circuit.
    pub identity: CallableIdentity,
    /// `KFunction::summarize()`, rendered here so the `DuplicateOverload` diagnostic can name the
    /// colliding overload without re-opening it.
    pub summary: String,
}

impl FunctionMirror {
    /// The bundle for a callable held live, computed straight off the reference.
    pub(crate) fn of_live(f: &KFunction<'_>, sealed: SealedFunction) -> Self {
        FunctionMirror {
            sealed,
            key: f.signature.untyped_key(),
            token: f.signature.dispatch_token(),
            identity: CallableIdentity::of(f),
            summary: f.summarize(),
        }
    }

    /// A second bundle naming the same callable: the dormant seal bit-copied beside owned copies of
    /// the derived data. The conditional-defer write doors duplicate before attempting so the
    /// original still rides a deferred retry.
    pub(crate) fn duplicate(&self) -> Self {
        FunctionMirror {
            sealed: self.sealed.duplicate(),
            key: self.key.clone(),
            token: self.token.clone(),
            identity: self.identity,
            summary: self.summary.clone(),
        }
    }
}

/// Project a bound value's dormant carrier onto the `KFunction` it wraps, under `pin` — the write
/// door's **mirror seal**, bundled with everything the table write path keys on. The witness rides
/// across verbatim, so the `functions` bucket entry and its `data` twin state one claim about one
/// value; a value that is not a callable yields `None` and mirrors nothing. The projected reference
/// is re-erased inside the pinned read and the derived fields are owned data, so nothing anchored
/// at the read's brand escapes it.
pub(crate) fn function_mirror_of(
    sealed: &crate::witnessed::Sealed<CarriedFamily, CarrierWitness>,
    pin: &Rc<FrameStorage>,
) -> Option<FunctionMirror> {
    let (projected, key, token, identity, summary) =
        sealed.open_with(pin, |carried: Carried<'_>| match carried {
            Carried::Object(object) => object.as_function().map(|f| {
                (
                    Erased::erase(f),
                    f.signature.untyped_key(),
                    f.signature.dispatch_token(),
                    CallableIdentity::of(f),
                    f.summarize(),
                )
            }),
            _ => None,
        })?;
    Some(FunctionMirror {
        sealed: crate::witnessed::Sealed::seal(Witnessed::from_erased(
            projected,
            *sealed.witness(),
        )),
        key,
        token,
        identity,
        summary,
    })
}

/// Koan's **retention predicate** for a copying relocation of `envelope`
/// ([`Delivered::transfer_into`](crate::witnessed::Delivered::transfer_into),
/// design/witness-hosting.md § Escape): whether `product` — the bytes the fold just built at the
/// destination — still borrows `region`, one of the regions the envelope pins.
///
/// Only the value's **home** region is ever released, and `region` is home exactly when it is the
/// residence the envelope's container supplied ([`Delivered::with_home_region`]) — the library hands
/// each pinned region over in turn, so residence is answered per region by identity with no probe
/// over the member set.
/// Every other member is kept: a foreign member may be reached through structure the product's own
/// walk cannot see (a `KFunction`'s captured environment, a `Module`'s child scope), so releasing
/// it on a walk that only follows the product's cells would dangle. Home is exact — the walk is
/// asking whether the copy left a leaf behind in the region it was copied out of, which is
/// precisely what [`still_borrows_host`] answers.
///
/// A `product` of `None` (the fold built no object — a type-channel cell) keeps every member.
/// Releasing home is what frees a tail loop's retiring region once its delivered carrier drops,
/// instead of chaining it into every successor region's arena.
pub(crate) fn product_still_borrows(
    envelope: &DeliveredCarried,
    product: Option<&KObject<'_>>,
    region: &KoanRegion,
) -> bool {
    let is_home = envelope.with_home_region(|home| std::ptr::eq(home, region));
    !is_home || product.is_none_or(|value| still_borrows_host(value, region))
}

/// [`product_still_borrows`] for a relocation whose product is a top-node
/// [`deep_clone`](KObject::deep_clone) of the source value — a scalar, a `KFunction` / `Module` /
/// `KExpression` leaf riding its borrow verbatim. Such a clone borrows exactly what the source
/// does, so the source *is* the product and the predicate answers off the envelope alone.
pub(crate) fn clone_still_borrows(envelope: &DeliveredCarried, region: &KoanRegion) -> bool {
    envelope.open(|live| product_still_borrows(envelope, live.as_object(), region))
}
