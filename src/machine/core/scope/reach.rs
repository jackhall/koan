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
use crate::machine::core::carrier_witness::{OpenedFunction, SealedFunction};
use crate::machine::core::kfunction::{KFunction, KFunctionFamily};
use crate::machine::core::{
    product_reaches_region, FoldingBrand, FrameCoverage, FrameReach, FrameStorage, KoanRegion,
    KoanRegionExt, KoanStorageProfile, ModuleRefFamily, ScopeRefFamily,
};
use crate::machine::model::{
    copy_or_pin, relocate_object_into, Carried, CarriedFamily, KObject, KType, Module,
    RegionEscape, TypeIdentifier,
};
use crate::machine::{CarrierWitness, DeliveredCarried, KError, SplicedCell};
use crate::witnessed::{Delivered, DropFree, Reattachable, RegionHandleFamily, Sealed, Witnessed};

// The sole test here pins the bind-seam pin (substrate-sharing) mechanism; the `seam-force-copy`
// build rebuilds the record instead, so the module cannot hold there. The equivalence battery proves
// language-output invisibility separately.
#[cfg(all(test, not(feature = "seam-force-copy")))]
mod tests;

impl<'a> Scope<'a> {
    /// Whether any scope on this scope's lexical `outer` chain (including `self`) lives in `region`.
    /// Holding a scope keeps its own region alive, so a region reached here is one this chain
    /// already pins. Used at `runtime/submit.rs`'s cart check.
    pub(crate) fn chain_reaches_region(&self, region: &KoanRegion) -> bool {
        self.ancestors()
            .any(|scope| std::ptr::eq(scope.region(), region))
    }

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
    /// bundle into the **region's** union bundle — the single reach-derivation door behind every
    /// bind, a veneer over the library's fused
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
    /// carrier — the one region-pure resident verb, generic over the carried family, for every
    /// terminal that hands a resident value out of a step un-sealed (a type terminal's
    /// `Carried::Type`, a relocation's bare destination handle). The description is minted on the
    /// confined arena surface ([`RegionBrand::seal_resident`]), so the residence is derived where
    /// the region handle is, never assembled at a call site.
    ///
    /// What "reaching nothing" covers, by family:
    ///
    /// - **Type carrier** (`CarriedFamily`, a `KType`): a `KType` is owned data, so the read pins no
    ///   foreign region and travels under the home-frame pin alone (the envelope host
    ///   [`Self::seal_resident_delivered`] pairs); the `Copy` handle rides in place, never re-cloned
    ///   into the region.
    /// - **Callable** (`KFunctionFamily`): the `FN` / `OP` registration doors. `FN` allocates the
    ///   callable into the very scope it captures, so it reaches nothing beyond the region it lives
    ///   in, which every read of it already pins.
    /// - **Destination handle** (`RegionHandleFamily` / `DestHandleFamily`): a bare region handle
    ///   borrows nothing at all.
    ///
    /// A **value carrier** whose borrows do reach somewhere is sealed by the composition that placed
    /// it, under the description [`Self::mint_retained`] derives from that composition's own
    /// operands. Its carrier pins nothing either — the reached regions are owned by this region's
    /// union bundle — so an entry read is a pointer copy, and a read that leaves the container
    /// re-owns the reach by lifting into a [`DeliveredCarried`] envelope ([`Self::lift_resident`]).
    pub(crate) fn resident<T: Reattachable + DropFree>(
        &self,
        value: T::At<'_>,
    ) -> Witnessed<T, CarrierWitness> {
        self.brand().seal_resident(value)
    }

    /// [`Self::resident`], sealed into its dormant binding form — the door a dispatch-bucket
    /// registration writes through.
    pub(crate) fn seal_resident<T: Reattachable + DropFree>(
        &self,
        value: T::At<'_>,
    ) -> Sealed<T, CarrierWitness> {
        Sealed::seal(self.resident(value))
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
    pub(crate) fn seal_reaching<T: Reattachable + DropFree>(
        &self,
        value: T::At<'_>,
        reach: &'a FrameReach,
    ) -> Sealed<T, CarrierWitness> {
        Sealed::seal(self.brand().seal_reaching(value, reach))
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
        sealed.open_with(&self.home(), read)
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
    pub(crate) fn rest_delivered(&self, cell: &DeliveredCarried) -> SplicedCell {
        cell.rest_in(self.brand().handle())
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
        cell.open_at(&self.home()).lift_out()
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
        Delivered::lift(sealed, self.home())
    }

    /// The step-terminal form of [`Self::lift_resident`]: the live carrier paired with the owned
    /// bundle the lift upgraded, for a producer handing its bound value out of the step
    /// (`StepCarried::born_pinned`). The pins ride the step so the terminal's reach is owned
    /// end-to-end rather than re-derived at the seal.
    pub(crate) fn lift_resident_parts(
        &self,
        sealed: SealedValue,
    ) -> (Witnessed<CarriedFamily, CarrierWitness>, FrameCoverage) {
        let (cell, coverage) = self.lift_resident(sealed).into_parts();
        (cell.unseal(), coverage)
    }

    /// Seal a resident carrier — a value already living in this scope's own region — into a
    /// [`DeliveredCarried`] envelope pinned by this scope's own region owner. The resident twin of
    /// the scheduler's [`dep_delivered`](crate::scheduler::Scheduler::dep_delivered): the pin is the
    /// home frame the caller reads the value under (`region_owner().upgrade()`, the same owner
    /// [`resident_value_carrier`](Self::resident_value_carrier) folds into the witness), so a spliced
    /// resident cell travels self-covering by its own witness *and* pinned by its home, identical in
    /// shape to a delivered dep — there is no `pin: None` resident special case at the splice sites.
    ///
    /// Generic over the carrier's family so a **destination operand** seals the same way a value
    /// does: a relocation's dest is a bare region handle living in this scope's own region, and the
    /// composition verbs take their destination as an envelope, so it is sealed here rather than
    /// paired with an asserted host at the call site.
    pub(crate) fn seal_resident_delivered<T: Reattachable + DropFree>(
        &self,
        witnessed: Witnessed<T, CarrierWitness>,
        coverage: FrameCoverage,
    ) -> Delivered<T, CarrierWitness, FrameStorage> {
        // The resident carrier's owned foreign reach — a clone of the binding entry's coverage,
        // threaded from the read — travels with the envelope, so the reached regions are owned
        // across transit rather than re-derived from the carrier's description.
        Delivered::seal(witnessed, self.home(), coverage)
    }

    /// Build an object into this scope's own region through a **zero-dep fold** and hand back the
    /// resident borrow. The door for a value that is region-pure in the sense that matters — every
    /// borrow it carries points into this region — but not `'static`, because a string literal's
    /// bytes are bumped here ([`RegionBrand::alloc_text`](crate::machine::core::RegionBrand::alloc_text)).
    ///
    /// The fold brand discharges the residence obligation at compile time — an ambient borrow cannot
    /// inhabit `KObject<'b>` — and the product is re-anchored at `'a` through the library's own fused
    /// adopt door. With no deps the fold composes no reach, so the adoption retains nothing and the
    /// value's residence stays exactly this region.
    pub(crate) fn fold_resident_object(
        &self,
        build: impl for<'b> FnOnce(FoldingBrand<'b>) -> KObject<'b>,
    ) -> &'a KObject<'a> {
        let built = KoanRegion::fold_witnessed(self.home(), |brand| {
            Carried::Object(brand.alloc_object_folded(build(brand)))
        });
        self.seal_resident_delivered(built, FrameCoverage::empty())
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
        self.seal_resident_delivered(
            self.resident::<CarriedFamily>(Carried::Object(object)),
            FrameCoverage::empty(),
        )
    }

    /// Place a **carrier-less** argument's value in this scope's own region. The shape is the
    /// enforcement: a value that arrives with no reach carrier is one the `arg_carriers` contract
    /// (`BodyCtx::arg_carriers`) calls region-pure, and the arms here are exactly the shapes a
    /// region-pure value can take — each placed through a door whose signature proves the product
    /// borrows only this region.
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
                Ok(self.fold_resident_object(|brand| KObject::KString(brand.alloc_text(text))))
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
    pub(crate) fn seal_pure_value(&self, value: &KObject<'a>) -> Result<SealedValue, KError> {
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
    /// argument, a call's argument delivery). `seam` selects the policy
    /// ([`adopt_disposition`] is the single home of the rules); this door then runs the mechanism
    /// the chooser named.
    ///
    /// The **type channel** never reaches the chooser: a `KType` is a lifetime-free handle and a
    /// `TypeIdentifier` is a bare surface name, so the envelope is opened and its content copied out
    /// — the handle by value, the name's bytes bumped into this scope's own region. That is a copy
    /// for every seam — the result borrows only this region, so no reach is minted and the
    /// producer's region is not pinned.
    ///
    /// Where [`seal_resident`](Self::seal_resident) seals a value already living **in** this
    /// region, adoption is the consumption verb for a carrier produced **elsewhere**.
    pub(crate) fn adopt_carried(&self, cell: &DeliveredCarried, seam: AdoptSeam) -> Carried<'a> {
        /// The content copied out of a type-channel envelope: a `Copy` `KType` handle, or an
        /// unlowered surface name whose bytes are re-bumped into this scope's region.
        enum AdoptedType<'t> {
            Lowered(KType),
            Unlowered(TypeIdentifier<'t>),
        }

        let brand = self.brand();
        let copied_type = cell.open(|live| match live {
            Carried::Type(kt) => Some(AdoptedType::Lowered(kt)),
            // The name is read at the envelope's own brand, so it is copied here rather than
            // carried out: the product names only this region.
            Carried::UnresolvedType(ti) => Some(AdoptedType::Unlowered(TypeIdentifier::leaf(
                brand.alloc_text(ti.as_str()),
            ))),
            Carried::Object(_) => None,
        });
        match copied_type {
            Some(AdoptedType::Lowered(handle)) => return Carried::Type(handle),
            Some(AdoptedType::Unlowered(ti)) => return Carried::UnresolvedType(ti),
            None => {}
        }

        let disposition = cell.open(|live| adopt_disposition(cell, live.object(), &seam));
        match disposition {
            // The whole envelope is adopted in place through the library's fused mint-and-retain
            // door: nothing is copied, so there is nothing to relocate.
            AdoptDisposition::Pin => cell.adopt_into(self.brand().handle()),
            AdoptDisposition::Relocate => {
                // The relocation lands the value in this scope's own region under the fold brand and
                // its composition retains the copy's reach here for the region's life; adopting the
                // product envelope re-anchors it at `'a` through the library's own fused
                // mint-and-retain door, so no re-box is needed to recover the reference.
                self.relocate_delivered(cell, |carried| Ok(carried.object()), RegionEscape::Copy)
                    .expect("a whole-value adoption's copy is infallible")
                    .adopt_into(self.brand().handle())
            }
        }
    }

    /// Adopt a delivered value's **projection** into this scope for **binding** — the door whose
    /// product is the dormant [`SealedValue`] a binding entry stores. `project` selects what to bind
    /// (identity for a whole-value bind, a `Tagged`/`Wrapped` payload for a MATCH/TRY `it`, the
    /// Ok/Err payload for TRY), read under the envelope's own pin.
    ///
    /// The seam is [`AdoptSeam::Binding`], the only one that admits the escape-seam cost chooser:
    /// the adopting scope's region union owns the minted reach for the region's life, so pinning a
    /// record in its producer region is affordable here and nowhere else. [`adopt_disposition`]
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
    ) -> Result<SealedValue, KError>
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
        Ok(self.relocate_delivered(cell, project, verb)?.into_cell())
    }

    /// Relocate a delivered value's **projection** into this scope's region through the fold door,
    /// under the escape `verb` the caller's disposition names. `project` selects what to move;
    /// [`relocate_object_into`] runs the verb — a `Copy` totally rebuilds whatever would otherwise
    /// keep region storage behind ([`KObject::needs_destination_door`]) so it lands here, a `Pin`
    /// pointer-copies the top node and lets its region-resident substrate borrow ride.
    ///
    /// The verb also fixes the retention claim the fold hands its composition:
    ///
    /// - **Copy** — release-exact. The predicate runs over the rebuilt value, so a plain-data
    ///   record drops the producer region (a tail loop's retiring frame does not ride this binding)
    ///   while a rebuild still borrowing its home keeps it.
    /// - **Pin** — nothing is released: the value stays where it lived and the producer transfers
    ///   by hold, so every region the source envelope named is kept. That is what covers the
    ///   pointer-copied substrate still living in the producer's region.
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
            .transfer_into_placing::<RegionHandleFamily<KoanStorageProfile>, CarriedFamily, KoanStorageProfile>(
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
    fn dest_operand(
        &self,
    ) -> Delivered<RegionHandleFamily<KoanStorageProfile>, CarrierWitness, FrameStorage> {
        self.seal_resident_delivered(
            self.resident::<RegionHandleFamily<KoanStorageProfile>>(self.brand().handle()),
            FrameCoverage::empty(),
        )
    }

    /// Wrap a resident `KFunction` in its `KObject` carrier — the store every `FN` / `OP`
    /// registration hands its fresh callable out through. `function` was born at its captured
    /// scope's own brand ([`KFunction::alloc_captured`](crate::machine::core::KFunction), whose
    /// `for<'b>` closure makes living in that scope's region a type fact), so the door envelopes the
    /// reference at this scope's home and **merges** it into the same region: the composition mints
    /// that region into the product's reach, which is the borrows-home fact the wrapper carries.
    /// Source and destination coincide, so the library's self rule strips the region from the
    /// retained bundle — a callable never pins the region it lives in.
    ///
    /// The claim is exact. A `KFunction`'s only region borrow is its captured scope, and that
    /// scope's own sealed reach-set transitively keeps every foreign region its bindings reach
    /// alive, so naming the callable's home region names the whole closure.
    ///
    /// Infallible, and check-free: the wrapping `KObject::KFunction` is built at the fold brand from
    /// the merge's own operand view, so an ambient-lifetime capture is a compile error at the
    /// closure's signature.
    pub(crate) fn store_function_object(
        &self,
        function: &'a KFunction<'a>,
    ) -> Witnessed<CarriedFamily, CarrierWitness> {
        let source = self.seal_resident_delivered(
            self.resident::<KFunctionFamily>(function),
            FrameCoverage::empty(),
        );
        source
            .merge_into_placing::<RegionHandleFamily<KoanStorageProfile>, CarriedFamily, KoanStorageProfile>(
                self.dest_operand(),
                |function_view, _handle, placement| {
                    let door = FoldingBrand::in_fold_closure(placement);
                    Carried::Object(door.alloc_object_folded(KObject::KFunction(function_view)))
                },
            )
            .into_cell()
            .unseal()
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
    pub(crate) fn store_module_object(&self, module: &'a Module<'a>) -> SealedValue {
        let child = module.child_scope();
        let source = child.seal_resident_delivered(
            child.resident::<ModuleRefFamily>(module),
            FrameCoverage::empty(),
        );
        source
            .merge_into_placing::<RegionHandleFamily<KoanStorageProfile>, CarriedFamily, KoanStorageProfile>(
                self.dest_operand(),
                |module_view, _handle, placement| {
                    let door = FoldingBrand::in_fold_closure(placement);
                    Carried::Object(door.alloc_object_folded(KObject::Module(module_view)))
                },
            )
            .into_cell()
    }

    /// The transparent-ascription store: a fresh `Module` tagged `name`, re-tagging a *foreign*
    /// source module's child scope, whose region is not this scope's own. The re-tagged `Module` is
    /// **built inside the fold** over that child scope as its operand view, so its borrow is the
    /// merge's own — which is what lets the composition, rather than a runtime walk, evidence the
    /// foreign region. `seal` installs the view's self-sig on the resident module before it is
    /// wrapped; it takes the module at the fold's own brand lifetime, so nothing it writes can
    /// smuggle a borrow out.
    ///
    /// Both the `Module` and the `KObject::Module` wrapping it are allocated at the same fold brand
    /// into this scope's region, and the composed reach names the source child's region — the one
    /// claim covering both, minted and retained here in one act.
    pub(crate) fn store_transparent_view(
        &self,
        name: String,
        source_child: &'a Scope<'a>,
        seal: impl for<'b> FnOnce(&'b Module<'b>),
    ) -> SealedValue {
        let source = source_child.seal_resident_delivered(
            source_child.resident::<ScopeRefFamily>(source_child),
            FrameCoverage::empty(),
        );
        source
            .merge_into_placing::<RegionHandleFamily<KoanStorageProfile>, CarriedFamily, KoanStorageProfile>(
                self.dest_operand(),
                |scope_view, _handle, placement| {
                    let door = FoldingBrand::in_fold_closure(placement);
                    let module = door.alloc_module_folded(Module::new(name, scope_view));
                    seal(module);
                    Carried::Object(door.alloc_object_folded(KObject::Module(module)))
                },
            )
            .into_cell()
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
/// [`ReHome`](Self::ReHome) means the caller re-homes the value anyway and holds no lasting reach,
/// so substrate carriers must copy: a tail loop's O(1) region turnover cannot afford pinning the
/// producer (argument delivery). The producer's region is not part of the copy's residence — the
/// copy carries its own release-exact reach — so the source envelope's hold is **released** when
/// the caller drops it. Released, not discarded: the copy retains everything it still reaches.
///
/// [`Binding`](Self::Binding) additionally admits the record cost chooser. It is minted only by
/// [`Scope::adopt_for_binding`] — [`BindSeam`]'s field is private to this module, so no caller
/// outside it can select cost-driven record pinning.
pub(crate) enum AdoptSeam {
    ReHome,
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
/// - [`AdoptSeam::ReHome`] copies every object.
/// - [`AdoptSeam::Binding`] copies too, except that a top-level record routes the cost chooser.
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
        AdoptSeam::ReHome => AdoptDisposition::Relocate,
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
