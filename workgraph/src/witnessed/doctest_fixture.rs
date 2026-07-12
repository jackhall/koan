//! Shared fixture for the witnessed module's `compile_fail` soundness guards: the
//! carrier families and the local region-owning witness every guard exercises, so a
//! signature change to `Witness` / `WitnessRegion` / `Reattachable` lands here once.
//! Hidden from docs and `pub` only because doctests compile as external crates and
//! must import it; it is not part of the module's real surface.

use std::cell::Cell;

use super::{
    AuditedStored, FamilyArena, PinsRegion, Reattachable, Region, RegionOwner, RegionSet,
    SealedExtern, StorageOf, StorageProfile, Stored, Witness, WitnessRegion, Witnessed,
};

/// A shared-reference carrier family: `&'r u32`.
pub struct RefFamily;
// SAFETY: `&'r u32` is one type generic only in `'r`.
unsafe impl Reattachable for RefFamily {
    type At<'r> = &'r u32;
}

/// An invariant carrier family: `Cell<&'r u32>`.
pub struct InvFamily;
// SAFETY: `Cell<&'r u32>` is one type generic only in `'r`.
unsafe impl Reattachable for InvFamily {
    type At<'r> = Cell<&'r u32>;
}

/// A local witness owning its region — the `Vec`'s heap buffer stays at a fixed
/// address across the witness's move, so a value built from `region()` stays pinned.
pub struct Cart(pub Vec<u32>);
// SAFETY: the owned `Vec`'s buffer is fixed-address for the `Cart`'s whole life.
unsafe impl Witness for Cart {}
// SAFETY: `region` borrows the buffer the `Witness` impl above pins.
unsafe impl WitnessRegion for Cart {
    type Region = [u32];
    fn region(&self) -> &[u32] {
        &self.0
    }
}
// SAFETY: `region` borrows the buffer the `Witness` impl pins; `Cart` has no ancestry, so
// identity (pointer equality) is the whole pins relation.
unsafe impl RegionOwner for Cart {
    type Region = [u32];
    fn region(&self) -> &[u32] {
        &self.0
    }
}
// SAFETY: a `Cart` has no ancestry — it pins exactly its own buffer, so identity (pointer
// equality) is the whole pins relation.
unsafe impl PinsRegion for Cart {
    fn pins_region(&self, region: &[u32]) -> bool {
        std::ptr::eq(&self.0[..], region)
    }
}

/// Build a set-witnessed carrier over a cart: yoked from the cart's own region (so the value is
/// provably region-derived), then re-bundled under the singleton [`RegionSet`] that pins the same
/// cart. Fixture-only: the doctests for the set-witnessed merge/transfer verbs need one, and the
/// crate-internal witness-retype they route is not part of the module's real surface.
pub fn set_witnessed(cart: std::rc::Rc<Cart>) -> Witnessed<RefFamily, RegionSet<Cart>> {
    Witnessed::<RefFamily, std::rc::Rc<Cart>>::yoke(std::rc::Rc::clone(&cart), |region| &region[0])
        .rewitness(RegionSet::singleton(cart))
}

/// Build a [`SealedExtern`] from a live carrier. `SealedExtern`'s constructors are all
/// crate-private (no production caller builds one from an arbitrary borrow), but a doctest
/// compiles as an external crate, so the `SealedExtern::open` guard and its compiling twin need
/// this in-crate wrapper to construct one at all.
pub fn seal_extern<T: Reattachable>(live: T::At<'_>) -> SealedExtern<T> {
    SealedExtern::erase(live)
}

/// A recorded-reference carrier family: `&'r u32` whose [`Stored::record_local`] records the
/// *pointee's* address into the region's membership side-table, so [`Region::owns_addr`] can later
/// answer whether a borrow points into a value resident in this region. The simplest honest shape
/// for an [`AuditedStored`] audit — [`RegionHandle::alloc_resident_checked`]'s doctests exercise a
/// passing store (a borrow of a resident value) and a rejecting one (a borrow the region does not
/// own).
pub struct RecordedRefFamily;
// SAFETY: `&'r u32` is one type generic only in `'r`.
unsafe impl Reattachable for RecordedRefFamily {
    type At<'r> = &'r u32;
}

/// Profile for the region/handle doctests: the reference family, the witness-set family the fold
/// verbs mint into, and the recorded-reference family the checked-store doctests audit against.
pub struct FixtureProfile;
impl StorageProfile for FixtureProfile {
    type Families = (RefFamily, (RegionSet<RegionCart>, (RecordedRefFamily, ())));
}
impl Stored<FixtureProfile> for RefFamily {
    fn cell(storage: &StorageOf<FixtureProfile>) -> &FamilyArena<Self> {
        &storage.0
    }
}
impl Stored<FixtureProfile> for RegionSet<RegionCart> {
    fn cell(storage: &StorageOf<FixtureProfile>) -> &FamilyArena<Self> {
        &storage.1 .0
    }
}
impl Stored<FixtureProfile> for RecordedRefFamily {
    fn cell(storage: &StorageOf<FixtureProfile>) -> &FamilyArena<Self> {
        &storage.1 .1 .0
    }
    fn record_local(frame: &Region<FixtureProfile>, stored: &&'static u32) {
        frame.record_addr(*stored as *const u32 as usize);
    }
}

// SAFETY: `audit` returns true only when `region` owns the address of the `u32` the incoming
// reference borrows — i.e. that `u32` was previously stored into (and recorded by) this region, so
// the borrow is genuinely resident. A permissive audit is not writable here without lying about
// that residence relation.
unsafe impl AuditedStored<FixtureProfile> for RecordedRefFamily {
    type AuditContext<'ctx> = ();
    fn audit(region: &Region<FixtureProfile>, value: &&u32, _context: ()) -> bool {
        region.owns_addr(*value as *const u32 as usize)
    }
}

/// A fresh [`Region`] for the fixture profile. `Region::new` is `pub(crate)` to `workgraph`, so a
/// doctest — which compiles as an external crate — has no direct route to one; this wraps the
/// crate-internal constructor for that one purpose.
pub fn fresh_region() -> Region<FixtureProfile> {
    Region::new()
}

/// A region owner for the fixture profile.
pub struct RegionCart(pub Region<FixtureProfile>);
// SAFETY: the owned `Region`'s arena pages stay fixed-address while the `RegionCart` is held
// (behind an `Rc` at every use site).
unsafe impl RegionOwner for RegionCart {
    type Region = Region<FixtureProfile>;
    fn region(&self) -> &Region<FixtureProfile> {
        &self.0
    }
}
// SAFETY: a `RegionCart` has no ancestry — it pins exactly its own region, so identity (pointer
// equality) is the whole pins relation.
unsafe impl PinsRegion for RegionCart {
    fn pins_region(&self, region: &Region<FixtureProfile>) -> bool {
        std::ptr::eq(&self.0, region)
    }
}
