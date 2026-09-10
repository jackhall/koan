//! The per-call allocation frame: the [`FrameReach`] / [`FrameCoverage`] reach-evidence aliases and
//! the [`Frame`] shell over a refcounted [`FrameStorage`] holding the per-call **resident** it was
//! opened around. The region and brand substrate these build on lives in [`region`](super::region).
//!
//! The shell is generic over the reattachable family it carries, and names no Koan value type: a
//! resident is born through [`Frame::open_under`], which takes the construction closure from the
//! caller that knows what a resident *is*. Koan's instantiation is
//! [`CallFrame`](crate::machine::core::CallFrame), whose resident is a child
//! [`Scope`](crate::machine::core::Scope).
//!
//! A frame is a region shell and nothing else. The run's lookup state and output sink belong to the
//! run, not to a frame that happens to be first, and live in
//! [`execute::RunFrame`](crate::machine::execute) with the frame that adopts the run-root resident.

use std::rc::Rc;

#[cfg(test)]
use super::region::FrameStorageExt;
use super::region::{FrameStorage, KoanRegion, RegionBrand};
use super::substrate::{
    Delivered, DropFree, ReachDescription, Reattachable, ReferenceFamily, RegionHandle, RegionHost,
    SealedExtern, StepCoverage,
};

/// The non-owning reach description backing carrier witnesses: names the regions a carrier's value
/// reaches, hosted in the value's home region's side table and referenced (never owned) by the
/// carrier. See [`ReachDescription`] for the shared mechanism (membership queries, the self rule);
/// Koan's member semantics are the library's [`PinsRegion`](super::substrate::PinsRegion) impl for
/// [`RegionHost`](super::substrate::RegionHost). Its owning counterpart is [`FrameCoverage`].
pub type FrameReach = ReachDescription<FrameStorage>;

/// The owned coverage a holder keeps to pin every region a value reaches — the ownership
/// counterpart of [`FrameReach`], released by ordinary `Drop` (a step carries one from the fold that
/// composed it to the seal that consumes it, the delivery envelope carries one across transit). See
/// [`StepCoverage`] for the surface: Koan holds, clones, threads and drops coverage, and computes
/// with it only through the container verbs on [`Delivered`] and
/// [`RegionHandle`](super::substrate::RegionHandle).
pub type FrameCoverage = StepCoverage<FrameStorage>;

/// One user-fn call's allocation frame: a thin shell over a refcounted [`FrameStorage`], carrying
/// one resident of the family `F`. `Rc`-pinned so the scheduler manages the frame by `Rc<Frame<F>>`;
/// an escaping closure extends only the *storage* (via [`Self::storage_rc`]), not the shell, so a
/// `FreshTail` tail hop can drop this frame's shell outright without foreclosing on the escapee.
///
/// See [per-call-region/README.md](../../design/per-call-region/README.md) for the
/// carrier set, escaping-value retention, ancestor chain, and TCO
/// frame reuse; [memory-model.md § Region lifetime erasure](../../design/memory-model.md#region-lifetime-erasure)
/// for the heap-pinning / drop-order invariants.
pub struct Frame<F: Reattachable + DropFree> {
    /// The per-call resident paired with the frame storage that owns its region, as one delivery
    /// [`Delivered`] envelope: the storage is the envelope's retained host, the resident its
    /// member-less carrier (hosted in that storage's own region), read back through
    /// [`Self::with_resident`] / [`Self::resident_sealed`] under that host pin. Co-ownership by one
    /// value makes storage-pins-the-resident a construction invariant of the envelope rather than a
    /// field-order convention, and dropping the sealed carrier never dereferences the resident
    /// pointer, so the shell carries no drop-order rule to hand-maintain.
    envelope: Delivered<F>,
    /// This frame's own [`FrameStorage`] — the owner of the region its resident lives in, and
    /// the pin every escapee extends ([`Self::storage_rc`]). Held beside the envelope rather than
    /// read off it: the envelope's members are one flat antichain in which a value's home is an
    /// ordinary member, so the frame's own storage is not recoverable from it by identity.
    storage: Rc<FrameStorage>,
}

impl<F: Reattachable + DropFree> Frame<F> {
    /// Wrap a finished `(storage, envelope)` pair as a frame shell. The *only* constructor, and
    /// private: the two halves arrive already coupled — the envelope's carrier is resident in
    /// `storage`'s own region, pinned by `storage` itself — so both spellings of that coupling are
    /// the doors below ([`Self::open_under`], [`Self::adopting`]) and no caller can pair a storage
    /// with a resident that does not live in it.
    fn around(storage: Rc<FrameStorage>, envelope: Delivered<F>) -> Rc<Self> {
        Rc::new(Frame { envelope, storage })
    }

    /// This frame's own `FrameStorage` — the owner of the region its resident lives in, which
    /// every constructor pairs with that resident.
    pub(crate) fn storage(&self) -> &Rc<FrameStorage> {
        &self.storage
    }

    /// The resident's externally-witnessed carrier by value (`SealedExtern<F>` is `Copy`) — the
    /// run-loop step's source for a `Yoked` slot, opened at the step brand alongside the
    /// continuation / contract / deps instead of re-anchored through the borrow-bounded `attach`.
    /// Re-sealed off the envelope's own carrier ([`Delivered::to_extern`]): the same erased
    /// reference, exposed witness-less so it [`zip`](SealedExtern::zip)s with the step's other
    /// externally-witnessed carriers under one brand (the envelope host is folded into that step
    /// witness separately). This frame owns the envelope it re-seals from and outlives the step that
    /// opens the zip, so the pin presented there covers the resident — the externally-witnessed
    /// tier's obligation, discharged by the holder rather than by the carrier.
    pub(crate) fn resident_sealed(&self) -> SealedExtern<F>
    where
        F::At<'static>: Copy,
    {
        self.envelope.to_extern()
    }

    /// Run `f` with this frame's resident opened at a `for<'b>` brand, folded onto `open` like the
    /// decide channel. Both the frame-side reads (the resident's id, the arg reach-set
    /// fold) and the seed-side binds (the user-fn param-bind, the deferred-return-type elaboration)
    /// take this read: a seed relocates its caller-`'a` value into
    /// the opened region through the substrate (a witnessed shortening) before binding it,
    /// so nothing fabricates a free `&'a`. The carrier opens against this frame's own storage `Rc`
    /// (the pin), and the rank-2 brand keeps the view from escaping the call, so no resident borrow
    /// rides up a `&mut self` path. Carries **no `unsafe`** — [`Delivered::open`] routes the
    /// substrate's single audited reattach, pinned by the envelope's own retained host.
    pub fn with_resident<R>(&self, f: impl for<'b> FnOnce(F::At<'b>) -> R) -> R
    where
        F::At<'static>: Copy,
    {
        self.envelope.open(f)
    }

    pub fn region(&self) -> &KoanRegion {
        self.storage().region()
    }

    /// Whether holding this frame keeps `brand`'s region alive — the gate a scheduler submission
    /// reads before storing a resident reference erased and frame-bounded
    /// (`NodeScope::YokedChild`), asked of the brand whose region the resident lives in.
    ///
    /// Answered from the **pin that actually holds**, not from any lexical graph above it: this
    /// frame's storage and the `outer` chain it keeps alive are the regions it owns a claim on, so
    /// the question is [`RegionHost::pins_region`](RegionHost::pins_region) over
    /// that chain. Storage at the **eternal tier** (the run root) needs no claim at all — its
    /// region outlives every per-call frame, which is exactly why
    /// [`RegionBrand::parent_frame_pin`] declines to chain it — so it answers `true` without
    /// consulting the chain.
    pub(crate) fn hosts(&self, brand: RegionBrand<'_>) -> bool {
        brand.is_eternal() || self.storage.pins_region(brand.region())
    }

    /// This frame's region [`RegionBrand`] allocation capability, minted from its owning storage.
    /// Test-only: production allocates through the resident (`scope.brand()`); the frame-level
    /// handle is a convenience for the arena / lift Miri tests that alloc against a bare frame.
    #[cfg(test)]
    pub(crate) fn brand(&self) -> RegionBrand<'_> {
        self.storage().brand()
    }

    /// Test fixture: seal a value born through this frame's **own** brand under the description its
    /// birth mint stamps — the frame-brand twin of
    /// [`Scope::seal_reaching`](crate::machine::core::Scope), for the suite that allocates at the
    /// frame lifetime rather than inside a transient [`Self::with_resident`] sub-brand. Value and
    /// description come off the same brand, which is the pairing
    /// [`RegionHandle::seal_reaching`](RegionHandle::seal_reaching) takes: a
    /// sub-brand's `'b` is universally quantified and outlives nothing, so a frame-lifetime value
    /// cannot be sealed through it at all.
    #[cfg(test)]
    pub(crate) fn seal_born_here<
        's,
        'v: 's,
        T: super::substrate::Reattachable + super::substrate::DropFree,
    >(
        &'s self,
        value: T::At<'v>,
        borrows_home: bool,
    ) -> super::substrate::Witnessed<T> {
        let brand = self.brand();
        let home = FrameCoverage::of(self.storage_rc());
        let sources: &[&FrameCoverage] = match borrows_home {
            true => &[&home],
            false => &[],
        };
        brand.seal_reaching(value, brand.handle().mint_retained(sources))
    }

    /// Clone this frame's `FrameStorage` Rc — the handle an escaping value (a returned closure, a
    /// module frame) pins to keep its captured environment alive independently of the shell: a
    /// `FreshTail` tail hop drops this frame's shell outright, and the escaped storage clone keeps
    /// the region it names alive regardless.
    pub fn storage_rc(&self) -> Rc<FrameStorage> {
        Rc::clone(self.storage())
    }
}

/// The two frame doors, for a frame whose resident is a **reference** to a payload family `K` —
/// the co-located shape a region allocation hands back ([`ReferenceFamily`]). Each takes the brand
/// of the region the caller is opening *under* and the reference the new resident hangs off, tied
/// by one `'a`: a caller passes a value and that value's own brand, so there is no pairing to
/// mis-wire and no residence for a caller to assert.
impl<K: Reattachable + DropFree> Frame<ReferenceFamily<K>> {
    /// **Open a fresh per-call frame under `parent`**: mint a region chained on
    /// [`parent.parent_frame_pin()`](RegionBrand::parent_frame_pin), birth `child` in it at the
    /// generative brand with `outer` re-anchored to that brand, deliver it resident, and hand back
    /// the shell holding the pair. The one entry for every per-call frame, the TCO fresh-tail cart
    /// included.
    ///
    /// The storage pin chained for the parent is **derived** from `parent`, so no caller can
    /// under-pin — there is no pin parameter to mis-wire. A fresh-tail hop's `outer` is the callee
    /// closure's captured (definition) scope, so chaining that scope's region owner is exactly what
    /// keeps a closure's captured frame alive across the hop that retires the caller. This never
    /// over-retains in the common tail loop — a top-level-defined recursive fn captures the run-root
    /// scope, whose [`parent_frame_pin`](RegionBrand::parent_frame_pin) is `None`, and a
    /// locally-defined tail-recursive helper captures one stable per-call def frame, pinned once
    /// (the same `Rc` every iteration). Only a loop that genuinely builds a fresh closure over each
    /// iteration's frame retains `O(N)` frames — an unavoidable data dependency, since evaluating
    /// the final closure reaches every one. The chain is a DAG (each frame's `outer` names a
    /// strictly older frame), so it forms no cycle; see `design/tail-call-optimization.md`.
    ///
    /// The resident is *born* at the destination: [`RegionHandle::bump_born_with`] hands `child` a
    /// placement over the fresh region at a `for<'b>` brand, with `outer` re-anchored to that same
    /// `'b`. The real invariant value is built coupling the two and stored in the same act, so
    /// residence is discharged by the brand rather than by a runtime check: an ambient `&Region`
    /// cannot coerce to `'b`, so the child's region is the destination's by construction. A family
    /// invariant in its lifetime is honoured for free — branding the outer reference and the region
    /// at *independent* `'b`s is what invariance rejects, and this door unifies them at a single
    /// one. No transient `&'a` is minted and nothing re-anchors outside the witnessed substrate.
    ///
    /// The child's reference to `outer` is a genuine cross-region borrow into a possibly foreign
    /// region: the child cannot rebuild at `'static`, and its liveness is not the reach-witness
    /// system's business to name. It is guaranteed instead by `FrameStorage`'s own `outer` `Rc`
    /// chain, the pin minted above. So the child seals under a description hosted in its own new
    /// region with **no members**, and the envelope covers that storage and nothing else.
    ///
    /// The storage is heap-pinned behind its own `Rc` from the mint on (its region minted lazily,
    /// on the resident's allocation), so the erased resident pointer stays valid as the `Rc` moves
    /// into the shell: the carrier holds a `&'static` reference, not a borrow of the local, and the
    /// `KoanRegion` stays at a fixed heap address behind the `Rc`.
    pub(crate) fn open_under<'a>(
        parent: RegionBrand<'a>,
        outer: &'a K::At<'a>,
        child: impl for<'b> FnOnce(&'b K::At<'b>, RegionBrand<'b>) -> K::At<'b>,
    ) -> Rc<Self> {
        let storage = RegionHost::fresh(parent.parent_frame_pin());
        let handle = RegionHandle::from_owner(&*storage);
        let live = handle.bump_born_with::<K, ReferenceFamily<K>, _>(
            SealedExtern::<ReferenceFamily<K>>::erase(outer),
            &storage,
            |placement, outer_b| child(outer_b, RegionBrand(placement.handle())),
        );
        let envelope = handle.deliver_resident::<ReferenceFamily<K>>(live);
        Self::around(storage, envelope)
    }

    /// **Adopt an already-built value as a frame's resident** rather than minting a child. The
    /// storage is **derived** from `brand`, not taken: the value's own region owner is by definition
    /// the storage that owns that region, so the frame's [`region`](Self::region) equals the value's
    /// and there is no second storage argument to disagree with it. The value reaches nothing beyond
    /// its own region, so the envelope covers that one region, and the borrow is erased into it
    /// exactly as a born resident's is.
    pub(crate) fn adopting<'a>(brand: RegionBrand<'a>, value: &'a K::At<'a>) -> Rc<Self> {
        Self::around(
            brand.frame(),
            brand.deliver_resident::<ReferenceFamily<K>>(value),
        )
    }
}
