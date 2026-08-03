//! Shared fixture for the witnessed module's `compile_fail` soundness guards: the
//! carrier families and the local region-owning witness every guard exercises, so a
//! signature change to `Witness` / `WitnessRegion` / `Reattachable` lands here once.
//! Hidden from docs and `pub` only because doctests compile as external crates and
//! must import it; it is not part of the module's real surface.

use std::cell::Cell;
use std::rc::Rc;

use super::{
    FamilyArena, PinBundle, PinsRegion, Reattachable, Region, RegionOwner, SealedExtern, StorageOf,
    StorageProfile, Stored, Witness, WitnessRegion, Witnessed,
};

/// A shared-reference carrier family: `&'r u32`.
pub struct RefFamily;
// SAFETY: `&'r u32` is one type generic only in `'r`.
unsafe impl Reattachable for RefFamily {
    type At<'r> = &'r u32;
}

/// A text carrier family: `&'r str` — the shape a bump-stored string takes. It has **no**
/// [`Stored`] impl on purpose: the bump door
/// ([`FoldedPlacement::fold_and_bump`](super::FoldedPlacement::fold_and_bump)) needs none, which is
/// what its doctests demonstrate.
pub struct StrFamily;
// SAFETY: `&'r str` is one type generic only in `'r` (a fat pointer, layout-invariant).
unsafe impl Reattachable for StrFamily {
    type At<'r> = &'r str;
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

/// Build a bundle-witnessed carrier over a cart: yoked from the cart's own region (so the value is
/// provably region-derived), then re-bundled under the singleton [`PinBundle`] that pins the same
/// cart. Fixture-only: the doctests for the set-witnessed merge/transfer verbs need one, and the
/// crate-internal witness-retype they route is not part of the module's real surface.
pub fn set_witnessed(cart: std::rc::Rc<Cart>) -> Witnessed<RefFamily, PinBundle<Cart>> {
    Witnessed::<RefFamily, std::rc::Rc<Cart>>::yoke(std::rc::Rc::clone(&cart), |region| &region[0])
        .rewitness(PinBundle::singleton(cart))
}

/// Build a [`SealedExtern`] from a live carrier. `SealedExtern`'s constructors are all
/// crate-private (no production caller builds one from an arbitrary borrow), but a doctest
/// compiles as an external crate, so the `SealedExtern::open` guard and its compiling twin need
/// this in-crate wrapper to construct one at all.
pub fn seal_extern<T: Reattachable>(live: T::At<'_>) -> SealedExtern<T> {
    SealedExtern::erase(live)
}

/// A value that names the region it is resident in, so its residence is legible in the value rather
/// than in a table kept beside it. The born doors' doctests build one at the brand and read its
/// `home` back, which is what makes "the value's region pointer is the destination's" observable.
#[derive(Clone, Copy)]
pub struct HomedRef<'r> {
    /// The region this value lives in — read back to show it is the destination the door built at.
    pub home: &'r Region<FixtureProfile>,
    /// The borrowed payload.
    pub value: &'r u32,
}

/// A homed-reference carrier family: [`HomedRef`], the simplest shape whose residence is readable
/// off the stored value — [`RegionHandle::alloc_resident_born`](super::RegionHandle)'s doctests build one at the brand
/// and its `compile_fail` twin tries to build one over an ambient region instead.
pub struct HomedRefFamily;
// SAFETY: `HomedRef<'r>` is one type generic only in `'r`, a pair of thin pointers whose layout is
// identical for every choice of `'r`.
unsafe impl Reattachable for HomedRefFamily {
    type At<'r> = HomedRef<'r>;
}

/// Profile for the region/handle doctests: the reference family, the witness-set family the fold
/// verbs mint into, and the homed-reference family the checked-store doctests audit against.
pub struct FixtureProfile;
impl StorageProfile for FixtureProfile {
    type Families = (RefFamily, (HomedRefFamily, ()));
    type FrameOwner = RegionCart;
}
impl Stored<FixtureProfile> for RefFamily {
    fn cell(storage: &StorageOf<FixtureProfile>) -> &FamilyArena<Self> {
        &storage.0
    }
}
impl Stored<FixtureProfile> for HomedRefFamily {
    fn cell(storage: &StorageOf<FixtureProfile>) -> &FamilyArena<Self> {
        &storage.1 .0
    }
}

/// A fresh region owner for the fixture profile, built through `Rc::new_cyclic` so its region is
/// handed the owner's own `Weak` at construction — the back-link every reach description minted into
/// that region stamps as its host. `Region::new` is `pub(crate)` to `workgraph`, so a doctest —
/// which compiles as an external crate — has no direct route to one; this wraps the crate-internal
/// constructor for that one purpose.
pub fn fresh_cart() -> Rc<RegionCart> {
    Rc::new_cyclic(|me| RegionCart(Region::new(me.clone())))
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
