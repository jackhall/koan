//! The reach / carrier derivation cluster on [`Scope`]: minting a delivered value's reach into
//! this scope's arena, the resident value / type carriers and their witness, sealing residents into
//! delivery envelopes, and the copy-free / copying adoption doors. Split out of the parent
//! `scope` module.

use std::rc::Rc;

use super::Scope;
use crate::machine::core::{
    FoldingBrand, FrameSet, FrameStorage, KoanRegion, KoanStorageProfile, StoredReach,
};
use crate::machine::model::{
    copy_object_into, copy_or_pin, still_borrows_host, Carried, CarriedFamily, KObject, KType,
    RegionEscape, TypeIdentifier, TypeRegistry,
};
use crate::machine::{CarrierWitness, DeliveredCarried, KError};
use crate::witnessed::{Delivered, Reattachable, RegionHandleFamily, Residence, Sealed, Witnessed};

// The sole test here pins the bind-seam pin (substrate-sharing) mechanism; the `seam-force-copy`
// build rebuilds the record instead, so the module cannot hold there. The equivalence battery proves
// language-output invisibility separately.
#[cfg(all(test, not(feature = "seam-force-copy")))]
mod tests;

impl<'a> Scope<'a> {
    /// Whether any scope on this scope's lexical `outer` chain (including `self`) lives in `region` —
    /// the lexical-ancestor half of [`Self::covers_region_ambiently`]. Holding a scope keeps its own
    /// region alive, so a region reached here is one this chain already pins. Used alone at
    /// `runtime/submit.rs`'s cart check, which needs only the lexical half.
    pub(crate) fn chain_reaches_region(&self, region: &KoanRegion) -> bool {
        self.ancestors()
            .any(|scope| std::ptr::eq(scope.region(), region))
    }

    /// Whether this scope's context already keeps `region` alive without any reach member: pinned
    /// by the home frame's storage `outer` chain ([`FrameStorage::pins_region`]) or reached by the
    /// lexical `outer` chain ([`Self::chain_reaches_region`]). This is the reach-mint omission
    /// predicate — [`Self::host_reach_of`] / [`Self::adopted_reach_of`] / [`Self::adopt_sealed`]
    /// materialize no member for a region it covers, because re-pinning one, paired with a sibling
    /// bind of a call's result, would close a `frame → region → scope → frame` cycle — and
    /// therefore also the evidence-tier audits' ambient coverage
    /// ([`Scope::alloc_object_reaching`] and siblings): evidence this scope minted is complete
    /// exactly relative to "destination ∪ evidence members ∪ this predicate", so mint and audit
    /// stay complements by sharing it.
    pub(crate) fn covers_region_ambiently(&self, region: &KoanRegion) -> bool {
        let home = self.region_owner.upgrade();
        home.as_ref().is_some_and(|h| h.pins_region(region)) || self.chain_reaches_region(region)
    }

    /// Mint a delivered value's reach into this scope's own arena and package it as the binding
    /// entry's stored reach, for a value that **keeps living** in its producer's region (the
    /// copy-free re-anchor — [`Self::adopt_sealed`]'s object channel): the envelope's host —
    /// the value's producer frame owner — materializes as a reach member unconditionally, so the
    /// residence stays pinned for the scope's life. The minted set is held by the arena for the
    /// region's life — the same schedule the scope itself is held on — and its `&'a` reference is
    /// stored on the entry (the reach). `None` when the value reaches nothing foreign.
    /// Home-omission: everything [`Self::covers_region_ambiently`] covers — a per-call frame
    /// carries no storage `outer` under TCO, so the lexical half is what catches a closure's
    /// captured (ancestor) scope, keeping a sibling bind of the call's result from closing a
    /// region cycle.
    pub(crate) fn host_reach_of(&self, cell: &DeliveredCarried) -> StoredReach<'a> {
        self.envelope_reach_of(cell, Residence::Kept)
    }

    /// The stored reach for a value **deep-copied** out of a delivered carrier into this scope's own
    /// region — the copy-bind twin of [`Self::host_reach_of`] (a parameter bind, a MATCH/TRY `it`
    /// bind, the LET value route). The copy does not reside in the producer's region, so residence
    /// alone pins nothing: the envelope's host materializes as a reach member only when the value's
    /// borrows genuinely reach it (the carrier's `borrows_host` bit). Dropping a residence-only host
    /// is what lets a tail loop's retiring region free once its delivered carrier drops, instead of
    /// riding every later incarnation's stored reach.
    pub(crate) fn adopted_reach_of(&self, cell: &DeliveredCarried) -> StoredReach<'a> {
        self.envelope_reach_of(cell, Residence::Copied)
    }

    /// Shared mint behind [`Self::host_reach_of`] / [`Self::adopted_reach_of`]: the library
    /// [`Delivered::mint_reach`](crate::witnessed::Delivered::mint_reach) with koan's omission
    /// policy ([`Self::covers_region_ambiently`]), taking the envelope itself rather than its
    /// decomposed witness/host pair.
    fn envelope_reach_of(&self, cell: &DeliveredCarried, mode: Residence) -> StoredReach<'a> {
        let (foreign, borrows_into_home) = cell.mint_reach(self.brand().handle(), mode, |region| {
            self.covers_region_ambiently(region)
        });
        StoredReach {
            foreign,
            borrows_into_home,
        }
    }

    /// Mint a delivered value's reach for a **record pin**, naming every region it borrows —
    /// including a producer host the binding scope would otherwise cover ambiently. A pinned record's
    /// substrate carries no home-naming borrow, so [`Residence::owns_substrate`] can only evidence it via
    /// a reach-set member; the non-omitting mint (`|_| false`) puts the host in `foreign` so the
    /// audit's `any_member_region` arm accepts the foreign substrate. Retention is the point of the
    /// pin, and the host is already ambiently rooted for the binding's life, so naming it explicitly
    /// adds no over-retention beyond the pin's own semantics. `Residence::Kept` materializes the host
    /// unconditionally; the library's `mint` excludes `dest`'s own region from `foreign` (that home
    /// borrow rides `borrows_into_home`), so this names only the foreign producer host.
    fn pinned_reach_of(&self, cell: &DeliveredCarried) -> StoredReach<'a> {
        let (foreign, borrows_into_home) =
            cell.mint_reach(self.brand().handle(), Residence::Kept, |_region| false);
        StoredReach {
            foreign,
            borrows_into_home,
        }
    }

    /// Reach of a value already resident in a region this scope's context covers ambiently (the
    /// run-teardown rehome path) — no delivery envelope in hand, so no host to fold: the value's
    /// reach set already lives in an arena the caller's context keeps live.
    pub(crate) fn resident_reach_of<T: Reattachable>(
        &self,
        cell: &Witnessed<T, CarrierWitness>,
    ) -> StoredReach<'a> {
        let (foreign, borrows_into_home) = cell
            .mint_resident_reach(self.brand().handle(), |region| {
                self.covers_region_ambiently(region)
            });
        StoredReach {
            foreign,
            borrows_into_home,
        }
    }

    /// Build the terminal carrier for a value living **in this scope's region** from its binding's
    /// stored reach: the reference-only `{ bit, ref }` over `foreign` (the value's home-omitted
    /// foreign reach, captured at bind time). The carrier pins nothing — the value and its reach
    /// set are covered by this scope's region (the container), and a read that leaves the container
    /// travels as a [`DeliveredCarried`] envelope pinned by the home frame
    /// ([`Self::seal_resident_delivered`]). The bundle runs on the confined arena surface
    /// ([`RegionBrand::seal_resident`]), so `Witnessed::resident` is never reached from a builtin.
    pub(crate) fn resident_value_carrier<'r>(
        &self,
        obj: &'a KObject<'a>,
        stored: StoredReach<'r>,
    ) -> Witnessed<CarriedFamily, CarrierWitness> {
        self.brand()
            .seal_resident(Carried::Object(obj), self.resident_witness(stored))
    }

    /// Build a resident carrier's witness: the reference-only `{ bit, ref }` over `foreign` — the
    /// value's binding-time-minted, already home-omitted reach. A reference-copy of an existing
    /// hosted set, never a rebuild: `foreign` was minted once, at bind time
    /// ([`Self::host_reach_of`]), into the binding scope's home arena, so referencing it costs no
    /// allocation, and the read that re-anchors it names its pin there (the home frame the resident
    /// seal pairs as the envelope host). A fully-owned value (`foreign: None`, bit unset) gets the
    /// empty carrier.
    ///
    /// When `self` is a transparent window over borrowed bindings ([`Self::child_transparent`]),
    /// the home frame is the call-site frame but `foreign` points into the *owning* (module)
    /// scope's own arena, not the call site's — the binding was minted there at the module's own
    /// bind time. Sound because the window's overlay reach-fold (`USING`'s body,
    /// `builtins/using_scope.rs`) mints the opened module's own carrier into the call-site arena at
    /// overlay construction, before any such carrier exists — so holding the call-site frame roots
    /// the module's arena one hop removed, and through it `foreign`'s pointee.
    fn resident_witness<'r>(&self, stored: StoredReach<'r>) -> CarrierWitness {
        CarrierWitness::new(stored.borrows_into_home, stored.foreign)
    }

    /// Seal a resident carrier — a value already living in this scope's own region — into a
    /// [`DeliveredCarried`] envelope pinned by this scope's own region owner. The resident twin of
    /// the scheduler's [`dep_delivered`](crate::scheduler::Scheduler::dep_delivered): the pin is the
    /// home frame the caller reads the value under (`region_owner().upgrade()`, the same owner
    /// [`resident_value_carrier`](Self::resident_value_carrier) folds into the witness), so a spliced
    /// resident cell travels self-covering by its own witness *and* pinned by its home, identical in
    /// shape to a delivered dep — there is no `pin: None` resident special case at the splice sites.
    pub(crate) fn seal_resident_delivered(
        &self,
        witnessed: Witnessed<CarriedFamily, CarrierWitness>,
    ) -> DeliveredCarried {
        let home = self
            .region_owner()
            .upgrade()
            .expect("the resident scope's region owner is held while its value is sealed");
        Delivered::seal(witnessed, home)
    }

    /// Adopt a sealed dep carrier into this scope. The two channels adopt differently:
    ///
    /// - **Object**: copy-free. [`Delivered::adopt_into`] mints the carrier's reach — with its
    ///   residence host materialized as a member ([`Residence::Kept`]) — into this scope's own arena
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
    /// Where [`resident_value_carrier`](Self::resident_value_carrier) seals a value already living
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
            None => cell.adopt_into(self.brand().handle(), |region| {
                self.covers_region_ambiently(region)
            }),
        }
    }

    /// Adopt a sealed dep carrier's **object** into this scope by structural copy — the
    /// value-channel twin of [`Self::adopt_sealed`]'s copy-free object arm, for a consumer that
    /// re-homes the value anyway (a call's argument delivery). The top node is `deep_clone`d into
    /// this scope's own arena, so the producer's region is *not* part of the copy's residence: the
    /// mint stores the copy's reach ([`Self::adopted_reach_of`] — reach members plus the host only
    /// when the value's borrows genuinely cover it), never a residence-only host pin. This is what
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
    /// A value that **embeds a record** (a bare record, or one behind a `Tagged`/`Wrapped` spine)
    /// is totally rebuilt into this scope's region through the record door
    /// ([`Self::rebuild_delivered_substrate`]) rather than taking the pointer-copy arm: the record's
    /// substrate is region-resident and cannot cross the checked audit by a `deep_clone` (which leaves
    /// the substrate in the retiring producer, uncovered when its home is only ambiently covered). This
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
            let (object, _stored) = self
                .rebuild_delivered_substrate(cell, |carried| Ok(carried.object()), types)
                .expect("a whole-value record adoption's copy is infallible");
            return Carried::Object(object);
        }
        // Mint FIRST: pin every region the copy still reaches (interior borrows survive
        // `deep_clone`) into this scope's arena before the copy's `&'a` is fabricated. Copied mode:
        // the producer host materializes only if the value's borrows genuinely reach it. Also the
        // deep-cloned copy's own residence evidence — its leaves may still embed the producer's
        // foreign borrows.
        let reach = self.adopted_reach_of(cell);
        cell.open(|live| {
            Carried::Object(
                self.alloc_object_delivered(
                    live.object().deep_clone(),
                    std::slice::from_ref(&reach),
                    types,
                )
                .expect("a deep copy's own residence must be covered by its own reach evidence"),
            )
        })
    }

    /// Bind a delivered value's record-embedding **projection** into this scope, routing the
    /// escape-seam cost chooser ([`copy_or_pin`]) over the projected record. `project` selects
    /// what to bind (identity for a whole-value bind, a `Tagged`/`Wrapped` payload for a MATCH/TRY
    /// `it`); the caller vets that it yields a value embedding a record (a bare record, or one behind
    /// a `Tagged`/`Wrapped` spine). The verb decides copy vs pin:
    ///
    /// - **Copy** — a priceable home-crossing record with a clear borrows-home bit and small cost
    ///   (copied out and released, the retiring producer frees), plus every unpriceable record and
    ///   any projection whose top is a `Tagged`/`Wrapped` spine embedding a record (still-`Rc` at the
    ///   top, unpriceable there): the value is totally rebuilt into this scope's region through the
    ///   record door ([`Self::rebuild_delivered_substrate`]).
    /// - **Pin** — a record that borrows its home region, a small home-crossing pin, or a foreign
    ///   (producer-resident) crossing: the projection is pointer-copied ([`KObject::deep_clone`], a
    ///   pointer copy for a record sharing the producer-region substrate) and moved in under the
    ///   binding's non-omitting `Kept` stored reach ([`Self::pinned_reach_of`]). A record substrate
    ///   carries no home-naming borrow, so that reach names the producer host explicitly — never
    ///   omitting it under ambient coverage — and the residence audit evidences the foreign substrate
    ///   through the `any_member_region` reach-member path rather than the dest-resident `owns_substrate`
    ///   check. The reach is the pin's liveness, so this verb is confined to the bind seam, where
    ///   [`Self::bind_delivered`] stores the reach on the binding entry — never the argument re-home
    ///   ([`Self::adopt_sealed_copied`]), which discards it and copies unconditionally.
    ///
    /// Returns the resident reference paired with the binding's stored reach (minted at the verb's
    /// residence mode), the same pair [`Self::bind_delivered`] / a caller's terminal seal consume.
    pub(crate) fn copy_delivered_substrate<P>(
        &self,
        cell: &DeliveredCarried,
        project: P,
        types: &TypeRegistry,
    ) -> Result<(&'a KObject<'a>, StoredReach<'a>), KError>
    where
        P: for<'b> Fn(&Carried<'b>) -> Result<&'b KObject<'b>, KError>,
    {
        let host_region = cell.host().region();
        let verb = cell.open(|live| match project(&live) {
            Ok(record) => match record {
                KObject::Record(substrate, _) => copy_or_pin(substrate, record, host_region),
                // A projection embedding a record behind a `Tagged`/`Wrapped` spine is unpriceable at
                // the top (still-`Rc`): copy with a probe-derived release bit.
                _ => RegionEscape::Copy {
                    released: !still_borrows_host(record, host_region),
                },
            },
            Err(_) => RegionEscape::Copy { released: false },
        });

        match verb {
            RegionEscape::Copy { .. } => self.rebuild_delivered_substrate(cell, project, types),
            // Pin: the record stays in its producer region; the projection is pointer-copied and
            // moved in under the non-omitting `Kept` stored reach ([`Self::pinned_reach_of`]), whose
            // explicitly named producer region covers the foreign substrate on the audit's
            // `any_member_region` reach-member path. The reach is the pin's liveness — the caller
            // ([`Self::bind_delivered`]) stores it on the binding.
            RegionEscape::Pin => {
                let stored = self.pinned_reach_of(cell);
                let allocated = cell.open(|live| {
                    let projected = project(&live)?;
                    self.alloc_object_delivered(
                        projected.deep_clone(),
                        std::slice::from_ref(&stored),
                        types,
                    )
                })?;
                Ok((allocated, stored))
            }
        }
    }

    /// Rebuild a delivered value's record-embedding **projection** into this scope's region through
    /// the record door — the copy path for a region-resident record substrate, which cannot be
    /// pointer-copied past the checked residence audit. `project` selects what to copy (identity for a
    /// whole-value bind, a `Tagged`/`Wrapped` payload for a MATCH/TRY `it`); the caller vets that it
    /// yields a value embedding a record (a bare record, or one behind a `Tagged`/`Wrapped` spine —
    /// [`copy_object_into`] rebuilds the whole spine). The value relocates at the record's own
    /// release-exact seam mode — a plain-data record `Residence::Released` (the retiring producer
    /// frees), a record still borrowing its producer `Residence::Copied` and pinned — with the copy's
    /// foreign reach minted into this scope's arena for liveness. The top node is then re-boxed
    /// through the checked door to recover the `&'a` reference; its O(1) `owns_substrate` membership
    /// passes because the rebuilt substrate is scope-resident, so no reach evidence is needed. Returns
    /// the resident reference paired with the binding's stored reach (minted at the same mode).
    ///
    /// This is the unconditional-copy half of [`Self::copy_delivered_substrate`]'s chooser: the argument
    /// re-home ([`Self::adopt_sealed_copied`]) calls it directly, and the chooser's `Copy` verb
    /// delegates here (a `Copy` verb's residence is exactly this release-exact mode — a clear
    /// borrows-home bit at a home crossing agrees with the probe, and the unpriceable / embedded-spine
    /// verbs read the probe directly).
    fn rebuild_delivered_substrate<P>(
        &self,
        cell: &DeliveredCarried,
        project: P,
        types: &TypeRegistry,
    ) -> Result<(&'a KObject<'a>, StoredReach<'a>), KError>
    where
        P: for<'b> Fn(&Carried<'b>) -> Result<&'b KObject<'b>, KError>,
    {
        let host_region = cell.host().region();
        let mode = cell.open(|live| match project(&live) {
            Ok(record) if still_borrows_host(record, host_region) => Residence::Copied,
            Ok(_) => Residence::Released,
            Err(_) => Residence::Copied,
        });
        // The binding's stored reach, minted at the copy's own mode: a `Released` plain-data record
        // materializes no producer host, so a tail loop's retiring frame does not ride this binding.
        let stored = self.envelope_reach_of(cell, mode);
        let dest = Witnessed::<RegionHandleFamily<KoanStorageProfile>, CarrierWitness>::resident(
            self.brand().handle(),
        );
        let mut projection_error: Option<KError> = None;
        let copied = cell
            .transfer_into_placing::<RegionHandleFamily<KoanStorageProfile>, CarriedFamily, KoanStorageProfile>(
                dest,
                mode,
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
        let pin = self
            .region_owner()
            .upgrade()
            .expect("the adopting scope's region owner is held while copying a delivered record");
        let object = Sealed::seal(copied).open_with(&pin, |live| {
            self.alloc_object_delivered(live.object().deep_clone(), &[], types)
                .expect("a rebuilt record's substrate is resident in the adopting scope's region")
        });
        Ok((object, stored))
    }

    /// Build the terminal carrier for a type living **in this scope's region** — the type-channel
    /// twin of [`Self::resident_value_carrier`]. The witness is empty: a `KType` is owned data, so
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

    /// The full stored token for a module minted in **this** scope from its `child` scope — the
    /// derivation door that folds the child's home-omitted foreign reach together with the
    /// home-borrow bit the mint derives (`true` iff a child entry set or the child's own region
    /// owner reaches this scope's region before home-omission). The foreign half is the seal-time
    /// union over the child's own **binding-entry** hosted sets (each already home-omitted in the
    /// child's arena), plus the child's own region owner (materialized, foreign to this parent
    /// scope); never recovered by walking the built module value. A co-located module (`MODULE`,
    /// opaque `:|`) folds nothing extra; a transparent `:!` view of a source module pins that
    /// source's (foreign) region and reach. Returning the whole [`StoredReach`], a MODULE finish /
    /// `:|` view seals its terminal from a token nothing outside the derivation can forge. The omit
    /// stays the home-pin-only half-predicate: a per-call child folds no lexical-ancestor omission,
    /// only the home frame's own storage pin.
    pub(crate) fn child_module_reach(&self, child: &Scope<'a>) -> StoredReach<'a> {
        let home = self.region_owner().upgrade();
        let entry_sets: Vec<&FrameSet> = child.bindings().entry_reaches();
        let hosts: Vec<Rc<FrameStorage>> = child.region_owner().upgrade().into_iter().collect();
        let (foreign, borrows_into_home) = self.brand().mint(&entry_sets, &hosts, |region| {
            home.as_ref().is_some_and(|h| h.pins_region(region))
        });
        StoredReach {
            foreign,
            borrows_into_home,
        }
    }
}
