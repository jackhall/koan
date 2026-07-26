//! The residence-audit machinery: the [`Residence`] ownership predicate and its call-site half
//! [`ResidenceEvidence`], the per-family [`AuditedStored`] impls that run each family's residence
//! walk, and the evidence-tier [`Scope`] move-in doors whose audits consume this scope's minted
//! reach. The tiers live beside [`Residence`] rather than in `scope.rs` because a [`ResidenceEvidence`]
//! is meaningful only relative to the scope that minted it (see the impl block's own doc). The
//! region/brand substrate lives in the parent `arena` module.

use std::cell::Cell;

use super::{FrameReach, KoanRegion, KoanRegionExt, KoanStorageProfile};
use crate::machine::core::bindings::SealedValue;
use crate::machine::core::{KError, KErrorKind, KFunction, Scope};
use crate::machine::model::{
    Carried, CarriedFamily, ContainerSubstrate, KObject, Module, TypeRegistry,
};
use crate::machine::{CarrierWitness, DeliveredCarried};
use crate::witnessed::{AuditedStored, Witnessed};

/// A move-in's minted reach evidence: the arena-hosted description (`None` == the empty set) and
/// the borrows-into-this-region bit, both derived by [`Scope::mint_retained`] from the same source
/// claim the value is copied under. It is meaningful only relative to the scope that minted it, so
/// it never leaves the door that derived it.
type MintedReach<'a> = (Option<&'a FrameReach>, bool);

/// The evidence-tier move-ins live on [`Scope`], not [`super::RegionBrand`]: a minted reach is
/// meaningful only relative to the scope that minted it — its description is hosted in that
/// scope's own arena — so the audit that consumes one must run against that same scope's region.
/// Taking the destination from `self` makes it the minting scope's own region by construction;
/// there is no scope parameter for a caller to mismatch. (The block lives here, beside the other
/// move-in tiers and [`Residence`], rather than in `scope.rs`.)
impl<'a> Scope<'a> {
    /// The evidence tier for an `o` whose region borrows may reach a *foreign* region this scope
    /// has already minted reach evidence for (a read-site's materialized `StoredReach`), not just
    /// its own region. Widens [`super::RegionBrand::alloc_object_checked`]'s dest-only audit to
    /// "this scope's region, or `evidence`'s reach members" — exact, because the mint applies no
    /// omission policy, so every region the value legitimately reaches is a named member. Placing
    /// an Object-arm module value takes this door — a module binds value-side — because the
    /// module's child scope lives in a region named by the derived stored reach, not necessarily
    /// this scope's own.
    pub(crate) fn store_object_adopted<P>(
        &self,
        cell: &DeliveredCarried,
        project: P,
        types: &TypeRegistry,
    ) -> Result<(&'a KObject<'a>, Option<&'a FrameReach>, bool), KError>
    where
        P: for<'b> Fn(&Carried<'b>) -> Result<&'b KObject<'b>, KError>,
    {
        let minted = self.copied_reach_of(cell);
        let obj = self.store_projection_reaching(cell, &project, minted, types)?;
        Ok((obj, minted.0, minted.1))
    }

    /// The pin twin of [`Self::store_object_adopted`]: mint the whole-envelope reach (naming every
    /// region the record borrows so the audit's `any_member_region` arm evidences the foreign
    /// substrate) and copy the projection in under it. Used by the bind seam's pin verb, where the
    /// region's union bundle holds the reach.
    pub(crate) fn store_object_pinned<P>(
        &self,
        cell: &DeliveredCarried,
        project: P,
        types: &TypeRegistry,
    ) -> Result<(&'a KObject<'a>, Option<&'a FrameReach>, bool), KError>
    where
        P: for<'b> Fn(&Carried<'b>) -> Result<&'b KObject<'b>, KError>,
    {
        let minted = self.mint_retained(cell.pins());
        let obj = self.store_projection_reaching(cell, &project, minted, types)?;
        Ok((obj, minted.0, minted.1))
    }

    /// Seal a resident `Module` value under the reach its own `child` scope mints — the object-arm
    /// module bind ([`Scope::bind_module`]) and an opaque ascription view. Derives the child's reach
    /// ([`Self::child_module_reach`]), audits the wrapping `KObject::Module` against it (the
    /// module's child scope may live in a region other than this scope's own — a co-located `MODULE`
    /// child, or a foreign source reused by a view), and seals. Value and reach are both derived from
    /// `child` inside this door, so no caller can pair the module with a foreign reach.
    pub(crate) fn store_module_object(
        &self,
        module: &'a Module<'a>,
        child: &Scope<'a>,
        types: &TypeRegistry,
    ) -> Result<SealedValue, KError> {
        let minted = self.child_module_reach(child);
        let obj = self.store_value_reaching(KObject::Module(module), minted, types)?;
        Ok(self.seal_resident_value(Carried::Object(obj), minted.0, minted.1))
    }

    /// The transparent-ascription store: a fresh re-tagged `Module` reusing a *foreign* source
    /// module's child scope, whose region is not this scope's own. Derives one reach off that
    /// `source_child` ([`Self::child_module_reach`]), allocates the re-tagged `Module` reaching it,
    /// runs `seal` on the resident module (the view's self-sig), then audits the wrapping
    /// `KObject::Module` against the *same* reach and seals. Both audits ride one derived reach, so
    /// value and reach cannot be mispaired.
    #[cfg_attr(not(feature = "ascription"), allow(dead_code))]
    pub(crate) fn store_transparent_view(
        &self,
        module: Module<'a>,
        source_child: &Scope<'a>,
        seal: impl FnOnce(&'a Module<'a>),
        types: &TypeRegistry,
    ) -> Result<SealedValue, KError> {
        let minted = self.child_module_reach(source_child);
        let sets: &[&FrameReach] = stored_sets(&minted);
        let new_module: &'a Module<'a> = self
            .brand()
            .0
            .alloc_resident_checked::<Module<'static>>(module, ResidenceEvidence::reaching(sets))
            .expect(
                "store_transparent_view: a Module's child scope must be covered by dest or the \
                 derived reach",
            );
        seal(new_module);
        let obj = self.store_value_reaching(KObject::Module(new_module), minted, types)?;
        Ok(self.seal_resident_value(Carried::Object(obj), minted.0, minted.1))
    }

    /// Audit a projection of the delivered `cell` (deep-cloned into this scope's region) against
    /// `stored`'s reach — the shared audit behind the object store doors. Widens
    /// [`super::RegionBrand::alloc_object_checked`]'s dest-only audit to "this scope's region, or
    /// `stored`'s reach members"; exact, because the mint omits nothing. Returns a structured
    /// `KError` on rejection so a bug in the caller's derivation surfaces catchably rather than
    /// crashing the interpreter. The projection is read under the cell's own pin
    /// ([`DeliveredCarried::open`]), so the source backing stays live for the deep clone.
    fn store_projection_reaching<P>(
        &self,
        cell: &DeliveredCarried,
        project: &P,
        minted: MintedReach<'_>,
        types: &TypeRegistry,
    ) -> Result<&'a KObject<'a>, KError>
    where
        P: for<'b> Fn(&Carried<'b>) -> Result<&'b KObject<'b>, KError>,
    {
        cell.open(|live| {
            let projected = project(&live)?;
            self.store_value_reaching(projected.deep_clone(), minted, types)
        })
    }

    /// Audit an already-resident-lifetime value `o` against `stored`'s reach —
    /// the no-projection twin of [`Self::store_projection_reaching`], for a value in hand (a module
    /// wrapped as an object). Private: its only callers are the fused store doors above, each of
    /// which derives `o` and `stored` from one source (a `cell`, a `child` scope), so the pairing is
    /// co-derived here and never assembled from a value and a reach held side by side. No `pub(crate)`
    /// door exposes this two-parameter shape.
    fn store_value_reaching(
        &self,
        o: KObject<'_>,
        minted: MintedReach<'_>,
        types: &TypeRegistry,
    ) -> Result<&'a KObject<'a>, KError> {
        let kt = o.ktype();
        let sets: &[&FrameReach] = stored_sets(&minted);
        self.brand()
            .0
            .alloc_resident_checked::<KObject<'static>>(o, ResidenceEvidence::reaching(sets))
            .ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{}: borrows a region not covered by dest or its reach evidence",
                    kt.name(types)
                )))
            })
    }

    /// Checked move-in of a fresh object into this scope's own region ([`super::RegionBrand::alloc_object_checked`]'s
    /// dest-only audit), paired with its derived reach: the description is `None` — a value that
    /// passes the dest-only audit borrows no foreign region — and the bit is the audit
    /// walk's saw-a-region-pointer flag ([`Residence::dest_only_seen`]), so the home-borrow bit is
    /// derived from the value's own borrows, never asserted.
    pub(crate) fn alloc_object_checked_stored(
        &self,
        value: KObject<'_>,
        types: &TypeRegistry,
    ) -> Result<(&'a KObject<'a>, MintedReach<'a>), KError> {
        let kt = value.ktype();
        let seen = Cell::new(false);
        let obj = self
            .brand()
            .0
            .alloc_resident_checked::<KObject<'static>>(
                value,
                ResidenceEvidence::dest_only_seen(&seen),
            )
            .ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{}: borrows a region other than its seal's destination",
                    kt.name(types)
                )))
            })?;
        Ok((obj, (None, seen.get())))
    }

    /// Checked alloc of a fresh object into this scope's region, derive its `(None, bit)` witness,
    /// and seal it as the resident carrier — one call for a value born carrier-less. The home-borrow
    /// bit is the checked audit's own saw-a-region-pointer flag, never a caller assertion.
    pub(crate) fn seal_fresh_object(
        &self,
        value: KObject<'_>,
        types: &TypeRegistry,
    ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
        let (obj, (_, borrows_home)) = self.alloc_object_checked_stored(value, types)?;
        Ok(self.brand().seal_resident(
            Carried::Object(obj),
            CarrierWitness::new(borrows_home, None),
        ))
    }

    /// Test affordance: drive the reaching/delivered residence audit directly from a value in hand
    /// and a fabricated reach, for a unit test exercising the audit predicate in isolation.
    /// `#[cfg(test)]`-gated so production keeps value and reach derived together inside a store
    /// door — the mispairing a bare `(value, reach)` door would allow is not reachable outside
    /// tests.
    #[cfg(test)]
    pub(crate) fn store_value_reaching_for_test(
        &self,
        o: KObject<'_>,
        minted: MintedReach<'_>,
        types: &TypeRegistry,
    ) -> Result<&'a KObject<'a>, KError> {
        self.store_value_reaching(o, minted, types)
    }
}

/// A minted reach's description as an audit slice: a one-element slice naming it, or empty when the
/// value reaches nothing. The reaching audits take a slice so one audit shape covers both the
/// reach-bearing and region-pure cases.
fn stored_sets<'s, 'r>(minted: &'s MintedReach<'r>) -> &'s [&'r FrameReach] {
    match &minted.0 {
        Some(fs) => std::slice::from_ref(fs),
        None => &[],
    }
}

/// Ownership predicate for the checked/reaching-tier residence audits: "`dest`, or the hosting
/// arena of some member of `reach`" — the `reach: &[]` case is the plain dest-only check
/// ([`KObject::resident_in_delivered`](KObject::resident_in_delivered)); the object delivered tier
/// widens it. Each `reach` set was minted
/// into `dest`'s own arena by the same scope the audit runs against
/// (`Scope::envelope_reach_of`), so membership here is dest-relative by construction — no separate
/// "is this evidence dest-relative" check is needed. A mint applies no omission policy, so `reach`
/// names every region the value borrows into and the two disjuncts are exhaustive.
pub(crate) struct Residence<'d> {
    dest: &'d KoanRegion,
    reach: &'d [&'d FrameReach],
    /// A saw-a-region-pointer recorder: each `owns_*` leaf (a `KFunction` / `Module`
    /// pointer — the residence side-table's recorded region pointers) sets it. A
    /// walk that passes the audit and set this reports a value whose borrows reach *some* region; a
    /// value freshly stored in the scope's own region (where every pointer is home by construction)
    /// reads it as its honest home-borrow bit ([`Scope::seal_fresh_object`]). `None` when a caller
    /// wants the plain audit with no recording.
    seen: Option<&'d Cell<bool>>,
}

impl<'d> Residence<'d> {
    /// [`Self::with_reach`] with no reach evidence plus a saw-a-region-pointer recorder — the
    /// [`Self::seen`] flag is set while the walk visits any `owns_*` region-pointer leaf.
    pub(crate) fn dest_only_seen(dest: &'d KoanRegion, seen: &'d Cell<bool>) -> Self {
        Residence {
            dest,
            reach: &[],
            seen: Some(seen),
        }
    }

    pub(crate) fn with_reach(dest: &'d KoanRegion, reach: &'d [&'d FrameReach]) -> Self {
        Residence {
            dest,
            reach,
            seen: None,
        }
    }

    /// Record a visited region-pointer leaf into [`Self::seen`], if a recorder is attached.
    fn note_region_pointer(&self) {
        if let Some(seen) = self.seen {
            seen.set(true);
        }
    }

    /// Whether `region` is `dest` itself or is covered by some `reach` member's own pin chain — the
    /// module store doors' ([`Scope::store_module_object`], [`Scope::store_transparent_view`])
    /// coverage check.
    /// [`ReachDescription::pins_region`](crate::witnessed::ReachDescription::pins_region) is the
    /// library's public reach-coverage query (unlike
    /// [`ReachDescription::members`](crate::witnessed::ReachDescription::members), which is gated to
    /// `test`/`test-hooks` — koan cannot enumerate a
    /// description's members in production, only ask it whether a given region is covered).
    pub(crate) fn covers_region(&self, region: &KoanRegion) -> bool {
        std::ptr::eq(self.dest, region) || self.reach.iter().any(|fs| fs.pins_region(region))
    }

    /// Whether `module`'s own storage is `dest`-resident (the address side-table check) or its
    /// child scope's region is covered by `reach` — [`Self::covers_region`] over the module's own
    /// region accessor, since a raw payload pointer's *owning* region cannot be recovered from
    /// `reach` without enumerating members.
    pub(crate) fn owns_module(&self, module: &Module<'_>) -> bool {
        self.note_region_pointer();
        self.dest.owns_module(module as *const Module<'_>)
            || self.covers_region(module.child_scope().region())
    }

    pub(crate) fn owns_function(&self, f: &KFunction<'_>) -> bool {
        self.note_region_pointer();
        self.dest.owns_function(f as *const KFunction<'_>)
            || self.covers_region(f.captured_scope().region())
    }

    /// Whether `substrate`'s own storage is `dest`-resident (the address side-table check) or some
    /// `reach` member's own region owns it. Unlike [`Self::owns_module`]/[`Self::owns_function`], a
    /// `ContainerSubstrate<C>` carries no borrow naming its own home region — there is no
    /// scope/captured-scope shortcut to widen through [`Self::covers_region`] — so this walks
    /// `reach`'s members directly via
    /// [`ReachDescription::any_member_region`](crate::witnessed::ReachDescription::any_member_region), the
    /// production-safe per-member query that answers the address check without enumerating members
    /// out to the caller.
    pub(crate) fn owns_substrate<C>(&self, substrate: &ContainerSubstrate<C>) -> bool {
        self.note_region_pointer();
        let ptr = substrate as *const ContainerSubstrate<C>;
        self.dest.owns_substrate(ptr)
            || self
                .reach
                .iter()
                .any(|fs| fs.any_member_region(|region| region.owns_substrate(ptr)))
    }
}

/// The typed residence evidence a move-in site hands to an [`AuditedStored`] audit — the
/// call-site half of a [`Residence`], without the destination region (the audit takes that from
/// the handle it runs against). A family's `audit` builds a [`Residence`] from `(region, self)`
/// and runs the family's own residence walk over it. Fields are private and mirror [`Residence`]'s
/// evidence fields: `reach` are the reach sets a foreign borrow may legitimately land in, and
/// `seen` is the walk's saw-a-region-pointer recorder.
///
/// [`Self::dest_only`] and [`Self::dest_only_seen`] are freely mintable within `machine::core`; the
/// reach-bearing form ([`Self::reaching`]) is module-private, minted only by [`Scope`]'s own
/// evidence-tier methods, so the reach sets are always ones that scope minted into its own arena.
pub struct ResidenceEvidence<'ctx> {
    reach: &'ctx [&'ctx FrameReach],
    seen: Option<&'ctx Cell<bool>>,
}

impl<'ctx> ResidenceEvidence<'ctx> {
    /// Dest-only evidence: the audit vets `value` against the destination region alone.
    pub(crate) fn dest_only() -> Self {
        ResidenceEvidence {
            reach: &[],
            seen: None,
        }
    }

    /// [`Self::dest_only`] with a saw-a-region-pointer recorder — the [`Residence::seen`] flag the
    /// checked-stored sites read after the store to derive a value's home-borrow bit.
    pub(crate) fn dest_only_seen(seen: &'ctx Cell<bool>) -> Self {
        ResidenceEvidence {
            reach: &[],
            seen: Some(seen),
        }
    }

    /// The reaching evidence tier: `reach`'s foreign sets. Module-private so only [`Scope`]'s
    /// evidence-tier methods mint it — binding the sets to the destination scope's own arena by
    /// construction.
    fn reaching(reach: &'ctx [&'ctx FrameReach]) -> Self {
        ResidenceEvidence { reach, seen: None }
    }
}

// SAFETY: `audit` returns true only when every region borrow the stored `KObject`
// carries is resident in `region` or covered by `context`'s reach evidence — the residence the
// `KObject` walk verifies. A `Wrapped { type_id }` tag needs no walk: `KType` is a Copy digest
// handle carrying no region borrow, so it reaches nothing outside `region`.
unsafe impl AuditedStored<KoanStorageProfile> for KObject<'static> {
    type AuditContext<'ctx> = ResidenceEvidence<'ctx>;
    fn audit(region: &KoanRegion, value: &KObject<'_>, context: ResidenceEvidence<'_>) -> bool {
        match context.seen {
            Some(seen) => value.resident_in_visiting(&Residence::dest_only_seen(region, seen)),
            None => value.resident_in_delivered(region, context.reach),
        }
    }
}

// SAFETY: `audit` returns true only when `region` is the very region that owns the stored
// `KFunction`'s captured scope — the function borrows that scope, so a store elsewhere would
// lengthen the borrow's lifetime past its region.
unsafe impl AuditedStored<KoanStorageProfile> for KFunction<'static> {
    type AuditContext<'ctx> = ();
    fn audit(region: &KoanRegion, value: &KFunction<'_>, _context: ()) -> bool {
        std::ptr::eq(region, value.captured_scope().region())
    }
}

// SAFETY: `audit` returns true only when `region` is the region the stored `Scope` names as its
// own — every `Scope` borrows its parent, so a store into any other region would dangle.
unsafe impl AuditedStored<KoanStorageProfile> for Scope<'static> {
    type AuditContext<'ctx> = ();
    fn audit(region: &KoanRegion, value: &Scope<'_>, _context: ()) -> bool {
        std::ptr::eq(region, value.region())
    }
}

// SAFETY: `audit` returns true only when the stored `Module`'s child scope's region is `region`
// itself or covered by `context`'s reach evidence — the `Module` borrows that child scope, so its
// region must be covered. Exact: the child-scope reference is the `Module`'s only region borrow.
// The `type_members` / `slot_type_tags` maps and the `self_sig` cell need no walk — a `KType` owns
// its content and borrows no region data, so nothing installed through them can reach outside
// `region`.
unsafe impl AuditedStored<KoanStorageProfile> for Module<'static> {
    type AuditContext<'ctx> = ResidenceEvidence<'ctx>;
    fn audit(region: &KoanRegion, value: &Module<'_>, context: ResidenceEvidence<'_>) -> bool {
        Residence::with_reach(region, context.reach).covers_region(value.child_scope().region())
    }
}
