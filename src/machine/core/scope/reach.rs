//! The reach / carrier derivation cluster on [`Scope`]: minting a value's residence and reach into
//! this scope's arena as one description, the resident-carrier verb over it, sealing residents into
//! delivery envelopes, the shape-split pure-value placement ([`Scope::place_pure_value`]), the two
//! adoption doors over one policy chooser ([`adopt_disposition`]), and the callable / module store
//! folds. Every door that moves a value into this region runs through a fold or merge whose
//! composition mints and retains the product's reach here, so residence is discharged by the fold
//! brand's rank-2 signature. Split out of the parent `scope` module.

use std::rc::Rc;

use super::Scope;
use crate::machine::core::bindings::SealedValue;
use crate::machine::core::carrier_witness::{DeliveredFunction, OpenedFunction, SealedFunction};
use crate::machine::core::kfunction::{KFunction, KFunctionFamily};
use crate::machine::core::ref_carriers::BindingsReferenceFamily;
use crate::machine::core::{
    FoldingBrand, FrameCoverage, FrameReach, FrameStorage, KoanRegion, KoanRegionExt,
    KoanStorageProfile, ModuleRefFamily, ScopeRefFamily, product_reaches_region,
};
use crate::machine::model::{
    Carried, CarriedFamily, KObject, KType, Module, ModuleDraft, OperatorGroup,
    OperatorGroupFamily, ReductionMode, RegionEscape, TypeIdentifier, copy_or_pin,
    relocate_object_into,
};
use crate::machine::{
    CarrierWitness, DeliveredCarried, DeliveredOperatorGroup, KError, SplicedCell,
};
use crate::witnessed::{
    Delivered, DropFree, Reattachable, RegionHandleFamily, Sealed, SealedExtern, Witnessed,
};

// The tests here pin the bind-seam pin (substrate-sharing) mechanism; the `seam-force-copy` build
// rebuilds the record instead, so they cannot hold there. The equivalence battery proves
// language-output invisibility separately.
#[cfg(all(test, not(feature = "seam-force-copy")))]
mod tests;

impl<'a> Scope<'a> {
    /// The live [`FrameStorage`] owning this scope's region — the pin every read of a resident
    /// carrier opens under. A live scope reference implies a live owner: the cart, a cart ancestor
    /// (through the `FrameStorage.outer` chain), or the run storage holds it for as long as the
    /// scope can run. Stated once here for the whole reach cluster;
    /// [`scope_frame`](crate::machine::core::scope_frame) is the crate-wide twin, which spells the
    /// same invariant out against the step context.
    fn home(&self) -> Rc<FrameStorage> {
        self.region_owner()
            .upgrade()
            .expect("a live scope reference implies a live region owner")
    }

    /// Mint `sources` into this scope's own arena, which is the same act that folds the composed
    /// bundle into the **region's** union bundle — the scope-side reach-derivation door, a veneer
    /// over the library's fused
    /// [`RegionHandle::mint_retained`](crate::witnessed::RegionHandle::mint_retained). Each source is
    /// a caller's owned claim (a delivery envelope's whole coverage, or a release-exact subset of
    /// it), which already names the value's home region as an ordinary member: there is no residence
    /// mode to choose.
    ///
    /// Returns the hosted description — the move-in's audit evidence and the resident seal's
    /// witness, derived here so no caller pairs a value with a reach some other value derived. It
    /// records **two** facts: its host is this scope's own region owner (the value's residence,
    /// stamped by the mint from the destination it freezes into) and its members are the regions the
    /// value's borrows reach, this scope's own among them exactly when a source claim names it. No
    /// omission policy: the mint applies subsumption and the self rule alone, so the description is
    /// the value's exact reach.
    ///
    /// Every value gets one, `sources` empty or not — a region-pure value's description has no
    /// members and still records where the value lives.
    ///
    /// The description is arena-hosted for the region's life and non-owning (`Weak` host and
    /// members); the owning bundle never crosses back out — the library folds it straight into the
    /// region's union bundle, which dedupes by region identity with outer-chain subsumption, so one
    /// owning `Rc` per distinct region covers everything resident here and drops whole at region
    /// death. Binding entries own nothing, and since a binding never dies before its scope, that
    /// pins no longer than a per-entry bundle would.
    pub(crate) fn mint_retained(&self, sources: &[&FrameCoverage]) -> &'a FrameReach {
        self.brand().handle().mint_retained(sources)
    }

    /// Test fixture: the description for a value **born in this scope's own region**, naming that
    /// same region as a member exactly when `borrows_home`. Production derives a value's reach from
    /// the composition that placed it — a fold, a merge, or a resident seal — so this hand-paired
    /// form exists only for a suite that allocates its own value and needs a description to seal it
    /// under ([`Self::seal_reaching`]).
    #[cfg(test)]
    pub(crate) fn mint_born_here(&self, borrows_home: bool) -> &'a FrameReach {
        match borrows_home {
            true => {
                let home = FrameCoverage::of(self.home());
                self.mint_retained(&[&home])
            }
            false => self.mint_retained(&[]),
        }
    }

    /// Fuse a value living **in this scope's region and reaching nothing** to its reference-only
    /// carrier — region-pure and generic over the carried family, for a terminal that hands a
    /// resident value out of a step un-sealed (a type terminal's `Carried::Type`, a relocation's
    /// bare destination handle). The description is minted on the confined arena surface
    /// ([`RegionBrand::seal_resident`]), so the residence is derived where the region handle is,
    /// never assembled at a call site.
    ///
    /// What "reaching nothing" covers, by family:
    ///
    /// - **Type carrier** (`CarriedFamily`, a `KType`): a `KType` is owned data, so the read pins no
    ///   foreign region and travels under the home-frame pin alone (the envelope host
    ///   [`Self::deliver_resident`] adds); the `Copy` handle rides in place, never re-cloned
    ///   into the region.
    /// - **Destination handle** (`RegionHandleFamily`): a bare region handle
    ///   borrows nothing at all.
    ///
    /// A **callable** does not qualify and takes no door here: a `KFunction` borrows its captured
    /// scope, so its description is composed by the birth that placed it
    /// ([`KFunction::alloc_captured`](crate::machine::core::KFunction)) and the registration doors
    /// rest that envelope. Likewise an **operator-group record**, whose region-purity is the yoke
    /// brand's compile-time fact rather than a claim ([`Self::birth_operator_group`]).
    ///
    /// A **value carrier** whose borrows do reach somewhere is sealed by the composition that placed
    /// it, under the description [`Self::mint_retained`] derives from that composition's own
    /// operands. Its carrier pins nothing either — the reached regions are owned by this region's
    /// union bundle — so an entry read is a pointer copy, and a read that leaves the container
    /// re-owns the reach by lifting into a [`DeliveredCarried`] envelope ([`Self::lift_resident`]).
    pub(crate) fn resident<'v: 'a, T: Reattachable + DropFree>(
        &self,
        value: T::At<'v>,
    ) -> Witnessed<T, CarrierWitness> {
        self.brand().seal_resident(value)
    }

    /// [`Self::resident`], sealed into its dormant binding form — the door a region-pure value bind
    /// writes through ([`Self::seal_pure_value`]).
    pub(crate) fn seal_resident<'v: 'a, T: Reattachable + DropFree>(
        &self,
        value: T::At<'v>,
    ) -> Sealed<'a, T, CarrierWitness> {
        Sealed::seal(self.resident(value), self.brand().handle())
    }

    /// Test fixture: seal a value living in this scope's region under a description hand-minted for
    /// it ([`Self::mint_born_here`], [`Self::mint_retained`]), where [`Self::seal_resident`] is the
    /// region-pure production door. `reach` must be this scope's own mint for this same value, which
    /// is what makes the residence it stamps the value's own. Production reaches this shape through
    /// the composition that placed the value instead, so no call site pairs a value with a
    /// description some other value derived.
    ///
    /// When `self` is a transparent window over borrowed bindings ([`Self::child_transparent`]), a
    /// binding read out of the window carries a description minted into the *owning* (module)
    /// scope's own arena, not the call site's — the binding was minted there at the module's own
    /// bind time, and that arena is where its host names the module's frame. Sound because the
    /// window's overlay reach-fold (`USING`'s body, `builtins/using_scope.rs`) mints the opened
    /// module's own carrier into the call-site arena at overlay construction, before any such
    /// carrier exists — so holding the call-site frame roots the module's arena one hop removed, and
    /// through it the description's pointee.
    #[cfg(test)]
    pub(crate) fn seal_reaching<'v: 'a, T: Reattachable + DropFree>(
        &self,
        value: T::At<'v>,
        reach: &'a FrameReach,
    ) -> Sealed<'a, T, CarrierWitness> {
        Sealed::seal(
            self.brand().seal_reaching(value, reach),
            self.brand().handle(),
        )
    }

    /// **Open** a dormant function carrier at this scope's own region lifetime: lift it into a
    /// delivery envelope under its home pin, then adopt it here
    /// ([`Delivered::open_adopted`]) — the mint stores its reach in this arena and the region
    /// retains the owning bundle for the region's life, so the returned open reads freely for the
    /// whole step rather than inside a pin borrow. `self` must be the scope the seal was read from
    /// (or one its region outlives): the lift upgrades the description hosted there.
    pub fn open_function(&self, sealed: &SealedFunction) -> OpenedFunction<'a> {
        self.lift_resident(sealed.duplicate())
            .open_adopted(self.brand().handle())
    }

    /// Read a dormant function carrier under this scope's own region owner — the probe form of
    /// [`Self::open_function`], for a caller that only inspects the signature. Nothing is minted and
    /// nothing escapes: the `for<'b>` brand confines the re-anchored callable to the call.
    pub fn read_function<R>(
        &self,
        sealed: &SealedFunction,
        read: impl for<'b> FnOnce(&'b KFunction<'b>) -> R,
    ) -> R {
        sealed.open(read)
    }

    /// Adopt a delivered value **as a callable** into this scope — the head-resolution twin of
    /// [`Self::open_function`], for a lane whose callable arrives in an envelope rather than out of
    /// a dispatch bucket (a value-bound head, a deferred head expression). The envelope is
    /// projected onto the `KFunction` it wraps *inside* its own coverage
    /// ([`Delivered::project`], which moves nothing and keeps the envelope's pins), then adopted
    /// here, so the callable's captured foreign environment is retained for this region's life and
    /// the returned open reads at this scope's lifetime. `None` when the value is not callable —
    /// the caller falls back to the whole-value classification for its diagnostic.
    pub(crate) fn adopt_delivered_function(
        &self,
        cell: &DeliveredCarried,
    ) -> Option<OpenedFunction<'a>> {
        let callable = cell.open(|live| matches!(live.as_object(), Some(KObject::KFunction(_))));
        if !callable {
            return None;
        }
        let function = cell.duplicate().project::<KFunctionFamily>(|live, _| {
            match live.as_object() {
                Some(KObject::KFunction(f)) => f,
                // The probe above read this same envelope: its value is a callable.
                _ => unreachable!("the callable probe ran over this envelope"),
            }
        });
        Some(function.open_adopted(self.brand().handle()))
    }

    /// Drop a delivered value **to rest** in this scope's own region, keeping the sealed cell alone
    /// ([`Delivered::rest_in`]) — the splice-install door. The envelope's whole coverage (the
    /// producer's region among its members) is lodged in this region's union bundle, so the returned
    /// [`SplicedCell`] is pure data: `Copy`, `Drop`-free, and readable for as long as this region
    /// lives. A value already resident here rests for free — the library's self rule strips this
    /// region from what is retained.
    ///
    /// The counterpart read is [`Self::lift_spliced`], an adoption, which names this scope's own
    /// region owner as its pin — what the retention above makes sufficient.
    pub(crate) fn rest_delivered(&self, cell: &DeliveredCarried) -> SplicedCell<'a> {
        cell.rest_in(self.brand().handle())
    }

    /// **Lift-then-rest as one door**: recover a resting splice cell (a resolved dep terminal's)
    /// into its producer's delivery envelope and rest that envelope into this scope's region — the
    /// whole coverage moving into the region's union bundle, so the value's backing stays retained
    /// until the consumer reads it. Fusing [`Self::lift_spliced`] with [`Self::rest_delivered`]
    /// removes the possibility of resting an envelope lifted under a different scope.
    pub(crate) fn rest_spliced(&self, cell: &SplicedCell) -> SplicedCell<'a> {
        self.rest_delivered(&self.lift_spliced(cell))
    }

    /// **Lift** a resting splice cell back into a delivery envelope owning its whole reach
    /// ([`Opened::lift_out`]) — the read door for a consumer that goes on to *adopt* the value, which
    /// needs the reach owned rather than merely named. The cell's own description supplies both its
    /// residence and its members, so nothing is paired with a reach some other value derived.
    ///
    /// `self` must be a scope whose region keeps the cell's producer alive: the region the cell was
    /// rested into ([`Self::rest_delivered`]), whose union bundle holds an `Rc` on every region the
    /// cell reaches, or one of its descendants. Holding this scope holds that region's owner, which
    /// is what makes the `Weak → Rc` upgrade behind the lift succeed.
    pub(crate) fn lift_spliced(&self, cell: &SplicedCell) -> DeliveredCarried {
        cell.open_at().lift_out()
    }

    /// Read a resting splice cell for a **verdict only** — the reader extracts region-free data (an
    /// interned [`KType`] handle, a rendered summary) and adopts nothing, so it needs the cell's
    /// pointee live for the read and no reach owned afterwards. The pin is this scope's own region
    /// owner, held across the whole call, which is what a rank-2 `for<'b>` read confines the value
    /// to.
    ///
    /// `self` must be a scope whose **own region hosts the cell** — the same relation
    /// [`Self::seal_resident`] establishes when it seals one. That is stricter than
    /// [`Self::lift_spliced`]'s (which also accepts a descendant of the resting region) because
    /// nothing here upgrades the cell's members: the pin names one region, so it must be the one
    /// the description lives in.
    ///
    /// The pin-less twin ([`read_resting`](crate::machine::core::read_resting)) stays for probes
    /// reached from signatures that carry no scope at all.
    pub(crate) fn read_spliced<R>(
        &self,
        cell: &SplicedCell,
        read: impl for<'b> FnOnce(Carried<'b>) -> R,
    ) -> R {
        cell.open(read)
    }

    /// **Lift** a binding's dormant carrier into a delivery envelope pinned by this scope's own
    /// region owner (`Sealed → Delivered`): the library [`Delivered::lift`] upgrades the sealed
    /// description's members `Weak → Rc` under that pin, so the value's whole reach travels owned
    /// and the envelope survives its source frame's death. `self` must be the **binding** scope —
    /// the region the value lives in, whose arena hosts the description the upgrade reads.
    pub(crate) fn lift_resident<T: Reattachable + DropFree>(
        &self,
        sealed: Sealed<T, CarrierWitness>,
    ) -> Delivered<T, CarrierWitness, FrameStorage> {
        Delivered::lift(crate::witnessed::Retained::from_sealed(sealed), self.home())
    }

    /// [`Self::resident`] handed out as a delivery envelope — the same value, the same member-less
    /// description, now pinned by this scope's own region owner. The resident twin of the
    /// scheduler's [`edge_resident`](crate::scheduler::Scheduler::edge_resident): a spliced resident
    /// cell travels self-covering by its own witness *and* pinned by its home, identical in shape to
    /// a delivered dep — there is no `pin: None` resident special case at the splice sites.
    ///
    /// Takes the value alone. The description's residence, the seal and the home pin all come off
    /// the library door on this scope's own region handle
    /// ([`RegionHandle::deliver_resident`](crate::witnessed::RegionHandle::deliver_resident)), so
    /// there is no home to pass and no coverage to assemble: a value reaching nothing beyond the
    /// region it lives in is covered by that region alone.
    ///
    /// Generic over the carrier's family so a **destination operand** travels the way a value does:
    /// a relocation's dest is a bare region handle living in this scope's own region, and the
    /// composition verbs take their destination as an envelope.
    pub(crate) fn deliver_resident<'v: 'a, T: Reattachable + DropFree>(
        &self,
        value: T::At<'v>,
    ) -> Delivered<T, CarrierWitness, FrameStorage> {
        self.brand().deliver_resident(value)
    }

    /// Build an object into this scope's own region through a **zero-dep fold** and hand back the
    /// resident borrow. The door for a value that is region-pure in the sense that matters — every
    /// borrow it carries points into this region — but not `'static`, because a string literal's
    /// bytes are bumped here ([`RegionBrand::allocator`](crate::machine::core::RegionBrand::allocator)).
    ///
    /// The fold brand discharges the residence obligation at compile time — an ambient borrow cannot
    /// inhabit `KObject<'b>` — and the product is re-anchored at `'a` through the library's own fused
    /// adopt door. With no deps the fold composes no reach, so the adoption retains nothing and the
    /// value's residence stays exactly this region.
    pub(crate) fn fold_resident_object(
        &self,
        build: impl for<'b> FnOnce(FoldingBrand<'b>) -> KObject<'b>,
    ) -> &'a KObject<'a> {
        KoanRegion::fold_witnessed(self.home(), |brand| {
            Carried::Object(brand.alloc_object_folded(build(brand)))
        })
        .adopt_into(self.brand().handle())
        .object()
    }

    /// Envelope a value already living in this scope's own region and reaching nothing — the
    /// delivery form of [`Self::resident`] over the value channel, pinned by this scope's own home
    /// frame under an empty foreign bundle. The door a producer hands a freshly placed value out
    /// through when its consumer binds at a `for<'b>` brand
    /// ([`CallFrame::with_scope`](crate::machine::CallFrame::with_scope)): a bare `&'a KObject<'a>`
    /// cannot cross that signature, while an envelope crosses as the witnessed shortening every
    /// other delivered value takes.
    pub(crate) fn deliver_resident_object(&self, object: &'a KObject<'a>) -> DeliveredCarried {
        self.deliver_resident::<CarriedFamily>(Carried::Object(object))
    }

    /// Place a **carrier-less** argument's value in this scope's own region. The shape is the
    /// enforcement: a value that arrives with no reach carrier is one the argument view's carrier
    /// contract (`BoundArgs::carrier` returning `None`) calls region-pure, and the arms here are
    /// exactly the shapes a region-pure value can take — each placed through a door whose signature
    /// proves the product borrows only this region.
    ///
    /// - A **scalar** (`Number` / `Bool` / `Null`) is owned data; the zero-dep fold rebuilds it here.
    /// - A **`KString`**'s bytes are re-bumped into this region by the same fold, never shared with
    ///   whatever bump the source lives in.
    /// - A **`KExpression`** is raw AST, which names no producer region, so it takes the expression
    ///   door ([`RegionBrand::alloc_expression`](crate::machine::core::RegionBrand::alloc_expression))
    ///   whose signature admits nothing else.
    ///
    /// Every other shape borrows a region this door cannot name — a callable's captured scope, a
    /// module's child scope, a substrate carrier's cells — so it reaches its destination as a
    /// delivery envelope instead, and arriving here is a construction bug the diagnostic reports.
    pub(crate) fn place_pure_value(&self, value: &KObject<'a>) -> Result<&'a KObject<'a>, KError> {
        match *value {
            KObject::Number(n) => Ok(self.fold_resident_object(|_| KObject::Number(n))),
            KObject::Bool(b) => Ok(self.fold_resident_object(|_| KObject::Bool(b))),
            KObject::Null => Ok(self.fold_resident_object(|_| KObject::Null)),
            KObject::KString(text) => {
                Ok(self
                    .fold_resident_object(|brand| KObject::KString(brand.allocator().text(text))))
            }
            KObject::KExpression(expression) => Ok(self.brand().alloc_expression(expression)),
            _ => Err(KError::new(crate::machine::KErrorKind::ShapeError(
                "internal: a carrier-less argument reached the pure-value door carrying a region \
                 borrow; a value that reaches a region travels as a delivery envelope"
                    .to_string(),
            ))),
        }
    }

    /// [`Self::place_pure_value`] sealed into its dormant binding form — the region-pure twin of
    /// [`Self::adopt_for_binding`]. The product borrows only this region, so it seals resident: the
    /// description records where the value lives and names no member.
    pub(crate) fn seal_pure_value(&self, value: &KObject<'a>) -> Result<SealedValue<'a>, KError> {
        Ok(self.seal_resident::<CarriedFamily>(Carried::Object(self.place_pure_value(value)?)))
    }

    /// [`Self::place_pure_value`] handed out as a delivery envelope
    /// ([`Self::deliver_resident_object`]) — the read-site door for a builtin whose lhs arrived
    /// carrier-less and whose consumer takes an envelope operand.
    pub(crate) fn deliver_pure_value(
        &self,
        value: &KObject<'a>,
    ) -> Result<DeliveredCarried, KError> {
        Ok(self.deliver_resident_object(self.place_pure_value(value)?))
    }

    /// Adopt a delivered carrier into this scope for **consumption** — the door whose product is a
    /// live [`Carried`] the caller goes on to use (a bare-name read, a head-callable, a spliced
    /// argument). `seam` selects the policy ([`adopt_disposition`] is the single home of the rules);
    /// this door then runs the mechanism the chooser named.
    ///
    /// The **type channel** never reaches the chooser ([`Self::adopt_type_channel`]): its content is
    /// copied out for every seam, so no reach is minted and the producer's region is not pinned.
    ///
    /// Where [`seal_resident`](Self::seal_resident) seals a value already living **in** this
    /// region, adoption is the consumption verb for a carrier produced **elsewhere**.
    pub(crate) fn adopt_carried(&self, cell: &DeliveredCarried, seam: AdoptSeam) -> Carried<'a> {
        if let Some(copied) = self.adopt_type_channel(cell) {
            return copied;
        }
        let disposition = cell.open(|live| adopt_disposition(cell, live.object(), &seam));
        match disposition {
            // The whole envelope is adopted in place through the library's fused mint-and-retain
            // door: nothing is copied, so there is nothing to relocate.
            AdoptDisposition::Pin => cell.adopt_into(self.brand().handle()),
            AdoptDisposition::Relocate => self.adopt_copied(cell),
        }
    }

    /// Adopt a delivered carrier into this scope by **copy**: the relocation lands the value in this
    /// scope's own region under the fold brand and its composition retains the copy's reach here for
    /// the region's life; adopting the product envelope re-anchors it at `'a` through the library's
    /// own fused mint-and-retain door, so no re-box is needed to recover the reference. The
    /// producer's region is not part of the copy's residence — the copy carries its own release-exact
    /// reach — so the source envelope's hold is **released** when the caller drops it. Released, not
    /// discarded: the copy retains everything it still reaches.
    fn adopt_copied(&self, cell: &DeliveredCarried) -> Carried<'a> {
        self.relocate_delivered(cell, |carried| Ok(carried.object()), RegionEscape::Copy)
            .expect("a whole-value adoption's copy is infallible")
            .adopt_into(self.brand().handle())
    }

    /// Test affordance: [`Self::adopt_copied`] as a door of its own, for the harness terminal
    /// extractor — which needs the copy (a scalar terminal must not keep its producer frame alive)
    /// with no binding entry to hold the seal. `#[cfg(test)]`-gated because production has no
    /// copying consumption seam: a call's arguments are copied by the frame bind itself, off the
    /// envelope they were delivered in, so nothing re-homes a delivered value ahead of the door that
    /// binds it.
    #[cfg(test)]
    pub(crate) fn adopt_copied_for_test(&self, cell: &DeliveredCarried) -> Carried<'a> {
        self.adopt_type_channel(cell)
            .unwrap_or_else(|| self.adopt_copied(cell))
    }

    /// The type channel's adoption, `None` for an object envelope: a `KType` is a lifetime-free
    /// handle and a `TypeIdentifier` is a bare surface name, so the envelope is opened and its
    /// content copied out — the handle by value, the name's bytes bumped into this scope's own
    /// region. The product borrows only this region, so no reach is minted.
    fn adopt_type_channel(&self, cell: &DeliveredCarried) -> Option<Carried<'a>> {
        /// The content copied out of a type-channel envelope: a `Copy` `KType` handle, or an
        /// unlowered surface name whose bytes are re-bumped into this scope's region.
        enum AdoptedType<'t> {
            Lowered(KType),
            Unlowered(TypeIdentifier<'t>),
        }

        let brand = self.brand();
        let copied = cell.open(|live| match live {
            Carried::Type(kt) => Some(AdoptedType::Lowered(kt)),
            // The name is read at the envelope's own brand, so it is copied here rather than
            // carried out: the product names only this region.
            Carried::UnresolvedType(ti) => Some(AdoptedType::Unlowered(TypeIdentifier::leaf(
                brand.allocator().text(ti.as_str()),
            ))),
            Carried::Object(_) => None,
        });
        match copied {
            Some(AdoptedType::Lowered(handle)) => Some(Carried::Type(handle)),
            Some(AdoptedType::Unlowered(ti)) => Some(Carried::UnresolvedType(ti)),
            None => None,
        }
    }

    /// Adopt a delivered value's **projection** into this scope for **binding** — the door whose
    /// product is the dormant [`SealedValue`] a binding entry stores. `project` selects what to bind
    /// (identity for a whole-value bind, a `Tagged`/`Wrapped` payload for a MATCH/TRY `it`, the
    /// Ok/Err payload for TRY), read under the envelope's own pin.
    ///
    /// The seam is [`AdoptSeam::Binding`], the only one that admits the escape-seam cost chooser:
    /// the adopting scope's region union owns the minted reach for the region's life, so pinning a
    /// record in its producer region is affordable here. [`adopt_disposition`]
    /// picks; this door runs the mechanism:
    ///
    /// Both arms run the same engine ([`Self::relocate_delivered`]), differing only in the verb, and
    /// both hand back a product envelope that is already dest-resident with its composed reach
    /// retained in this region — so the product's cell **is** the binding entry: it seals straight
    /// into the table with no re-box and no move-in audit.
    ///
    /// - **Relocate** — the copy verb, which rebuilds what would otherwise keep region storage
    ///   behind at the destination door ([`KObject::needs_destination_door`]) and pointer-copies
    ///   everything else. The composition's retention claim is release-exact, so a plain-data copy
    ///   lets the producer's region free.
    /// - **Pin** — the record stays in its producer region and the projection is pointer-copied at
    ///   the fold brand, its substrate borrow riding verbatim. The composition keeps every member
    ///   the envelope named, the producer's region among them, which is the pin's liveness — and
    ///   the region's union bundle owns it for this region's life.
    pub(crate) fn adopt_for_binding<P>(
        &self,
        cell: &DeliveredCarried,
        project: P,
    ) -> Result<SealedValue<'a>, KError>
    where
        P: for<'b> Fn(&Carried<'b>) -> Result<&'b KObject<'b>, KError>,
    {
        let disposition = cell.open(|live| match project(&live) {
            Ok(object) => adopt_disposition(cell, object, &AdoptSeam::binding()),
            // A projection failure surfaces from the engine below, which runs `project` again under
            // the fold's own pin and smuggles its `KError` back out; either verb does, so the
            // copying one stands in.
            Err(_) => AdoptDisposition::Relocate,
        });

        let verb = match disposition {
            AdoptDisposition::Relocate => RegionEscape::Copy,
            AdoptDisposition::Pin => RegionEscape::Pin,
        };
        Ok(self
            .relocate_delivered(cell, project, verb)?
            .rest_into(self.brand().handle()))
    }

    /// Relocate a delivered value's **projection** into this scope's region through the fold door,
    /// under the escape `verb` the caller's disposition names. `project` selects what to move;
    /// [`relocate_object_into`] runs the verb — a `Copy` totally rebuilds whatever would otherwise
    /// keep region storage behind ([`KObject::needs_destination_door`]) so it lands here, a `Pin`
    /// pointer-copies the top node and lets its region-resident substrate borrow ride.
    ///
    /// The verb also fixes the retention claim the fold hands its composition, under the release
    /// rule ([value-substrates.md § Sectioned reach](../../../../design/value-substrates.md#sectioned-reach)):
    /// a `Copy`'s predicate runs over the rebuilt value, so a plain-data record drops the producer
    /// region and a tail loop's retiring frame does not ride the binding, while a `Pin` keeps every
    /// region the source envelope named — which is what covers the pointer-copied substrate still
    /// living in the producer's region.
    ///
    /// The fold's own composition mints the product's exact reach into this scope's arena and
    /// retains the owning bundle here for the region's life, so the witnessed product is already
    /// the finished carrier: it is enveloped under this scope's region owner and handed back. There
    /// is **no re-box**: the value the caller consumes is the one the fold brand allocated, and the
    /// brand's rank-2 signature is what proves it borrows only the fold's declared operands.
    ///
    /// The private engine under both adopt doors: [`AdoptDisposition`] names the verb, this runs it.
    fn relocate_delivered<P>(
        &self,
        cell: &DeliveredCarried,
        project: P,
        verb: RegionEscape,
    ) -> Result<DeliveredCarried, KError>
    where
        P: for<'b> Fn(&Carried<'b>) -> Result<&'b KObject<'b>, KError>,
    {
        // The destination operand is this scope's own region handle, whose residence the composition
        // gives the product.
        let dest = self.dest_operand();
        let mut projection_error: Option<KError> = None;
        // The rebuild's cells read their own stored reach at the door; `cell`'s coverage is the
        // holder-rule proof for a cell whose substrate stays foreign, captured here because a
        // `for<'b>` fold closure has no route back to its operand's pins.
        let holder = cell.coverage().clone();
        // The destination is a bare region handle (empty reach), so its operand bundle is empty and
        // the composition mints exactly what the verb's retention claim keeps — release-exact for a
        // Copy, everything the envelope named for a Pin. The composition also retains its bundle in
        // this scope's region, which is what covers the relocated value read in place.
        let copied = cell
            .transfer_into::<RegionHandleFamily<KoanStorageProfile>, CarriedFamily, KoanStorageProfile>(
                dest,
                |product, region| match verb {
                    RegionEscape::Pin => true,
                    RegionEscape::Copy => {
                        product_reaches_region(cell, product.as_object(), region)
                    }
                },
                |value, _handle, placement| {
                    let door = FoldingBrand::in_fold_closure(placement).with_holder(&holder);
                    match project(&value) {
                        Ok(record) => Carried::Object(
                            door.alloc_object_folded(relocate_object_into(record, verb, door)),
                        ),
                        Err(error) => {
                            projection_error = Some(error);
                            Carried::Object(door.alloc_object_folded(KObject::Null))
                        }
                    }
                },
            );
        if let Some(error) = projection_error {
            return Err(error);
        }
        // The product lives in this scope's region and its composed reach is already retained here.
        // The transfer gives the product the destination operand's own residence — this scope's
        // region owner — so the envelope it hands back is already the finished one.
        Ok(copied)
    }

    /// This scope's own region handle as a destination operand — the envelope every fold door here
    /// merges into. A bare handle borrows nothing, so its coverage is empty and the composition
    /// mints exactly the source operands' reach into this region, stamping the product with this
    /// scope's own residence.
    pub(crate) fn dest_operand(
        &self,
    ) -> Delivered<RegionHandleFamily<KoanStorageProfile>, CarrierWitness, FrameStorage> {
        self.deliver_resident::<RegionHandleFamily<KoanStorageProfile>>(self.brand().handle())
    }

    /// Wrap a freshly born `KFunction` in its `KObject` carrier — the store every `FN` / `OP`
    /// registration hands its callable out through. `cell` is the birth envelope
    /// ([`KFunction::alloc_captured`](crate::machine::core::KFunction)), so the wrapper's
    /// composition takes the callable's *own composed* description as its source operand and
    /// **merges** it into this scope's region. The borrows-home fact the wrapper carries therefore
    /// arrives from the birth rather than being restated here. Source and destination coincide, so
    /// the library's self rule strips the region from the retained bundle — a callable never pins
    /// the region it lives in.
    ///
    /// The claim is exact. A `KFunction`'s only region borrow is its captured scope, and that
    /// scope's own sealed reach-set transitively keeps every foreign region its bindings reach
    /// alive, so naming the callable's home region names the whole closure.
    ///
    /// Infallible, and check-free: the wrapping `KObject::KFunction` is built at the fold brand from
    /// the merge's own operand view, so an ambient-lifetime capture is a compile error at the
    /// closure's signature.
    pub(crate) fn store_function_cell(&self, cell: &DeliveredFunction) -> SealedValue<'a> {
        cell.duplicate()
            .merge_into::<RegionHandleFamily<KoanStorageProfile>, CarriedFamily, KoanStorageProfile>(
                self.dest_operand(),
                |function_view, _handle, placement| {
                    let door = FoldingBrand::in_fold_closure(placement);
                    Carried::Object(door.alloc_object_folded(KObject::KFunction(function_view)))
                },
            )
            .rest_into(self.brand().handle())
    }

    /// **Birth an operator-group record in this scope's own region.** The record is built inside a
    /// [`yoke_branded`](KoanRegionExt::yoke_branded) whose `for<'b>` brand is the region-purity
    /// proof: [`OperatorGroup::alloc`] re-homes the member texts and any combiner name at that same
    /// brand, so the finished record borrows nothing but the region it was born into, and the
    /// closure has no way to smuggle an ambient borrow into `&'b OperatorGroup<'b>`.
    ///
    /// The envelope carries the description the yoke composed — hosted here, no members — which is
    /// what [`GroupSeal::of_delivered`](crate::machine::core::carrier_witness::GroupSeal) rests into
    /// the registry rather than minting a claim of its own. `members` and `mode` are read at their
    /// own ambient lifetimes: only what the closure *returns* is confined by the brand.
    pub(crate) fn birth_operator_group(
        &self,
        members: &[&str],
        mode: ReductionMode<'_>,
    ) -> DeliveredOperatorGroup {
        KoanRegion::yoke_branded::<OperatorGroupFamily, _>(self.home(), |brand| {
            OperatorGroup::alloc(brand, members, mode)
        })
    }

    /// Adopt a freshly born group record at this scope's own region lifetime — the door
    /// [`Scope::alloc_group_child`](crate::machine::core::Scope) takes when it needs the record
    /// itself (the child scope stores a bare `&'a OperatorGroup<'a>`) alongside the seal its
    /// registry entries hold. `self` must be the region the record was born into; the adoption's
    /// mint is a self-rule no-op there.
    pub(crate) fn adopt_group_record(
        &self,
        cell: &DeliveredOperatorGroup,
    ) -> &'a OperatorGroup<'a> {
        cell.adopt_into(self.brand().handle())
    }

    /// Seal a resident `Module` value into this scope — the Object-arm module bind
    /// ([`Scope::seal_module`]) and an opaque ascription view. `module` lives in its own child
    /// scope's region — [`Module::alloc_at_child_scope`](crate::machine::model::Module) derives the
    /// destination from that scope, and the module carries it
    /// ([`Module::child_scope`](crate::machine::model::Module::child_scope)), so the door reads the
    /// home off the value rather than taking it as a parameter a caller could mismatch. It
    /// envelopes that reference at the child's own home and **merges** it into this scope's region:
    /// the composition mints the child's region as a member of the product's reach and retains it
    /// here for this region's life.
    ///
    /// That claim is exact. A `KObject::Module`'s only region borrow is its child scope, and every
    /// member value the module surfaces lives inside that child's own bindings; the child's region
    /// owns the union bundle for everything those members reach, so naming it pins the whole member
    /// closure. A co-located module (`MODULE`, opaque `:|`) names a region this scope's chain
    /// already holds — the library's self rule strips it from the retained bundle.
    ///
    /// Infallible, and check-free: the wrapping `KObject::Module` is built at the fold brand from
    /// the merge's own operand view, so an ambient-lifetime capture is a compile error at the
    /// closure's signature.
    pub(crate) fn store_module_object(&self, module: &'a Module<'a>) -> SealedValue<'a> {
        let child = module.child_scope();
        let source = child.deliver_resident::<ModuleRefFamily>(module);
        source
            .merge_into::<RegionHandleFamily<KoanStorageProfile>, CarriedFamily, KoanStorageProfile>(
                self.dest_operand(),
                |module_view, _handle, placement| {
                    let door = FoldingBrand::in_fold_closure(placement);
                    Carried::Object(door.alloc_object_folded(KObject::Module(module_view)))
                },
            )
            .rest_into(self.brand().handle())
    }

    /// Open the module `delivered` carries as a `USING … SCOPE` window on this scope — the root that
    /// keeps the opened module's region alive, the borrowed-bindings window
    /// ([`Scope::alloc_child_transparent`]), and the owned block layer stacked inside it, in that
    /// order, as one act. The **block scope** is what comes back: the window stays an internal
    /// middle link, so no caller can write into a borrowed table. `None` if the envelope carries
    /// something other than a module.
    ///
    /// The two-scope stack is the whole visibility story. A block statement binds into the block
    /// scope at its own plain statement index, and the resolver walk — block, then window, then the
    /// call-site chain — gives the block its earlier binds, the module's members, and the call site,
    /// in that precedence. Statements after the block never reach the block scope on their ancestor
    /// walk, so a block bind's death at block close is structural.
    ///
    /// The window surfaces the module's members by *borrowing* its child scope's binding table, so
    /// the window's reads are only as valid as the module's own region. That region is pinned by the
    /// eager `m` argument's delivery envelope, which dies with the step that opened the window,
    /// while the block runs in later steps — so the envelope's coverage is minted into this scope's
    /// region, whose union then roots the module's region for that region's life. Both children are
    /// same-region with this scope, so they inherit that root as their own; an escaping closure
    /// captures the block scope, which anchors the call-site frame, which pins the folded region.
    ///
    /// **One operand carries both facts.** The table is read off the module inside the envelope
    /// ([`Module::child_scope`](crate::machine::model::Module::child_scope)) and the root is that
    /// same envelope's coverage, so there is no pair of arguments a caller could draw from two
    /// different values — and the mint runs before the window that borrows into it exists.
    ///
    /// The envelope is required, not optional: the module argument fills a value slot of a
    /// non-name-literal type, so every part shape that can carry a module into it — a bare name
    /// through the auto-wrap rail, a `(…)` through the eager rail — is spliced before the call, and
    /// a spliced part always delivers. A co-located module's coverage is stripped by the library's
    /// self rule, so "nothing to root" is an empty member set, not an absent envelope.
    pub(crate) fn open_module_window(
        &'a self,
        delivered: &DeliveredCarried,
    ) -> Option<&'a Scope<'a>> {
        // The table crosses the born door as an erased operand: it lives in the opened module's own
        // region, so it is re-anchored at the construction brand rather than at an ambient `'a` it
        // has no outlives relation to. The open's pins cover the read, and the mint below keeps the
        // pointee live for the re-anchor inside the door.
        let opened = delivered.open_at();
        let Carried::Object(KObject::Module(module)) = opened.value() else {
            return None;
        };
        let bindings =
            SealedExtern::<BindingsReferenceFamily>::erase(module.child_scope().bindings());
        // Root first. Non-owning mint: the owning bundle folds straight into this region's union, so
        // the root outlives every read through the window rather than the returned description.
        self.mint_retained(&[delivered.coverage()]);
        let window = self.alloc_child_transparent(bindings);
        Some(window.alloc_child_under())
    }

    /// The transparent-ascription store: a fresh `Module` tagged `name`, re-tagging a *foreign*
    /// source module's child scope, whose region is not this scope's own. The re-tagged `Module` is
    /// **built inside the fold** over that child scope as its operand view, so its borrow is the
    /// merge's own — which is what lets the composition, rather than a runtime walk, evidence the
    /// foreign region. A module is assembled complete, so `self_sig` derives the view's self-sig
    /// from the operand view inside the fold and the module is born carrying it; taking the scope at
    /// the fold's own brand lifetime is what keeps the derivation from smuggling a borrow out.
    ///
    /// Both the `Module` and the `KObject::Module` wrapping it are allocated at the same fold brand
    /// into this scope's region — including the `name` bytes and the (empty) member tables
    /// [`Module::assemble`](crate::machine::model::Module) bumps there — and the composed reach
    /// names the source child's region, the one claim covering both, minted and retained here in one
    /// act.
    pub(crate) fn store_transparent_view(
        &self,
        name: String,
        source_child: &'a Scope<'a>,
        self_sig: impl for<'b> FnOnce(&'b Scope<'b>) -> KType,
    ) -> SealedValue<'a> {
        let source = source_child.deliver_resident::<ScopeRefFamily>(source_child);
        source
            .merge_into::<RegionHandleFamily<KoanStorageProfile>, CarriedFamily, KoanStorageProfile>(
                self.dest_operand(),
                |scope_view, _handle, placement| {
                    let door = FoldingBrand::in_fold_closure(placement);
                    let sig = self_sig(scope_view);
                    let module = door.alloc_module_folded(Module::assemble(
                        *door,
                        &name,
                        scope_view,
                        ModuleDraft::empty(),
                        sig,
                    ));
                    Carried::Object(door.alloc_object_folded(KObject::Module(module)))
                },
            )
            .rest_into(self.brand().handle())
    }
}

/// Which seam is adopting — the one axis the adoption policy turns on, beside the shape of the
/// value itself.
///
/// [`Retaining`](Self::Retaining) means the adopting scope retains the minted reach in its region's
/// union bundle, so pinning the value in place is affordable: the dep survives past its resolving
/// step as its carrier rather than as a relocated copy (a bare-name read, the head-deferred
/// callable, a spliced argument).
///
/// [`Binding`](Self::Binding) additionally admits the record cost chooser. It carries a
/// [`BindSeam`] admission token whose field is private to this module, so no caller outside the
/// module can select cost-driven record pinning.
pub(crate) enum AdoptSeam {
    Retaining,
    Binding(BindSeam),
}

/// Admission token for [`AdoptSeam::Binding`]: an empty struct whose field is private to this
/// module, so the bind-seam variant is unconstructible anywhere else.
pub(crate) struct BindSeam(());

impl AdoptSeam {
    /// The bind seam — private to this module by [`BindSeam`]'s own field.
    fn binding() -> Self {
        AdoptSeam::Binding(BindSeam(()))
    }
}

/// How an adopted value is moved in, and with it how its reach evidence is kept. There is no
/// discard arm: every disposition retains reach — the designer invariant. Both arms run through
/// [`Scope::relocate_delivered`], differing in the [`RegionEscape`] verb they name.
enum AdoptDisposition {
    /// The value stays in its producer region: the relocation pointer-copies its top node at the
    /// fold brand and the substrate borrow rides. The verb's retention claim keeps every member the
    /// source envelope named — home included — so every region the value reaches stays alive for
    /// the adopting region's life.
    Pin,
    /// The value is relocated into the adopting scope's region through the fold door — whatever
    /// would otherwise keep region storage behind ([`KObject::needs_destination_door`]) totally
    /// rebuilt there, every other value's top node cloned — and the composition
    /// derives and retains the copy's release-exact reach. The source envelope's hold is released
    /// when the caller drops it.
    Relocate,
}

/// The single home of the adoption rules. `projected` is what the caller will actually move —
/// identity for a whole-value adoption, an interior payload for a MATCH/TRY `it` — read inside the
/// envelope's own open. By seam:
///
/// - [`AdoptSeam::Retaining`] pins every object, substrate or not. The adopting scope's region union
///   holds the minted reach, so the value can stay put; that is what lets a spliced argument or a
///   head-deferred callable survive its producing step as its carrier rather than as a copy.
/// - [`AdoptSeam::Binding`] copies every object, except that a top-level record routes the cost
///   chooser.
///
/// The shape rules behind that table:
///
/// - A **substrate carrier** (`Record` / `List` / `Dict` / `Tagged` / `Wrapped`) is region-resident,
///   so a copying seam must rebuild it at the destination door rather than pointer-copy it: a
///   `deep_clone` would leave the substrate in the retiring producer, uncovered once the copy's
///   reach releases it. A bare **`KString`** joins it ([`KObject::needs_destination_door`]): a
///   pointer copy would share bump bytes the producer owns. Both are the `Copy` verb's own rule
///   ([`relocate_object_into`]), so the chooser names the verb and the relocation applies the shape
///   rule — there is no separate copy-node disposition.
/// - Only a top-level **record**, and only at the bind seam, is cost-driven ([`copy_or_pin`]). Every
///   other substrate carrier copies unconditionally there: pinning a bound value retains its
///   producer region, which a tail loop's O(1) region turnover cannot afford (`it` in a
///   `MATCH`-mediated tail hop binds a `Tagged` payload every iteration). Records reach the pin arm
///   only outside tail position.
/// - A record's crossing is priced against the region the delivered value *lives in* — the host its
///   own reach description names, read off the carrier under the envelope's pins — not against the
///   projection, which may be an interior payload.
fn adopt_disposition(
    cell: &DeliveredCarried,
    projected: &KObject<'_>,
    seam: &AdoptSeam,
) -> AdoptDisposition {
    match seam {
        AdoptSeam::Retaining => AdoptDisposition::Pin,
        AdoptSeam::Binding(_) if !projected.needs_destination_door() => AdoptDisposition::Relocate,
        AdoptSeam::Binding(_) => cell
            .open_at()
            .with_home_region(|host_region| match projected {
                KObject::Record(substrate, _) => match copy_or_pin(substrate, host_region) {
                    RegionEscape::Copy => AdoptDisposition::Relocate,
                    RegionEscape::Pin => AdoptDisposition::Pin,
                },
                _ => AdoptDisposition::Relocate,
            }),
    }
}
