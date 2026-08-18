//! [`Delivered<T, W, F>`] — the **delivery envelope**: a sealed carrier paired with the frame-owner
//! `Rc` that retains its value's backing *in transit*, from a scheduler pull to the point a consumer
//! adopts or re-homes it. See
//! [design/reach.md § Retention model](../../design/reach.md#retention-model).
//!
//! Liveness *at rest* is the scheduler's retention table (a producer frame stays held while any
//! consumer edge is undischarged). Liveness *in transit* — from a pull to its adoption — is this
//! envelope: it bundles the dormant [`Sealed`] carrier with one owned [`PinBundle`] holding an
//! `Rc<F>` for every region the value reaches, **its home among them**, so a consumer reads the
//! value under a pin it does not have to thread separately. The bundle is private and every
//! constructor is a surface that has the true home owner in hand, so an envelope whose pins
//! disagree with its payload is not constructible.
//!
//! The envelope records no residence of its own: the value's home is the host of the reach
//! description its carrier references, so a residence question is asked of the payload — through
//! [`Opened::with_home_region`] on an open, or internally by
//! [`Delivered::coverage_releasing_home`] — and cannot drift from what the value records about
//! itself. Home riding the pins as an ordinary member is what lets the
//! envelope-bearing mint verbs — [`Delivered::adopt_into`] and [`Delivered::transfer_into`] — fold a
//! producer frame into a minted destination description with no separate materialization arm: the
//! home pin is already in the bundle the composition folds. A relocation site chooses nothing about
//! that bundle: [`Delivered::transfer_into`] *derives* the source claim by running the site's retention
//! predicate over the folded product against each pinned region in turn, so what the product still
//! reaches is a checked property of the bytes rather than a promise made before they existed. See
//! [design/reach.md § Composition](../../design/reach.md#composition-minting-a-description-and-retaining-its-pins).
//!
//! The fusion holds on the way out too. The public exits to a bare [`Sealed`] —
//! [`Delivered::rest_in`] and [`Delivered::rest_into`] — both take the destination and lodge the
//! coverage there, so a cell is obtainable only once its pins are somewhere durable; and a holder
//! that needs the pair *before* it knows its home takes [`Delivered::unhost`], which drops only the
//! home pin and keeps the rest fused as an [`Unhosted`] whose sole exit pins a home back on.
//!
//! [`Delivered::adopt_into`] fuses the mint with the re-anchor it justifies into one copy-free
//! adoption verb, so a caller cannot split the pin from the reattach it pins — and the mint itself
//! establishes the destination's retention ([`ReachDescription::mint_resident`]), so there is no
//! moment at which anyone holds the adopted value's pins loose.

use std::rc::Rc;

use smallvec::SmallVec;

use super::{
    Carrier, DropFree, Erased, FoldToken, FoldedPlacement, HasRegionHandle, Opened, PinBundle,
    PinsRegion, ReachDescription, Reattachable, Region, RegionHandle, RegionHandleFamily,
    RegionOwner, Retained, Sealed, SealedExtern, StepCoverage, StorageProfile, Witnessed,
};

/// Inline capacity for an N-ary relocation's staging buffers — the run of erased source values,
/// the envelopes beside it, and their pin bundles. Sized to a typical aggregate arity (a record's
/// fields, a small list literal, a step's deps), so those sites stage entirely on the stack and a
/// wider run pays one heap growth per buffer rather than one per element.
const STAGED_INLINE: usize = 8;

/// A sealed carrier paired with the owned `PinBundle` that pins every region its value reaches —
/// the value's home region among them, as an ordinary member. `T` is the carrier's value family,
/// `W` its reach witness, `F` the workload's frame-owner type. The carrier's reach description is
/// non-owning; the envelope's bundle is the ownership that keeps the value's whole reach alive
/// across transit — from a scheduler pull to the point a consumer adopts or re-homes it.
pub struct Delivered<T: Reattachable + DropFree, W, F: PinsRegion> {
    /// The dormant carrier — value, residence and reach description as one unit.
    cell: Retained<T, W>,
    /// The owned pins for every region the value's borrows reach, **home included** — the ownership
    /// counterpart of the carrier's non-owning reach description, composed at construction from the
    /// home owner the constructor is handed and the reach bundle threaded in with it. It is what
    /// keeps those regions alive while the envelope sits parked in a scheduler slot, where the
    /// description's `Weak` members alone would not, and what covers the reads
    /// ([`open`](Self::open) / [`open_at`](Self::open_at)) the envelope serves.
    /// [`duplicate`](Self::duplicate) clones it, so every fan-out consumer holds its own pins.
    pins: StepCoverage<F>,
}

impl<T: Reattachable + DropFree, W, F: PinsRegion> Delivered<T, W, F> {
    /// Read the delivered value at a **rank-2** (`for<'b>`) brand, pinned by the envelope's own
    /// owned pins ([`Sealed::open_with`]) — the single read verb for a delivered value, whose
    /// carrier witness bundles no pin of its own. The `for<'b>` quantifier confines the re-anchored
    /// value exactly as [`Sealed::open`] does.
    pub fn open<R>(&self, f: impl for<'b> FnOnce(T::At<'b>) -> R) -> R
    where
        T::At<'static>: Copy,
    {
        self.cell.open_with(&self.pins, f)
    }

    /// [`Self::open`] handing `f` the re-anchored value **by reference**, for a value family whose
    /// views are not `Copy` — pinned by the envelope's own owned pins, exactly as `open` is. The
    /// by-ref read for a delivered value; like every read here it takes no pin parameter.
    pub fn open_ref<R>(&self, f: impl for<'b> FnOnce(&'b T::At<'b>) -> R) -> R {
        self.cell.open_ref_with(&self.pins, f)
    }

    /// Open the delivered value into the **in-use** [`Opened`] state at the step lifetime `'b`,
    /// pinned by the envelope's own owned pins — the one-line convenience over
    /// [`Sealed::open_at`] that supplies its own owned coverage (the envelope already holds the pins
    /// its value's backing needs). Returns an [`Opened<'b, T, W>`] the step reads freely and
    /// [`reseal`](Opened::reseal)s or lifts at step end.
    pub fn open_at<'b>(&'b self) -> Opened<'b, T, W>
    where
        T::At<'static>: Copy,
        W: Clone,
    {
        self.cell.open_at_with(&self.pins)
    }

    /// The reference-only reach witness — the value's reach description, for a reach query or a
    /// mint. Freely passable (a bit-copy / reference-copy); it keeps nothing alive on its own.
    pub fn witness(&self) -> &W {
        self.cell.witness()
    }

    /// The owned coverage pinning every region the value reaches, home included — the ownership
    /// counterpart of the carrier's non-owning reach description, and what every read of this
    /// envelope runs under. Borrowed, not cloned: a mint reads it as a source without paying an
    /// `Rc` bump per member.
    pub fn coverage(&self) -> &StepCoverage<F> {
        &self.pins
    }

    /// The envelope's own pins, for the crate-internal composition engine.
    pub(crate) fn pins(&self) -> &PinBundle<F> {
        &self.pins.0
    }

    /// The coverage this envelope's value **still claims** after a relocation, derived by running
    /// the workload's retention predicate over each pinned region in turn — the standalone form of
    /// the claim [`transfer_into`](Self::transfer_into) derives internally, for a site whose copy
    /// is not built by a fold (a top-node clone at a bind seam). A region the predicate answers
    /// `false` for is dropped, so its owner frees at retention discharge.
    pub fn coverage_retaining(&self, keep: impl FnMut(&F::Region) -> bool) -> StepCoverage<F> {
        StepCoverage(self.pins.0.retaining(keep))
    }

    /// Drop the **home pin** and keep the pair: the envelope's carrier and its whole coverage as an
    /// [`Unhosted`] — the state for a holder that has a value in hand before it knows which frame
    /// will own it (an embedder's step-scoped brand, whose host is the finalizing node's anchor
    /// owner). The pair stays fused: the only way back out is [`Unhosted::host`], which re-pins a
    /// home and hands back an envelope.
    pub fn unhost(self) -> Unhosted<T, W, F> {
        Unhosted {
            cell: self.cell,
            pins: self.pins,
        }
    }

    /// Recover the dormant carrier, consuming the envelope and dropping its coverage — the
    /// **shape probe** the witnessed slates read a built envelope's cell back out through. Not a
    /// production door: it drops the pins without saying where the retention went, so the exits a
    /// holder actually takes are [`rest_in`](Self::rest_in) / [`rest_into`](Self::rest_into), which
    /// lodge the coverage in the region they name.
    #[cfg(test)]
    pub(crate) fn into_cell(self) -> Retained<T, W> {
        self.cell
    }

    /// The carrier's erased inner re-sealed **witness-less**, into the externally-witnessed tier —
    /// for a holder that zips this value with other carriers under one presented pin (an embedder's
    /// step open). The reach witness is dropped, so the returned [`SealedExtern`] carries no
    /// evidence of its own: opening it takes a pin at the call, and *that* pin covering this
    /// value's home is the caller's obligation, checked by nothing (the externally-witnessed tier's
    /// standing prose contract). The envelope is borrowed, so a holder that keeps it — a frame that
    /// owns the envelope it is re-sealing from — is itself the coverage.
    pub fn to_extern(&self) -> SealedExtern<T>
    where
        Erased<T>: Copy,
    {
        SealedExtern::seal(*self.cell.erased())
    }

    /// Drop the delivered value **to rest** in `dest`'s region (`Delivered → Sealed`): duplicate the
    /// dormant carrier and lodge this envelope's whole coverage — the value's home among its
    /// members — in `dest`'s union bundle, retained for that region's life. The returned [`Sealed`]
    /// is pure data: `Copy` and `Drop`-free whenever its family and witness are, so it rests inside
    /// an embedder's own `Copy` value while the pins that keep its pointee alive live one level
    /// down, in the region.
    ///
    /// This and [`rest_into`](Self::rest_into) are the whole public exit from an envelope to a bare
    /// cell, and both take the destination: a caller that received the cell and the coverage
    /// separately could store the cell and drop the pins, and every later read of it would dangle.
    /// Here the only way to obtain the cell is to have already lodged its coverage, so any read
    /// under a hold on `dest`'s region — [`Sealed::open_with`], or an [`Opened`] at `dest`'s own
    /// lifetime — is covered by construction, for as long as that region lives.
    ///
    /// Distinct from [`adopt_into`](Self::adopt_into), which *mints* the value's reach into `dest`
    /// and re-anchors the value at `dest`'s own lifetime: nothing is minted here and the value keeps
    /// referencing the description its producer stamped, so this is the door for a cell whose reach
    /// a later composition reads rather than re-homes. The envelope is borrowed, not consumed — a
    /// producer's value fans out to several resting cells, each duplicate taking its own `Rc` on
    /// every pinned region.
    ///
    /// Retention widens to the region's life: what an envelope released when it dropped is now held
    /// until `dest`'s region dies. That is the price of a `Drop`-free resting cell. A value already
    /// resident in `dest`'s own region rests for free: [`RegionHandle::retain_reach`]'s self rule
    /// strips that region from the bundle, so the coverage may be handed over whole without the
    /// caller first asking where the value lives.
    pub fn rest_in<'d, P>(&self, dest: RegionHandle<'d, P>) -> Sealed<'d, T, W>
    where
        P: StorageProfile<FrameOwner = F>,
        F: RegionOwner<Region = Region<P>>,
        Erased<T>: Copy,
        W: Clone,
    {
        self.duplicate().rest_into(dest)
    }

    /// [`rest_in`](Self::rest_in) **consuming** the envelope instead of duplicating it — the drop to
    /// rest for a holder that is done with the value in transit, which is every relocation product:
    /// a composition mints its product's reach into `dest`'s own side table and retains it there, so
    /// resting one lodges a bundle the region already holds and the envelope's transit copy has
    /// nothing left to do. Free of [`rest_in`](Self::rest_in)'s `Copy`/`Clone` bounds, since nothing
    /// is duplicated.
    pub fn rest_into<'d, P>(self, dest: RegionHandle<'d, P>) -> Sealed<'d, T, W>
    where
        P: StorageProfile<FrameOwner = F>,
        F: RegionOwner<Region = Region<P>>,
    {
        let Delivered { cell, pins } = self;
        dest.retain_reach(pins);
        cell.brand_to(dest)
    }

    /// Re-family the delivered value **in place**: re-anchor it under the envelope's own pins at a
    /// `for<'b>` brand, project it with `f`, and re-erase. Nothing moves and nothing is minted — the
    /// envelope keeps its residence, its coverage and its witness, which stay correct because `f`
    /// selects a part *of* the value the envelope already covers (the callable a value wraps, a
    /// variant's payload) and so can reach nothing the whole did not. The witness may therefore
    /// over-state the projection's reach; it never under-states it.
    ///
    /// This is how a family-specific carrier is reached without splitting the value from its pins:
    /// a projection that went out through a bare read would arrive somewhere else with no proven
    /// reach at all.
    pub fn project<P: Reattachable + DropFree>(
        self,
        f: impl for<'b> FnOnce(T::At<'b>, FoldToken<'b>) -> P::At<'b>,
    ) -> Delivered<P, W, F> {
        let Delivered { cell, pins } = self;
        // The envelope's own pins cover the re-anchor for the whole call — the same coverage every
        // read of this envelope runs under.
        let projected = cell.into_retained_inner().map_pinned(&pins, f);
        Delivered {
            cell: Retained::from_witnessed(projected),
            pins,
        }
    }

    /// Duplicate the envelope: [`duplicate`](Sealed::duplicate) the sealed carrier (bit-copy value +
    /// witness clone) and clone the owned `PinBundle`, leaving the source intact — the producer
    /// keeps its terminal for other consumers, and every pinned region (home among them) gains one
    /// `Rc` clone, dropped when this duplicate's consumer is done.
    pub fn duplicate(&self) -> Self
    where
        Erased<T>: Copy,
        W: Clone,
    {
        Delivered {
            cell: self.cell.duplicate(),
            pins: self.pins.clone(),
        }
    }
}

/// The **destination-operand** constructor, on the envelope family whose payload is a bare region
/// handle rather than a value.
impl<P: StorageProfile + 'static>
    Delivered<RegionHandleFamily<P>, Carrier<P::FrameOwner>, P::FrameOwner>
where
    P::FrameOwner: RegionOwner<Region = Region<P>>,
{
    /// **The destination operand for a relocation**: `frame`'s own region handle, resident in that
    /// same region and sealed into the envelope the composition verbs take — its residence is the
    /// frame itself, so the product a composition builds inherits it. Co-located by construction:
    /// the handle, the residence, and the home pin all come off the one owner, so there is nothing
    /// for a caller to pair wrongly. A bare handle reaches nothing beyond its own region, so the
    /// operand's coverage is its home alone.
    ///
    /// The one door to the bare-handle destination: the scheduler's delivery walk builds its
    /// per-destination operand here, and an embedder relocating toward a frame it holds builds the
    /// same operand the same way.
    pub fn destination(frame: Rc<P::FrameOwner>) -> Self {
        let handle = RegionHandle::from_owner(&*frame);
        handle.deliver_resident::<RegionHandleFamily<P>>(handle)
    }
}

/// The envelope-bearing verbs over the reference-only [`Carrier`] witness. The envelope is the
/// holder that owns the value's pins, so it is what a mint folds and what covers every read here.
impl<T: Reattachable + DropFree, F: PinsRegion + 'static> Delivered<T, Carrier<F>, F> {
    /// Pair a sealed carrier with the owner of the region its value lives in and the owned
    /// `PinBundle` pinning every other region it reaches, unioning the two into the envelope's
    /// single member set. `home` is the pin the transit needs — the residence itself is already the
    /// host of the carrier's description.
    ///
    /// **Crate-private**, because it takes the two as separate arguments and checks neither against
    /// the other. Every caller holds them as one unit already: an edge's resident plus the
    /// destination host the slab reads back for it, a composition's freshly minted product
    /// and the destination residence it was minted into, or [`RegionHandle::deliver_resident`] /
    /// [`RegionHandle::deliver_yoked`], which read both off one handle. An embedder reaches the
    /// envelope through those, never through this.
    pub(crate) fn hosted(
        cell: Retained<T, Carrier<F>>,
        home: Rc<F>,
        reach: StepCoverage<F>,
    ) -> Self {
        let pins = StepCoverage(PinBundle::union(&PinBundle::singleton(home), &reach.0));
        Delivered { cell, pins }
    }

    /// The owner of the region the value **lives in**, read off its carrier's reach description
    /// (whose host the mint stamped) under the envelope's own pins. Crate-internal: it hands out an
    /// owned pin, the ownership tier an embedder has no vocabulary for — the relocation verbs use it
    /// to give their product the destination's own residence.
    fn home_owner(&self) -> Rc<F> {
        // The envelope's pins cover its value's home region for the envelope's whole life, so the
        // description (hosted in that same region's side table) is readable and its host upgrades.
        self.cell.witness().home_owner()
    }

    /// This envelope's coverage with its **own residence** dropped — for a holder that already owns
    /// the home region by another route and would otherwise take a second `Rc` on it (the retention
    /// hold, whose `owner` field *is* the home frame: re-listing it there is a pin on the very
    /// frame the hold's release frees).
    pub fn coverage_releasing_home(&self) -> StepCoverage<F>
    where
        F: RegionOwner,
    {
        let home = self.home_owner();
        StepCoverage(self.pins.0.without_region(RegionOwner::region(&*home)))
    }

    /// **Lift** a [`Sealed`] carrier at rest into a delivery envelope in transit (`Sealed →
    /// Delivered`): upgrade the sealed carrier's reach description `Weak → Rc` into an owned inline
    /// `PinBundle` under `home`, and union `home` itself in. The owned set is what lets the value
    /// travel after its source frame dies — an arena-hosted `&ReachDescription` would dangle in
    /// transit, so the lift re-owns the claimed subset while the reached regions are still covered
    /// (the holder rule under `home`). `home` is the value's residence owner, covering both its
    /// backing and its description's hosting arena; it is unioned in explicitly because the self
    /// rule strips it from the bundle a mint into that same region hands back.
    pub fn lift(cell: Retained<T, Carrier<F>>, home: Rc<F>) -> Self {
        let reach = cell.witness().upgrade_bundle(&home);
        let pins = StepCoverage(PinBundle::union(&PinBundle::singleton(home), &reach));
        Delivered { cell, pins }
    }

    /// [`Self::hosted`] taking a live [`Witnessed`] carrier rather than a sealed one — it seals on
    /// the way in. Crate-private for the same reason, and reached from the same places: a
    /// destination operand yoked over its own frame, and the handle doors an embedder uses.
    pub(crate) fn seal(
        witnessed: Witnessed<T, Carrier<F>>,
        home: Rc<F>,
        reach: StepCoverage<F>,
    ) -> Self {
        let pins = StepCoverage(PinBundle::union(&PinBundle::singleton(home), &reach.0));
        Delivered {
            cell: Retained::from_witnessed(witnessed),
            pins,
        }
    }

    /// [`Self::seal`] handed out for white-box assertion — for a suite that needs an envelope whose
    /// owned coverage is *exactly* what it names, with no mint and no composition standing behind
    /// it. That is what lets a test make the envelope's bundle the sole owner of a region and watch
    /// the region die with it; every production envelope's coverage comes from a mint, a lift or a
    /// composition, each of which retains what it names somewhere else too.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn seal_for_test(
        witnessed: Witnessed<T, Carrier<F>>,
        home: Rc<F>,
        reach: StepCoverage<F>,
    ) -> Self {
        Self::seal(witnessed, home, reach)
    }

    /// Copy-free adoption: mints this envelope's reach — home included, as an ordinary member —
    /// into `dest`'s region, which is the same act that **retains** it there for the region's life,
    /// then re-anchors the sealed value at `dest`'s lifetime. Fused so the re-anchor cannot be
    /// reached without the pin that keeps it live: the minted description names the value's
    /// reach and the region's retention owns it for the region's life ⊇ `'d`, so every
    /// region the value reaches (its home included) outlives the returned borrow.
    pub fn adopt_into<'d, P>(&self, dest: RegionHandle<'d, P>) -> T::At<'d>
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
        T::At<'static>: Copy,
    {
        self.open_adopted(dest).into_value()
    }

    /// Adopt the delivered value into `dest` and hand it back in the **in-use** [`Opened`] state at
    /// the destination region's own lifetime `'d` — [`Self::adopt_into`]'s carrier-bearing form,
    /// for a consumer that reads the value across a step at `'d` and re-seals it
    /// ([`Opened::reseal`], which reproduces exactly [`Self::adopt`]'s seal) when it escapes onward.
    ///
    /// Where [`Sealed::open_at`] takes its `'b` from a borrowed pin, this open takes it from the
    /// destination region: the mint stores the value's reach into `dest`'s side table and retains a
    /// bundle owning it for the region's life ⊇ `'d`, so the coverage justifying the open outlives
    /// every use of it. That is what lets an adopted value ride a step-lifetime type position no
    /// rank-2 read and no pin borrow can reach.
    pub fn open_adopted<'d, P>(&self, dest: RegionHandle<'d, P>) -> Opened<'d, T, Carrier<F>>
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
        T::At<'static>: Copy,
    {
        // The envelope's own pins are the source the composition folds, so the value's home rides in
        // as an ordinary member. The description is non-owning and the adopted value lives for the
        // region's life, so the mint's own retention (dropped only at region death) is what provides
        // the liveness the description cannot.
        let minted = ReachDescription::mint_resident(dest, &[self.pins()]);
        let erased: Erased<T> = self.open(Erased::<T>::erase);
        Opened::adopted(
            // SAFETY: the mint above stored this carrier's reach into `dest`'s side table and
            // retained a bundle pinning every region it names for the region's life ⊇ 'd; the
            // re-anchored borrow cannot outlive its pin.
            unsafe { erased.reattach::<'d>() },
            Carrier::new(minted),
        )
    }

    /// [`Self::transfer_into`] handing `relocate` a bare [`FoldToken`] and the destination's live
    /// form instead of a [`FoldedPlacement`] over its handle — the raw fold the public door adapts.
    /// Crate-internal: an embedder that reached this would have to pair the destination handle with
    /// the brand by hand, which is exactly the pairing the placement exists to make structural.
    pub(in crate::witnessed) fn transfer_into_token<
        B: Reattachable + DropFree,
        P: Reattachable + DropFree,
        Pr,
    >(
        &self,
        dest: Delivered<B, Carrier<F>, F>,
        still_borrows: impl for<'b> FnMut(&P::At<'b>, &F::Region) -> bool,
        relocate: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldToken<'b>) -> P::At<'b>,
    ) -> Delivered<P, Carrier<F>, F>
    where
        Pr: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<Pr>>,
        for<'b> B::At<'b>: HasRegionHandle<'b, Pr>,
        T::At<'static>: Copy,
    {
        let dest_home = dest.home_owner();
        // The destination contributes its *foreign* reach alone. Its own residence is stripped: the
        // mint targets that same region, so listing it as a source would make every product claim to
        // borrow into the region it merely lives in.
        let dest_reach = dest.pins().without_region(RegionOwner::region(&*dest_home));
        // The re-anchor runs under the envelope's own pins, whatever the predicate later claims the
        // product reaches: reading the source value is always covered here.
        let (product, bundle) = self.cell.duplicate().into_retained_inner().merge_composed(
            dest.cell.into_retained_inner(),
            self.pins(),
            relocate_then_compose::<T, B, P, F, Pr>(
                self.pins(),
                &dest_reach,
                still_borrows,
                relocate,
            ),
        );
        // The product lives in `dest`'s region, so that is its residence; the composed bundle is
        // its reach with `dest`'s own region stripped by the self rule, which `hosted` unions back
        // as the transit pin.
        Delivered::hosted(
            Retained::from_witnessed(product),
            dest_home,
            StepCoverage(bundle),
        )
    }

    /// Relocate the delivered value into a destination and re-seal it under the composed carrier
    /// that names everything it reaches from there — the envelope-bearing form of the witnessed
    /// transfer, and the only relocation verb for a carrier-witnessed value. The sealed carrier is
    /// duplicated (the envelope keeps its cell for other consumers), re-anchored at a shared
    /// `for<'b>` brand with `dest`'s live form under the envelope's own pins, and handed to
    /// `relocate` — the workload's structural copy/fold, which builds into `dest` at the brand
    /// natively. The composed witness mints both operands' reach into `dest`'s own arena.
    ///
    /// `relocate` receives a [`FoldedPlacement`] over the destination operand's own handle, minted
    /// over exactly the handle [`Carrier::compose_into`] mints the composed reach set over, so the
    /// folded store rides the same confinement the composition establishes — the destination is the
    /// engine's own operand region, never a caller-captured handle.
    ///
    /// What the relocated product still reaches *from the source side* is **derived, not accepted**:
    /// once `relocate` has built the product, `still_borrows` is run over it against each region
    /// this envelope pins, and the source claim is the members it answers `true` for (design §
    /// Escape). A `false` verdict drops that region from the composed bundle, so its owner frees at
    /// retention discharge — the tail-loop turnover rule; a `true` verdict keeps it, so the producer
    /// transfers by hold. The claim is therefore a checked property of the bytes that exist. A
    /// predicate that answers conservatively costs retention, never soundness.
    pub fn transfer_into<B: Reattachable + DropFree, P: Reattachable + DropFree, Pr>(
        &self,
        dest: Delivered<B, Carrier<F>, F>,
        still_borrows: impl for<'b> FnMut(&P::At<'b>, &F::Region) -> bool,
        relocate: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldedPlacement<'b, Pr>) -> P::At<'b>,
    ) -> Delivered<P, Carrier<F>, F>
    where
        Pr: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<Pr>>,
        for<'b> B::At<'b>: HasRegionHandle<'b, Pr>,
        T::At<'static>: Copy,
    {
        self.transfer_into_token::<B, P, Pr>(
            dest,
            still_borrows,
            super::place_over_dest::<T, B, P, Pr>(relocate),
        )
    }

    /// The engine under [`Self::transfer_all_into`]: the same relocation over an
    /// [`ExactSizeIterator`] of source envelopes instead of a slice of them. Crate-internal so a
    /// library caller whose run is already a slice of *borrows* ([`StepContext::alloc_with`]'s dep
    /// list) feeds it `iter().copied()` rather than gathering a second slice purely to match the
    /// public currency — the one adapter allocation the door exists to avoid.
    ///
    /// [`StepContext::alloc_with`]: super::StepContext::alloc_with
    pub(in crate::witnessed) fn transfer_each_into<'s, B, P, C, Pr>(
        sources: impl ExactSizeIterator<Item = &'s Self>,
        dest: Delivered<B, Carrier<F>, F>,
        still_borrows: impl for<'b> FnMut(&Self, &C::At<'b>, &F::Region) -> bool,
        relocate: impl for<'b> FnOnce(
            &'b [T::At<'b>],
            B::At<'b>,
            FoldedPlacement<'b, Pr>,
        ) -> (P::At<'b>, &'b [C::At<'b>]),
    ) -> Delivered<P, Carrier<F>, F>
    where
        B: Reattachable + DropFree,
        P: Reattachable + DropFree,
        C: Reattachable,
        Pr: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<Pr>>,
        for<'b> B::At<'b>: HasRegionHandle<'b, Pr>,
        T::At<'static>: Copy,
        Self: 's,
    {
        let dest_home = dest.home_owner();
        // The self rule, as in `transfer_into`: the destination contributes its foreign reach alone,
        // never the residence the mint is about to stamp as the product's own.
        let dest_reach = dest.pins().without_region(RegionOwner::region(&*dest_home));
        // Stage the sources' erased forms into one run, keeping the envelopes beside it so the
        // retention pass can name each source itself rather than an index into the run. Copying an
        // already-erased value fabricates no lifetime — the run is inert until the staged merge
        // re-anchors it, under the very envelopes borrowed here.
        let capacity = sources.len();
        let mut envelopes: SmallVec<[&'s Self; STAGED_INLINE]> = SmallVec::with_capacity(capacity);
        let mut staged: SmallVec<[T::At<'static>; STAGED_INLINE]> =
            SmallVec::with_capacity(capacity);
        for source in sources {
            staged.push(*source.cell.erased().as_static());
            envelopes.push(source);
        }
        // The pin the staged re-anchor runs under has to cover every source's backing at once, and
        // the borrowed slice of their bundles already is that coverage — it is itself the `Witness`,
        // so nothing is unioned to present one. The claims those bundles carry are filtered
        // individually inside the fold.
        let source_pins: SmallVec<[&PinBundle<F>; STAGED_INLINE]> =
            envelopes.iter().map(|envelope| envelope.pins()).collect();
        let (product, bundle) = dest.cell.into_retained_inner().merge_staged_composed(
            &staged,
            &source_pins[..],
            relocate_run_then_compose::<T, B, P, C, F, Pr>(
                &envelopes,
                &source_pins,
                &dest_reach,
                still_borrows,
                relocate,
            ),
        );
        Delivered::hosted(
            Retained::from_witnessed(product),
            dest_home,
            StepCoverage(bundle),
        )
    }

    /// **Relocate N sources into one destination in a single act** — the N-ary
    /// [`Self::transfer_into`], for a site building one aggregate out of many delivered values.
    /// `relocate` receives every source's live form as one slice at the shared brand, so it stores
    /// the whole run through the placement *once*.
    ///
    /// This is the door an aggregate build takes, and the reason it exists is asymptotic. Folding
    /// the same N sources pairwise makes each step's product the next step's destination, so the
    /// accumulator has to carry the run gathered so far — and it can only carry it as region-bumped
    /// bytes, since the destination family rests glue-free between steps ([`DropFree`]) and each
    /// step's brand is fresh, so no buffer named outside a step can receive a value built inside
    /// one. The run is then re-bumped per step: quadratic in region bytes, none of them reclaimable
    /// before the frame dies. Staging the sources and re-anchoring them together instead gathers the
    /// run once and builds the product in one pass, so both are linear in N.
    ///
    /// The retention rule is [`Self::transfer_into`]'s, asked **per source**: `relocate` returns the
    /// product together with the run of cells it produced, and this door zips that run with the
    /// source envelopes to ask `still_borrows(source, its own cell, region)` against each region
    /// that source pins. Asking per source is what preserves the pairwise door's answer rather than
    /// approximating it — an embedder's predicate reads a region against the source that pinned it,
    /// so a source asked about a region it never named would answer for the wrong claim and retain
    /// everything. The surviving members compose to one antichain, the source side of the mint.
    ///
    /// **Staging-order contract.** `relocate` returns its cells in staging order: `cells[i]` is the
    /// product cell built from `sources[i]`. That is the whole of what the door trusts a relocate
    /// hook for — the pairing it derives from it is handed to `still_borrows` as a (source, cell)
    /// pair, so no embedder-facing signature carries an index into a run it would have to trust. A
    /// debug assert checks the run lengths agree.
    pub fn transfer_all_into<B, P, C, Pr>(
        sources: &[Self],
        dest: Delivered<B, Carrier<F>, F>,
        still_borrows: impl for<'b> FnMut(&Self, &C::At<'b>, &F::Region) -> bool,
        relocate: impl for<'b> FnOnce(
            &'b [T::At<'b>],
            B::At<'b>,
            FoldedPlacement<'b, Pr>,
        ) -> (P::At<'b>, &'b [C::At<'b>]),
    ) -> Delivered<P, Carrier<F>, F>
    where
        B: Reattachable + DropFree,
        P: Reattachable + DropFree,
        C: Reattachable,
        Pr: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<Pr>>,
        for<'b> B::At<'b>: HasRegionHandle<'b, Pr>,
        T::At<'static>: Copy,
    {
        Self::transfer_each_into::<B, P, C, Pr>(sources.iter(), dest, still_borrows, relocate)
    }

    /// [`Self::merge_into`] handing `f` a bare [`FoldToken`] instead of a [`FoldedPlacement`] over
    /// the destination operand's handle — the raw fold the public door adapts, crate-internal for
    /// the same reason [`Self::transfer_into_token`] is.
    pub(in crate::witnessed) fn merge_into_token<B, P, Pr>(
        self,
        other: Delivered<B, Carrier<F>, F>,
        f: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldToken<'b>) -> P::At<'b>,
    ) -> Delivered<P, Carrier<F>, F>
    where
        B: Reattachable + DropFree,
        P: Reattachable + DropFree,
        Pr: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<Pr>>,
        for<'b> B::At<'b>: HasRegionHandle<'b, Pr>,
    {
        let dest_home = other.home_owner();
        let Delivered {
            cell: left,
            pins: left_pins,
        } = self;
        let Delivered {
            cell: right,
            pins: right_pins,
        } = other;
        let left_pins = left_pins.0;
        let mut right_pins = right_pins.0;
        let pin = PinBundle::union(&left_pins, &right_pins);
        // As in [`Self::transfer_into`]: the destination operand contributes its foreign reach
        // alone, never its own residence — the mint targets that region and stamps it as the
        // product's host. Stripped in place: the envelope was consumed above, so these pins are
        // ours to narrow.
        right_pins.remove_region(RegionOwner::region(&*dest_home));
        let (product, bundle) = left.into_retained_inner().merge_composed(
            right.into_retained_inner(),
            &pin,
            |_left, _right, value, dest, token| {
                // Both operands ride un-copied, so neither claim depends on the product:
                // compose off the destination's handle before `f` consumes it.
                let (witness, bundle) =
                    Carrier::compose_into(&left_pins, &right_pins, dest.region_handle());
                (f(value, dest, token), witness, bundle)
            },
        );
        Delivered::hosted(
            Retained::from_witnessed(product),
            dest_home,
            StepCoverage(bundle),
        )
    }

    /// Merge two envelopes into one — the composition verb for two values already in hand, where
    /// [`Self::transfer_into`] is the one for a value being relocated *out* of this envelope.
    /// `other` is the **destination** operand: its region is what both operands' reach is minted
    /// into, and its residence becomes the product's. Both operands' pins cover the shared `for<'b>`
    /// re-anchor, so neither side needs a pin threaded in from the caller.
    ///
    /// `f` receives a [`FoldedPlacement`] over the destination operand's own handle, so a value the
    /// fold builds stores through the same confinement the composition establishes.
    ///
    /// Nothing is copied out of either operand — the fold reads both live forms and builds a value
    /// that borrows them verbatim — so there is no retention predicate here: the composed bundle
    /// keeps every member both envelopes named.
    pub fn merge_into<B, P, Pr>(
        self,
        other: Delivered<B, Carrier<F>, F>,
        f: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldedPlacement<'b, Pr>) -> P::At<'b>,
    ) -> Delivered<P, Carrier<F>, F>
    where
        B: Reattachable + DropFree,
        P: Reattachable + DropFree,
        Pr: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<Pr>>,
        for<'b> B::At<'b>: HasRegionHandle<'b, Pr>,
    {
        self.merge_into_token::<B, P, Pr>(other, super::place_over_dest::<T, B, P, Pr>(f))
    }

    /// Re-stamp the delivered value **in place, in its own home region** — the single-seam escape
    /// verb. The destination is `home`'s own region (not an ancestor), so `relocate` re-anchors the
    /// value where it already resides: no bytes move and nothing re-homes. `relocate` builds the
    /// re-stamped value into that region through the folded placement — for a koan declared return,
    /// re-tagging the top node while sharing its substrate borrow verbatim. Its second operand is
    /// the same home-region handle the placement covers, redundant with it (a caller ignores it),
    /// retained only to match the transfer fold's shape.
    ///
    /// `home` is passed in rather than read off the envelope: the envelope's pins are one flat
    /// antichain with no distinguished home, and the single-seam caller holds the frame it is
    /// re-stamping into anyway. It must be the owner of the region the value lives in — re-stamping
    /// against any other region would re-anchor the value into storage that does not hold it.
    ///
    /// The product's description is minted into `home`'s own region, so its host is that region's
    /// owner — the residence the value already had. Its members are everything this envelope pins,
    /// `home` included when the value genuinely borrows there: the retention predicate keeps every
    /// member, because a re-stamp copies nothing. The self rule then drops `home` from the composed
    /// *bundle*, leaving exactly the value's foreign reach — which the transfer retains into that
    /// same region, covering the restamped value read in place.
    ///
    /// This is also the door a **birth site** takes when it has no relocation to ride: a value whose
    /// substrate can only be born through a fold door mints its description here, naming the birth
    /// region as an ordinary member, instead of correcting a fold-composed witness after the fact.
    /// The product rides back as its own envelope, so the birth site keeps the pins the mint
    /// composed rather than re-deriving them.
    pub fn restamp_in_place<P: Reattachable + DropFree, Pr>(
        &self,
        home: &Rc<F>,
        relocate: impl for<'b> FnOnce(
            T::At<'b>,
            RegionHandle<'b, Pr>,
            FoldedPlacement<'b, Pr>,
        ) -> P::At<'b>,
    ) -> Delivered<P, Carrier<F>, F>
    where
        Pr: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<Pr>>,
        T::At<'static>: Copy,
    {
        // The destination operand is the value's own region, yoked out of `home` — the region the
        // value already lives in. Re-stamping into it re-anchors, never relocates.
        let dest = Delivered::seal(
            Witnessed::<RegionHandleFamily<Pr>, Rc<F>>::yoke_handle(Rc::clone(home), |handle| {
                handle
            })
            .into_reference_only::<Pr>(),
            Rc::clone(home),
            StepCoverage::empty(),
        );
        // The destination is a bare region handle (empty reach), so the destination operand pins
        // only its own home; nothing is copied out, so the retention predicate keeps every member —
        // the restamped value borrows exactly what the original did.
        self.transfer_into::<RegionHandleFamily<Pr>, P, Pr>(
            dest,
            |_product, _region| true,
            relocate,
        )
    }
}

/// A [`Delivered`] **before its home is known**: the same fused carrier-plus-coverage pair, minus the
/// home pin. It exists for the one holder shape the envelope cannot serve — a value built in a step
/// whose host is the frame that will *finalize* it, chosen after the step body has already produced
/// the carrier. Rather than let such a holder carry the cell and the coverage as two fields it could
/// separate, it carries this: the fields are private, no accessor hands the cell back, and the only
/// exit is [`host`](Self::host), which supplies the missing pin and yields the envelope.
///
/// The coverage travels whole. Where the eventual host *is* the region the value already lives in,
/// [`host`](Self::host)'s union collapses the duplicate; where the value was lifted out of some
/// outer region, that region is a genuine member and dropping it would discard the only pin naming
/// it.
pub struct Unhosted<T: Reattachable + DropFree, W, F: PinsRegion> {
    cell: Retained<T, W>,
    pins: StepCoverage<F>,
}

impl<T: Reattachable + DropFree, W, F: PinsRegion> Unhosted<T, W, F> {
    /// Hold a **no-reach** carrier hostless: a value whose borrows reach nothing beyond the region
    /// it lives in, so there is no coverage to carry and the empty bundle is exact. The premise is
    /// the construction door's, not checked here — a carrier whose mint composed no source has a
    /// description with no members at all, and the membership query lives on the opened carrier,
    /// which this state holds no pin to reach. A value that *does* reach elsewhere arrives through
    /// [`Delivered::unhost`], carrying its bundle.
    pub fn born(witnessed: Witnessed<T, W>) -> Self {
        Unhosted {
            cell: Retained::from_witnessed(witnessed),
            pins: StepCoverage::empty(),
        }
    }

    /// Pin the home and become a delivery envelope — the sole exit. `home` is unioned into the
    /// coverage exactly as the envelope's own constructors union theirs, so a hosted pair is
    /// indistinguishable from one that was never unhosted. The caller supplies the right owner (the
    /// frame the value is being stored against); this door's contract is only that it is the one way
    /// out.
    pub fn host(self, home: Rc<F>) -> Delivered<T, W, F> {
        Delivered {
            cell: self.cell,
            pins: StepCoverage(PinBundle::union(&PinBundle::singleton(home), &self.pins.0)),
        }
    }
}

/// [`Delivered::transfer_into`]'s [`Witnessed::merge_composed`] fold: run `relocate` at the brand,
/// derive the source claim by running the **retention predicate** over the product against each
/// region `source` pins, then compose. The destination handle is read off the operand *before*
/// `relocate` consumes it, so the mint still targets the engine's own operand region.
///
/// Built as a returned `impl for<'b> FnOnce` for the same reason as
/// [`place_over_dest`](super::place_over_dest): an inline closure written inside `transfer_into`,
/// whose scope binds `T::At<'static>: Copy`, is not coerced to `for<'b>` and trips a spurious
/// `'b: 'static`.
#[allow(clippy::type_complexity)]
fn relocate_then_compose<'s, T, B, P, F, Pr>(
    source: &'s PinBundle<F>,
    dest_bundle: &'s PinBundle<F>,
    mut still_borrows: impl for<'b> FnMut(&P::At<'b>, &F::Region) -> bool + 's,
    relocate: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldToken<'b>) -> P::At<'b> + 's,
) -> impl for<'b> FnOnce(
    &Carrier<F>,
    &Carrier<F>,
    T::At<'b>,
    B::At<'b>,
    FoldToken<'b>,
) -> (P::At<'b>, Carrier<F>, PinBundle<F>)
+ 's
where
    T: Reattachable,
    B: Reattachable,
    P: Reattachable,
    F: PinsRegion + RegionOwner<Region = Region<Pr>> + 'static,
    Pr: StorageProfile<FrameOwner = F> + 'static,
    for<'b> B::At<'b>: HasRegionHandle<'b, Pr>,
{
    move |_left, _right, left_value, live_dest, token| {
        let handle = live_dest.region_handle();
        let product = relocate(left_value, live_dest, token);
        let source_pins = source.retaining(|region| still_borrows(&product, region));
        let (witness, bundle) = Carrier::compose_into(&source_pins, dest_bundle, handle);
        (product, witness, bundle)
    }
}

/// [`Delivered::transfer_all_into`]'s [`Witnessed::merge_staged_composed`] fold — the run-shaped
/// twin of [`relocate_then_compose`]: `relocate` receives the whole staged run instead of one
/// source value and hands back the cells it built alongside the product, and the retention pass
/// walks each source's bundle under the (envelope, cell) pair that names whose claim it is. The
/// composed source side is built in that same walk ([`PinBundle::union_all_retained`]), so N
/// sources cost the one antichain the mint takes. The placement is minted over the destination
/// operand's own handle here rather than through [`super::place_over_dest`], which pairs with the
/// single-value fold shape.
///
/// Factored into a returned `impl for<'b> FnOnce` for the reason [`relocate_then_compose`] is: an
/// inline closure written inside a scope binding `T::At<'static>: Copy` is not coerced to `for<'b>`
/// and trips a spurious `'b: 'static`.
#[allow(clippy::type_complexity)]
fn relocate_run_then_compose<'s, T, B, P, C, F, Pr>(
    envelopes: &'s [&'s Delivered<T, Carrier<F>, F>],
    source_pins: &'s [&'s PinBundle<F>],
    dest_bundle: &'s PinBundle<F>,
    mut still_borrows: impl for<'b> FnMut(&Delivered<T, Carrier<F>, F>, &C::At<'b>, &F::Region) -> bool
    + 's,
    relocate: impl for<'b> FnOnce(
        &'b [T::At<'b>],
        B::At<'b>,
        FoldedPlacement<'b, Pr>,
    ) -> (P::At<'b>, &'b [C::At<'b>])
    + 's,
) -> impl for<'b> FnOnce(
    &Carrier<F>,
    &'b [T::At<'b>],
    B::At<'b>,
    FoldToken<'b>,
) -> (P::At<'b>, Carrier<F>, PinBundle<F>)
+ 's
where
    T: Reattachable + DropFree,
    B: Reattachable,
    P: Reattachable,
    C: Reattachable,
    F: PinsRegion + RegionOwner<Region = Region<Pr>> + 'static,
    Pr: StorageProfile<FrameOwner = F> + 'static,
    for<'b> B::At<'b>: HasRegionHandle<'b, Pr>,
{
    move |_dest_witness, run, live_dest, _token| {
        let handle = live_dest.region_handle();
        let (product, cells) = relocate(run, live_dest, FoldedPlacement::mint(handle));
        // The staging-order contract the door states: one cell per source, in the order they were
        // staged. Length is what an assert can see; the pairing itself is the hook's obligation.
        debug_assert_eq!(
            cells.len(),
            envelopes.len(),
            "a relocate hook returns one product cell per staged source, in staging order"
        );
        let source_pins =
            PinBundle::union_all_retained(source_pins.iter().copied(), |index, region| {
                still_borrows(envelopes[index], &cells[index], region)
            });
        let (witness, bundle) = Carrier::compose_into(&source_pins, dest_bundle, handle);
        (product, witness, bundle)
    }
}
