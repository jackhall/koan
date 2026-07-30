//! The reach / carrier derivation cluster on [`Scope`]: minting a value's residence and reach into
//! this scope's arena as one description, the two resident-carrier verbs over it (region-pure and
//! reach-carrying), sealing residents into delivery envelopes, and the two adoption doors over one
//! policy chooser ([`adopt_disposition`]). Split out of the parent `scope` module.

use std::rc::Rc;

use super::Scope;
use crate::machine::core::bindings::SealedValue;
use crate::machine::core::carrier_witness::{OpenedFunction, SealedFunction};
use crate::machine::core::kfunction::{KFunction, KFunctionFamily};
use crate::machine::core::{
    product_reaches_region, FoldingBrand, FrameCoverage, FrameReach, FrameStorage, KoanRegion,
    KoanRegionExt, KoanStorageProfile,
};
use crate::machine::model::{
    copy_object_into, copy_or_pin, Carried, CarriedFamily, KObject, KType, RegionEscape,
    TypeIdentifier, TypeRegistry,
};
use crate::machine::{CarrierWitness, DeliveredCarried, KError};
use crate::witnessed::{Delivered, Reattachable, RegionHandleFamily, Sealed, Witnessed};

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

    /// Retain an owning [`FrameCoverage`] in this scope's own region for the region's life — the
    /// scope-facing door onto [`RegionHandle::retain_reach`](crate::witnessed::RegionHandle::retain_reach).
    /// A value **resident** in this scope whose reach is not carried by a binding entry (a USING
    /// overlay rooting an opened module's region, an adoption whose product carries no entry of its
    /// own) parks its coverage here so the non-owning description it mirrors stays backed for the
    /// scope's life.
    pub(crate) fn retain_reach(&self, coverage: FrameCoverage) {
        self.brand().handle().retain_reach(coverage);
    }

    /// Mint `sources` into this scope's own arena and fold the owned bundle the mint hands back into
    /// the **region's** union bundle — the single reach-derivation door behind every bind, a veneer
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

    /// The description for a value **born in this scope's own region** that passed the dest-only
    /// residence audit: its host is this scope's region owner, and that same region enters the
    /// members exactly when `borrows_home` — the audit walk's own saw-a-region-pointer verdict
    /// ([`Residence::seen`](crate::machine::core::Residence)) — says the value genuinely holds a
    /// borrow into it. The audit passed, so every region the value borrows *is* this one; there is
    /// nothing else a member could name. A region-pure scalar mints an empty member set and records
    /// its residence all the same.
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
    /// A **value carrier** whose borrows do reach somewhere takes [`Self::seal_reaching`] under the
    /// description [`Self::mint_retained`] derived for it. Its carrier pins nothing either — the
    /// reached regions are owned by this region's union bundle — so an entry read is a pointer copy,
    /// and a read that leaves the container re-owns the reach by lifting into a [`DeliveredCarried`]
    /// envelope ([`Self::lift_resident`]).
    pub(crate) fn resident<T: Reattachable>(
        &self,
        value: T::At<'_>,
    ) -> Witnessed<T, CarrierWitness> {
        self.brand().seal_resident(value)
    }

    /// [`Self::resident`], sealed into its dormant binding form — the door a dispatch-bucket
    /// registration writes through.
    pub(crate) fn seal_resident<T: Reattachable>(
        &self,
        value: T::At<'_>,
    ) -> Sealed<T, CarrierWitness> {
        Sealed::seal(self.resident(value))
    }

    /// Seal a value living in this scope's region under the description already minted for it — the
    /// reach-carrying door a binding entry writes through, where [`Self::seal_resident`] is the
    /// region-pure one. `reach` must be this scope's own mint for this same value
    /// ([`Self::mint_retained`]), which is what makes the residence it stamps the value's own.
    ///
    /// When `self` is a transparent window over borrowed bindings ([`Self::child_transparent`]), a
    /// binding read out of the window carries a description minted into the *owning* (module)
    /// scope's own arena, not the call site's — the binding was minted there at the module's own
    /// bind time, and that arena is where its host names the module's frame. Sound because the
    /// window's overlay reach-fold (`USING`'s body, `builtins/using_scope.rs`) mints the opened
    /// module's own carrier into the call-site arena at overlay construction, before any such
    /// carrier exists — so holding the call-site frame roots the module's arena one hop removed, and
    /// through it the description's pointee.
    pub(crate) fn seal_reaching<T: Reattachable>(
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

    /// **Lift** a binding's dormant carrier into a delivery envelope pinned by this scope's own
    /// region owner (`Sealed → Delivered`): the library [`Delivered::lift`] upgrades the sealed
    /// description's members `Weak → Rc` under that pin, so the value's whole reach travels owned
    /// and the envelope survives its source frame's death. `self` must be the **binding** scope —
    /// the region the value lives in, whose arena hosts the description the upgrade reads.
    pub(crate) fn lift_resident<T: Reattachable>(
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
    pub(crate) fn seal_resident_delivered<T: Reattachable>(
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
    /// [`alloc_object_checked`](crate::machine::core::RegionBrand::alloc_object_checked) cannot take
    /// such a value: the bump keeps no address table, so the residence audit answers `false` for a
    /// string and the store is refused. The fold brand discharges the same obligation at compile time
    /// instead — an ambient borrow cannot inhabit `KObject<'b>` — and the product is re-anchored at
    /// `'a` through the library's own fused adopt door. With no deps the fold composes no reach, so
    /// the adoption retains nothing and the value's residence stays exactly this region.
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

    /// Adopt a delivered carrier into this scope for **consumption** — the door whose product is a
    /// live [`Carried`] the caller goes on to use (a bare-name read, a head-callable, a spliced
    /// argument, a call's argument delivery). `seam` selects the policy
    /// ([`adopt_disposition`] is the single home of the rules); this door then runs the mechanism
    /// the chooser named.
    ///
    /// The **type channel** never reaches the chooser: a `KType` and a `TypeIdentifier` are fully
    /// owned data, so the envelope is opened, the content cloned out, and the clone allocated into
    /// this scope's own region. That is a copy for every seam — the result borrows only this region,
    /// so no reach is minted and the producer's region is not pinned.
    ///
    /// Where [`seal_resident`](Self::seal_resident) seals a value already living **in** this
    /// region, adoption is the consumption verb for a carrier produced **elsewhere**.
    pub(crate) fn adopt_carried(
        &self,
        cell: &DeliveredCarried,
        seam: AdoptSeam<'_>,
    ) -> Carried<'a> {
        /// The content copied out of a type-channel envelope: a `Copy` `KType` handle, or an
        /// unlowered surface name re-allocated into this scope's region.
        enum AdoptedType {
            Lowered(KType),
            Unlowered(TypeIdentifier),
        }

        let cloned_type = cell.open(|live| match live {
            Carried::Type(kt) => Some(AdoptedType::Lowered(kt)),
            Carried::UnresolvedType(ti) => Some(AdoptedType::Unlowered(ti.clone())),
            Carried::Object(_) => None,
        });
        match cloned_type {
            Some(AdoptedType::Lowered(handle)) => return Carried::Type(handle),
            Some(AdoptedType::Unlowered(ti)) => {
                return Carried::UnresolvedType(self.brand().alloc_type_identifier(ti))
            }
            None => {}
        }

        let disposition = cell.open(|live| adopt_disposition(cell, live.object(), &seam));
        match disposition {
            AdoptDisposition::Pin => cell.adopt_into(self.brand().handle()),
            AdoptDisposition::Rebuild => {
                // The rebuild lands the value in this scope's own region under the fold brand and its
                // composition retains the copy's reach here for the region's life; adopting the product
                // envelope re-anchors it at `'a` through the library's own fused mint-and-retain door,
                // so no re-box is needed to recover the reference.
                self.rebuild_delivered_substrate(cell, |carried| Ok(carried.object()))
                    .expect("a whole-value substrate adoption's copy is infallible")
                    .adopt_into(self.brand().handle())
            }
            AdoptDisposition::CopyNode(types) => {
                // The fused door mints the copy's release-exact reach and deep-clones the top node under
                // it in one step, so the copy's residence is audited against a reach derived for that same
                // value, and folds the owning bundle into this region's union — the deep copy's surviving
                // interior foreign borrows would otherwise be pinned by nothing.
                let (object, _reach) = self
                    .store_object_adopted(cell, |carried| Ok(carried.object()), types)
                    .expect(
                        "a deep copy's own residence must be covered by its own reach evidence",
                    );
                Carried::Object(object)
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
    /// - **Rebuild** — the fold door's product is already dest-resident and its composition retained
    ///   the copy's reach in this region, so the product envelope's cell **is** the binding entry: it
    ///   seals straight into the table with no re-box.
    /// - **Pin** — the record stays in its producer region; the projection is pointer-copied
    ///   ([`KObject::deep_clone`], a pointer copy for a record sharing the producer-region substrate)
    ///   and moved in under the whole-envelope minted reach
    ///   ([`Self::store_object_pinned`]), whose explicitly named producer region covers the foreign
    ///   substrate on the audit's `any_member_region` reach-member path — a record substrate carries
    ///   no home-naming borrow, so the dest-resident `owns_substrate` check cannot evidence it. The
    ///   reach is the pin's liveness, and the region's union bundle owns it.
    /// - **CopyNode** — every projection that needs no destination door deep-clones its top node
    ///   under the cell's copied-mode reach; the mint runs *before* the copy so the copy's own
    ///   residence audit sees the evidence.
    pub(crate) fn adopt_for_binding<P>(
        &self,
        cell: &DeliveredCarried,
        project: P,
        types: &TypeRegistry,
    ) -> Result<SealedValue, KError>
    where
        P: for<'b> Fn(&Carried<'b>) -> Result<&'b KObject<'b>, KError>,
    {
        let disposition = cell.open(|live| match project(&live) {
            Ok(object) => adopt_disposition(cell, object, &AdoptSeam::binding(types)),
            // A projection failure surfaces from the store door below, which runs `project` again
            // under the same pin and returns its `KError`; the node-copy arm is the one that does.
            Err(_) => AdoptDisposition::CopyNode(types),
        });

        match disposition {
            AdoptDisposition::Rebuild => {
                Ok(self.rebuild_delivered_substrate(cell, project)?.into_cell())
            }
            AdoptDisposition::Pin => {
                let (object, reach) = self.store_object_pinned(cell, project, types)?;
                Ok(self.seal_reaching(Carried::Object(object), reach))
            }
            AdoptDisposition::CopyNode(types) => {
                let (object, reach) = self.store_object_adopted(cell, project, types)?;
                Ok(self.seal_reaching(Carried::Object(object), reach))
            }
        }
    }

    /// Rebuild a delivered value's substrate-carrier **projection** into this scope's region through
    /// the fold door — the copy path for a region-resident substrate, which cannot be pointer-copied
    /// past the checked residence audit. `project` selects what to copy; the chooser has already
    /// vetted that it yields a substrate carrier ([`copy_object_into`] rebuilds the whole reachable
    /// structure). The value relocates under the retention predicate's release-exact answer — a
    /// plain-data rebuild releases the producer (which then frees), a rebuild still borrowing its
    /// home keeps it.
    ///
    /// The fold's own composition mints the product's exact reach into this scope's arena and
    /// retains the owning bundle here for the region's life, so the witnessed product is already
    /// the finished carrier: it is enveloped under this scope's region owner and handed back. There
    /// is **no re-box** — the value the caller consumes is the one the fold brand allocated, not a
    /// deep clone of it re-audited through the checked door.
    ///
    /// The private engine under both adopt doors: [`AdoptDisposition::Rebuild`] is exactly this.
    fn rebuild_delivered_substrate<P>(
        &self,
        cell: &DeliveredCarried,
        project: P,
    ) -> Result<DeliveredCarried, KError>
    where
        P: for<'b> Fn(&Carried<'b>) -> Result<&'b KObject<'b>, KError>,
    {
        // The destination operand is this scope's own region handle, sealed into an envelope under
        // this scope's own region owner — its residence, which the composition gives the product.
        let dest = self.seal_resident_delivered(
            self.resident::<RegionHandleFamily<KoanStorageProfile>>(self.brand().handle()),
            FrameCoverage::empty(),
        );
        let mut projection_error: Option<KError> = None;
        // The rebuild's cells read their own stored reach at the door; `cell`'s coverage is the
        // holder-rule proof for a cell whose substrate stays foreign, captured here because a
        // `for<'b>` fold closure has no route back to its operand's pins.
        let holder = cell.coverage().clone();
        // The destination is a bare region handle (empty reach), so its operand bundle is empty and
        // the composition mints exactly the copy's release-exact reach: the retention predicate runs
        // over the rebuilt value, so a plain-data record drops the producer region and a tail loop's
        // retiring frame does not ride this binding, while a rebuild that still borrows its home
        // keeps it. The composition also retains its bundle in this scope's region, which is what
        // covers the rebuilt value read in place.
        let copied = cell
            .transfer_into_placing::<RegionHandleFamily<KoanStorageProfile>, CarriedFamily, KoanStorageProfile>(
                dest,
                |product, region| product_reaches_region(cell, product.as_object(), region),
                |value, _handle, placement| {
                    let door = FoldingBrand::in_fold_closure(placement).with_holder(&holder);
                    match project(&value) {
                        Ok(record) => {
                            Carried::Object(door.alloc_object_folded(copy_object_into(record, door)))
                        }
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

    /// The reach a module value minted in **this** scope claims from its `child` scope — the
    /// derivation door for the module store paths. The minted description is hosted here (the module
    /// value's own residence) and names the child's region as a member.
    ///
    /// The claim is the child's **own region owner**, and that is exact: a `KObject::Module`'s only
    /// region borrow is its child scope, and every member value the module surfaces lives inside
    /// that child's own bindings. The child's region owns the union bundle for everything those
    /// members reach ([`Self::mint_retained`]), so pinning the child region transitively pins the
    /// whole member closure — there is nothing to union in per entry. A co-located module (`MODULE`,
    /// opaque `:|`) claims a region this scope's own chain already holds; a transparent `:!` view of
    /// a source module claims that source's (foreign) region. Never recovered by walking the built
    /// module value.
    pub(crate) fn child_module_reach(&self, child: &Scope<'a>) -> &'a FrameReach {
        let child_home: FrameCoverage = match child.region_owner().upgrade() {
            Some(owner) => FrameCoverage::of(owner),
            None => FrameCoverage::empty(),
        };
        self.mint_retained(&[&child_home])
    }
}

/// Which seam is adopting — the one axis the adoption policy turns on, beside the shape of the
/// value itself.
///
/// [`Retaining`](Self::Retaining) means the adopting scope retains the minted reach in its region's
/// union bundle, so pinning the value in place is affordable: the dep survives past its resolving
/// step as its carrier rather than as a relocated copy (a bare-name read, the head-deferred
/// callable, a spliced argument). It never copies an object, so it needs no [`TypeRegistry`] for a
/// move-in audit — which is why it carries none.
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
pub(crate) enum AdoptSeam<'t> {
    ReHome(&'t TypeRegistry),
    Retaining,
    Binding(&'t TypeRegistry, BindSeam),
}

/// Admission token for [`AdoptSeam::Binding`]: an empty struct whose field is private to this
/// module, so the bind-seam variant is unconstructible anywhere else.
pub(crate) struct BindSeam(());

impl<'t> AdoptSeam<'t> {
    /// The bind seam over `types` — private to this module by [`BindSeam`]'s own field.
    fn binding(types: &'t TypeRegistry) -> Self {
        AdoptSeam::Binding(types, BindSeam(()))
    }
}

/// How an adopted value is moved in, and with it how its reach evidence is kept. There is no
/// discard arm: every disposition retains reach — the designer invariant.
enum AdoptDisposition<'t> {
    /// The value stays in its producer region; its whole minted reach (home riding as an ordinary
    /// member) is retained in the adopting scope's region union, so every region the value reaches
    /// — its own home included — stays alive for that region's life.
    Pin,
    /// The value is rebuilt into the adopting scope's region through the fold door
    /// ([`Scope::rebuild_delivered_substrate`]); the composition derives and retains the copy's
    /// release-exact reach there. The source envelope's hold is released when the caller drops it.
    Rebuild,
    /// The projection's top node is deep-cloned into the adopting scope's region under the fused
    /// mint, which stores the copy's release-exact reach ([`Delivered::coverage_retaining`] — the
    /// copy's own members, its home among them), never a residence-only host pin. Carries the
    /// [`TypeRegistry`] the move-in audit needs, so only a seam that supplies one can be answered
    /// with it.
    CopyNode(&'t TypeRegistry),
}

/// The single home of the adoption rules. `projected` is what the caller will actually move —
/// identity for a whole-value adoption, an interior payload for a MATCH/TRY `it` — read inside the
/// envelope's own open. By seam:
///
/// - [`AdoptSeam::Retaining`] pins every object, substrate or not. The adopting scope's region union
///   holds the minted reach, so the value can stay put; that is what lets a spliced argument or a
///   head-deferred callable survive its producing step as its carrier rather than as a copy.
/// - [`AdoptSeam::ReHome`] copies every object: the top node through the fused mint when it needs no
///   destination door, the whole structure through the fold door when it does.
/// - [`AdoptSeam::Binding`] copies such a projection's top node, and routes a top-level record
///   through the cost chooser; every other value needing a door rebuilds.
///
/// The shape rules behind that table:
///
/// - A **substrate carrier** (`Record` / `List` / `Dict` / `Tagged` / `Wrapped`) is region-resident
///   and cannot cross the checked audit by a `deep_clone`, which would leave the substrate in the
///   retiring producer, uncovered once the copy's reach releases it. So a copying seam rebuilds it
///   through the fold door rather than taking the pointer-copy arm. A bare **`KString`** joins it
///   ([`KObject::needs_destination_door`]): a pointer copy would share bump bytes the producer owns,
///   and no audit can catch that — the bump keeps no address table — so the copy has to re-bump at
///   the destination.
/// - Only a top-level **record**, and only at the bind seam, is cost-driven ([`copy_or_pin`]). Every
///   other substrate carrier copies unconditionally there: pinning a bound value retains its
///   producer region, which a tail loop's O(1) region turnover cannot afford (`it` in a
///   `MATCH`-mediated tail hop binds a `Tagged` payload every iteration). Records reach the pin arm
///   only outside tail position.
/// - A record's crossing is priced against the region the delivered value *lives in* — the host its
///   own reach description names, read off the carrier under the envelope's pins — not against the
///   projection, which may be an interior payload.
fn adopt_disposition<'t>(
    cell: &DeliveredCarried,
    projected: &KObject<'_>,
    seam: &AdoptSeam<'t>,
) -> AdoptDisposition<'t> {
    let needs_door = projected.needs_destination_door();
    match seam {
        AdoptSeam::Retaining => AdoptDisposition::Pin,
        AdoptSeam::ReHome(types) if !needs_door => AdoptDisposition::CopyNode(types),
        AdoptSeam::ReHome(_) => AdoptDisposition::Rebuild,
        AdoptSeam::Binding(types, _) if !needs_door => AdoptDisposition::CopyNode(types),
        AdoptSeam::Binding(..) => cell
            .open_at()
            .with_home_region(|host_region| match projected {
                KObject::Record(substrate, _) => {
                    match copy_or_pin(substrate, projected, host_region) {
                        RegionEscape::Copy { .. } => AdoptDisposition::Rebuild,
                        RegionEscape::Pin => AdoptDisposition::Pin,
                    }
                }
                _ => AdoptDisposition::Rebuild,
            }),
    }
}
