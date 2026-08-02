//! The residence-audit machinery: the [`Residence`] ownership predicate and its call-site half
//! [`ResidenceEvidence`], the per-family [`AuditedStored`] impls that run each family's residence
//! walk, and the [`Scope`] move-in doors that drive it. What remains is the **dest-only** tier: a
//! value that borrows only the region it is being stored into. A value reaching *elsewhere* is
//! built at a fold brand instead ([`FoldingBrand`](super::FoldingBrand)), where the rank-2 signature
//! proves it borrows nothing but the fold's declared operands — a compile error rather than a walk.
//! See [design/witness-hosting.md § Residence enforcement](../../../../design/witness-hosting.md#residence-enforcement).
//!
//! The doors live beside [`Residence`] rather than in `scope.rs` because a [`ResidenceEvidence`] is
//! meaningful only relative to the scope it is minted for (see the impl block's own doc). The
//! region/brand substrate lives in the parent `arena` module.

use std::cell::Cell;

use super::{FrameReach, KoanRegion, KoanRegionExt, KoanStorageProfile};
use crate::machine::core::{KError, KErrorKind, KFunction, Scope};
use crate::machine::model::{
    Carried, CarriedFamily, ContainerSubstrate, KObject, Module, TypeRegistry,
};
use crate::machine::CarrierWitness;
use crate::witnessed::{AuditedStored, Witnessed};

/// A move-in's minted description: the arena-hosted record [`Scope::mint_born_here`] derives for the
/// stored value. It records the value's residence (the minting scope's own region, stamped as the
/// description's host) and its exact reach in one record, so a door that holds one holds everything
/// the seal needs. It is meaningful only relative to the scope that minted it, so it never leaves
/// the door that derived it.
type MintedReach<'a> = &'a FrameReach;

/// The checked move-ins live on [`Scope`], not [`super::RegionBrand`]: the description a store
/// mints is meaningful only relative to the scope that minted it — it is hosted in that scope's own
/// arena — so the audit that pairs with one must run against that same scope's region. Taking the
/// destination from `self` makes it the minting scope's own region by construction; there is no
/// scope parameter for a caller to mismatch. (The block lives here, beside [`Residence`], rather
/// than in `scope.rs`.)
impl<'a> Scope<'a> {
    /// Checked move-in of a fresh object into this scope's own region — the dest-only residence
    /// walk ([`KObject::resident_in_visiting`]) paired with the description minted for the value
    /// ([`Scope::mint_born_here`]). A value that passes the walk borrows no foreign region, so the
    /// only member the description can carry is this scope's own — and it carries it exactly when
    /// the walk's saw-a-region-pointer flag ([`Residence::dest_only_seen`]) says the value genuinely
    /// holds one. Residence is recorded either way, as the description's host: a region-pure scalar
    /// still records where it lives.
    ///
    /// This is the last door where a runtime walk stands in for the fold brand's compile-time proof.
    /// What takes it is a carrier-less read-site value re-sealed here — a shape the `arg_carriers`
    /// contract says is region-pure, which a shape-split door proves at its signature instead.
    pub(crate) fn alloc_object_checked_stored(
        &self,
        value: KObject<'_>,
        types: &TypeRegistry,
    ) -> Result<(&'a KObject<'a>, MintedReach<'a>), KError> {
        // A string never crosses the audit: its bytes live in some region's bump and the bump keeps
        // no address table, so [`KObject::resident_in_visiting`] answers `false` for one and the
        // store would be refused. It takes the fold door's text arm instead — which is the same copy
        // this tier makes for every other fresh value, made where bytes can actually be placed. The
        // product borrows only the region it now lives in, so its description is this scope's own
        // region as host with no member: exactly what the audit walk mints for a value that saw no
        // region pointer at all.
        if let KObject::KString(text) = value {
            let obj = self.fold_resident_object(|brand| KObject::KString(brand.alloc_text(text)));
            return Ok((obj, self.mint_born_here(false)));
        }
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
        Ok((obj, self.mint_born_here(seen.get())))
    }

    /// Checked alloc of a fresh object into this scope's region under the description minted for it,
    /// bundled as the resident carrier — one call for a value born carrier-less. Whether the
    /// description names this region as a member is the checked audit's own saw-a-region-pointer
    /// flag, never a caller assertion.
    pub(crate) fn seal_fresh_object(
        &self,
        value: KObject<'_>,
        types: &TypeRegistry,
    ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
        let (obj, reach) = self.alloc_object_checked_stored(value, types)?;
        Ok(self.brand().seal_reaching(Carried::Object(obj), reach))
    }
}

/// Ownership predicate for the checked residence walk: the value's every region borrow must point
/// into `dest`. A value reaching anywhere else has no route through this tier at all — it is built
/// at a fold brand, where the rank-2 signature proves the same thing at compile time.
pub(crate) struct Residence<'d> {
    dest: &'d KoanRegion,
    /// A saw-a-region-pointer recorder: each `owns_*` leaf (a `KFunction` / `Module`
    /// pointer — the residence side-table's recorded region pointers) sets it. A
    /// walk that passes the audit and set this reports a value whose borrows reach *some* region; a
    /// value freshly stored in the scope's own region (where every pointer is home by construction)
    /// reads it as the verdict on whether its own region belongs in its minted description's members
    /// ([`Scope::mint_born_here`]). `None` when a caller wants the plain audit with no recording.
    seen: Option<&'d Cell<bool>>,
}

impl<'d> Residence<'d> {
    /// The dest-only predicate with no recorder — the plain "does this value borrow only `dest`"
    /// question. The store door always records, so this form is the walk's own unit-test probe.
    #[cfg(test)]
    pub(crate) fn dest_only(dest: &'d KoanRegion) -> Self {
        Residence { dest, seen: None }
    }

    /// [`Self::dest_only`] plus a saw-a-region-pointer recorder — the [`Self::seen`] flag is set
    /// while the walk visits any `owns_*` region-pointer leaf.
    pub(crate) fn dest_only_seen(dest: &'d KoanRegion, seen: &'d Cell<bool>) -> Self {
        Residence {
            dest,
            seen: Some(seen),
        }
    }

    /// Record a visited region-pointer leaf into [`Self::seen`], if a recorder is attached.
    fn note_region_pointer(&self) {
        if let Some(seen) = self.seen {
            seen.set(true);
        }
    }

    /// Whether `module`'s own storage is `dest`-resident (the address side-table check) or its child
    /// scope's region *is* `dest` — the `Module` borrows that child scope, so either answer places
    /// every borrow it carries inside `dest`.
    pub(crate) fn owns_module(&self, module: &Module<'_>) -> bool {
        self.note_region_pointer();
        self.dest.owns_module(module as *const Module<'_>)
            || std::ptr::eq(self.dest, module.child_scope().region())
    }

    pub(crate) fn owns_function(&self, f: &KFunction<'_>) -> bool {
        self.note_region_pointer();
        self.dest.owns_function(f as *const KFunction<'_>)
            || std::ptr::eq(self.dest, f.captured_scope().region())
    }

    /// Whether `substrate`'s own storage is `dest`-resident. Unlike
    /// [`Self::owns_module`]/[`Self::owns_function`], a `ContainerSubstrate<C>` carries no borrow
    /// naming its own home region, so the address side-table check is the whole question.
    pub(crate) fn owns_substrate<C>(&self, substrate: &ContainerSubstrate<C>) -> bool {
        self.note_region_pointer();
        self.dest
            .owns_substrate(substrate as *const ContainerSubstrate<C>)
    }
}

/// The typed residence evidence a move-in site hands to an [`AuditedStored`] audit — the call-site
/// half of a [`Residence`], without the destination region (the audit takes that from the handle it
/// runs against). A family's `audit` builds a [`Residence`] from `(region, self)` and runs the
/// family's own residence walk over it. Its one field is the walk's saw-a-region-pointer recorder;
/// there is no reach-bearing form, because a value reaching a foreign region never reaches this
/// tier.
pub struct ResidenceEvidence<'ctx> {
    seen: &'ctx Cell<bool>,
}

impl<'ctx> ResidenceEvidence<'ctx> {
    /// Dest-only evidence with its saw-a-region-pointer recorder — the [`Residence::seen`] flag the
    /// checked-stored site reads after the store to decide whether the destination region enters the
    /// value's minted description as a member.
    pub(crate) fn dest_only_seen(seen: &'ctx Cell<bool>) -> Self {
        ResidenceEvidence { seen }
    }
}

// SAFETY: `audit` returns true only when every region borrow the stored `KObject` carries is
// resident in `region` — the residence the `KObject` walk verifies. A `Wrapped { type_id }` tag
// needs no walk: `KType` is a Copy digest handle carrying no region borrow, so it reaches nothing
// outside `region`.
unsafe impl AuditedStored<KoanStorageProfile> for KObject<'static> {
    type AuditContext<'ctx> = ResidenceEvidence<'ctx>;
    fn audit(region: &KoanRegion, value: &KObject<'_>, context: ResidenceEvidence<'_>) -> bool {
        value.resident_in_visiting(&Residence::dest_only_seen(region, context.seen))
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

// SAFETY: `audit` returns true only when `region` is the region owning the stored `Module`'s child
// scope — the `Module` borrows that child scope, so a store into any other region would lengthen
// the borrow past its region. Exact: the child-scope reference is the `Module`'s only region borrow.
// The `type_members` / `slot_type_tags` maps and the `self_sig` cell need no walk — a `KType` owns
// its content and borrows no region data, so nothing installed through them can reach outside
// `region`. A `Module` re-tagging a *foreign* child scope has no route here: it is built at a fold
// brand instead ([`Scope::store_transparent_view`]).
unsafe impl AuditedStored<KoanStorageProfile> for Module<'static> {
    type AuditContext<'ctx> = ();
    fn audit(region: &KoanRegion, value: &Module<'_>, _context: ()) -> bool {
        std::ptr::eq(region, value.child_scope().region())
    }
}
