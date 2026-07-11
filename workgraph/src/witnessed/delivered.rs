//! [`Delivered<T, W, F>`] — the **delivery envelope**: a sealed carrier paired with the frame-owner
//! `Rc` that retains its value's backing *in transit*, from a scheduler pull to the point a consumer
//! adopts or re-homes it. See
//! [design/witness-hosting.md § Retention model](../../../design/witness-hosting.md#retention-model).
//!
//! Liveness *at rest* is the scheduler's retention table (a producer frame stays held while any
//! consumer edge is undischarged). Liveness *in transit* — from a pull to its adoption — is this
//! envelope: it bundles the dormant [`Sealed`] carrier with the retained `Rc<F>` that owns the
//! region the carrier's value and its hosted reach set live in, so a consumer reads the value under
//! a pin it does not have to thread separately. The fields are private and every constructor is a
//! surface that has the true owner in hand, so an envelope whose pin disagrees with its payload is
//! not constructible — the co-location the carrier's owned host arm once kept by convention is
//! enforcement by construction.
//!
//! The envelope is also where a value's **residence becomes a named reach member**: the carrier is
//! reference-only (it has no `Rc` to materialize), so the envelope-bearing mint verbs —
//! [`Delivered::mint_reach`] and [`Delivered::transfer_into`] — are the only places a producer
//! frame is folded into a minted destination set, gated on the [`Residence`] mode (unconditionally
//! for a value that keeps living in the producer's region; on `borrows_host` for a value copied
//! out). See [design/witness-hosting.md § Composition](../../../design/witness-hosting.md#composition-minting-a-set).
//!
//! [`Delivered::mint_reach`] is the sole envelope-bearing mint entry a consumer routes; a value with
//! no envelope in hand instead mints through [`Witnessed::mint_resident_reach`](super::Witnessed::mint_resident_reach).
//! Both are thin callers over the crate-internal [`Carrier::mint_into`](super::Carrier::mint_into)
//! core. [`Delivered::adopt_into`] fuses that mint with the re-anchor it justifies into one
//! copy-free adoption verb, so a caller cannot split the pin from the reattach it pins.

use std::rc::Rc;

use super::{
    Carrier, Erased, FoldToken, HasRegionHandle, PinsRegion, Reattachable, Region, RegionHandle,
    RegionOwner, RegionSet, Residence, Sealed, StorageProfile, Stored, Witnessed,
};

/// A sealed carrier paired with the retained frame owner that pins its value's backing in transit.
/// `T` is the carrier's value family, `W` its reach witness, `F` the workload's frame-owner type
/// (`Rc<F>` is the residence pin).
pub struct Delivered<T: Reattachable, W, F> {
    /// The dormant carrier — value and reach as one unit.
    cell: Sealed<T, W>,
    /// The retained frame owner whose region the value lives in. Every producer seeds a retention
    /// hold at finalize (the run frame's storage owns the run region), so the owner always exists;
    /// a resident seal pairs the home region's owner.
    host: Rc<F>,
}

impl<T: Reattachable, W, F> Delivered<T, W, F> {
    /// Pair a sealed carrier with the retained frame owner that pins its value's backing. The
    /// caller supplies the true owner — the scheduler's retention hold for a delivered dep, or the
    /// region owner for a resident seal — so the pairing is co-located by construction.
    pub fn hosted(cell: Sealed<T, W>, host: Rc<F>) -> Self {
        Delivered { cell, host }
    }

    /// Seal a live [`Witnessed`] carrier into a delivery envelope pinned by `host` — the resident
    /// seal veneer's library half. Bundles the born-witnessed carrier with the region owner the
    /// caller already holds, so a resident value travels as an envelope pinned by its home frame,
    /// identical in shape to a delivered dep.
    pub fn seal(witnessed: Witnessed<T, W>, host: Rc<F>) -> Self {
        Delivered {
            cell: Sealed::seal(witnessed),
            host,
        }
    }

    /// Read the delivered value at a **rank-2** (`for<'b>`) brand, pinned by the retained frame
    /// owner ([`Sealed::open_with`]) — the single read verb for a delivered value, whose carrier
    /// witness bundles no pin of its own. The `for<'b>` quantifier confines the re-anchored value
    /// exactly as [`Sealed::open`] does.
    pub fn open<R>(&self, f: impl for<'b> FnOnce(T::At<'b>) -> R) -> R
    where
        T::At<'static>: Copy,
    {
        self.cell.open_with(&self.host, f)
    }

    /// The reference-only reach witness — the value's reach description, for a reach query or a
    /// mint. Freely passable (a bit-copy / reference-copy); it keeps nothing alive on its own.
    pub fn witness(&self) -> &W {
        self.cell.witness()
    }

    /// The retained frame owner pinning this delivery in transit.
    pub fn host(&self) -> &Rc<F> {
        &self.host
    }

    /// The dormant carrier cell — for a consumer that reads the erased inner (a `SealedExtern`
    /// zip) or threads the seal onward while the envelope keeps covering it.
    pub fn cell(&self) -> &Sealed<T, W> {
        &self.cell
    }

    /// Recover the dormant carrier, consuming the envelope and dropping the retained pin — for a
    /// consumer that re-homes the value under its own liveness and no longer needs the transit host
    /// (the single-part pass-through's `unseal`).
    pub fn into_cell(self) -> Sealed<T, W> {
        self.cell
    }

    /// Duplicate the envelope: [`duplicate`](Sealed::duplicate) the sealed carrier (bit-copy value +
    /// witness clone) and clone the host `Rc`, leaving the source intact — the producer keeps its
    /// terminal for other consumers, and the retained hold gains one `Rc` clone (dropped at
    /// adoption).
    pub fn duplicate(&self) -> Self
    where
        Erased<T>: Copy,
        W: Clone,
    {
        Delivered {
            cell: self.cell.duplicate(),
            host: Rc::clone(&self.host),
        }
    }
}

/// The envelope-bearing verbs over the reference-only [`Carrier`] witness — the **only** places a
/// value's residence host materializes as a member of a minted set, because only the envelope has
/// the true owner in hand. Everything here reads the carrier's reach under the retained host pin.
impl<T: Reattachable, F: PinsRegion + 'static> Delivered<T, Carrier<F>, F> {
    /// Collapse to a plain [`RegionSet`] naming every region this delivery keeps alive — the
    /// retained host ∪ the carrier's reach members. The step-open liveness pin a consumer folds
    /// when it has no arena to host-mint against (the run-loop step pin, the head-deferred
    /// callable's reach).
    pub fn liveness_frameset(&self) -> RegionSet<F> {
        let mut set = RegionSet::singleton(Rc::clone(&self.host));
        self.witness().with_reach(Some(&self.host), |reach| {
            if let Some(reach) = reach {
                set = RegionSet::union(&set, reach);
            }
        });
        set
    }

    /// Mint this value's reach into `dest`, materializing the retained host per `mode`
    /// ([`Residence::Kept`]: unconditionally — the value keeps living there; [`Residence::Copied`]:
    /// only when its borrows genuinely reach it, the `borrows_host` bit), under `omit` — the
    /// embedder's omission policy (regions the destination's container already pins). Returns the
    /// minted set (`None` == empty, no allocation) and the borrows-into-dest bit — the pieces a
    /// binding entry stores. The reach read runs under the retained host pin.
    pub fn mint_reach<'d, P>(
        &self,
        dest: RegionHandle<'d, P>,
        mode: Residence,
        omit: impl Fn(&Region<P>) -> bool,
    ) -> (Option<&'d RegionSet<F>>, bool)
    where
        P: StorageProfile + 'static,
        F: RegionOwner<Region = Region<P>>,
        RegionSet<F>: Stored<P> + for<'r> Reattachable<At<'r> = RegionSet<F>>,
    {
        self.witness().mint_into(dest, Some(&self.host), mode, omit)
    }

    /// Copy-free adoption: mints this envelope's reach — residence host
    /// materialized (`Residence::Kept`) — into `dest`'s region, then re-anchors
    /// the sealed value at `dest`'s lifetime. Fused so the re-anchor cannot be
    /// reached without the mint that pins it: the minted set lives in `dest`'s
    /// arena for the region's life, so every region the value reaches (its home
    /// included) outlives the returned borrow. `omit` names regions the caller's
    /// context covers ambiently, as in [`Self::mint_reach`].
    pub fn adopt_into<'d, P>(
        &self,
        dest: RegionHandle<'d, P>,
        omit: impl Fn(&Region<P>) -> bool,
    ) -> T::At<'d>
    where
        P: StorageProfile + 'static,
        F: RegionOwner<Region = Region<P>>,
        RegionSet<F>: Stored<P> + for<'r> Reattachable<At<'r> = RegionSet<F>>,
        T::At<'static>: Copy,
    {
        let _ = self.mint_reach(dest, Residence::Kept, omit);
        let erased: Erased<T> = self.open(Erased::<T>::erase);
        // SAFETY: the mint above stored this carrier's reach — residence host
        // materialized as a member — into `dest`'s arena, held for the region's
        // life ⊇ 'd; the re-anchored borrow cannot outlive its pin.
        unsafe { erased.reattach() }
    }

    /// Relocate the delivered value into a destination and re-seal it under the composed carrier
    /// that names everything it reaches from there — the envelope-bearing form of the witnessed
    /// transfer, and the only relocation verb for a carrier-witnessed value. The sealed carrier is
    /// duplicated (the envelope keeps its cell for other consumers), re-anchored at a shared
    /// `for<'b>` brand with `dest`'s live form under the retained host pin, and handed to
    /// `relocate` — the workload's structural copy/fold, which builds into `dest` at the brand
    /// natively. The composed witness mints both operands' reach into `dest`'s own arena and
    /// materializes the retained host per `mode` — birth-frame-as-member happens exactly here.
    pub fn transfer_into<B: Reattachable, P: Reattachable, Pr>(
        &self,
        dest: Witnessed<B, Carrier<F>>,
        mode: Residence,
        relocate: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldToken<'b>) -> P::At<'b>,
    ) -> Witnessed<P, Carrier<F>>
    where
        Pr: StorageProfile + 'static,
        F: RegionOwner<Region = Region<Pr>>,
        for<'b> B::At<'b>: HasRegionHandle<'b, Pr>,
        RegionSet<F>: Stored<Pr> + for<'r> Reattachable<At<'r> = RegionSet<F>>,
        T::At<'static>: Copy,
    {
        let host = &self.host;
        self.cell.duplicate().unseal().merge_composed(
            dest,
            host,
            |left, right, live_dest| {
                Carrier::compose_into(left, right, live_dest.region_handle(), Some(host), mode)
            },
            relocate,
        )
    }
}
