//! The reach / carrier derivation cluster on [`Scope`]: minting a delivered value's reach into
//! this scope's arena, the resident value / type carriers and their witness, sealing residents into
//! delivery envelopes, and the copy-free / copying adoption doors. Split out of the parent
//! `scope` module.

use super::Scope;
use crate::machine::core::bindings::SealedValue;
use crate::machine::core::carrier_witness::{
    function_mirror_of, FunctionMirror, OpenedFunction, SealedFunction,
};
use crate::machine::core::kfunction::{KFunction, KFunctionFamily};
use crate::machine::core::{
    product_still_borrows, FoldingBrand, FrameCoverage, FrameReach, FrameStorage, KoanRegion,
    KoanStorageProfile,
};
use crate::machine::model::{
    copy_object_into, copy_or_pin, still_borrows_host, Carried, CarriedFamily, KObject, KType,
    RegionEscape, TypeIdentifier, TypeRegistry,
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

    /// Retain an owning [`FrameCoverage`] in this scope's own region for the region's life — the
    /// scope-facing door onto [`RegionHandle::retain_reach`](crate::witnessed::RegionHandle::retain_reach).
    /// A value **resident** in this scope whose reach is not carried by a binding entry (a USING
    /// overlay rooting an opened module's region, a copy-free adoption that discards its stored
    /// reach) parks its coverage here so the non-owning description it mirrors stays backed for the
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
    /// Returns the hosted description (`None` == empty, no allocation) and the borrows-into-this-
    /// region bit — the move-in's audit evidence and the resident seal's witness, both derived here
    /// so no caller pairs a value with a reach some other value derived. No omission policy: the
    /// mint applies subsumption and the self rule alone, so the description is the value's exact
    /// reach.
    ///
    /// The description is arena-hosted for the region's life and non-owning (`Weak` members); the
    /// owning bundle never crosses back out — the library folds it straight into the region's union
    /// bundle, which dedupes by region identity with outer-chain subsumption, so one owning `Rc` per
    /// distinct region covers everything resident here and drops whole at region death. Binding
    /// entries own nothing, and since a binding never dies before its scope, that pins no longer
    /// than a per-entry bundle would.
    pub(crate) fn mint_retained(
        &self,
        sources: &[&FrameCoverage],
    ) -> (Option<&'a FrameReach>, bool) {
        self.brand().handle().mint_retained(sources)
    }

    /// Seal a value living **in this scope's region** into its dormant binding form: the value
    /// fused to the reference-only `{ bit, ref }` carrier over its freshly-minted exact reach. The
    /// carrier pins nothing — the reached regions are owned by this region's union bundle
    /// ([`Self::mint_retained`]) — so an entry read is a bit-copy, and a read that leaves the
    /// container re-owns the reach by lifting into a [`DeliveredCarried`] envelope
    /// ([`Self::lift_resident`]). The bundle runs on the confined arena surface
    /// ([`RegionBrand::seal_resident`]), so `Witnessed::resident` is never reached from a builtin.
    ///
    /// When `self` is a transparent window over borrowed bindings ([`Self::child_transparent`]),
    /// the home frame is the call-site frame but the description points into the *owning* (module)
    /// scope's own arena, not the call site's — the binding was minted there at the module's own
    /// bind time. Sound because the window's overlay reach-fold (`USING`'s body,
    /// `builtins/using_scope.rs`) mints the opened module's own carrier into the call-site arena at
    /// overlay construction, before any such carrier exists — so holding the call-site frame roots
    /// the module's arena one hop removed, and through it the description's pointee.
    pub(crate) fn seal_resident_value(
        &self,
        carried: Carried<'a>,
        reach: Option<&FrameReach>,
        borrows_home: bool,
    ) -> SealedValue {
        Sealed::seal(
            self.brand()
                .seal_resident(carried, CarrierWitness::new(borrows_home, reach)),
        )
    }

    /// Seal a `KFunction` living **in this scope's region** into its dormant registration form — the
    /// function-family twin of [`Self::seal_resident_value`], for the bare-`FN` door, which binds no
    /// value and so has no mirrored `data` seal to derive its claim from. The reach is the exact
    /// empty set: `FN` allocates the callable into the very scope it captures, so its only region
    /// borrow is home, which every read of it already pins. Returns the whole
    /// [`FunctionMirror`] bundle — the callable is held live here, so everything the bucket write
    /// keys on is read straight off the reference and travels as plain data.
    pub(crate) fn seal_resident_function(&self, f: &'a KFunction<'a>) -> FunctionMirror {
        let sealed = Sealed::seal(
            self.brand()
                .seal_resident::<KFunctionFamily>(f, CarrierWitness::new(false, None)),
        );
        FunctionMirror::of_live(f, sealed)
    }

    /// The **mirror seal**: project a bound value's dormant carrier onto the `KFunction` it wraps,
    /// carrying that value's own witness across unchanged — so a `functions` bucket entry states
    /// exactly the claim its `data` twin does instead of holding a reference with no reach at all.
    /// `None` when the value is not a callable. The projection reads under this scope's own region
    /// owner, which is where the value it seals lives, and computes everything the bucket write
    /// keys on inside that one open, so the write path itself opens nothing.
    pub(crate) fn seal_function_mirror(&self, sealed: &SealedValue) -> Option<FunctionMirror> {
        let home = self
            .region_owner()
            .upgrade()
            .expect("the binding scope's region owner is held while its value is mirrored");
        function_mirror_of(sealed, &home)
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
        let pin = self
            .region_owner()
            .upgrade()
            .expect("the binding scope's region owner is held while its overloads are read");
        sealed.open_with(&pin, read)
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
        let home = self
            .region_owner()
            .upgrade()
            .expect("the binding scope's region owner is held while its value is lifted");
        Delivered::lift(sealed, home)
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
        let home = self
            .region_owner()
            .upgrade()
            .expect("the resident scope's region owner is held while its value is sealed");
        // The resident carrier's owned foreign reach — a clone of the binding entry's coverage,
        // threaded from the read — travels with the envelope, so the reached regions are owned
        // across transit rather than re-derived from the carrier's description.
        Delivered::seal(witnessed, home, coverage)
    }

    /// Adopt a sealed dep carrier into this scope. The two channels adopt differently:
    ///
    /// - **Object**: copy-free. [`Delivered::adopt_into`] mints the carrier's reach — with its
    ///   home riding as an ordinary member — into this scope's own arena
    ///   for liveness, so every region the value reaches, its own home included, stays alive for the
    ///   scope's life; then re-anchors the sealed value at this scope's brand. The value stays put in
    ///   its producer's region and the mint is what pins that region, so the dep survives past its
    ///   resolving step as its carrier rather than as a relocated copy (the head-deferred callable, a
    ///   spliced argument).
    /// - **Type / unlowered type name**: clone at the door. A `KType` and a `TypeIdentifier` are
    ///   both fully owned data, so the envelope is opened, the content cloned out, and the clone
    ///   allocated into this scope's own region through its storage door. The result borrows only
    ///   this region, so no reach is minted and the producer's region is not pinned.
    ///
    /// Where [`seal_resident_value`](Self::seal_resident_value) seals a value already living
    /// **in** this region, adoption is the consumption verb for a carrier produced **elsewhere**.
    pub(crate) fn adopt_sealed(&self, cell: &DeliveredCarried) -> Carried<'a> {
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
            Some(AdoptedType::Lowered(handle)) => Carried::Type(handle),
            Some(AdoptedType::Unlowered(ti)) => {
                Carried::UnresolvedType(self.brand().alloc_type_identifier(ti))
            }
            None => cell.adopt_into(self.brand().handle()),
        }
    }

    /// Adopt a sealed dep carrier's **object** into this scope by structural copy — the
    /// value-channel twin of [`Self::adopt_sealed`]'s copy-free object arm, for a consumer that
    /// re-homes the value anyway (a call's argument delivery). The top node is `deep_clone`d into
    /// this scope's own arena, so the producer's region is *not* part of the copy's residence: the
    /// mint stores the copy's release-exact reach ([`Delivered::coverage_retaining`] — the copy's
    /// own members, its home among them), never a residence-only host pin. This is what
    /// frees a tail loop's retiring region once its delivered carrier drops (the working expression
    /// at step end), instead of chaining it into every successor region's arena.
    ///
    /// The **type** channel forwards to [`Self::adopt_sealed`], whose type arm already copies: an
    /// owned `KType` clone lands region-locally with nothing left pointing at the producer.
    ///
    /// The value copy reads the producer under the envelope's own pin — the retained frame owner
    /// ([`Delivered::open`]) — so the source backing stays live for the read; a resident-sealed
    /// envelope, or a frameless / run producer whose backing already outlives the read, reads under
    /// the carrier's bundled witness instead (the `None`-host arm of the envelope's open).
    ///
    /// A value that **is a substrate carrier** (a `Record` / `List` / `Dict` / `Tagged` / `Wrapped`)
    /// is totally rebuilt into this scope's region through the fold door
    /// ([`Self::rebuild_delivered_substrate`]) rather than taking the pointer-copy arm: the substrate
    /// is region-resident and cannot cross the checked audit by a `deep_clone` (which leaves the
    /// substrate in the retiring producer, uncovered once the copy's reach releases it). This
    /// path re-homes the value and discards its reach, so it always copies — the bind seam's pin verb
    /// ([`Self::copy_delivered_substrate`]) is reachable only where the binding retains the reach.
    pub(crate) fn adopt_sealed_copied(
        &self,
        cell: &DeliveredCarried,
        types: &TypeRegistry,
    ) -> Carried<'a> {
        let is_object = cell.open(|live| matches!(live, Carried::Object(_)));
        if !is_object {
            return self.adopt_sealed(cell);
        }
        let embeds_substrate =
            cell.open(|live| live.as_object().is_some_and(|o| o.embeds_substrate()));
        if embeds_substrate {
            // The rebuild lands the value in this scope's own region under the fold brand and its
            // composition retains the copy's reach here for the region's life; adopting the product
            // envelope re-anchors it at `'a` through the library's own fused mint-and-retain door,
            // so no re-box is needed to recover the reference.
            let rebuilt = self
                .rebuild_delivered_substrate(cell, |carried| Ok(carried.object()))
                .expect("a whole-value record adoption's copy is infallible");
            return self.adopt_sealed(&rebuilt);
        }
        // The fused door mints the copy's release-exact reach and deep-clones the top node under it
        // in one step, so the copy's residence is audited against a reach derived for that same
        // value, and folds the owning bundle into this region's union — the deep copy's surviving
        // interior foreign borrows would otherwise be pinned by nothing.
        let (object, _reach, _borrows_home) = self
            .store_object_adopted(cell, |carried| Ok(carried.object()), types)
            .expect("a deep copy's own residence must be covered by its own reach evidence");
        Carried::Object(object)
    }

    /// Bind a delivered value's substrate-carrier **projection** into this scope. `project` selects
    /// what to bind (identity for a whole-value bind, a `Tagged`/`Wrapped` payload for a MATCH/TRY
    /// `it`); the caller vets that it yields a substrate carrier (a bare `Record` / `List` / `Dict` /
    /// `Tagged` / `Wrapped`). Only a top-level **record** routes the escape-seam cost chooser
    /// ([`copy_or_pin`]); every other carrier copies unconditionally. The verb decides copy vs pin:
    ///
    /// - **Copy** — a priceable home-crossing record with a clear borrows-home bit and small cost
    ///   (copied out and released, the retiring producer frees), every unpriceable record, and every
    ///   non-record substrate carrier (`List` / `Dict` / `Tagged` / `Wrapped`): the value is totally
    ///   rebuilt into this scope's region through the fold door
    ///   ([`Self::rebuild_delivered_substrate`]). Non-records copy rather than price because pinning a
    ///   bound value retains its producer region — which a tail loop's O(1) region turnover cannot
    ///   afford (`it` in a `MATCH`-mediated tail hop binds a `Tagged` payload every iteration); a copy
    ///   frees the producer. Records reach the pin arm only outside tail position.
    /// - **Pin** — a record that borrows its home region, a small home-crossing pin, or a foreign
    ///   (producer-resident) crossing: the projection is pointer-copied ([`KObject::deep_clone`], a
    ///   pointer copy for a record sharing the producer-region substrate) and moved in under the
    ///   whole-envelope minted reach ([`Self::store_object_pinned`]). A record substrate
    ///   carries no home-naming borrow, so the residence audit evidences the foreign substrate
    ///   through the `any_member_region` reach-member path rather than the dest-resident `owns_substrate`
    ///   check — which the exact mint's explicitly named producer region is what makes possible.
    ///   The reach is the pin's liveness, so this verb is confined to the bind seam, where the
    ///   region's union bundle holds it — never the argument re-home
    ///   ([`Self::adopt_sealed_copied`]), which discards it and copies unconditionally.
    ///
    /// Returns the bound value's dormant carrier — value fused to its exact reach — the same
    /// [`SealedValue`] [`Self::bind_delivered`] writes and a caller's terminal seal lifts.
    pub(crate) fn copy_delivered_substrate<P>(
        &self,
        cell: &DeliveredCarried,
        project: P,
        types: &TypeRegistry,
    ) -> Result<SealedValue, KError>
    where
        P: for<'b> Fn(&Carried<'b>) -> Result<&'b KObject<'b>, KError>,
    {
        // The crossing is priced against the region the delivered value *lives in* — the residence
        // the envelope's container supplied ([`Delivered::with_home_region`]) — not against the
        // projection, which may be an interior payload.
        let verb = cell.open(|live| {
            let Ok(projected) = project(&live) else {
                return RegionEscape::Copy { released: false };
            };
            cell.with_home_region(|host_region| match projected {
                KObject::Record(substrate, _) => copy_or_pin(substrate, projected, host_region),
                // Only a top-level record is cost-driven here. Every other substrate carrier
                // (`List` / `Dict` / `Tagged` / `Wrapped`) rebuilds unconditionally: pinning would
                // retain the producer region, breaking the O(1) region turnover a tail loop's
                // per-iteration `it` bind depends on. Copy with a walk-derived release bit.
                _ => RegionEscape::Copy {
                    released: !still_borrows_host(projected, host_region),
                },
            })
        });

        match verb {
            // Copy: the fold door's product is already dest-resident and its composition retained
            // the copy's reach in this region, so the product envelope's cell **is** the binding
            // entry — it seals straight into the table with no re-box.
            RegionEscape::Copy { .. } => {
                Ok(self.rebuild_delivered_substrate(cell, project)?.into_cell())
            }
            // Pin: the record stays in its producer region; the projection is pointer-copied and
            // moved in under the whole-envelope minted reach, whose explicitly named producer region
            // covers the foreign substrate on the audit's `any_member_region` reach-member path. The
            // reach is the pin's liveness — the region's union bundle owns it.
            RegionEscape::Pin => {
                let (object, reach, borrows_home) =
                    self.store_object_pinned(cell, project, types)?;
                Ok(self.seal_resident_value(Carried::Object(object), reach, borrows_home))
            }
        }
    }

    /// Rebuild a delivered value's substrate-carrier **projection** into this scope's region through
    /// the fold door — the copy path for a region-resident substrate, which cannot be pointer-copied
    /// past the checked residence audit. `project` selects what to copy (identity for a whole-value
    /// bind, a `Tagged`/`Wrapped` payload for a MATCH/TRY `it`); the caller vets that it yields a
    /// substrate carrier ([`copy_object_into`] rebuilds the whole reachable structure). The value
    /// relocates under the retention predicate's release-exact answer — a plain-data rebuild
    /// releases the producer (which then frees), a rebuild still borrowing its home keeps it.
    ///
    /// The fold's own composition mints the product's exact reach into this scope's arena and
    /// retains the owning bundle here for the region's life, so the witnessed product is already
    /// the finished carrier: it is enveloped under this scope's region owner and handed back. There
    /// is **no re-box** — the value the caller consumes is the one the fold brand allocated, not a
    /// deep clone of it re-audited through the checked door.
    ///
    /// This is the unconditional-copy half of [`Self::copy_delivered_substrate`]'s chooser: the argument
    /// re-home ([`Self::adopt_sealed_copied`]) calls it directly, and the chooser's `Copy` verb
    /// delegates here (a `Copy` verb's claim is exactly this one — the predicate walks the rebuilt
    /// product, which is what the verb's own `released` bit predicted).
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
            Witnessed::<RegionHandleFamily<KoanStorageProfile>, CarrierWitness>::resident(
                self.brand().handle(),
            ),
            FrameCoverage::empty(),
        );
        let mut projection_error: Option<KError> = None;
        // The destination is a bare region handle (empty reach), so its operand bundle is empty and
        // the composition mints exactly the copy's release-exact reach: the retention predicate runs
        // over the rebuilt value, so a plain-data record drops the producer region and a tail loop's
        // retiring frame does not ride this binding, while a rebuild that still borrows its home
        // keeps it. The composition also retains its bundle in this scope's region, which is what
        // covers the rebuilt value read in place.
        let copied = cell
            .transfer_into_placing::<RegionHandleFamily<KoanStorageProfile>, CarriedFamily, KoanStorageProfile>(
                dest,
                |product, region| product_still_borrows(cell, product.as_object(), region),
                |value, _handle, placement| {
                    let door = FoldingBrand::in_fold_closure(placement);
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

    /// Build the terminal carrier for a type living **in this scope's region** — the type-channel
    /// twin of [`Self::seal_resident_value`]. The witness is empty: a `KType` is owned data, so
    /// the read pins no foreign region and travels under the home-frame pin alone (the envelope host
    /// [`Self::seal_resident_delivered`] pairs). The bundle runs on the confined arena surface
    /// ([`RegionBrand::seal_resident`]), so a type read carries the `Copy` handle in place — no
    /// re-clone into the region.
    pub(crate) fn resident_type_carrier(
        &self,
        kt: crate::machine::model::KType,
    ) -> Witnessed<CarriedFamily, CarrierWitness> {
        self.brand()
            .seal_resident(Carried::Type(kt), CarrierWitness::new(false, None))
    }

    /// The reach a module value minted in **this** scope claims from its `child` scope — the
    /// derivation door for the module store paths, alongside the home-borrow bit the mint derives
    /// (`true` iff the child's own region reaches this scope's region).
    ///
    /// The claim is the child's **own region owner**, and that is exact: a `KObject::Module`'s only
    /// region borrow is its child scope, and every member value the module surfaces lives inside
    /// that child's own bindings. The child's region owns the union bundle for everything those
    /// members reach ([`Self::mint_retained`]), so pinning the child region transitively pins the
    /// whole member closure — there is nothing to union in per entry. A co-located module (`MODULE`,
    /// opaque `:|`) claims a region this scope's own chain already holds; a transparent `:!` view of
    /// a source module claims that source's (foreign) region. Never recovered by walking the built
    /// module value.
    pub(crate) fn child_module_reach(&self, child: &Scope<'a>) -> (Option<&'a FrameReach>, bool) {
        let child_home: FrameCoverage = match child.region_owner().upgrade() {
            Some(owner) => FrameCoverage::of(owner),
            None => FrameCoverage::empty(),
        };
        self.mint_retained(&[&child_home])
    }
}
