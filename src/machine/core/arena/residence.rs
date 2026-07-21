//! The residence-audit machinery: the [`Residence`] ownership predicate and its call-site half
//! [`ResidenceEvidence`], the per-family [`AuditedStored`] impls that run each family's residence
//! walk, and the evidence-tier [`Scope`] move-in doors whose audits consume this scope's minted
//! reach. The tiers live beside [`Residence`] rather than in `scope.rs` because a [`ResidenceEvidence`]
//! is meaningful only relative to the scope that minted it (see the impl block's own doc). The
//! region/brand substrate lives in the parent `arena` module.

use std::cell::Cell;

use super::{FrameSet, KoanRegion, KoanRegionExt, KoanStorageProfile};
use crate::machine::core::{KError, KErrorKind, KFunction, Scope, StoredReach};
use crate::machine::model::{CarriedFamily, KObject, Module, TypeRegistry};
use crate::machine::CarrierWitness;
use crate::witnessed::{AuditedStored, Witnessed};

/// The evidence-tier move-ins live on [`Scope`], not [`super::RegionBrand`]: a [`StoredReach`] is
/// meaningful only relative to the scope that minted it — the mint materializes no member for a
/// region [`Scope::covers_region_ambiently`] already covers — so the audit that consumes one must
/// run against that same scope's region and ambient coverage. Taking the destination from `self`
/// makes it the minting scope's own region by construction; there is no scope parameter for a
/// caller to mismatch. (The block lives here, beside the other move-in tiers and [`Residence`],
/// rather than in `scope.rs`.)
impl<'a> Scope<'a> {
    /// The evidence tier for an `o` whose region borrows may reach a *foreign* region this scope
    /// has already minted reach evidence for (a read-site's materialized `StoredReach`), not just
    /// its own region. Widens [`super::RegionBrand::alloc_object_checked`]'s dest-only audit to
    /// "this scope's region, `evidence`'s reach members, or a region
    /// [`Self::covers_region_ambiently`] covers" — the last disjunct is the exact complement of the
    /// mint's omission policy, which materializes no member for an ambiently covered region, so a
    /// dest/evidence-only audit would under-cover a value legitimately reaching one (a module bound
    /// at an outer/root scope, read by a nested per-call functor body). Placing an Object-arm module value
    /// takes this door — a module binds value-side — because the module's child scope lives in a
    /// region named by the derived stored reach, not necessarily this scope's own.
    pub(crate) fn alloc_object_reaching(
        &self,
        o: KObject<'_>,
        evidence: &StoredReach<'_>,
        types: &TypeRegistry,
    ) -> Result<&'a KObject<'a>, KError> {
        let kt = o.ktype();
        let sets: &[&FrameSet] = match &evidence.foreign {
            Some(fs) => std::slice::from_ref(fs),
            None => &[],
        };
        let ambient = |r: &KoanRegion| self.covers_region_ambiently(r);
        self.brand()
            .0
            .alloc_resident_checked::<KObject<'static>>(
                o,
                ResidenceEvidence::reaching_ambient(sets, &ambient),
            )
            .ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{}: borrows a region other than its seal's destination, evidence reach, \
                     or the destination scope's ambient coverage",
                    kt.name(types)
                )))
            })
    }

    /// The object evidence tier: for an `o` built from (or embedding a projection of) values
    /// whose reach this scope has already minted as `evidence` — a delivered arg carrier's
    /// `adopted_reach_of`/`host_reach_of`, or several for a multi-carrier fold (an args record).
    /// Widens the coverage predicate over every evidence member's hosting arena, same partiality
    /// as [`super::RegionBrand::alloc_object_checked`] — plus a region [`Self::covers_region_ambiently`]
    /// covers (see [`Self::alloc_object_reaching`]'s doc for why the evidence alone under-covers
    /// that case). Returns a structured `KError` on rejection — the item's decided non-panicking
    /// conversion-failure policy — so a bug in the caller's evidence computation surfaces as a
    /// catchable error rather than crashing the interpreter; a caller with no `KError` channel in
    /// hand (e.g. a seed closure with no `Result` return) calls `.expect(...)` naming the site
    /// invariant instead.
    pub(crate) fn alloc_object_delivered(
        &self,
        o: KObject<'_>,
        evidence: &[StoredReach<'_>],
        types: &TypeRegistry,
    ) -> Result<&'a KObject<'a>, KError> {
        let kt = o.ktype();
        let sets: Vec<&FrameSet> = evidence.iter().filter_map(|r| r.foreign).collect();
        let ambient = |r: &KoanRegion| self.covers_region_ambiently(r);
        self.brand()
            .0
            .alloc_resident_checked::<KObject<'static>>(
                o,
                ResidenceEvidence::reaching_ambient(&sets, &ambient),
            )
            .ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{}: borrows a region not covered by dest, the supplied evidence, or \
                     the destination scope's ambient coverage",
                    kt.name(types)
                )))
            })
    }

    /// Placement for a `Module` whose child scope legitimately lives in a region other than this
    /// scope's own — transparent-ascribe's re-tagged `Module`, which reuses the foreign source
    /// module's child scope. `evidence` is the `StoredReach` the caller minted for that child
    /// scope's region *before* this call ([`Scope::child_module_reach`]), so the audit widens
    /// [`super::RegionBrand::alloc_module`]'s dest-only check to "this scope's region, `evidence`'s
    /// reach, or a region [`Self::covers_region_ambiently`] covers" (see
    /// [`Self::alloc_object_reaching`]'s doc for why the last disjunct is needed).
    pub(crate) fn alloc_module_reaching(
        &self,
        m: Module<'_>,
        evidence: &StoredReach<'_>,
    ) -> &'a Module<'a> {
        let sets: &[&FrameSet] = match &evidence.foreign {
            Some(fs) => std::slice::from_ref(fs),
            None => &[],
        };
        let ambient = |region: &KoanRegion| self.covers_region_ambiently(region);
        self.brand()
            .0
            .alloc_resident_checked::<Module<'static>>(
                m,
                ResidenceEvidence::reaching_ambient(sets, &ambient),
            )
            .expect(
                "alloc_module_reaching: a Module's child scope must be covered by dest, the \
                 supplied evidence reach, or the destination scope's ambient coverage",
            )
    }

    /// Checked move-in of a fresh object into this scope's own region ([`super::RegionBrand::alloc_object_checked`]'s
    /// dest-only audit), paired with its derived [`StoredReach`]: `foreign` is `None` — a value that
    /// passes the dest-only audit borrows no foreign region — and `borrows_into_home` is the audit
    /// walk's saw-a-region-pointer flag ([`Residence::dest_only_seen`]), so the home-borrow bit is
    /// derived from the value's own borrows, never asserted.
    pub(crate) fn alloc_object_checked_stored(
        &self,
        value: KObject<'_>,
        types: &TypeRegistry,
    ) -> Result<(&'a KObject<'a>, StoredReach<'a>), KError> {
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
        Ok((
            obj,
            StoredReach {
                foreign: None,
                borrows_into_home: seen.get(),
            },
        ))
    }

    /// Checked alloc of a fresh object into this scope's region, derive its `(None, bit)` witness,
    /// and seal it as the resident carrier — one call for a value born carrier-less. The home-borrow
    /// bit is the checked audit's own saw-a-region-pointer flag, never a caller assertion.
    pub(crate) fn seal_fresh_object(
        &self,
        value: KObject<'_>,
        types: &TypeRegistry,
    ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
        let (obj, stored) = self.alloc_object_checked_stored(value, types)?;
        Ok(self.resident_value_carrier(obj, stored))
    }
}

/// Ownership predicate for the checked/reaching-tier residence audits: "`dest`, or the hosting
/// arena of some member of `reach`, or a region `ambient` reports as already covered" —
/// [`KObject::resident_in`](KObject::resident_in)'s dest-only check is the `reach: &[]`,
/// `ambient: None` case; the object delivered tier widens it. Each `reach` set was minted into `dest`'s own arena by
/// the same scope the audit runs against (`Scope::host_reach_of` / `adopted_reach_of`), so
/// membership here is dest-relative by construction — no separate "is this evidence dest-relative"
/// check is needed. `ambient`, when supplied, is the destination scope's own
/// [`Scope::covers_region_ambiently`](super::scope::Scope::covers_region_ambiently) — the exact
/// predicate every `host_reach_of` / `adopted_reach_of` mint omits by, so a region the mint left
/// out of `reach` is still resident — omitted from the *reach set*, never from *residence*. Only
/// [`Scope`]'s own evidence-tier methods construct the `ambient` form, binding the predicate to
/// the destination scope by construction.
pub(crate) struct Residence<'d> {
    dest: &'d KoanRegion,
    reach: &'d [&'d FrameSet],
    ambient: Option<&'d dyn Fn(&KoanRegion) -> bool>,
    /// A saw-a-region-pointer recorder: each `owns_*` leaf (a `KFunction` / `Module`
    /// pointer — the residence side-table's recorded region pointers) sets it. A
    /// walk that passes the audit and set this reports a value whose borrows reach *some* region; a
    /// value freshly stored in the scope's own region (where every pointer is home by construction)
    /// reads it as its honest home-borrow bit ([`Scope::seal_fresh_object`]). `None` when a caller
    /// wants the plain audit with no recording.
    seen: Option<&'d Cell<bool>>,
}

impl<'d> Residence<'d> {
    pub(crate) fn dest_only(dest: &'d KoanRegion) -> Self {
        Residence {
            dest,
            reach: &[],
            ambient: None,
            seen: None,
        }
    }

    /// [`Self::dest_only`] with a saw-a-region-pointer recorder — the [`Self::seen`] flag is set
    /// while the walk visits any `owns_*` region-pointer leaf.
    pub(crate) fn dest_only_seen(dest: &'d KoanRegion, seen: &'d Cell<bool>) -> Self {
        Residence {
            dest,
            reach: &[],
            ambient: None,
            seen: Some(seen),
        }
    }

    pub(crate) fn with_reach(dest: &'d KoanRegion, reach: &'d [&'d FrameSet]) -> Self {
        Residence {
            dest,
            reach,
            ambient: None,
            seen: None,
        }
    }

    /// [`Self::with_reach`] plus the destination scope's own ambient coverage
    /// ([`Scope::covers_region_ambiently`]) — see the type doc's `ambient` paragraph.
    pub(crate) fn with_reach_and_ambient(
        dest: &'d KoanRegion,
        reach: &'d [&'d FrameSet],
        ambient: &'d dyn Fn(&KoanRegion) -> bool,
    ) -> Self {
        Residence {
            dest,
            reach,
            ambient: Some(ambient),
            seen: None,
        }
    }

    /// Record a visited region-pointer leaf into [`Self::seen`], if a recorder is attached.
    fn note_region_pointer(&self) {
        if let Some(seen) = self.seen {
            seen.set(true);
        }
    }

    /// Whether `region` is `dest` itself, is covered by some `reach` member's own pin chain, or is
    /// reported covered by `ambient` — [`Scope::alloc_module_reaching`]'s coverage check.
    /// [`RegionSet::pins_region`](crate::witnessed::RegionSet::pins_region) is the library's public
    /// reach-coverage query (unlike
    /// [`RegionSet::members`](crate::witnessed::RegionSet::members), which is gated to
    /// `test`/`test-hooks` — koan cannot enumerate a
    /// set's members in production, only ask it whether a given region is covered).
    pub(crate) fn covers_region(&self, region: &KoanRegion) -> bool {
        std::ptr::eq(self.dest, region)
            || self.reach.iter().any(|fs| fs.pins_region(region))
            || self.ambient.is_some_and(|f| f(region))
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
}

/// The typed residence evidence a move-in site hands to an [`AuditedStored`] audit — the
/// call-site half of a [`Residence`], without the destination region (the audit takes that from
/// the handle it runs against). A family's `audit` builds a [`Residence`] from `(region, self)`
/// and runs the family's own residence walk over it. Fields are private and mirror [`Residence`]'s
/// evidence fields: `reach` are the reach sets a foreign borrow may legitimately land in, `ambient`
/// (when present) is the destination scope's own [`Scope::covers_region_ambiently`], and `seen` is
/// the walk's saw-a-region-pointer recorder.
///
/// [`Self::dest_only`] and [`Self::dest_only_seen`] are freely mintable within `machine::core`; the
/// ambient-bearing form ([`Self::reaching_ambient`]) is module-private, minted only by [`Scope`]'s
/// own evidence-tier methods, so the ambient predicate is always the destination scope's own
/// coverage — a builtin cannot mint a permissive (always-true ambient) context.
pub struct ResidenceEvidence<'ctx> {
    reach: &'ctx [&'ctx FrameSet],
    ambient: Option<&'ctx dyn Fn(&KoanRegion) -> bool>,
    seen: Option<&'ctx Cell<bool>>,
}

impl<'ctx> ResidenceEvidence<'ctx> {
    /// Dest-only evidence: the audit vets `value` against the destination region alone.
    pub(crate) fn dest_only() -> Self {
        ResidenceEvidence {
            reach: &[],
            ambient: None,
            seen: None,
        }
    }

    /// [`Self::dest_only`] with a saw-a-region-pointer recorder — the [`Residence::seen`] flag the
    /// checked-stored sites read after the store to derive a value's home-borrow bit.
    pub(crate) fn dest_only_seen(seen: &'ctx Cell<bool>) -> Self {
        ResidenceEvidence {
            reach: &[],
            ambient: None,
            seen: Some(seen),
        }
    }

    /// The reaching evidence tier: `reach`'s foreign sets plus the destination scope's own ambient
    /// coverage. Module-private so only [`Scope`]'s evidence-tier methods mint it — binding
    /// `ambient` to the destination scope by construction.
    fn reaching_ambient(
        reach: &'ctx [&'ctx FrameSet],
        ambient: &'ctx dyn Fn(&KoanRegion) -> bool,
    ) -> Self {
        ResidenceEvidence {
            reach,
            ambient: Some(ambient),
            seen: None,
        }
    }
}

// SAFETY: `audit` returns true only when every region borrow the stored `KObject`
// carries is resident in `region`, covered by `context`'s reach evidence, or (when the ambient
// predicate is present) covered by the destination scope's own ambient coverage — the residence the
// `KObject` walk verifies. A `Wrapped { type_id }` tag needs no walk: `KType` is a Copy digest
// handle carrying no region borrow, so it reaches nothing outside `region`.
unsafe impl AuditedStored<KoanStorageProfile> for KObject<'static> {
    type AuditContext<'ctx> = ResidenceEvidence<'ctx>;
    fn audit(region: &KoanRegion, value: &KObject<'_>, context: ResidenceEvidence<'_>) -> bool {
        match (context.ambient, context.seen) {
            (Some(ambient), _) => {
                // The plain evidence-only check first (cheap, directly unit-testable); only fall
                // back to the ambient-widened walk when it declines.
                value.resident_in_delivered(region, context.reach)
                    || value.resident_in_visiting(&Residence::with_reach_and_ambient(
                        region,
                        context.reach,
                        ambient,
                    ))
            }
            (None, Some(seen)) => {
                value.resident_in_visiting(&Residence::dest_only_seen(region, seen))
            }
            (None, None) => value.resident_in(region),
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
// itself, covered by `context`'s reach evidence, or covered by the destination scope's ambient
// coverage — the `Module` borrows that child scope, so its region must be covered. Exact: the
// child-scope reference is the `Module`'s only region borrow. The `type_members` /
// `slot_type_tags` maps and the `self_sig` cell need no walk — a `KType` owns its content and
// borrows no region data, so nothing installed through them can reach outside `region`.
unsafe impl AuditedStored<KoanStorageProfile> for Module<'static> {
    type AuditContext<'ctx> = ResidenceEvidence<'ctx>;
    fn audit(region: &KoanRegion, value: &Module<'_>, context: ResidenceEvidence<'_>) -> bool {
        let residence = match context.ambient {
            Some(ambient) => Residence::with_reach_and_ambient(region, context.reach, ambient),
            None => Residence::dest_only(region),
        };
        residence.covers_region(value.child_scope().region())
    }
}
