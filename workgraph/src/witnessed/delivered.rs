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
//! already in the bundle the composition folds. What a relocation site still chooses is *which*
//! bundle it hands the fold — its own pins to keep the producer alive, or the empty bundle for a
//! true deep copy that must let the producer die. See
//! [design/witness-hosting.md § Composition](../../../design/witness-hosting.md#composition-minting-a-description-and-retaining-its-pins).
//!
//! [`Delivered::mint_reach`] is the envelope-bearing mint entry a consumer routes, a thin caller
//! over the crate-internal [`Carrier::mint_into`](super::Carrier::mint_into) core.
//! [`Delivered::adopt_into`] fuses that mint with the re-anchor it justifies into one copy-free
//! adoption verb, so a caller cannot split the pin from the reattach it pins.

use std::rc::Rc;

use super::{
    Carrier, Erased, FoldToken, FoldedPlacement, HasRegionHandle, Opened, PinBundle, PinsRegion,
    ReachDescription, Reattachable, Region, RegionHandle, RegionHandleFamily, RegionOwner, Sealed,
    StorageProfile, Witnessed,
};

/// A sealed carrier paired with the owned [`PinBundle`] that pins every region its value reaches —
/// the value's home region among them, as an ordinary member. `T` is the carrier's value family,
/// `W` its reach witness, `F` the workload's frame-owner type. The carrier's reach description is
/// non-owning; the envelope's bundle is the ownership that keeps the value's whole reach alive
/// across transit — from a scheduler pull to the point a consumer adopts or re-homes it.
pub struct Delivered<T: Reattachable, W, F: PinsRegion> {
    /// The dormant carrier — value and reach description as one unit.
    cell: Sealed<T, W>,
    /// The owned pins for every region the value's borrows reach, **home included** — the ownership
    /// counterpart of the carrier's non-owning reach description, composed at construction from the
    /// home owner the constructor is handed and the reach bundle threaded in with it. It is what
    /// keeps those regions alive while the envelope sits parked in a scheduler slot, where the
    /// description's `Weak` members alone would not, and what covers the reads
    /// ([`open`](Self::open) / [`open_at`](Self::open_at)) the envelope serves.
    /// [`duplicate`](Self::duplicate) clones it, so every fan-out consumer holds its own pins.
    pins: PinBundle<F>,
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

    /// The owned [`PinBundle`] pinning every region the value reaches, home included — the
    /// ownership counterpart of the carrier's non-owning reach description, and the coverage a
    /// relocation site hands the fold to keep this value's whole reach alive. A consumer that
    /// re-seeds a retention hold from an envelope it already holds (the value-terminal finalize)
    /// clones it.
    pub fn pins(&self) -> &PinBundle<F> {
        &self.pins
    }

    /// Consume the envelope into its parts — the dormant carrier and the owned pins — for a
    /// consumer that re-seeds a hold or re-envelopes the value under the same ownership it already
    /// holds (the value-terminal finalize keeps the pins and re-seals the carrier).
    pub fn into_parts(self) -> (Sealed<T, W>, PinBundle<F>) {
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
            pins: self.pins.clone(),
        }
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
    pub fn hosted(cell: Sealed<T, Carrier<F>>, home: Rc<F>, reach: PinBundle<F>) -> Self {
        Delivered {
            cell,
            pins: PinBundle::union(&PinBundle::singleton(home), &reach),
        }
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
        Delivered {
            cell,
            pins: PinBundle::union(&PinBundle::singleton(home), &reach),
        }
    }

    /// Seal a live [`Witnessed`] carrier into a delivery envelope pinned by `home` and the owned
    /// [`PinBundle`] `reach` — the resident / Done-arm seal veneer's library half. Bundles the
    /// born-witnessed carrier with the region owner the caller already holds and the owned bundle
    /// threaded in (the binding entry's pins, the Done-arm carrier's pins), so a resident value
    /// travels as an envelope pinned by its home frame, identical in shape to a delivered dep.
    pub fn seal(witnessed: Witnessed<T, Carrier<F>>, home: Rc<F>, reach: PinBundle<F>) -> Self {
        Delivered {
            cell: Sealed::seal(witnessed),
            pins: PinBundle::union(&PinBundle::singleton(home), &reach),
        }
    }

    /// Mint this value's reach into `dest` under `omit` — the embedder's omission policy (regions
    /// the destination's container already pins). The envelope's own pins are the source the
    /// composition folds, so the value's home rides in as an ordinary member and the self rule
    /// alone decides whether it survives into the returned bundle. Returns the minted description
    /// (`None` == empty, no allocation) hosted in `dest`, the owned [`PinBundle`] the binding entry
    /// keeps to pin its members, and the borrows-into-dest bit.
    pub fn mint_reach<'d, P>(
        &self,
        dest: RegionHandle<'d, P>,
        omit: impl Fn(&Region<P>) -> bool,
    ) -> (Option<&'d ReachDescription<F>>, PinBundle<F>, bool)
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
    {
        self.witness().mint_into(&self.pins, dest, omit)
    }

    /// Copy-free adoption: mints this envelope's reach — home included, as an ordinary member —
    /// into `dest`'s region, **retains** the resulting owned [`PinBundle`] for the region's life,
    /// then re-anchors the sealed value at `dest`'s lifetime. Fused so the re-anchor cannot be
    /// reached without the pin that keeps it live: the minted description names the value's
    /// reach and the retained bundle owns it for the region's life ⊇ `'d`, so every
    /// region the value reaches (its home included) outlives the returned borrow.
    /// `omit` names regions the caller's context covers ambiently, as in
    /// [`Self::mint_reach`].
    pub fn adopt_into<'d, P>(
        &self,
        dest: RegionHandle<'d, P>,
        omit: impl Fn(&Region<P>) -> bool,
    ) -> T::At<'d>
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
        T::At<'static>: Copy,
    {
        let (_desc, bundle, _borrows_into_dest) = self.mint_reach(dest, omit);
        // The description is non-owning; the adopted value lives for the region's life, so the
        // region retains the owning bundle (dropped only at region death) — the liveness the old
        // arena-hosted owning set provided, now carried by the region's retention list.
        dest.region().retain_reach(bundle);
        let erased: Erased<T> = self.open(Erased::<T>::erase);
        // SAFETY: the mint above stored this carrier's reach into `dest`'s side table and the
        // retained bundle pins every region it names for the region's life ⊇ 'd; the re-anchored
        // borrow cannot outlive its pin.
        unsafe { erased.reattach() }
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
    /// entry / node slot instead. `omit` names regions `dest`'s context covers ambiently, as in
    /// [`Self::mint_reach`].
    pub fn adopt<'d, P>(
        &self,
        dest: RegionHandle<'d, P>,
        omit: impl Fn(&Region<P>) -> bool,
    ) -> (Sealed<T, Carrier<F>>, PinBundle<F>)
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
        T::At<'static>: Copy,
    {
        let (minted, bundle, borrows_into_dest) = self.mint_reach(dest, omit);
        let erased: Erased<T> = self.open(Erased::<T>::erase);
        let sealed = Sealed::seal(Witnessed::from_erased(
            erased,
            Carrier::new(borrows_into_dest, minted),
        ));
        (sealed, bundle)
    }

    /// Relocate the delivered value into a destination and re-seal it under the composed carrier
    /// that names everything it reaches from there — the envelope-bearing form of the witnessed
    /// transfer, and the only relocation verb for a carrier-witnessed value. The sealed carrier is
    /// duplicated (the envelope keeps its cell for other consumers), re-anchored at a shared
    /// `for<'b>` brand with `dest`'s live form under the envelope's own pins, and handed to
    /// `relocate` — the workload's structural copy/fold, which builds into `dest` at the brand
    /// natively. The composed witness mints both operands' reach into `dest`'s own arena.
    ///
    /// `source_pins` is what the relocated product is claimed to reach *from the source side*, and
    /// it is where the site prices copy against pin (design § Escape):
    ///
    /// - [`self.pins()`](Self::pins) — the product still borrows into this value's regions, home
    ///   among them, so the fold keeps the producer alive. The copy-free re-anchor and any copy
    ///   whose leaves still point back.
    /// - [`PinBundle::empty()`] — a true deep copy with no surviving borrow into the source. The
    ///   producer's region is free to die at retention release (the tail-loop turnover rule). The
    ///   caller asserts the copy left no leaf behind; passing the empty bundle for a product that
    ///   does still borrow is a dangling read.
    pub fn transfer_into<B: Reattachable, P: Reattachable, Pr>(
        &self,
        dest: Witnessed<B, Carrier<F>>,
        dest_bundle: &PinBundle<F>,
        source_pins: &PinBundle<F>,
        relocate: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldToken<'b>) -> P::At<'b>,
    ) -> (Witnessed<P, Carrier<F>>, PinBundle<F>)
    where
        Pr: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<Pr>>,
        for<'b> B::At<'b>: HasRegionHandle<'b, Pr>,
        T::At<'static>: Copy,
    {
        // The re-anchor runs under the envelope's own pins, whatever `source_pins` claims the
        // product reaches: reading the source value is always covered here.
        self.cell.duplicate().unseal().merge_composed(
            dest,
            &self.pins,
            |_left, right, live_dest| {
                Carrier::compose_into(right, source_pins, dest_bundle, live_dest.region_handle())
            },
            relocate,
        )
    }

    /// [`Self::transfer_into`] handing `relocate` a [`FoldedPlacement`] over the destination operand's
    /// own handle instead of a bare [`FoldToken`]. The placement is minted over exactly the handle
    /// [`Carrier::compose_into`] mints the composed reach set over, so the folded store rides the same
    /// confinement the composition establishes — the destination is the engine's own operand region,
    /// never a caller-captured handle.
    pub fn transfer_into_placing<B: Reattachable, P: Reattachable, Pr>(
        &self,
        dest: Witnessed<B, Carrier<F>>,
        dest_bundle: &PinBundle<F>,
        source_pins: &PinBundle<F>,
        relocate: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldedPlacement<'b, Pr>) -> P::At<'b>,
    ) -> (Witnessed<P, Carrier<F>>, PinBundle<F>)
    where
        Pr: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<Pr>>,
        for<'b> B::At<'b>: HasRegionHandle<'b, Pr>,
        T::At<'static>: Copy,
    {
        self.cell.duplicate().unseal().merge_composed(
            dest,
            &self.pins,
            |_left, right, live_dest| {
                Carrier::compose_into(right, source_pins, dest_bundle, live_dest.region_handle())
            },
            super::place_over_dest::<T, B, P, Pr>(relocate),
        )
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
        let dest: Witnessed<RegionHandleFamily<Pr>, Carrier<F>> =
            Witnessed::<RegionHandleFamily<Pr>, Rc<F>>::yoke_handle(Rc::clone(home), |handle| {
                handle
            })
            .into_reference_only();
        // The destination is a bare region handle (empty reach), so the destination operand bundle
        // is empty; the source claim is the envelope's own pins, since nothing is copied out. The
        // composed bundle the transfer retains in the home region covers the restamped value read
        // in place, so it is discarded here.
        let (restamped, _composed) = self.transfer_into_placing::<RegionHandleFamily<Pr>, P, Pr>(
            dest,
            &PinBundle::empty(),
            &self.pins,
            relocate,
        );
        restamped
    }
}
