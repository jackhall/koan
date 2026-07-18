//! The per-call allocation frame: [`FrameStorage`] (the Koan [`RegionHost`] alias), the run-root
//! storage entry, the [`FrameSet`] reach-set alias, the witnessed child-scope construction door, and
//! the [`CallFrame`] shell over a refcounted `FrameStorage` that holds the per-call child [`Scope`].
//! The region/brand substrate these build on lives in the parent `arena` module.

use std::cell::Cell;
use std::rc::Rc;

use super::{KoanRegion, KoanStorageProfile, RegionBrand};
use crate::machine::core::kfunction::NodeId;
use crate::machine::core::{Scope, ScopeId, ScopeRefFamily};
use crate::machine::model::types::TypeRegistry;
use crate::machine::CarrierWitness;
use crate::witnessed::{
    Delivered, RegionHandle, RegionHandleFamily, RegionHost, RegionSet, Sealed, SealedExtern,
    Witnessed,
};

/// Koan's per-call region owner: the library's [`RegionHost`], instantiated for the Koan family
/// set. `RegionHost` lazily mints its region on first allocation — reached by the child `Scope`
/// [`CallFrame::new`] builds immediately, so a constructed frame's region is minted by the time
/// anything reads it — and the `outer` link chains the lexical-ancestor frames' storage alive. An
/// escaping value (a returned closure, a module frame) pins *this* — not the [`CallFrame`] shell —
/// so a tail hop's shell can drop outright while the escapee's captured
/// environment rides the old `FrameStorage` it still holds.
/// The library's raw-region constructor is sealed to `workgraph`, so nothing outside the library
/// can mint a `KoanRegion` directly; the Koan-typed [`RegionBrand`] mint over a `FrameStorage` lives
/// on [`FrameStorageExt`] (an extension trait, since a type alias takes no inherent impls of its own).
pub type FrameStorage = RegionHost<KoanStorageProfile>;

/// The run-root storage: a fresh run region with no `outer` link. Held by `run_program` (and the
/// test harness) so the run-root scope's region has an owning Rc; [`CallFrame::adopting`] reuses
/// it as the run frame's storage and the run-root scope records a `Weak` to it as its
/// `region_owner`. Public: it is the one Koan-side entry point a caller (production or an
/// integration test) uses to obtain run-root storage — it mints nothing itself, only building the
/// library's `RegionHost` shell whose region lazily mints on first allocation.
pub fn run_root_storage() -> Rc<FrameStorage> {
    RegionHost::fresh(None)
}

/// Koan's [`RegionBrand`] mint over a [`FrameStorage`] — an extension trait because `FrameStorage`
/// is a `workgraph` type alias, so Koan cannot add an inherent method to it directly.
pub(crate) trait FrameStorageExt {
    /// Mint this storage's region's [`RegionBrand`] — the **sole** allocation capability for this
    /// storage's region. Minting is the library's [`RegionHandle::from_owner`] rule (it requires the
    /// storage that *owns* the region, via its `RegionOwner` impl); this method pairs it with the
    /// Koan veneer. Allocation is reachable only by riding this brand (it is stored on the [`Scope`]
    /// built at region-open, and threaded from there).
    fn brand(&self) -> RegionBrand<'_>;
}

impl FrameStorageExt for FrameStorage {
    fn brand(&self) -> RegionBrand<'_> {
        RegionBrand(RegionHandle::from_owner(self))
    }
}

/// The reach set backing carrier witnesses: the set of `Rc<FrameStorage>` whose regions a
/// carrier's value reaches. See [`RegionSet`] for the shared mechanism (subsumption, folding,
/// union); Koan's member semantics are the library's [`PinsRegion`](crate::witnessed::PinsRegion)
/// impl for [`RegionHost`].
pub type FrameSet = RegionSet<FrameStorage>;

/// Build a per-call frame's child scope **witnessed**, sealing it to the externally-witnessed
/// [`SealedExtern<ScopeRefFamily>`] the [`CallFrame`] holds — the construction door that re-anchors the
/// longer-lived lexical parent into the fresh region, with no retype outside the witnessed substrate.
///
/// The fresh region's [`RegionHandle`] and the foreign parent (as [`ScopeRefFamily`]) are erased and
/// [`zip`](SealedExtern::zip)ped, then opened at **one** `for<'b>` brand against `storage` — the fresh
/// frame's `Rc`, which pins both the region it owns and, via its `outer` chain, the parent. Inside
/// the brand the real invariant `Scope<'b>` is built coupling the parent at `'b` (its `root`
/// falling out as `outer.root`), allocated through the brand's [`RegionBrand`], and erased witness-less.
/// `Scope`'s invariance is honoured by construction — the only retypes are the substrate's audited brand
/// ([`SealedExtern::open`]) and store ([`RegionHandle::alloc`]) — so the per-call child stops being a
/// re-anchor audited outside Witnessed/Sealed. Branding the two refs at *independent* `'b`s is what
/// invariance rejects; one [`zip`](SealedExtern::zip)ped `open` unifies them at a single `'b`.
pub(crate) fn build_frame_child_witnessed<'p>(
    outer: &'p Scope<'p>,
    storage: &Rc<FrameStorage>,
) -> Sealed<ScopeRefFamily, CarrierWitness> {
    let handle = SealedExtern::<RegionHandleFamily<KoanStorageProfile>>::erase(
        RegionHandle::from_owner(&**storage),
    );
    let parent = SealedExtern::<ScopeRefFamily>::erase(outer);
    let region_owner = Rc::downgrade(storage);
    handle.zip(parent).open(storage, |(handle_b, outer_b)| {
        // `handle_b: RegionHandle<'b, KoanStorageProfile>`, `outer_b: &'b Scope<'b>` — the region
        // handle and parent unified at the one brand. The child stores both by plain coercion (no
        // retype of its own). The child scope lives in `storage`'s own region, so it seals under the
        // empty (`resident`) carrier witness — its liveness is the frame storage, paired with it as the
        // envelope host by the `CallFrame` constructor.
        //
        // `child.outer` is a genuine cross-region borrow into the lexical parent's (possibly foreign)
        // region — unlike every other resident move-in in this file, `child` cannot rebuild at
        // `'static` and its liveness is not the reach-witness system's business to name: it is
        // guaranteed instead by `FrameStorage`'s own `outer` `Rc` chain (see this fn's doc), a
        // structural invariant this construction door alone upholds by always chaining `storage`'s
        // `outer` to the same frame that owns `outer_b`'s region. That chain is **derived**, not
        // asserted: `CallFrame::new` computes it from the parent scope's own `region_owner`
        // (`Scope::parent_frame_pin`), root-region parents chain nothing, and the one deliberate
        // no-chain frame is the reserved `CallFrame::new_tail`.
        //
        // The store runs the real `Scope` family audit — the same live O(1)
        // `ptr::eq(region, value.region())` as `alloc_scope`. `child` is built over
        // `RegionBrand(handle_b)`, so `child.region()` is `handle_b`'s own region and the check
        // holds by construction; the parent-liveness chain above stays typed by `CallFrame::new`.
        let child = Scope::child_for_frame_witnessed(outer_b, RegionBrand(handle_b), region_owner);
        let live = handle_b
            .alloc_resident_checked::<Scope<'static>>(child, ())
            .expect("frame child is built over this frame's own region");
        Sealed::seal(Witnessed::<ScopeRefFamily, CarrierWitness>::resident(live))
    })
}

/// One user-fn call's allocation frame: a thin shell over a refcounted [`FrameStorage`]. `Rc`-pinned
/// so the scheduler manages the frame by `Rc<CallFrame>`; an escaping closure extends only the
/// *storage* (via [`Self::storage_rc`]), not the shell, so a `FreshTail` tail hop can drop this
/// frame's shell outright without foreclosing on the escapee.
///
/// See [per-call-region/README.md](../../../../design/per-call-region/README.md) for the
/// carrier set, escaping-value retention, ancestor chain, and TCO
/// frame reuse; [memory-model.md § Region lifetime erasure](../../../../design/memory-model.md#region-lifetime-erasure)
/// for the heap-pinning / drop-order invariants.
pub struct CallFrame {
    /// The per-call child scope paired with the frame storage that owns its region, as one delivery
    /// [`Delivered`] envelope: the storage is the envelope's retained host, the scope its
    /// empty-witness ([`resident`](Witnessed::resident)) carrier, read back through
    /// [`Self::with_scope`] / [`Self::scope_sealed`] under that host pin. Co-ownership by one value
    /// replaces the former hand-maintained `(storage, scope_carrier)` field pair: the
    /// storage-pins-the-scope co-location the pair kept by field-order convention is now a
    /// construction invariant of the envelope, and dropping the sealed carrier never dereferences the
    /// child pointer, so no drop-order rule is left to hand-maintain.
    envelope: Delivered<ScopeRefFamily, CarrierWitness, FrameStorage>,
    /// True only for the scheduler-owned run frame, which carries the top-level run scope and
    /// never drops mid-run. Its `region` is empty (top-level values live in the externally-owned
    /// run region, reached via `scope.region`), so there is nothing to lift out of it: the Done
    /// boundary skips the lift for a non-dying frame (lift exists to rescue values from a *dying*
    /// per-call region). Every per-call frame is `false`.
    non_dying: bool,
    /// The slot this frame was installed for — the body that finalizes it. Set at install; checked at
    /// that slot's `Done` / tail-`Continue` to close the frame's scope exactly when its body completes.
    /// A `Yoked` sub-expression slot sharing the frame is not the owner, so its `Done` does not close.
    owner: Cell<Option<NodeId>>,
    /// The run's subtype-verdict store, `Some` only on the run frame ([`Self::adopting`]). Per-call
    /// frames reach it through the execution context rather than owning one, so a verdict recorded
    /// anywhere in the run is visible everywhere in it; the map drops when the run frame does.
    type_registry: Option<Rc<TypeRegistry>>,
}

impl CallFrame {
    /// Build a fresh per-call frame whose child `Scope` uses `outer` as its `outer` link. The
    /// storage pin chained for the parent is **derived** from `outer` via
    /// [`Scope::parent_frame_pin`]: the parent scope's own region owner when it is per-call, or no
    /// chain when the parent lives in the run-root region (which outlives the run). No caller can
    /// under-pin — there is no pin parameter to mis-wire; the one deliberate no-chain frame is the
    /// TCO fresh-tail cart, minted by the reserved [`Self::new_tail`].
    pub fn new<'p>(outer: &'p Scope<'p>) -> Rc<CallFrame> {
        Self::with_parent_pin(outer, outer.parent_frame_pin())
    }

    /// The TCO fresh-tail cart: a frame that strong-owns no ancestor (`outer_frame = None`), so tail
    /// recursion stays constant-space and no back-edge forms. The captured scope's liveness rides the
    /// closure value's carrier and the return contract's witness, not the `FrameStorage.outer` chain
    /// (see `design/tail-call-optimization.md`). `pub(in crate::machine)` reserves it to the
    /// fresh-tail placement (`resolve_frame_placement`, in `crate::machine`); builtins live in
    /// `crate::builtins` and cannot name it, so the no-chain shape is unreachable to them.
    pub(in crate::machine) fn new_tail<'p>(outer: &'p Scope<'p>) -> Rc<CallFrame> {
        Self::with_parent_pin(outer, None)
    }

    /// Shared body of [`Self::new`] and [`Self::new_tail`]: build the frame with `outer_frame` as the
    /// parent pin the fresh storage's `outer` chain holds.
    fn with_parent_pin<'p>(
        outer: &'p Scope<'p>,
        outer_frame: Option<Rc<FrameStorage>>,
    ) -> Rc<CallFrame> {
        // The storage is heap-pinned behind its own `Rc` from this point on (its region minted
        // lazily, on the child scope's allocation below), so the erased child-scope pointer stays
        // valid as the storage Rc moves into the shell.
        let storage = RegionHost::fresh(outer_frame);
        // The child scope is born externally-witnessed through the construction door: it brands the
        // fresh region and the longer-lived lexical parent at one `for<'b>`, builds the real invariant
        // `Scope<'b>` coupling them, allocs it through the brand, and erases it straight into a
        // `SealedExtern` — no transient `&'a` minted, no re-anchor outside the substrate. The local
        // borrow of `storage` ends here (the carrier holds a `&'static` reference, not a borrow of
        // `storage`), so `storage` moves into the shell below; the `KoanRegion` stays at a fixed heap
        // address behind the Rc, keeping the erased reference valid.
        let scope_carrier = build_frame_child_witnessed(outer, &storage);
        Rc::new(CallFrame {
            envelope: Delivered::hosted(scope_carrier, storage),
            non_dying: false,
            owner: Cell::new(None),
            type_registry: None,
        })
    }

    /// The scheduler-owned **run frame**: a frame that *carries an already-built run scope*
    /// rather than minting a child. Top-level execution runs against this frame so `active_frame`
    /// is never `None`, which makes a body's re-dispatch-against-its-own-scope uniformly framed
    /// (Yoked) at every depth — top level included. Marked `non_dying` so the Done boundary skips
    /// the (pointless) self-lift of top-level results.
    ///
    /// `run_storage` is the `Rc<FrameStorage>` that owns the run region — the same storage `scope`
    /// (the run root) lives in. Adopting it (rather than minting an empty region) makes this frame's
    /// `region()` equal the run-root region, so a top-level-defined FN's captured-region owner
    /// resolves to this frame's storage. The adopted run scope's borrow is erased into
    /// `scope_carrier` exactly as every per-call child scope is — the fabrication hazard is deferred
    /// to the witness-bounded re-attach.
    pub fn adopting<'a>(scope: &'a Scope<'a>, run_storage: Rc<FrameStorage>) -> Rc<CallFrame> {
        debug_assert!(
            std::ptr::eq(run_storage.region(), scope.region() as *const KoanRegion),
            "adopting run_storage must own the run-root scope's region"
        );
        let scope_carrier =
            Sealed::seal(Witnessed::<ScopeRefFamily, CarrierWitness>::resident(scope));
        Rc::new(CallFrame {
            envelope: Delivered::hosted(scope_carrier, run_storage),
            non_dying: true,
            owner: Cell::new(None),
            type_registry: Some(Rc::new(TypeRegistry::new())),
        })
    }

    /// The run's subtype-verdict store — `Some` only on the run frame. The execution context reads
    /// it from there (`AmbientContext::type_registry`) and hands `&TypeRegistry` to the memoized
    /// predicates.
    pub(crate) fn type_registry(&self) -> Option<&Rc<TypeRegistry>> {
        self.type_registry.as_ref()
    }

    /// True only for the scheduler-owned run frame (see [`Self::adopting`]). The Done boundary
    /// reads this to skip the self-lift that a never-dying frame would otherwise perform.
    pub fn non_dying(&self) -> bool {
        self.non_dying
    }

    /// Record the slot that finalizes this frame's scope (the body installed into it). Read by the
    /// finalize-time close so it seals exactly the scope whose body just completed.
    pub fn set_owner(&self, slot: NodeId) {
        self.owner.set(Some(slot));
    }

    /// The slot that finalizes this frame's scope, if installed.
    pub fn owner(&self) -> Option<NodeId> {
        self.owner.get()
    }

    /// This frame's own `FrameStorage` — the envelope's retained host, which every constructor
    /// pairs with the child scope.
    pub(crate) fn storage(&self) -> &Rc<FrameStorage> {
        self.envelope.host()
    }

    /// The child scope's externally-witnessed carrier by value (`SealedExtern<ScopeRefFamily>` is
    /// `Copy`) — the run-loop step's source for a `Yoked` slot, opened at the step brand alongside the
    /// continuation / contract / deps instead of re-anchored through the borrow-bounded `attach`.
    /// Reconstructed from the envelope's sealed carrier: the same erased `&Scope`, exposed witness-less
    /// so it [`zip`](SealedExtern::zip)s with the step's other externally-witnessed carriers under one
    /// brand (the envelope host is folded into that step witness separately).
    pub(crate) fn scope_sealed(&self) -> SealedExtern<ScopeRefFamily> {
        SealedExtern::seal(*self.envelope.cell().erased())
    }

    /// Run `f` with this frame's child scope opened at a `for<'b>` brand — the sole scope read, folded
    /// onto `open` like the decide channel. Both the frame-side reads (scope id, the arg reach-set
    /// fold) and the seed-side binds (the MATCH / TRY arm `it`-bind, the user-fn param-bind, the
    /// deferred-return-type elaboration) take this read: a seed relocates its caller-`'a` value into
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

    /// This frame's region [`RegionBrand`] allocation capability, minted from its owning storage.
    /// Test-only: production allocates through the scope (`scope.brand()`); the frame-level handle is
    /// a convenience for the arena / lift Miri tests that alloc against a bare frame.
    #[cfg(test)]
    pub(crate) fn brand(&self) -> RegionBrand<'_> {
        self.storage().brand()
    }

    /// Clone this frame's `FrameStorage` Rc — the handle an escaping value (a returned closure, a
    /// module frame) pins to keep its captured environment alive independently of the shell: a
    /// `FreshTail` tail hop drops this frame's shell outright, and the escaped storage clone keeps
    /// the region it names alive regardless.
    pub fn storage_rc(&self) -> Rc<FrameStorage> {
        Rc::clone(self.storage())
    }
}
