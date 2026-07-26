//! [`Delivered<T, W, F>`] — the **delivery envelope**: a sealed carrier paired with the frame-owner
//! `Rc` that retains its value's backing *in transit*, from a scheduler pull to the point a consumer
//! adopts or re-homes it. See
//! [design/witness-hosting.md § Retention model](../../../design/witness-hosting.md#retention-model).
//!
//! Liveness *at rest* is the scheduler's retention table (a producer frame stays held while any
//! consumer edge is undischarged). Liveness *in transit* — from a pull to its adoption — is this
//! envelope: it bundles the dormant [`Sealed`] carrier with one owned [`PinBundle`] holding an
//! `Rc<F>` for every region the value reaches, **its home among them as an ordinary member**, so a
//! consumer reads the value under a pin it does not have to thread separately. The field is private
//! and every constructor is a surface that has the true home owner in hand, so an envelope whose
//! pins disagree with its payload is not constructible — the co-location the carrier's owned host
//! arm once kept by convention is enforcement by construction.
//!
//! Home riding as an ordinary member is what lets the envelope-bearing mint verbs —
//! [`Delivered::mint_reach`] and [`Delivered::transfer_into`] — fold a producer frame into a minted
//! destination set with no separate materialization arm and no residence mode: the home pin is
//! already in the bundle the composition folds. A relocation site chooses nothing about that
//! bundle: [`Delivered::transfer_into`] *derives* the source claim by running the site's retention
//! predicate over the folded product against each pinned region in turn, so what the product still
//! reaches is a checked property of the bytes rather than a promise made before they existed. See
//! [design/witness-hosting.md § Composition](../../../design/witness-hosting.md#composition-minting-a-description-and-retaining-its-pins).
//!
//! [`Delivered::mint_reach`] is the envelope-bearing mint entry a consumer routes, a thin caller
//! over the crate-internal [`Carrier::mint_into`](super::Carrier::mint_into) core.
//! [`Delivered::adopt_into`] fuses that mint with the re-anchor it justifies into one copy-free
//! adoption verb, so a caller cannot split the pin from the reattach it pins.

use std::rc::{Rc, Weak};

use super::{
    Carrier, Erased, FoldToken, FoldedPlacement, HasRegionHandle, Opened, PinBundle, PinsRegion,
    ReachDescription, Reattachable, Region, RegionHandle, RegionHandleFamily, RegionOwner, Sealed,
    StepCoverage, StorageProfile, Witnessed,
};

/// A sealed carrier paired with the owned [`PinBundle`] that pins every region its value reaches —
/// the value's home region among them, as an ordinary member. `T` is the carrier's value family,
/// `W` its reach witness, `F` the workload's frame-owner type. The carrier's reach description is
/// non-owning; the envelope's bundle is the ownership that keeps the value's whole reach alive
/// across transit — from a scheduler pull to the point a consumer adopts or re-homes it.
pub struct Delivered<T: Reattachable, W, F: PinsRegion> {
    /// The dormant carrier — value and reach description as one unit.
    cell: Sealed<T, W>,
    /// The owner of the region the value **lives in** — its residence, supplied by whichever
    /// container built the envelope (the retention hold's retained owner for a pulled dep, the
    /// scope's region owner for a resident seal, the anchor for a step's Done product). It carries
    /// *identity*, not liveness, and is what [`with_home_region`](Self::with_home_region) hands a
    /// workload policy so residence is never searched for in a member set.
    ///
    /// `Weak` because `pins` already owns the liveness: home enters the bundle at construction and
    /// survives there unless an outer member subsumes it, in which case that member pins home's
    /// storage instead ([`PinsRegion`]'s contract). Either way the upgrade succeeds for the
    /// envelope's whole life, and the residence pointer costs no strong count of its own.
    home: Weak<F>,
    /// The owned pins for every region the value's borrows reach, **home included** — the ownership
    /// counterpart of the carrier's non-owning reach description, composed at construction from the
    /// home owner the constructor is handed and the reach bundle threaded in with it. It is what
    /// keeps those regions alive while the envelope sits parked in a scheduler slot, where the
    /// description's `Weak` members alone would not, and what covers the reads
    /// ([`open`](Self::open) / [`open_at`](Self::open_at)) the envelope serves.
    /// [`duplicate`](Self::duplicate) clones it, so every fan-out consumer holds its own pins.
    pins: StepCoverage<F>,
}

impl<T: Reattachable, W, F: PinsRegion> Delivered<T, W, F> {
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
        self.cell.open_at(&self.pins)
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

    /// Consume the envelope into its parts — the dormant carrier and the owned coverage — for a
    /// consumer that re-seeds a hold or re-envelopes the value under the same ownership it already
    /// holds (the value-terminal finalize keeps the coverage and re-seals the carrier).
    pub fn into_parts(self) -> (Sealed<T, W>, StepCoverage<F>) {
        (self.cell, self.pins)
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
    pub fn project<P: Reattachable>(
        self,
        f: impl for<'b> FnOnce(T::At<'b>, FoldToken<'b>) -> P::At<'b>,
    ) -> Delivered<P, W, F> {
        let Delivered { cell, home, pins } = self;
        // The envelope's own pins cover the re-anchor for the whole call — the same coverage every
        // read of this envelope runs under.
        let projected = cell.unseal().map_pinned(&pins, f);
        Delivered {
            cell: Sealed::seal(projected),
            home,
            pins,
        }
    }

    /// Duplicate the envelope: [`duplicate`](Sealed::duplicate) the sealed carrier (bit-copy value +
    /// witness clone) and clone the owned [`PinBundle`], leaving the source intact — the producer
    /// keeps its terminal for other consumers, and every pinned region (home among them) gains one
    /// `Rc` clone, dropped when this duplicate's consumer is done.
    pub fn duplicate(&self) -> Self
    where
        Erased<T>: Copy,
        W: Clone,
    {
        Delivered {
            cell: self.cell.duplicate(),
            home: self.home.clone(),
            pins: self.pins.clone(),
        }
    }

    /// Run `f` against the region the value **lives in** — the residence the container that built
    /// this envelope supplied. Exact by construction: every constructor takes the true home owner,
    /// so residence is read off the envelope rather than recovered by probing its member set. This
    /// is the region a workload's escape-seam policy prices a crossing *out of*, and the one member
    /// a relocation's retention predicate can release.
    pub fn with_home_region<R>(&self, f: impl FnOnce(&F::Region) -> R) -> R {
        f(RegionOwner::region(&*self.home_owner()))
    }

    /// The residence owner itself, upgraded. Crate-internal: it hands out an owned pin, which is
    /// the ownership tier an embedder has no vocabulary for — the relocation verbs use it to give
    /// their product the destination's own residence.
    fn home_owner(&self) -> Rc<F> {
        self.home
            .upgrade()
            .expect("the envelope's own pins cover its home region for the envelope's whole life")
    }
}

/// The envelope-bearing verbs over the reference-only [`Carrier`] witness. The envelope is the
/// holder that owns the value's pins, so it is what a mint folds and what covers every read here.
impl<T: Reattachable, F: PinsRegion + 'static> Delivered<T, Carrier<F>, F> {
    /// Pair a sealed carrier with the owner of the region its value lives in and the owned
    /// [`PinBundle`] pinning every other region it reaches, unioning the two into the envelope's
    /// single member set — this is where a value's home becomes an ordinary reach member. The
    /// caller supplies the true home owner and the owned bundle threaded from the mint, never
    /// re-derived from the carrier's description: the scheduler's retention hold (which carries
    /// `owner` + `reach` as one unit) for a delivered dep, or the region owner + the entry's pins
    /// for a resident seal — so the pairing is co-located by construction.
    pub fn hosted(cell: Sealed<T, Carrier<F>>, home: Rc<F>, reach: StepCoverage<F>) -> Self {
        let pins = StepCoverage(PinBundle::union(
            &PinBundle::singleton(Rc::clone(&home)),
            &reach.0,
        ));
        let home = Rc::downgrade(&home);
        Delivered { cell, home, pins }
    }

    /// **Lift** a [`Sealed`] carrier at rest into a delivery envelope in transit (`Sealed →
    /// Delivered`): upgrade the sealed carrier's reach description `Weak → Rc` into an owned inline
    /// [`PinBundle`] under `home`, and union `home` itself in. The owned set is what lets the value
    /// travel after its source frame dies — an arena-hosted `&ReachDescription` would dangle in
    /// transit, so the lift re-owns the claimed subset while the reached regions are still covered
    /// (the holder rule under `home`). `home` is the value's residence owner, covering both its
    /// backing and its description's hosting arena; it is unioned in explicitly because the self
    /// rule strips it from the bundle a mint into that same region hands back.
    pub fn lift(cell: Sealed<T, Carrier<F>>, home: Rc<F>) -> Self {
        let reach = cell.witness().upgrade_bundle(&home);
        let pins = StepCoverage(PinBundle::union(
            &PinBundle::singleton(Rc::clone(&home)),
            &reach,
        ));
        let home = Rc::downgrade(&home);
        Delivered { cell, home, pins }
    }

    /// Seal a live [`Witnessed`] carrier into a delivery envelope pinned by `home` and the owned
    /// [`PinBundle`] `reach` — the resident / Done-arm seal veneer's library half. Bundles the
    /// born-witnessed carrier with the region owner the caller already holds and the owned bundle
    /// threaded in (the binding entry's pins, the Done-arm carrier's pins), so a resident value
    /// travels as an envelope pinned by its home frame, identical in shape to a delivered dep.
    pub fn seal(witnessed: Witnessed<T, Carrier<F>>, home: Rc<F>, reach: StepCoverage<F>) -> Self {
        let pins = StepCoverage(PinBundle::union(
            &PinBundle::singleton(Rc::clone(&home)),
            &reach.0,
        ));
        Delivered {
            cell: Sealed::seal(witnessed),
            home: Rc::downgrade(&home),
            pins,
        }
    }

    /// Mint this value's reach into `dest`. The envelope's own pins are the source the composition
    /// folds, so the value's home rides in as an ordinary member and the self rule alone decides
    /// whether it survives into the returned bundle. No policy is threaded in: the minted
    /// description is the value's **exact** reach. Returns the minted description (`None` == empty,
    /// no allocation) hosted in `dest`, the owned [`PinBundle`] the binding entry keeps to pin its
    /// members, and the borrows-into-dest bit.
    pub(crate) fn mint_reach<'d, P>(
        &self,
        dest: RegionHandle<'d, P>,
    ) -> (Option<&'d ReachDescription<F>>, PinBundle<F>, bool)
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
    {
        self.witness().mint_into(self.pins(), dest)
    }

    /// Copy-free adoption: mints this envelope's reach — home included, as an ordinary member —
    /// into `dest`'s region, **retains** the resulting owned [`PinBundle`] for the region's life,
    /// then re-anchors the sealed value at `dest`'s lifetime. Fused so the re-anchor cannot be
    /// reached without the pin that keeps it live: the minted description names the value's
    /// reach and the retained bundle owns it for the region's life ⊇ `'d`, so every
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
    /// destination region: the mint stores the value's reach into `dest`'s side table and the
    /// retained bundle owns it for the region's life ⊇ `'d`, so the coverage justifying the open
    /// outlives every use of it. That is what lets an adopted value ride a step-lifetime type
    /// position no rank-2 read and no pin borrow can reach.
    pub fn open_adopted<'d, P>(&self, dest: RegionHandle<'d, P>) -> Opened<'d, T, Carrier<F>>
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
        T::At<'static>: Copy,
    {
        let (minted, bundle, borrows_into_dest) = self.mint_reach(dest);
        // The description is non-owning; the adopted value lives for the region's life, so the
        // region retains the owning bundle (dropped only at region death) — the liveness the old
        // arena-hosted owning set provided, now carried by the region's retention list.
        dest.region().retain_reach(bundle);
        let erased: Erased<T> = self.open(Erased::<T>::erase);
        Opened::adopted(
            // SAFETY: the mint above stored this carrier's reach into `dest`'s side table and the
            // retained bundle pins every region it names for the region's life ⊇ 'd; the
            // re-anchored borrow cannot outlive its pin.
            unsafe { erased.reattach::<'d>() },
            Carrier::new(borrows_into_dest, minted),
        )
    }

    /// **Adopt** the delivered value into `dest`, dropping to rest as a [`Sealed`] (`Delivered →
    /// Sealed`): mint the value's reach into `dest`'s arena (home an ordinary member, with `dest`'s
    /// own region stripped from the owned bundle by the never-pin-a-region-into-itself rule) and
    /// re-seal the value under a resident [`Carrier`] that references the minted (now weak)
    /// description. The value keeps its identity in the producer's region.
    ///
    /// Hands the owned [`PinBundle`] **back to the caller** rather than retaining it into `dest`'s
    /// region: the bind seam that adopts a value is the holder of the scope's union bundle (design
    /// § Threading), and folding the adopted pins there is what makes them drop at scope death
    /// instead of at region death. The seal is only readable while that bundle is held.
    ///
    /// The dual of [`Self::adopt_into`] (which re-anchors to a live `T::At<'d>` and retains into the
    /// region, having no holder to hand pins to): `adopt` hands back a dormant seal for a table
    /// entry / node slot instead.
    pub fn adopt<'d, P>(
        &self,
        dest: RegionHandle<'d, P>,
    ) -> (Sealed<T, Carrier<F>>, StepCoverage<F>)
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
        T::At<'static>: Copy,
    {
        let (minted, bundle, borrows_into_dest) = self.mint_reach(dest);
        let erased: Erased<T> = self.open(Erased::<T>::erase);
        let sealed = Sealed::seal(Witnessed::from_erased(
            erased,
            Carrier::new(borrows_into_dest, minted),
        ));
        (sealed, StepCoverage(bundle))
    }

    /// Relocate the delivered value into a destination and re-seal it under the composed carrier
    /// that names everything it reaches from there — the envelope-bearing form of the witnessed
    /// transfer, and the only relocation verb for a carrier-witnessed value. The sealed carrier is
    /// duplicated (the envelope keeps its cell for other consumers), re-anchored at a shared
    /// `for<'b>` brand with `dest`'s live form under the envelope's own pins, and handed to
    /// `relocate` — the workload's structural copy/fold, which builds into `dest` at the brand
    /// natively. The composed witness mints both operands' reach into `dest`'s own arena.
    ///
    /// What the relocated product still reaches *from the source side* is **derived, not accepted**:
    /// once `relocate` has built the product, `still_borrows` is run over it against each region
    /// this envelope pins, and the source claim is the members it answers `true` for (design §
    /// Escape). A `false` verdict drops that region from the composed bundle, so its owner frees at
    /// retention discharge — the tail-loop turnover rule; a `true` verdict keeps it, so the producer
    /// transfers by hold. The claim is therefore a checked property of the bytes that exist. A
    /// predicate that answers conservatively costs retention, never soundness.
    pub fn transfer_into<B: Reattachable, P: Reattachable, Pr>(
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
        // mint targets that same region, so listing it as a source would make every product claim
        // to borrow into its own destination and force the borrows-into-dest bit true everywhere.
        let dest_reach = dest.pins().without_region(RegionOwner::region(&*dest_home));
        // The re-anchor runs under the envelope's own pins, whatever the predicate later claims the
        // product reaches: reading the source value is always covered here.
        let (product, bundle) = self.cell.duplicate().unseal().merge_composed(
            dest.cell.unseal(),
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
        Delivered::hosted(Sealed::seal(product), dest_home, StepCoverage(bundle))
    }

    /// [`Self::transfer_into`] handing `relocate` a [`FoldedPlacement`] over the destination operand's
    /// own handle instead of a bare [`FoldToken`]. The placement is minted over exactly the handle
    /// [`Carrier::compose_into`] mints the composed reach set over, so the folded store rides the same
    /// confinement the composition establishes — the destination is the engine's own operand region,
    /// never a caller-captured handle.
    pub fn transfer_into_placing<B: Reattachable, P: Reattachable, Pr>(
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
        self.transfer_into::<B, P, Pr>(
            dest,
            still_borrows,
            super::place_over_dest::<T, B, P, Pr>(relocate),
        )
    }

    /// Merge two envelopes into one — the composition verb for two values already in hand, where
    /// [`Self::transfer_into`] is the one for a value being relocated *out* of this envelope.
    /// `other` is the **destination** operand: its region is what both operands' reach is minted
    /// into, and its residence becomes the product's. Both operands' pins cover the shared `for<'b>`
    /// re-anchor, so neither side needs a pin threaded in from the caller.
    ///
    /// Nothing is copied out of either operand — the fold reads both live forms and builds a value
    /// that borrows them verbatim — so there is no retention predicate here: the composed bundle
    /// keeps every member both envelopes named.
    pub fn merge_into<B, P, Pr>(
        self,
        other: Delivered<B, Carrier<F>, F>,
        f: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldToken<'b>) -> P::At<'b>,
    ) -> Delivered<P, Carrier<F>, F>
    where
        B: Reattachable,
        P: Reattachable,
        Pr: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<Pr>>,
        for<'b> B::At<'b>: HasRegionHandle<'b, Pr>,
    {
        let dest_home = other.home_owner();
        let Delivered {
            cell: left,
            pins: left_pins,
            ..
        } = self;
        let Delivered {
            cell: right,
            pins: right_pins,
            ..
        } = other;
        let left_pins = left_pins.0;
        let right_pins = right_pins.0;
        let pin = PinBundle::union(&left_pins, &right_pins);
        // As in [`Self::transfer_into`]: the destination operand contributes its foreign reach
        // alone, never its own residence — the mint targets that region.
        let right_pins = right_pins.without_region(RegionOwner::region(&*dest_home));
        let (product, bundle) = left.unseal().merge_composed(
            right.unseal(),
            &pin,
            |_left, right_witness, value, dest, token| {
                // Both operands ride un-copied, so neither claim depends on the product:
                // compose off the destination's handle before `f` consumes it.
                let (witness, bundle) = Carrier::compose_into(
                    right_witness,
                    &left_pins,
                    &right_pins,
                    dest.region_handle(),
                );
                (f(value, dest, token), witness, bundle)
            },
        );
        Delivered::hosted(Sealed::seal(product), dest_home, StepCoverage(bundle))
    }

    /// [`Self::merge_into`] handing `f` a [`FoldedPlacement`] over the destination operand's own
    /// handle instead of a bare [`FoldToken`], so a value the fold builds stores through the same
    /// confinement the composition establishes.
    pub fn merge_into_placing<B, P, Pr>(
        self,
        other: Delivered<B, Carrier<F>, F>,
        f: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldedPlacement<'b, Pr>) -> P::At<'b>,
    ) -> Delivered<P, Carrier<F>, F>
    where
        B: Reattachable,
        P: Reattachable,
        Pr: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<Pr>>,
        for<'b> B::At<'b>: HasRegionHandle<'b, Pr>,
    {
        self.merge_into::<B, P, Pr>(other, super::place_over_dest::<T, B, P, Pr>(f))
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
    /// The composed witness is identical to the input's: the destination is the value's own home
    /// region, so the self rule drops home from the composed bundle, leaving exactly the value's
    /// foreign reach — which the transfer retains into that same region, covering the restamped
    /// value read in place.
    pub fn restamp_in_place<P: Reattachable, Pr>(
        &self,
        home: &Rc<F>,
        relocate: impl for<'b> FnOnce(
            T::At<'b>,
            RegionHandle<'b, Pr>,
            FoldedPlacement<'b, Pr>,
        ) -> P::At<'b>,
    ) -> Witnessed<P, Carrier<F>>
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
            .into_reference_only(),
            Rc::clone(home),
            StepCoverage::empty(),
        );
        // The destination is a bare region handle (empty reach), so the destination operand pins
        // only its own home; nothing is copied out, so the retention predicate keeps every member —
        // the restamped value borrows exactly what the original did. The product envelope's pins
        // cover the restamped value read in place, so only its carrier is handed back.
        self.transfer_into_placing::<RegionHandleFamily<Pr>, P, Pr>(
            dest,
            |_product, _region| true,
            relocate,
        )
        .into_cell()
        .unseal()
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
    move |_left, right_witness, left_value, live_dest, token| {
        let handle = live_dest.region_handle();
        let product = relocate(left_value, live_dest, token);
        let source_pins = source.retaining(|region| still_borrows(&product, region));
        let (witness, bundle) =
            Carrier::compose_into(right_witness, &source_pins, dest_bundle, handle);
        (product, witness, bundle)
    }
}
