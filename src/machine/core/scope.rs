use std::cell::{Cell, RefCell};
use std::io::Write;
use std::rc::{Rc, Weak};

use crate::machine::model::types::{KType, RecursiveSet};

use super::arena::{FrameSet, FrameStorage, KoanRegion, RegionBrand};
use super::bindings::{ApplyOutcome, BindKind, BindingIndex, Bindings, NameLookup};
use super::kerror::{KError, KErrorKind};
use super::lexical_frame::LexicalFrame;
use super::pending::PendingQueue;
use super::scope_id::ScopeId;
use crate::machine::core::kfunction::{KFunction, NodeId};
use crate::machine::model::values::{Carried, CarriedFamily, KObject};
use crate::witnessed::{Sealed, Witnessed};

/// Lexical environment. Only the root scope holds a writer in `out`; child scopes
/// have `None` and `write_out` walks `outer` to find one.
///
/// All mutable binding state lives in the embedded [`Bindings`] façade
/// (interior-mutable `RefCell`s), so a `&'a Scope<'a>` is shareable across scheduler
/// nodes. Writes that hit a borrow conflict route through [`PendingQueue`];
/// `drain_pending` replays them between dispatch nodes.
pub struct Scope<'a> {
    /// Lexical parent, held as a plain `&'a Scope<'a>` and read through the [`Scope::outer`]
    /// accessor (a bare field read). The reference re-anchors to `'a` together with the rest of the
    /// `Scope` when the holder is read out of its region; a per-call child whose parent is
    /// longer-lived couples it at the construction door's generative brand
    /// ([`child_for_frame_witnessed`](Self::child_for_frame_witnessed)), so the child needs no common
    /// lifetime with its parent. `Scope` is invariant in `'a`, so the stored reference keeps
    /// `Scope<'a>` invariant in `'a`.
    outer: Option<&'a Scope<'a>>,
    /// Direct reference to the run-global [`ScopeKind::Root`] (builtins only, immutable), read
    /// through [`Scope::root_scope`]. `None` iff `self` is the root. Every other scope points
    /// straight at it, so a builtin lookup or the no-shadow consult reaches the root in one hop
    /// instead of walking `outer` — the root holds the builtins and never changes for a run. Held as
    /// a plain `&'a Scope<'a>`; a per-call child's root falls out of its branded parent at the same
    /// construction-door brand ([`child_for_frame_witnessed`](Self::child_for_frame_witnessed)).
    root: Option<&'a Scope<'a>>,
    bindings: ScopeBindings<'a>,
    pub out: RefCell<Option<Box<dyn Write + 'a>>>,
    /// The region this scope lives in, held as its [`RegionBrand`] allocation capability — minted at
    /// region-open and inherited by same-region children. Allocation sites reach it through
    /// [`Self::brand`]; identity compares read the bare region through [`Self::region`]. Storing the
    /// brand (not a bare `&KoanRegion`) is what lets a scope hand out the alloc capability without a
    /// forgeable constructor: nothing can turn the bare `region()` back into a brand.
    brand: RegionBrand<'a>,
    /// Owning-on-upgrade handle to the [`FrameStorage`] whose region this scope lives in — the
    /// "scope bundled with its storage." Read via [`Self::region_owner`] to recover a captured
    /// function's / module's region owner (the frame a consumer retains so an escaping value
    /// outlives its producer), without walking any frame chain. A [`Weak`] (the storage owns the
    /// region owns this scope — an `Rc` back-edge would leak); upgrades whenever the region is live.
    /// Set at construction: a region-boundary scope ([`Self::run_root`], [`Self::child_for_frame_witnessed`])
    /// takes its frame's storage; a same-region child inherits its parent's. Empty (`Weak::new()`)
    /// for a test scope built outside any `FrameStorage` — such scopes own no escapable region.
    region_owner: Weak<FrameStorage>,
    /// Position-independent origin id recorded on a sealed `NominalMember` (diagnostics)
    /// and on `KType::Signature { sig, .. }` (via `sig.sig_id()`) so dispatch on
    /// user-declared types compares ids rather than scope pointers.
    pub id: ScopeId,
    pending: PendingQueue<'a>,
    pub kind: ScopeKind,
    /// Set iff this is a `RECURSIVE TYPES` block's child scope: the shared [`RecursiveSet`]
    /// whose members are co-declared and threaded together. The elaborator lowers a bare
    /// leaf naming one of its members to a transient `RecursiveRef` back-edge, so
    /// cross-references inside the block resolve regardless of lexical order — the block is
    /// the one cross-order resolution that survives strict source-order type-name lookup.
    recursive_set: Option<Rc<RecursiveSet<'a>>>,
    /// Set once the scope's defining block / frame finishes: no further bind is legal (rebinds are
    /// already rejected; this also rejects *new* binds). The seal point for its reach-set. `Cell`
    /// because it flips once, late, outside the bind hot path.
    closed: Cell<bool>,
    /// The scope's **reach-set**: the foreign per-call regions its bound values still borrow into,
    /// folded as each bind lands ([`Self::fold_reach`]). Each bound value's full carrier [`FrameSet`]
    /// folds in via [`Self::fold_reach`], which omits the scope's own home frame so a resident
    /// value never witnesses the frame it lives in (the `region → scope → set → frame` cycle). The
    /// set is a mutable builder while the scope is open and frozen once [`closed`](Self::closed) flips
    /// — `close` is the seal point (folds past it are rejected like binds are). Held inside the
    /// region-resident `Scope`, so a closure capturing the scope keeps every foreign region the scope's
    /// bindings reach alive for its life. `RefCell` because folds accrue between scheduler steps; a
    /// read (once frozen) never overlaps a fold. Empty until a region-referencing value is bound.
    reach: RefCell<FrameSet>,
}

/// A scope's binding storage. `Owned` is the default. `Borrowed` is the
/// `USING … SCOPE` transparent window: a read-only view onto another scope's
/// façade. Writes through a `Borrowed` window forward to `outer` (the call site),
/// so block-local binds persist after the block ends.
// Boxing `Owned` would add an allocation and an indirection on the hot `bindings()`
// read path; inlining the large variant is the deliberate trade.
#[allow(clippy::large_enum_variant)]
enum ScopeBindings<'a> {
    Owned(Bindings<'a>),
    /// `&'a Bindings<'a>` (not a shorter borrow) keeps `Scope<'a>` invariant in `'a`.
    /// The borrowed façade lives in the opened module's child-scope region; the
    /// `USING` builtin keeps that region alive by rooting the module value in the
    /// call-site region.
    Borrowed(&'a Bindings<'a>),
}

impl<'a> ScopeBindings<'a> {
    fn get(&self) -> &Bindings<'a> {
        match self {
            ScopeBindings::Owned(b) => b,
            ScopeBindings::Borrowed(b) => b,
        }
    }

    fn is_borrowed(&self) -> bool {
        matches!(self, ScopeBindings::Borrowed(_))
    }
}

/// Lexical classification for a [`Scope`]. The SIG-body gate walks outward and
/// pivots on the first non-`Anonymous` variant: `Sig` admits VAL declarators and
/// rejects LET-by-example; `Module` is the opposite. The per-variant `name` field
/// is the surface label for diagnostics.
///
/// `Root` marks the immutable run-global scope holding the builtins. It is
/// transparent to the SIG-body gate (like `Anonymous`); its distinct typing is the
/// lever for routing builtin lookups and the no-shadow consult through a genuinely
/// run-lived scope.
#[derive(Debug, Clone)]
pub enum ScopeKind {
    Root,
    Anonymous,
    Sig { name: String },
    Module { name: String },
}

impl<'a> Scope<'a> {
    pub fn run_root(
        storage: &'a Rc<FrameStorage>,
        outer: Option<&'a Scope<'a>>,
        out: Box<dyn Write + 'a>,
    ) -> Self {
        Self {
            outer,
            root: None,
            bindings: ScopeBindings::Owned(Bindings::new()),
            out: RefCell::new(Some(out)),
            // Region borrow and owning `Weak` both derive from the one run `storage` handle.
            brand: storage.brand(),
            region_owner: Rc::downgrade(storage),
            id: ScopeId::next(),
            pending: PendingQueue::new(),
            kind: ScopeKind::Root,
            recursive_set: None,
            closed: Cell::new(false),
            reach: RefCell::new(FrameSet::empty()),
        }
    }

    /// The [`FrameStorage`] (cloned `Weak`) whose region this scope lives in — see [`Self::brand`]'s
    /// sibling field. Upgrades to the owning `Rc` whenever the region is live.
    pub(crate) fn region_owner(&self) -> Weak<FrameStorage> {
        self.region_owner.clone()
    }

    /// The bare region this scope lives in — for identity compares (`ptr::eq`, region-pointer
    /// membership). Read-only: a bare `&KoanRegion` cannot allocate, so handing it out opens no hole.
    pub fn region(&self) -> &'a KoanRegion {
        self.brand.region()
    }

    /// The scope's [`RegionBrand`] allocation capability — the handle every alloc site into this
    /// scope's region routes (`scope.brand().alloc_object(…)`). Inherited unchanged by same-region
    /// children; minted at region-open for a region-boundary scope.
    pub(crate) fn brand(&self) -> RegionBrand<'a> {
        self.brand
    }

    /// Mark this scope closed: its defining block / frame has finished, so no further bind is legal and
    /// its reach-set freezes — `close` is the reach-set's seal point. Idempotent.
    pub fn close(&self) {
        self.closed.set(true);
    }

    /// Whether [`Self::close`] has run — a bind past this point is an invariant violation.
    pub fn is_closed(&self) -> bool {
        self.closed.get()
    }

    /// Spike guard: a bind after [`Self::close`] means the scope's defining block finished yet a
    /// write still arrived. `debug_assert` so release builds pay nothing.
    fn assert_open(&self, name: &str) {
        debug_assert!(
            !self.closed.get(),
            "bind `{name}` into closed scope {:?}",
            self.id,
        );
    }

    /// Fold a value's reach (a [`FrameSet`]) into this scope's reach-set, omitting any frame the home
    /// frame already pins (its own region or an ancestor). Called as a
    /// value is bound under the scope (a `let` / user-fn arg / applied-here closure) and at the
    /// run-root drain, so the foreign regions it borrows into stay alive for the scope's life. A bind
    /// folds the value's full carried union, so a multi-region value contributes *every* region it
    /// reaches. A fold past the seal ([`Self::close`]) is the same invariant violation as a bind past
    /// close, so it mirrors [`Self::assert_open`]'s `debug_assert`.
    pub(crate) fn fold_reach(&self, witness: &FrameSet) {
        debug_assert!(
            !self.closed.get(),
            "fold_reach into sealed scope {:?}",
            self.id,
        );
        let home = self.region_owner.upgrade();
        self.reach.borrow_mut().fold_omitting(witness, |region| {
                // Omit any region this scope already keeps alive: its own / a storage-`outer` ancestor
                // ([`FrameStorage::pins_region`]), or a **lexical** `outer`-chain ancestor. A per-call
                // frame's storage `outer` is None under TCO, so the lexical walk is what catches the
                // closure's captured (ancestor) scope — a region the closure already pins, which the
                // reach-set must not re-pin (re-pinning it, paired with a sibling bind of the call's
                // result, closes a region cycle).
                home.as_ref().is_some_and(|h| h.pins_region(region))
                    || self
                        .ancestors()
                        .any(|scope| std::ptr::eq(scope.region(), region))
            });
    }

    pub fn child_for_call(&'a self) -> Scope<'a> {
        Self::child_under(self)
    }

    /// The mutable run scope: the direct child of the immutable run-global root. Unlike the
    /// generic [`Self::child_under`] — which copies the parent's *own* `root` handle — this stamps
    /// `root` to `run_root` itself, because the run-global root carries no `root` of its own
    /// (`root: None` marks "I am the root"). The only caller is `default_scope`, which holds the
    /// root as a genuine `&'a`.
    pub fn run_child(run_root: &'a Scope<'a>) -> Scope<'a> {
        let mut child = Self::child_under(run_root);
        child.root = Some(run_root);
        child
    }

    /// Shared skeleton for a **same-region** child of `outer`: inherits `outer`'s region, its
    /// `region_owner`, and its `root` handle, and takes a fresh id. The five public same-region
    /// constructors below differ only in what they pass here — the binding storage, the kind stamp,
    /// and any recursive-set membership — so the inherit-from-`outer` field set lives in one place.
    /// (The two cross-region constructors, [`Self::run_root`] and [`Self::child_for_frame_witnessed`], do not
    /// route this: they set `root`/`region`/`region_owner` from a fresh frame, not from `outer`.)
    fn child_inheriting(
        outer: &'a Scope<'a>,
        bindings: ScopeBindings<'a>,
        kind: ScopeKind,
        recursive_set: Option<Rc<RecursiveSet<'a>>>,
    ) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            root: outer.root,
            bindings,
            out: RefCell::new(None),
            brand: outer.brand,
            region_owner: outer.region_owner.clone(),
            id: ScopeId::next(),
            pending: PendingQueue::new(),
            kind,
            recursive_set,
            closed: Cell::new(false),
            reach: RefCell::new(FrameSet::empty()),
        }
    }

    /// `outer` is the lexical parent — for FN bodies the captured definition scope,
    /// not the call site.
    pub fn child_under(outer: &'a Scope<'a>) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Owned(Bindings::new()),
            ScopeKind::Anonymous,
            None,
        )
    }

    /// Per-call frame child built **witnessed**, at the construction-door brand `'a`. The lexical
    /// parent and the fresh region arrive *already coupled at one generative `'a`* — the door
    /// ([`build_frame_child_witnessed`](super::arena::build_frame_child_witnessed)) brands them together
    /// — so this constructor stores every field by **plain coercion**: `outer` is the branded parent,
    /// `root` falls out as `outer.root`, `brand` is the branded fresh region, `region_owner` the fresh
    /// frame's `Weak`. The longer-lived parent is content-shortened into the child's region *by the
    /// brand*, honouring `Scope`'s invariance with **no retype** of its own — the per-call child's
    /// construction carries no re-anchor audited outside the witnessed substrate. The door is the only
    /// caller; the brand `'a` is un-nameable and the result erases
    /// witness-less, so nothing at the brand escapes. The frame `Rc` pins the real parent (via
    /// `FrameStorage.outer`) and the run-global root, so the coupled references never out-claim a live
    /// pointee.
    pub(crate) fn child_for_frame_witnessed(
        outer: &'a Scope<'a>,
        brand: RegionBrand<'a>,
        region_owner: Weak<FrameStorage>,
    ) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            root: outer.root,
            bindings: ScopeBindings::Owned(Bindings::new()),
            out: RefCell::new(None),
            brand,
            region_owner,
            id: ScopeId::next(),
            pending: PendingQueue::new(),
            kind: ScopeKind::Anonymous,
            recursive_set: None,
            closed: Cell::new(false),
            reach: RefCell::new(FrameSet::empty()),
        }
    }

    /// `child_under`, stamped as a SIG decl_scope.
    pub fn child_under_sig(outer: &'a Scope<'a>, name: String) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Owned(Bindings::new()),
            ScopeKind::Sig { name },
            None,
        )
    }

    /// `child_under`, stamped as a MODULE body (also used for the per-ascription view
    /// minted by `:|`).
    pub fn child_under_module(outer: &'a Scope<'a>, name: String) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Owned(Bindings::new()),
            ScopeKind::Module { name },
            None,
        )
    }

    /// Child scope for a `RECURSIVE TYPES` block body: carries the shared [`RecursiveSet`]
    /// whose members are co-declared. Members dispatch against this scope, so the elaborator
    /// threads the group (a member name lowers to `RecursiveRef`). `outer` is the lexical
    /// parent; the sealed members are mirrored up into it at the block's dep-finish.
    pub fn child_recursive_group(outer: &'a Scope<'a>, set: Rc<RecursiveSet<'a>>) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Owned(Bindings::new()),
            ScopeKind::Anonymous,
            Some(set),
        )
    }

    /// The shared [`RecursiveSet`] of the nearest enclosing `RECURSIVE TYPES` block, if any.
    /// The elaborator consults this to decide whether a bare leaf is a co-declared member:
    /// only the *nearest* group is considered, so a reference to an outer block's member
    /// falls through to ordinary resolution (an external `SetRef`), not a back-edge into the
    /// inner set.
    pub fn nearest_recursive_set(&self) -> Option<Rc<RecursiveSet<'a>>> {
        self.ancestors().find_map(|s| s.recursive_set.clone())
    }

    /// Transparent `USING … SCOPE` child scope. `outer` is the call site (the lexical
    /// parent, not the opened module's def site); bindings are a read-only window onto
    /// `module_bindings`. Reads consult the window first then walk `outer`; writes
    /// forward to `outer`. `region` is `outer.region` so block-body allocations outlive
    /// the block (forwarded binds are sound).
    pub fn child_transparent(outer: &'a Scope<'a>, module_bindings: &'a Bindings<'a>) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Borrowed(module_bindings),
            ScopeKind::Anonymous,
            None,
        )
    }

    pub fn bindings(&self) -> &Bindings<'a> {
        self.bindings.get()
    }

    /// Scope-bound `TypeIdentifier → &KType` memo read. A transparent `USING` window returns
    /// `None`: its resolutions depend on the call-site chain, so caching them into the
    /// module's shared memo would poison the module's own def-site resolution.
    pub(crate) fn type_identifier_memo_get(
        &self,
        te: &crate::machine::model::ast::TypeIdentifier,
        cutoff: Option<usize>,
    ) -> Option<(&'a crate::machine::model::types::KType<'a>, FrameSet)> {
        if self.bindings.is_borrowed() {
            return None;
        }
        self.bindings.get().type_identifier_memo_get(te, cutoff)
    }

    /// Memo write — no-op on a transparent `USING` window (see
    /// [`Self::type_identifier_memo_get`]). `reach` is the resolved type binding's home-omitted
    /// foreign reach, cached alongside the `&KType` so a memo hit rebuilds the read carrier.
    pub(crate) fn type_identifier_memo_insert(
        &self,
        te: crate::machine::model::ast::TypeIdentifier,
        cutoff: Option<usize>,
        kt: &'a crate::machine::model::types::KType<'a>,
        reach: FrameSet,
    ) {
        if self.bindings.is_borrowed() {
            return;
        }
        self.bindings
            .get()
            .type_identifier_memo_insert(te, cutoff, kt, reach);
    }

    /// Call-site scope a `Borrowed` window forwards writes to. Panics if `Borrowed`
    /// but rootless — the transparent constructor always sets `outer`, so this would
    /// be a construction bug.
    fn write_target(&self) -> &Scope<'a> {
        self.outer().expect(
            "a Borrowed (USING transparent) scope must have an outer call-site to forward \
             writes to",
        )
    }

    /// The lexical parent — a bare field read of the stored `&'a Scope<'a>`, already at `'a` because
    /// the holder was re-anchored to `'a` (the substrate retype that produced this `&Scope<'a>`)
    /// before this read.
    pub fn outer(&self) -> Option<&'a Scope<'a>> {
        self.outer
    }

    /// Iterate `self` and its `outer` chain. Per-step `RefCell` guards taken inside a
    /// `find_map` / `find` closure drop at the closure boundary, so a deep walk never
    /// accumulates live read borrows.
    pub fn ancestors(&self) -> impl Iterator<Item = &Scope<'a>> {
        std::iter::once(self).chain(std::iter::successors(self.outer(), |s| s.outer()))
    }

    /// The run-global [`ScopeKind::Root`] (builtins only). `self` if it is the root,
    /// else the direct `root` reference every scope carries — one hop, no `outer` walk.
    pub(crate) fn root_scope(&self) -> &Scope<'a> {
        match self.root {
            Some(r) => r,
            None => self,
        }
    }

    /// True iff `name` is a builtin type. The builtins live once in the immutable
    /// run-global root, so a user type declaration colliding with one is a `Rebind` at
    /// any depth — the consult hits the root directly rather than each layer of the
    /// `outer` chain. TraceFrame-local bindings (FN parameters, MATCH/TRY `it`) live below
    /// the root, so ordinary user-vs-user cross-scope shadowing is unaffected.
    fn shadows_builtin_type(&self, name: &str) -> bool {
        self.root_scope().bindings().has_builtin_type(name)
    }

    /// True iff `key` names a builtin dispatch bucket — a finalized overload lives
    /// under it in the run-global root. Builtins are immutable and unshadowable, so a
    /// user FN/FUNCTOR whose untyped signature key collides with a builtin is a
    /// `Rebind`; it must never merge into the builtin bucket. The consult reads the
    /// root directly.
    fn shadows_builtin_function(&self, key: &crate::machine::model::types::UntypedKey) -> bool {
        self.root_scope().bindings().has_builtin_function(key)
    }

    /// True iff `probe` resolves a builtin operator group in the run-global root.
    /// Operators are builtins too — a user operator over a builtin probe is rejected
    /// rather than shadowing or extending it.
    fn shadows_builtin_operator(&self, probe: &str) -> bool {
        self.root_scope().bindings().has_builtin_operator(probe)
    }

    /// True iff the nearest non-`Anonymous` enclosing scope is a SIG decl_scope. A
    /// `Module` short-circuits to `false`; `Anonymous` frames are transparent.
    pub fn is_in_sig_body(&self) -> bool {
        self.ancestors()
            .find_map(|s| match &s.kind {
                ScopeKind::Sig { .. } => Some(true),
                ScopeKind::Module { .. } => Some(false),
                ScopeKind::Root | ScopeKind::Anonymous => None,
            })
            .unwrap_or(false)
    }

    /// Bind `name` in this scope. Errors `Rebind` if `data` already holds `name`
    /// (same-scope rebind rejected; cross-scope shadowing allowed). Removes any
    /// matching placeholder this scope owns on success.
    ///
    /// Conditional-defer: direct mutation first, falls back to the `pending` queue
    /// iff a borrow conflict would otherwise panic.
    pub fn bind_value(
        &self,
        name: String,
        obj: &'a KObject<'a>,
        index: BindingIndex,
        reach: FrameSet,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            // Transparent `USING` window: reads consult the window before the call
            // site, so a local bind whose name is already a surfaced module member
            // would be silently shadowed. Reject it; otherwise forward to the call
            // site under the caller's `index` (the bind belongs to the call site's
            // block, at the call site's statement position), carrying the value's reach.
            if matches!(
                self.bindings.get().lookup_value(&name, None),
                Some(NameLookup::Bound(_))
            ) {
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "USING: local bind `{name}` collides with a surfaced module member; \
                     rename it to avoid silently shadowing the module's `{name}`",
                ))));
            }
            return self.write_target().bind_value(name, obj, index, reach);
        }
        self.assert_open(&name);
        match self
            .bindings
            .get()
            .try_bind_value(&name, obj, index, reach.clone())?
        {
            ApplyOutcome::Applied => Ok(()),
            ApplyOutcome::Conflict => {
                self.pending.defer_value(name, obj, index, reach);
                Ok(())
            }
        }
    }

    /// Add `fn_ref` to the `functions` bucket keyed by its untyped signature, then
    /// insert `obj` into `data[name]`. Errors:
    /// - `DuplicateOverload` if the bucket already holds an exact-signature match.
    /// - `Rebind` if `data[name]` holds a non-function.
    ///
    /// Same conditional-defer shape as `bind_value`.
    pub fn register_function(
        &self,
        name: String,
        fn_ref: &'a KFunction<'a>,
        obj: &'a KObject<'a>,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            return self
                .write_target()
                .register_function(name, fn_ref, obj, index);
        }
        self.assert_open(&name);
        // A user overload may not join a builtin's bucket — builtins are immutable and
        // unshadowable. The root registers its own builtins at `BUILTIN`, so only a
        // non-`BUILTIN` index is gated.
        if index != BindingIndex::BUILTIN
            && self.shadows_builtin_function(&fn_ref.signature.untyped_key())
        {
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        match self
            .bindings
            .get()
            .try_register_function(&name, fn_ref, obj, index)?
        {
            ApplyOutcome::Applied => Ok(()),
            ApplyOutcome::Conflict => {
                self.pending.defer_function(name, fn_ref, obj, index);
                Ok(())
            }
        }
    }

    /// Register `name` as a type-valued binding. Lives in [`Bindings::types`] as an
    /// region-allocated `&KType`; reads go through [`Self::resolve_type`]. Same
    /// conditional-defer shape as [`Self::bind_value`]. Infallible: a name collision
    /// at builtin registration is a programming error, so the [`KError`] is dropped.
    pub fn register_type(
        &self,
        name: String,
        ktype: crate::machine::model::types::KType<'a>,
        index: BindingIndex,
        reach: FrameSet,
    ) {
        if self.bindings.is_borrowed() {
            self.write_target().register_type(name, ktype, index, reach);
            return;
        }
        self.assert_open(&name);
        let kt_ref: &'a crate::machine::model::types::KType<'a> = self.brand().alloc_ktype(ktype);
        match self
            .bindings
            .get()
            .try_register_type(&name, kt_ref, index, reach.clone())
        {
            Ok(ApplyOutcome::Applied) => {}
            Ok(ApplyOutcome::Conflict) => self.pending.defer_type(name, kt_ref, index, reach),
            Err(_) => {}
        }
    }

    /// User-facing type registration (`LET <TypeIdentifier> = …`, `VAL`): rejects a collision
    /// with a builtin type before delegating to the infallible [`Self::register_type`].
    /// Builtins are immutable and unshadowable, so a user type that names one is a
    /// `Rebind` at any depth — including a SIG/MODULE-local abstract member — and the
    /// [`Self::shadows_builtin_type`] consult reads the root directly. Builtin
    /// registration itself stays on the infallible `register_type`.
    pub fn register_user_type(
        &self,
        name: String,
        ktype: crate::machine::model::types::KType<'a>,
        index: BindingIndex,
        reach: FrameSet,
    ) -> Result<(), KError> {
        if self.shadows_builtin_type(&name) {
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        self.register_type(name, ktype, index, reach);
        Ok(())
    }

    /// Upsert install for a type-only nominal finalize (STRUCT / named UNION / Result /
    /// MODULE). Writes the sealed `SetRef` identity into [`Bindings::types`], overwriting
    /// a `PartialEq`-equal `SetRef` a `RECURSIVE TYPES` block pre-installed (same set + index).
    /// Returns the region-allocated `&KType` so the caller can yield it as a
    /// `Carried::Type`. Same conditional-defer shape as [`Self::register_type`];
    /// `Err(Rebind)` on a genuine non-equal collision.
    ///
    /// Finalize runs post-dep-finish, past the re-entrant queue point — a `Conflict` here
    /// is a programming error, so it panics rather than deferring (deferring would risk
    /// a window where the type resolves with the pre-install's empty payload).
    pub fn register_type_upsert(
        &self,
        name: String,
        ktype: crate::machine::model::types::KType<'a>,
        index: BindingIndex,
        reach: FrameSet,
    ) -> Result<&'a crate::machine::model::types::KType<'a>, KError> {
        if self.bindings.is_borrowed() {
            return self
                .write_target()
                .register_type_upsert(name, ktype, index, reach);
        }
        if self.shadows_builtin_type(&name) {
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        let kt_ref: &'a crate::machine::model::types::KType<'a> = self.brand().alloc_ktype(ktype);
        match self
            .bindings
            .get()
            .try_register_type_upsert(&name, kt_ref, index, reach)?
        {
            ApplyOutcome::Applied => Ok(kt_ref),
            ApplyOutcome::Conflict => panic!(
                "register_type_upsert borrow conflict on `{name}` — nominal finalize sites \
                 run post-dep-finish outside the re-entrant bind hot path",
            ),
        }
    }

    /// Synchronous pre-install of a nominal type's identity — `name` → `ktype` (a
    /// `KType::SetRef` into the declaring set's shared `RecursiveSet`) — into
    /// [`Bindings::types`] *before* the declaration's schema finalizes, so the body can
    /// reference the name (self-recursion, or sibling members in a `RECURSIVE TYPES` block).
    /// Unlike the finalize-time upsert it panics on borrow conflict instead of deferring,
    /// and panics on `Rebind` — the identity must not already be in `types`.
    ///
    /// Callers run this with no outer `bindings` borrow held; a conflict here is a
    /// programming error. The schema is filled later, at the declaration's own finalize,
    /// against the same shared set recovered from this `SetRef`.
    pub fn preinstall_identity(
        &self,
        name: String,
        ktype: crate::machine::model::types::KType<'a>,
        index: BindingIndex,
    ) {
        if self.bindings.is_borrowed() {
            self.write_target().preinstall_identity(name, ktype, index);
            return;
        }
        let kt_ref: &'a crate::machine::model::types::KType<'a> = self.brand().alloc_ktype(ktype);
        // A pre-installed nominal identity is a `KType::SetRef` into the declaring set — owned data
        // reaching no foreign region — so its stored reach is empty.
        match self
            .bindings
            .get()
            .try_register_type(&name, kt_ref, index, FrameSet::empty())
        {
            Ok(ApplyOutcome::Applied) => {}
            Ok(ApplyOutcome::Conflict) => panic!(
                "preinstall_identity borrow conflict on `{name}` — runs with no outer \
                 types borrow held",
            ),
            Err(e) => panic!(
                "preinstall_identity Rebind for `{name}`: {e} — the identity should not \
                 already be in bindings.types",
            ),
        }
    }

    /// Apply queued writes between dispatch nodes. Items that still hit a borrow
    /// conflict stay queued (eventually-consistent), and drain-time `Err`s are
    /// debug-asserted (production drops them — dispatch nodes have no caller frame to
    /// surface them to).
    pub fn drain_pending(&self) {
        // Transparent `USING` window writes forward to the call site, so its pending
        // queue lives there too — flush the call site.
        if self.bindings.is_borrowed() {
            self.write_target().drain_pending();
            return;
        }
        self.pending.drain(self.bindings.get());
    }

    /// Nearest value binding of `name` up the `outer` chain. Collapses a `Parked`
    /// producer and a miss to `None`. Visibility unfiltered — use
    /// [`Self::lookup_with_chain`] from a dispatch-driven path.
    pub fn lookup(&self, name: &str) -> Option<&'a KObject<'a>> {
        self.lookup_with_chain(name, None)
    }

    /// Chain-gated companion to [`Self::lookup`]. Filter consults `chain` per
    /// [`visible`].
    pub fn lookup_with_chain(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<&'a KObject<'a>> {
        self.resolve_with_chain(name, chain)
            .and_then(NameLookup::bound)
    }

    /// Resolve `name` against this scope and the `outer` chain. Stops at the first
    /// per-scope hit, checking `data` then `placeholders` — an inner placeholder
    /// shadows an outer value binding, because the inner producer hasn't finalized
    /// and the consumer must park rather than read through.
    ///
    /// Type-side bindings are not consulted — see [`Self::resolve_type`].
    /// Visibility unfiltered; dispatch-driven reads use [`Self::resolve_with_chain`].
    pub fn resolve(&self, name: &str) -> Option<NameLookup<&'a KObject<'a>>> {
        self.resolve_with_chain(name, None)
    }

    /// Chain-gated companion to [`Self::resolve`]. Per-scope hits are filtered through
    /// [`visible`] before being returned; hidden entries (later siblings, or
    /// value-style binders before their lexical position) are skipped and the walk
    /// continues outward.
    /// The chain-derived visibility cutoff for a per-scope `bindings` lookup, or `None` when this
    /// scope's bindings are all unconditionally visible. A transparent `USING` window
    /// ([`Self::child_transparent`]) surfaces a finalized module's members as imports available
    /// throughout the block — index-0 semantics, like builtins and bound parameters — so they
    /// carry no lexical-ordering relationship to the reading position and take no cutoff. Without
    /// this, a body statement dispatched into the window via `enter_block` (chain frame
    /// `(window, i)`) would filter the surfaced members by an unrelated index and miss them.
    pub(crate) fn binding_cutoff(&self, chain: Option<&LexicalFrame>) -> Option<usize> {
        if self.bindings.is_borrowed() {
            None
        } else {
            chain.and_then(|c| c.index_for(self.id))
        }
    }

    pub fn resolve_with_chain(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<NameLookup<&'a KObject<'a>>> {
        self.ancestors().find_map(|scope| {
            scope
                .bindings()
                .lookup_value(name, scope.binding_cutoff(chain))
        })
    }

    /// Carrier-returning twin of [`Self::resolve_with_chain`]: resolve `name` to the bound value
    /// wrapped in a [`Witnessed`] carrier naming its reach, so an object-value read embeds a carrier
    /// by construction instead of reconstructing the reach from the value. Walks the same `outer`
    /// chain, but at the **binding** scope wraps the value via [`Self::bound_value_carrier`] — the
    /// witness is that scope's home frame, not the reading scope's. The non-`Bound` dispositions mirror
    /// [`Self::resolve_with_chain`].
    pub(crate) fn resolve_value_carrier(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<NameLookup<Witnessed<CarriedFamily, FrameSet>>> {
        self.ancestors().find_map(|scope| {
            match scope
                .bindings()
                .lookup_value_carrier(name, scope.binding_cutoff(chain))?
            {
                NameLookup::Bound(hit) => Some(NameLookup::Bound(
                    scope.resident_value_carrier(hit.obj, &hit.reach),
                )),
                NameLookup::Parked(producer) => Some(NameLookup::Parked(producer)),
            }
        })
    }

    /// Build the terminal carrier for a value living **in this scope's region** from its binding's
    /// stored reach: witness = this scope's home frame ∪ `foreign` (the value's home-omitted foreign
    /// reach, captured at bind time). The home frame is fetched **fresh** here (never stored — that
    /// would close the `frame → region → scope → bindings → frame` cycle) and pins this scope's own
    /// region; `foreign` names every other region the value reaches, so the carrier is self-contained.
    /// Both a name / ATTR read and the FN-def / LET define sites route this, so a read never
    /// reconstructs the reach by walking the value. The bundle itself runs on the confined arena
    /// surface ([`RegionBrand::seal_resident`]), so `Witnessed::resident` is never reached from a
    /// builtin.
    pub(crate) fn resident_value_carrier(
        &self,
        obj: &'a KObject<'a>,
        foreign: &FrameSet,
    ) -> Witnessed<CarriedFamily, FrameSet> {
        let home = self
            .region_owner()
            .upgrade()
            .expect("the binding scope's region owner is held while its value is read");
        let mut witness = FrameSet::singleton(home.clone());
        witness.fold_omitting(foreign, |region| home.pins_region(region));
        self.brand().seal_resident(Carried::Object(obj), witness)
    }

    /// The home-omitted foreign reach of `witness` as this scope would fold it into its reach-set —
    /// its home frame and lexical-ancestor regions omitted (mirrors [`Self::fold_reach`]) — as a
    /// standalone [`FrameSet`] to **store on a binding entry**. Cycle-safe by the same rule
    /// `fold_reach` obeys: the home frame's own `Rc` never lands in the set, so storing it inside the
    /// scope's own region opens no `frame → region → scope → bindings → frame` strong cycle. A
    /// region-pure or ancestor-resident value yields the empty set.
    pub(crate) fn foreign_reach_of(&self, witness: &FrameSet) -> FrameSet {
        let home = self.region_owner.upgrade();
        let mut foreign = FrameSet::empty();
        foreign.fold_omitting(witness, |region| {
            home.as_ref().is_some_and(|h| h.pins_region(region))
                || self
                    .ancestors()
                    .any(|scope| std::ptr::eq(scope.region(), region))
        });
        foreign
    }

    /// Fold this scope's home frame onto an already-witnessed `carrier` whose value lives **in this
    /// scope's region** — the seal that names a constructed value's reach. The carrier arrives born
    /// under the empty (foreign-reach-only) set from the witnessed alloc; sealing folds the home frame,
    /// and because the scope's sealed reach-set lives in that same region, holding the home transitively
    /// pins every foreign region the value reaches, so a single-frame fold names the full reach. An
    /// `embedded` carrier — a computed value the sealed value was projected out of, whose reach the
    /// read-site frame may not pin (an attr field of a delivered `Wrapped`, a FROM record's shared
    /// backing) — folds its foreign reach on top, omitting any frame the home already pins. Folds
    /// onto the received carrier via [`Witnessed::reseal_under`], so it carries no re-anchor of an
    /// externally-built value.
    pub(crate) fn seal_value(
        &self,
        carrier: Witnessed<CarriedFamily, FrameSet>,
        embedded: Option<&Sealed<CarriedFamily, FrameSet>>,
    ) -> Witnessed<CarriedFamily, FrameSet> {
        let home = self
            .region_owner()
            .upgrade()
            .expect("the sealing scope's region owner is held while its value is read");
        let mut witness = FrameSet::singleton(home.clone());
        if let Some(embedded) = embedded {
            witness.fold_omitting(embedded.witness(), |region| home.pins_region(region));
        }
        carrier.reseal_under(witness)
    }

    /// Build the terminal carrier for a type living **in this scope's region** from its binding's
    /// stored reach — the type-channel twin of [`Self::resident_value_carrier`]. Witness = this
    /// scope's home frame ∪ `foreign` (the type's home-omitted foreign reach: empty for owned data, a
    /// module's child-scope reach folded at construction). The home frame is fetched fresh (never
    /// stored) and the bundle runs on the confined arena surface ([`RegionBrand::seal_resident`]), so
    /// a type read witnesses the existing `&'a KType` in place — no `alloc_ktype_witnessed` re-clone,
    /// no `child_scope()` walk to rebuild the reach.
    pub(crate) fn resident_type_carrier(
        &self,
        kt: &'a crate::machine::model::types::KType<'a>,
        foreign: &FrameSet,
    ) -> Witnessed<CarriedFamily, FrameSet> {
        let home = self
            .region_owner()
            .upgrade()
            .expect("the binding scope's region owner is held while its type is read");
        let mut witness = FrameSet::singleton(home.clone());
        witness.fold_omitting(foreign, |region| home.pins_region(region));
        self.brand().seal_resident(Carried::Type(kt), witness)
    }

    /// The home-omitted foreign reach a module minted in **this** scope gets from its `child` scope,
    /// folded from the child scope held **directly** at construction (never recovered by walking a
    /// built `KType::Module`): the child's `region_owner` ∪ its sealed reach-set, omitting any region
    /// this scope's home frame already pins. A co-located module (`MODULE`, opaque `:|`) folds nothing
    /// extra; a transparent `:!` view of a source module pins that source's (foreign) region and reach.
    /// Stored on the module's `types` binding and passed to [`Self::resident_type_carrier`] at reads,
    /// so a module's reach is folded once at construction and never rebuilt by walking the value.
    pub(crate) fn reach_of_child(&self, child: &Scope<'a>) -> FrameSet {
        let home = self.region_owner().upgrade();
        let mut foreign = FrameSet::empty();
        if let Some(child_home) = child.region_owner().upgrade() {
            foreign.fold_omitting(&FrameSet::singleton(child_home), |region| {
                home.as_ref().is_some_and(|h| h.pins_region(region))
            });
        }
        foreign.fold_omitting(&child.reach.borrow(), |region| {
            home.as_ref().is_some_and(|h| h.pins_region(region))
        });
        foreign
    }

    /// Install a dispatch-time placeholder for `name` -> producer slot `idx`. See
    /// [`Bindings::try_install_placeholder`] for `Rebind` rules and the asymmetry with
    /// `try_bind_*` (panics on borrow conflict rather than queueing).
    pub fn install_placeholder(
        &self,
        name: String,
        idx: NodeId,
        index: BindingIndex,
        kind: BindKind,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            return self
                .write_target()
                .install_placeholder(name, idx, index, kind);
        }
        self.bindings
            .get()
            .try_install_placeholder(name, idx, index, kind)
    }

    /// Error-path companion to [`Self::install_placeholder`]: remove any value-side
    /// placeholder pointing at `producer`. Routes to the same target the install used so a
    /// failed binder body can't leak a scheduler-local placeholder into a later run on a
    /// persistent scope. See [`Bindings::clear_placeholders_for_producer`].
    pub fn clear_placeholders_for_producer(&self, producer: NodeId) {
        if self.bindings.is_borrowed() {
            self.write_target()
                .clear_placeholders_for_producer(producer);
            return;
        }
        self.bindings
            .get()
            .clear_placeholders_for_producer(producer);
    }

    /// Bucket-keyed companion to [`Self::install_placeholder`]: appends a
    /// `pending_overloads[bucket]` entry so dispatch's no-bucket fallback parks
    /// bare-arg calls on the producing FN/FUNCTOR binder. Sibling installs sharing the
    /// bucket each append a distinct entry; entries are removed on finalize by
    /// matching the producing binder's `BindingIndex`. See
    /// [`Bindings::try_install_pending_overload`].
    pub fn install_pending_overload(
        &self,
        bucket: crate::machine::model::types::UntypedKey,
        idx: NodeId,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            return self
                .write_target()
                .install_pending_overload(bucket, idx, index);
        }
        self.bindings
            .get()
            .try_install_pending_overload(bucket, idx, index)
    }

    /// Resolve a *finalized* type, unfiltered. The `Option<&KType>` adapter over
    /// [`Self::resolve_type_with_chain`]: an in-flight [`NameLookup::Parked`]
    /// collapses to `None` here, so callers that must park on the producer use
    /// `resolve_type_with_chain` and match its `Parked` arm.
    pub fn resolve_type(&self, name: &str) -> Option<&'a crate::machine::model::types::KType<'a>> {
        self.resolve_type_with_chain(name, None)
            .and_then(NameLookup::bound)
    }

    /// Chain-gated type-side resolution — the type-language mirror of
    /// [`Self::resolve_with_chain`]. Per-scope `types` (and `BindKind::Type` placeholder)
    /// hits are filtered through [`visible`], so a type binding declared lexically later in
    /// the same block is invisible to an earlier sibling — a forward type reference is a
    /// position error. Surfaces a still-finalizing producer as [`NameLookup::Parked`]
    /// so a type consumer parks on it (rather than bootstrapping off the value-side lookup).
    pub fn resolve_type_with_chain(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<NameLookup<&'a KType<'a>>> {
        // Builtins are unshadowable, so a builtin type is authoritative: consult the
        // immutable root in one hop and return it without walking the user chain. The
        // `idx == 0` gate keeps this to genuine builtins, so a synthetic root-position
        // user type still resolves by innermost-wins precedence below.
        let root = self.root_scope();
        if root.bindings().has_builtin_type(name) {
            return root.bindings().lookup_type(name, None);
        }
        self.ancestors().find_map(|scope| {
            scope
                .bindings()
                .lookup_type(name, scope.binding_cutoff(chain))
        })
    }

    /// The home-omitted foreign reach of the `types` binding `name` resolves to under `chain` — the
    /// reach a bare-type-leaf read stores on its carrier, computed at the memo-miss so a hit rebuilds
    /// the carrier without re-walking. Mirrors [`Self::resolve_type_with_chain`]'s walk (builtins
    /// first, then innermost-wins) via the reach-carrying [`Bindings::lookup_type_carrier`]. A builtin,
    /// a `from_name` / `RecursiveRef` fallback that names no binding, or a placeholder reaches nothing
    /// foreign, so all yield the empty set.
    pub(crate) fn resolve_type_reach(&self, name: &str, chain: Option<&LexicalFrame>) -> FrameSet {
        if self.root_scope().bindings().has_builtin_type(name) {
            return FrameSet::empty();
        }
        self.ancestors()
            .find_map(|scope| {
                scope
                    .bindings()
                    .lookup_type_carrier(name, scope.binding_cutoff(chain))
            })
            .map(|hit| match hit {
                NameLookup::Bound(bound) => bound.reach,
                NameLookup::Parked(_) => FrameSet::empty(),
            })
            .unwrap_or_default()
    }

    /// Resolve a chain's operator-group probe against this scope and the `outer`
    /// chain, paralleling [`Self::resolve_type_with_chain`]: per-scope `operators`
    /// hits are filtered through [`visible`], so the innermost visible registration
    /// wins (operator shadowing falls out of the walk). `chain = None` is the
    /// test/builtin-registration unfiltered mode.
    pub fn resolve_operator_group_with_chain(
        &self,
        probe: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<&'a crate::machine::model::operators::OperatorGroup> {
        // A builtin operator group is unshadowable and authoritative — consult the root
        // in one hop. The `idx == 0` gate keeps synthetic root-position user operators on
        // the innermost-wins walk below.
        let root = self.root_scope();
        if root.bindings().has_builtin_operator(probe) {
            return root.bindings().lookup_operator_group(probe, None);
        }
        self.ancestors().find_map(|scope| {
            scope
                .bindings()
                .lookup_operator_group(probe, scope.binding_cutoff(chain))
        })
    }

    /// Register `probe → group` in this scope's operator registry. The `OP` binder
    /// installs one entry per size-≥2 subset of the declared operators; test fixtures
    /// register the subsets they exercise. Same conditional-defer-free shape as the
    /// type registry — a borrow conflict is queued is not expected here (registration
    /// runs outside the re-entrant bind hot path), so `Conflict` panics.
    pub fn register_operator_group(
        &self,
        probe: String,
        group: &'a crate::machine::model::operators::OperatorGroup,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            return self
                .write_target()
                .register_operator_group(probe, group, index);
        }
        // Operators are builtins too: a user operator over a builtin probe is a
        // `Rebind`, never a shadow. The root registers its own at `BUILTIN`.
        if index != BindingIndex::BUILTIN && self.shadows_builtin_operator(&probe) {
            return Err(KError::new(KErrorKind::Rebind { name: probe }));
        }
        match self
            .bindings
            .get()
            .try_register_operator_group(probe.clone(), group, index)?
        {
            ApplyOutcome::Applied => Ok(()),
            ApplyOutcome::Conflict => panic!(
                "register_operator_group borrow conflict on `{probe}` — operator \
                 registration runs outside the re-entrant bind hot path",
            ),
        }
    }

    /// Write `bytes` to the nearest writer up the `outer` chain. Writer errors are
    /// silently dropped.
    pub fn write_out(&self, bytes: &[u8]) {
        for scope in self.ancestors() {
            if let Some(w) = scope.out.borrow_mut().as_mut() {
                let _ = w.write_all(bytes);
                return;
            }
        }
    }

    pub fn lookup_kfunction(&self, name: &str) -> Option<&'a KFunction<'a>> {
        match self.lookup(name)? {
            KObject::KFunction(f) => Some(*f),
            _ => None,
        }
    }
}
