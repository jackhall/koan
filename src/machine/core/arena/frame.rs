//! The per-call allocation frame: [`FrameStorage`] (the Koan [`RegionHost`] alias), the run-root
//! storage entry, the [`FrameReach`] / [`FrameCoverage`] reach-evidence aliases, the witnessed
//! child-scope construction door, and
//! the [`CallFrame`] shell over a refcounted `FrameStorage` that holds the per-call child [`Scope`].
//! The region/brand substrate these build on lives in the parent `arena` module.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::{KoanRegion, KoanStorageProfile, RegionBrand};
use crate::machine::core::kfunction::NodeId;
use crate::machine::core::{scope_frame, Scope, ScopeId, ScopeRefFamily};
use crate::machine::model::types::TypeRegistry;
use crate::machine::CarrierWitness;
use crate::witnessed::{
    Delivered, ReachDescription, RegionHandle, RegionHost, Sealed, SealedExtern, StepCoverage,
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

/// The run-root storage: a fresh run region with no `outer` link, stamped at the eternal tier
/// ([`RegionHost::is_eternal`]) so anything holding it can tell the run region from a per-call one.
/// Held by `run_program` (and the test harness) so the run-root scope's region has an owning Rc;
/// [`CallFrame::adopting`] reuses it as the run frame's storage, and the run-root scope reads it
/// back as its region owner through the region's own host back-link. Public: it is the one Koan-side entry point a caller
/// (production or an integration test) uses to obtain run-root storage — it mints nothing itself,
/// only building the library's `RegionHost` shell whose region lazily mints on first allocation.
pub fn run_root_storage() -> Rc<FrameStorage> {
    RegionHost::fresh_eternal()
}

/// The **program storage**: where program text and the raw AST live, outside the region model's
/// per-call tier and above even the run root. Stood up by
/// [`interpret_with_writer_path`](crate::machine::execute::interpret_with_writer_path) before the run
/// region and held for the whole run, so it is created first and released last.
///
/// Same species as [`run_root_storage`] — a [`FrameStorage`] at the eternal tier — which is what
/// makes an expression whose parts live only here reach nothing: [`RegionHost::is_eternal`] drives
/// `needs_no_pin`, and the eternal rule filters such a member out of every pin bundle and reach
/// description with no special case anywhere. It never enters the frame lifecycle or the scheduler:
/// no `CallFrame` adopts it, no `Scope` lives in its region, and its only capability in use is
/// the bump its [`brand`](ProgramStorage::brand) hands parse output.
pub fn program_storage() -> ProgramStorage {
    ProgramStorage(RegionHost::fresh_eternal())
}

/// The one host whose region an AST may borrow. Its own type, not a [`FrameStorage`] alias, because
/// the property the AST's reach answers rest on is a property of *this host* rather than of the
/// eternal tier at large: the run root is eternal too, but a `CallFrame` adopts it and a `Scope`
/// names it, so it can be a `home` and a pin-bundle member. Program storage is neither, and
/// [`program_storage`] is the sole way to obtain one.
pub struct ProgramStorage(Rc<FrameStorage>);

impl ProgramStorage {
    /// Mint this storage's [`ProgramBrand`] — the only allocation capability the parser accepts.
    pub fn brand(&self) -> ProgramBrand<'_> {
        ProgramBrand(self.0.brand())
    }
}

/// A [`RegionBrand`] carrying the proof that its region is [`ProgramStorage`]'s. The parse entry
/// points take this rather than a bare `RegionBrand`, so a parsed AST's storage tier is checked at
/// every call site rather than held by the discipline of one.
///
/// This is also the value channel's only key. Two answers in
/// [`KObject`](crate::machine::model::KObject) — `object_cell_reach` calling an expression's cell
/// `Owned`, `retains_home` answering `false` — hold because no expression reaching the value
/// channel borrows a region a holder can outlive, and so does the expression door's own claim that
/// the cell it bumps names no producer region ([`RegionBrand::alloc_expression`]). All three cite
/// one type rather than a flow: the channel admits only a
/// [`ProgramExpression`](crate::machine::model::ast::ProgramExpression), which this brand's doors
/// alone mint. A node built at an ordinary [`RegionBrand`] carries no such marker, so the channel
/// is closed to it by type — a runtime-synthesized node dispatches in place instead, and a site
/// that needs one as a value takes this brand (`op_def`'s bridge body) or threads the proof out of
/// the arm it matched.
///
/// The distinction needs a type because `KExpression` is covariant: a node borrowing a per-call
/// region coerces to any shorter lifetime, so the borrow checker sees nothing to object to.
/// Widening through [`ProgramBrand::region`] is free; the reverse does not exist.
#[derive(Clone, Copy)]
pub struct ProgramBrand<'a>(RegionBrand<'a>);

impl<'a> ProgramBrand<'a> {
    /// The plain allocation capability underneath — for the parser's own `alloc_*` calls, which
    /// need no more than a region to bump into.
    pub fn region(self) -> RegionBrand<'a> {
        self.0
    }
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

/// **The run's output sink** — where `PRINT` writes. One per run, owned by the run [`CallFrame`]
/// beside the run's [`TypeRegistry`] and reached the same way: through the execution context, never
/// off a scope. `RefCell` because writing is a `&self` act on a value the whole run shares, and
/// nothing holds the borrow across a call.
///
/// Write errors are dropped: `PRINT` is a statement with no error channel, so there is nothing for a
/// caller to do with one. This is a stopgap — see
/// [monadic side effects](../../../../roadmap/libraries/monadic-side-effects.md), which replaces
/// direct writer plumbing with an effect the language expresses.
pub struct RunWriter(RefCell<Box<dyn std::io::Write>>);

impl RunWriter {
    /// Wrap the caller-supplied sink. `'static` is what every entry point already passes — stdout, a
    /// sink, an `Rc`-shared buffer — and it is what lets the writer rest on a frame that names no
    /// region.
    pub fn new(out: Box<dyn std::io::Write>) -> Self {
        RunWriter(RefCell::new(out))
    }

    /// Write `bytes` to the run's sink, dropping any write error.
    pub fn write_out(&self, bytes: &[u8]) {
        let _ = self.0.borrow_mut().write_all(bytes);
    }
}

/// The non-owning reach description backing carrier witnesses: names the regions a carrier's value
/// reaches, hosted in the value's home region's side table and referenced (never owned) by the
/// carrier. See [`ReachDescription`] for the shared mechanism (membership queries, the self rule);
/// Koan's member semantics are the library's [`PinsRegion`](crate::witnessed::PinsRegion) impl for
/// [`RegionHost`]. Its owning counterpart is [`FrameCoverage`].
pub type FrameReach = ReachDescription<FrameStorage>;

/// The owned coverage a holder keeps to pin every region a value reaches — the ownership
/// counterpart of [`FrameReach`], released by ordinary `Drop` (a step carries one from the fold that
/// composed it to the seal that consumes it, the delivery envelope carries one across transit). See
/// [`StepCoverage`] for the surface: Koan holds, clones, threads and drops coverage, and computes
/// with it only through the container verbs on [`Delivered`] and
/// [`RegionHandle`](crate::witnessed::RegionHandle).
pub type FrameCoverage = StepCoverage<FrameStorage>;

/// Build a per-call frame's child scope **witnessed**, sealing it to the externally-witnessed
/// [`SealedExtern<ScopeRefFamily>`] the [`CallFrame`] holds — the construction door that re-anchors the
/// longer-lived lexical parent into the fresh region, with no retype outside the witnessed substrate.
///
/// The child is *born* at the destination: [`RegionHandle::bump_born_with`] hands the
/// construction closure a placement over the fresh region at a `for<'b>` brand, with the foreign
/// parent (as [`ScopeRefFamily`]) re-anchored to that same `'b`. The real invariant `Scope<'b>` is
/// built coupling the two (its `root` falling out as `outer.root`) and stored in the same act, so
/// residence is discharged by the brand rather than by a runtime check: an ambient `&Region` cannot
/// coerce to `'b`, so `child.region()` is the destination's by construction. `Scope`'s invariance is
/// honoured for free — branding the parent and the region at *independent* `'b`s is what invariance
/// rejects, and the door unifies them at a single one.
///
/// `child.outer` is a genuine cross-region borrow into the lexical parent's (possibly foreign)
/// region — unlike every other resident move-in in this file, `child` cannot rebuild at `'static`,
/// and its liveness is not the reach-witness system's business to name. It is guaranteed instead by
/// `FrameStorage`'s own `outer` `Rc` chain, the pin this call hands the door: a structural invariant
/// this construction door alone upholds by always chaining `storage`'s `outer` to the same frame that
/// owns the parent's region. That chain is **derived**, not asserted — [`CallFrame::new`] computes it
/// from the parent scope's own region owner ([`Scope::parent_frame_pin`]), and root-region parents
/// chain nothing. A fresh-tail hop's parent is the callee closure's captured scope, so the same chain
/// keeps that captured (possibly per-call) region alive across the hop that retires the caller.
///
/// The child scope lives in `storage`'s own region, so it seals under a description hosted there with
/// no members — its liveness is the frame storage, paired with it as the envelope host by the
/// [`CallFrame`] constructor.
pub(crate) fn build_frame_child_witnessed<'p>(
    outer: &'p Scope<'p>,
    storage: &Rc<FrameStorage>,
) -> Sealed<ScopeRefFamily, CarrierWitness> {
    let handle = RegionHandle::from_owner(&**storage);
    let live = handle.bump_born_with::<Scope<'static>, ScopeRefFamily, _>(
        SealedExtern::<ScopeRefFamily>::erase(outer),
        storage,
        |placement, outer_b| {
            Scope::child_for_frame_witnessed(outer_b, RegionBrand(placement.handle()))
        },
    );
    Sealed::seal(RegionBrand(handle).seal_resident::<ScopeRefFamily>(live))
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
    /// member-less resident carrier (hosted in that storage's own region), read back through
    /// [`Self::with_scope`] / [`Self::scope_sealed`] under that host pin. Co-ownership by one value
    /// replaces the former hand-maintained `(storage, scope_carrier)` field pair: the
    /// storage-pins-the-scope co-location the pair kept by field-order convention is now a
    /// construction invariant of the envelope, and dropping the sealed carrier never dereferences the
    /// child pointer, so no drop-order rule is left to hand-maintain.
    envelope: Delivered<ScopeRefFamily, CarrierWitness, FrameStorage>,
    /// This frame's own [`FrameStorage`] — the owner of the region its child scope lives in, and
    /// the pin every escapee extends ([`Self::storage_rc`]). Held beside the envelope rather than
    /// read off it: the envelope's members are one flat antichain in which a value's home is an
    /// ordinary member, so the frame's own storage is not recoverable from it by identity.
    storage: Rc<FrameStorage>,
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
    /// The run's output sink, `Some` only on the run frame ([`Self::adopting`]) — the same home and
    /// the same reach path as [`type_registry`](Self::type_registry): per-call frames own none and
    /// `PRINT` reaches this one through the execution context.
    writer: Option<RunWriter>,
}

impl CallFrame {
    /// Build a fresh per-call frame whose child `Scope` uses `outer` as its `outer` link. The
    /// storage pin chained for the parent is **derived** from `outer` via
    /// [`Scope::parent_frame_pin`]: the parent scope's own region owner when it is per-call, or no
    /// chain when the parent lives in the run-root region (which outlives the run). No caller can
    /// under-pin — there is no pin parameter to mis-wire.
    ///
    /// The one entry for every per-call frame, the TCO fresh-tail cart included: a fresh-tail hop's
    /// `outer` is the callee closure's captured (definition) scope, so chaining that scope's region
    /// owner is exactly what keeps a closure's captured frame alive across the hop that retires the
    /// caller. This never over-retains in the common tail loop — a top-level-defined recursive fn
    /// captures the run-root scope, whose [`Scope::parent_frame_pin`] is `None` (no chain), and a
    /// locally-defined tail-recursive helper captures one stable per-call def frame, pinned once (the
    /// same `Rc` every iteration). Only a loop that genuinely builds a fresh closure over each
    /// iteration's frame retains `O(N)` frames — an unavoidable data dependency, since evaluating the
    /// final closure reaches every one. The chain is a DAG (each frame's `outer` names a strictly
    /// older frame), so it forms no cycle; see `design/tail-call-optimization.md`.
    pub fn new<'p>(outer: &'p Scope<'p>) -> Rc<CallFrame> {
        let outer_frame = outer.parent_frame_pin();
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
            // The child scope seals under a description hosted in this storage's own region with no
            // members — its cross-region borrow into the parent rides `FrameStorage`'s own `outer`
            // `Rc` chain, not the reach system — so the envelope's foreign bundle is empty.
            envelope: Delivered::hosted(scope_carrier, Rc::clone(&storage), FrameCoverage::empty()),
            storage,
            non_dying: false,
            owner: Cell::new(None),
            type_registry: None,
            writer: None,
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
    ///
    /// `out` is the run's output sink, taken here for the same reason the type registry is minted
    /// here: both are run-lifetime state with exactly one home, and this is the one frame that has
    /// one.
    pub fn adopting<'a>(
        scope: &'a Scope<'a>,
        run_storage: Rc<FrameStorage>,
        out: Box<dyn std::io::Write>,
    ) -> Rc<CallFrame> {
        debug_assert!(
            std::ptr::eq(run_storage.region(), scope.region() as *const KoanRegion),
            "adopting run_storage must own the run-root scope's region"
        );
        let scope_carrier = Sealed::seal(scope.brand().seal_resident::<ScopeRefFamily>(scope));
        Rc::new(CallFrame {
            // The run scope lives in the run region (empty reach), so the envelope's foreign bundle
            // is empty.
            envelope: Delivered::hosted(
                scope_carrier,
                Rc::clone(&run_storage),
                FrameCoverage::empty(),
            ),
            storage: run_storage,
            non_dying: true,
            owner: Cell::new(None),
            type_registry: Some(Rc::new(TypeRegistry::new())),
            writer: Some(RunWriter::new(out)),
        })
    }

    /// The run's subtype-verdict store — `Some` only on the run frame. The execution context reads
    /// it from there (`AmbientContext::type_registry`) and hands `&TypeRegistry` to the memoized
    /// predicates.
    pub(crate) fn type_registry(&self) -> Option<&Rc<TypeRegistry>> {
        self.type_registry.as_ref()
    }

    /// The run's output sink — `Some` only on the run frame, read the same way the registry is
    /// (`AmbientContext::writer`), and handed to a builtin body as `BodyCtx::out`.
    pub(crate) fn writer(&self) -> Option<&RunWriter> {
        self.writer.as_ref()
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

    /// This frame's own `FrameStorage` — the owner of the region its child scope lives in, which
    /// every constructor pairs with that scope.
    pub(crate) fn storage(&self) -> &Rc<FrameStorage> {
        &self.storage
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

    /// Whether holding this frame keeps `scope`'s region alive — the gate a scheduler submission
    /// reads before storing a scope reference erased and frame-bounded
    /// (`runtime/submit.rs`'s `NodeScope::YokedChild`).
    ///
    /// Answered from the **pin that actually holds**, not from the lexical scope graph: this
    /// frame's storage and the `outer` chain it keeps alive are the regions it owns a claim on, so
    /// the question is [`RegionHost::pins_region`](crate::witnessed::RegionHost::pins_region) over
    /// that chain, asked of the storage `scope` names as its own region's owner. A scope living at
    /// the **eternal tier** (the run root) needs no claim at all — its region outlives every
    /// per-call frame, which is exactly why [`Scope::parent_frame_pin`] declines to chain it — so
    /// it answers `true` without consulting the chain.
    pub(crate) fn pins_scope_region(&self, scope: &Scope<'_>) -> bool {
        let owner = scope_frame(scope);
        owner.is_eternal() || self.storage.pins_region(owner.region())
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

    /// Build a [`KFunction`] capturing a scope **in this frame's own region**, and hand it back at the
    /// frame borrow's lifetime — the shape a closure capturing its defining frame takes.
    ///
    /// Test-only. Production functions take [`KFunction::alloc_captured`] directly, with a scope the
    /// caller already holds; the Miri shapes need the same value at the *frame's* lifetime. The
    /// captured scope is minted here rather than read off [`Self::scope_sealed`]: the bump door stores
    /// the function at the destination's own `'f`, so it needs a `&'f Scope<'f>`, and the frame's
    /// envelope opens only at a rank-2 brand nothing escapes. What the tests exercise — a callable
    /// whose captured scope lives in the region the callable itself lives in — holds either way, since
    /// the minted scope is allocated in `frame`'s storage.
    #[cfg(test)]
    pub(crate) fn alloc_capturing_scope<'f>(
        frame: &'f Rc<CallFrame>,
        signature: crate::machine::model::SignatureDraft<'f>,
        body: crate::machine::core::Body<'f>,
        types: &TypeRegistry,
    ) -> &'f crate::machine::core::KFunction<'f> {
        let captured = Scope::alloc_run_root(frame.storage());
        crate::machine::core::KFunction::alloc_captured(captured, signature, body, false, types)
    }
}
