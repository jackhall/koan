//! The per-call allocation frame: the [`FrameReach`] / [`FrameCoverage`] reach-evidence aliases and
//! the [`CallFrame`] shell over a refcounted [`FrameStorage`] that holds the per-call child
//! [`Scope`]. The region and brand substrate these build on lives in [`region`](super::region); the
//! child scope itself is born by [`Scope::open_frame`](crate::machine::core::Scope::open_frame),
//! which hands [`CallFrame::around`] the finished pair.
//!
//! A frame is a region shell and nothing else. The run's lookup state and output sink belong to the
//! run, not to a frame that happens to be first, and live in
//! [`execute::RunFrame`](crate::machine::execute) with the frame that adopts the run-root scope.

use std::rc::Rc;

use super::region::{FrameStorage, KoanRegion};
#[cfg(test)]
use super::region::{FrameStorageExt, RegionBrand};
use super::substrate::{Delivered, ReachDescription, SealedExtern, StepCoverage};
use crate::machine::core::{Scope, ScopeId, ScopeRefFamily};

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

/// One user-fn call's allocation frame: a thin shell over a refcounted [`FrameStorage`]. `Rc`-pinned
/// so the scheduler manages the frame by `Rc<CallFrame>`; an escaping closure extends only the
/// *storage* (via [`Self::storage_rc`]), not the shell, so a `FreshTail` tail hop can drop this
/// frame's shell outright without foreclosing on the escapee.
///
/// See [per-call-region/README.md](../../design/per-call-region/README.md) for the
/// carrier set, escaping-value retention, ancestor chain, and TCO
/// frame reuse; [memory-model.md § Region lifetime erasure](../../design/memory-model.md#region-lifetime-erasure)
/// for the heap-pinning / drop-order invariants.
pub struct CallFrame {
    /// The per-call child scope paired with the frame storage that owns its region, as one delivery
    /// [`Delivered`] envelope: the storage is the envelope's retained host, the scope its
    /// member-less resident carrier (hosted in that storage's own region), read back through
    /// [`Self::with_scope`] / [`Self::scope_sealed`] under that host pin. Co-ownership by one value
    /// makes storage-pins-the-scope a construction invariant of the envelope rather than a
    /// field-order convention, and dropping the sealed carrier never dereferences the child
    /// pointer, so the shell carries no drop-order rule to hand-maintain.
    envelope: Delivered<ScopeRefFamily>,
    /// This frame's own [`FrameStorage`] — the owner of the region its child scope lives in, and
    /// the pin every escapee extends ([`Self::storage_rc`]). Held beside the envelope rather than
    /// read off it: the envelope's members are one flat antichain in which a value's home is an
    /// ordinary member, so the frame's own storage is not recoverable from it by identity.
    storage: Rc<FrameStorage>,
}

impl CallFrame {
    /// Wrap a finished `(storage, envelope)` pair as a frame shell. The *only* constructor: the
    /// two halves arrive already coupled — the envelope's carrier is a scope resident in `storage`'s
    /// own region, pinned by `storage` itself — so there is no pairing left for a caller to get
    /// wrong and no scope-building this file has to know about.
    ///
    /// Both spellings of that coupling live with `Scope`, which owns what a scope is:
    /// [`Scope::open_frame`](crate::machine::core::Scope::open_frame) mints a fresh region and
    /// births a child in it, and [`Scope::adopt_as_run_frame`](crate::machine::core::Scope::adopt_as_run_frame)
    /// adopts an already-built run root into the storage that already owns its region.
    pub(crate) fn around(
        storage: Rc<FrameStorage>,
        envelope: Delivered<ScopeRefFamily>,
    ) -> Rc<CallFrame> {
        Rc::new(CallFrame { envelope, storage })
    }

    /// This frame's own `FrameStorage` — the owner of the region its child scope lives in, which
    /// every constructor pairs with that scope.
    pub(crate) fn storage(&self) -> &Rc<FrameStorage> {
        &self.storage
    }

    /// The child scope's externally-witnessed carrier by value (`SealedExtern<ScopeRefFamily>` is
    /// `Copy`) — the run-loop step's source for a `Yoked` slot, opened at the step brand alongside the
    /// continuation / contract / deps instead of re-anchored through the borrow-bounded `attach`.
    /// Re-sealed off the envelope's own carrier ([`Delivered::to_extern`]): the same erased `&Scope`,
    /// exposed witness-less so it [`zip`](SealedExtern::zip)s with the step's other
    /// externally-witnessed carriers under one brand (the envelope host is folded into that step
    /// witness separately). This frame owns the envelope it re-seals from and outlives the step that
    /// opens the zip, so the pin presented there covers the scope — the externally-witnessed tier's
    /// obligation, discharged by the holder rather than by the carrier.
    pub(crate) fn scope_sealed(&self) -> SealedExtern<ScopeRefFamily> {
        self.envelope.to_extern()
    }

    /// Run `f` with this frame's child scope opened at a `for<'b>` brand, folded onto `open` like the
    /// decide channel. Both the frame-side reads (scope id, the arg reach-set
    /// fold) and the seed-side binds (the user-fn param-bind, the deferred-return-type elaboration)
    /// take this read: a seed relocates its caller-`'a` value into
    /// the opened scope's own region through the substrate (a witnessed shortening) before binding it,
    /// so nothing fabricates a free `&'a`. The carrier opens against this frame's own storage `Rc`
    /// (the pin), and the rank-2 brand keeps the `&Scope<'b>` from escaping the call, so no scope
    /// borrow rides up a `&mut self` path. Carries **no `unsafe`** — [`Delivered::open`] routes the
    /// substrate's single audited reattach, pinned by the envelope's own retained host.
    pub fn with_scope<R>(&self, f: impl for<'b> FnOnce(&'b Scope<'b>) -> R) -> R {
        self.envelope.open(f)
    }

    /// This frame's child scope id, copied out through [`Self::with_scope`] — the scalar read for the
    /// sites that need only the id, with no `&Scope` escaping the open.
    pub fn scope_id(&self) -> ScopeId {
        self.with_scope(|s| s.id)
    }

    pub fn region(&self) -> &KoanRegion {
        self.storage().region()
    }

    /// Whether holding this frame keeps `owner`'s region alive — the gate a scheduler submission
    /// reads before storing a scope reference erased and frame-bounded
    /// (`NodeScope::YokedChild`), asked of the storage that owns the scope's region
    /// ([`Scope::frame`](crate::machine::core::Scope::frame)).
    ///
    /// Answered from the **pin that actually holds**, not from the lexical scope graph: this
    /// frame's storage and the `outer` chain it keeps alive are the regions it owns a claim on, so
    /// the question is [`RegionHost::pins_region`](RegionHost::pins_region) over
    /// that chain. Storage at the **eternal tier** (the run root) needs no claim at all — its
    /// region outlives every per-call frame, which is exactly why
    /// [`Scope::parent_frame_pin`](crate::machine::core::Scope::parent_frame_pin) declines to chain
    /// it — so it answers `true` without consulting the chain.
    pub(crate) fn pins_storage_region(&self, owner: &FrameStorage) -> bool {
        owner.is_eternal() || self.storage.pins_region(owner.region())
    }

    /// This frame's region [`RegionBrand`] allocation capability, minted from its owning storage.
    /// Test-only: production allocates through the scope (`scope.brand()`); the frame-level handle is
    /// a convenience for the arena / lift Miri tests that alloc against a bare frame.
    #[cfg(test)]
    pub(crate) fn brand(&self) -> RegionBrand<'_> {
        self.storage().brand()
    }

    /// Test fixture: seal a value born through this frame's **own** brand under the description its
    /// birth mint stamps — the frame-brand twin of
    /// [`Scope::seal_reaching`](crate::machine::core::Scope), for the suite that allocates at the
    /// frame lifetime rather than inside a transient [`Self::with_scope`] sub-brand. Value and
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
