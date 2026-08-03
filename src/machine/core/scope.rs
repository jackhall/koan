use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::Write;
use std::rc::{Rc, Weak};

use crate::machine::model::KType;
use crate::machine::model::OperatorGroup;
use crate::machine::model::RecursiveGroupWindow;

use super::arena::{FrameStorage, FrameStorageExt, KoanRegion, RegionBrand};
use super::bindings::Bindings;
use super::scope_id::ScopeId;

mod reach;
mod registry;
mod resolve;

pub(crate) use reach::AdoptSeam;

/// Lexical environment. Only the root scope holds a writer in `out`; child scopes
/// have `None` and `write_out` walks `outer` to find one.
///
/// All mutable binding state lives in the embedded [`Bindings`] façade
/// (interior-mutable `RefCell`s), so a `&'a Scope<'a>` is shareable across scheduler
/// nodes. A write into a published scope is not performed in place: it rides the step
/// outcome as a [`WriteOp`](crate::machine::core::bindings::WriteOp) the run loop applies.
pub struct Scope<'a> {
    /// Lexical parent, read through [`Scope::outer`]. Held as `&'a Scope<'a>` (not a shorter borrow)
    /// so `Scope<'a>` stays invariant in `'a`; a per-call child couples to a longer-lived parent at
    /// the construction door's generative brand
    /// ([`child_for_frame_witnessed`](Self::child_for_frame_witnessed)), so it needs no common
    /// lifetime with its parent.
    outer: Option<&'a Scope<'a>>,
    /// Direct reference to the run-global [`ScopeKind::Root`] (builtins only, immutable), read
    /// through [`Scope::root_scope`]. `None` iff `self` is the root. Every other scope points
    /// straight at it, so a builtin lookup or the no-shadow consult reaches the root in one hop
    /// instead of walking `outer`. A per-call child's root falls out of its branded parent at the
    /// construction door ([`child_for_frame_witnessed`](Self::child_for_frame_witnessed)).
    root: Option<&'a Scope<'a>>,
    bindings: ScopeBindings<'a>,
    pub out: RefCell<Option<Box<dyn Write + 'a>>>,
    /// The region this scope lives in, held as its [`RegionBrand`] allocation capability — minted at
    /// region-open and inherited by same-region children. Allocation sites reach it through
    /// [`Self::brand`]; identity compares read the bare region through [`Self::region`]. Storing the
    /// brand (not a bare `&KoanRegion`) is what lets a scope hand out the alloc capability without a
    /// forgeable constructor: the no-forgeable-constructor rule is the library's — `RegionBrand` wraps a
    /// `RegionHandle`, whose only public minter is `RegionHandle::from_owner` and whose field and `new`
    /// are crate-private to `workgraph` — so nothing can turn the bare `region()` back into a brand.
    brand: RegionBrand<'a>,
    /// Owning-on-upgrade handle to the [`FrameStorage`] whose region this scope lives in. Read via
    /// [`Self::region_owner`] to recover a captured function's / module's region owner without
    /// walking any frame chain. A [`Weak`] because the storage owns the region owns this scope — an
    /// `Rc` back-edge would leak; upgrades whenever the region is live. Set at construction: a
    /// region-boundary scope ([`Self::run_root`], [`Self::child_for_frame_witnessed`]) takes its
    /// frame's storage, a same-region child inherits its parent's; empty (`Weak::new()`) for a test
    /// scope built outside any `FrameStorage`.
    region_owner: Weak<FrameStorage>,
    /// Position-independent origin id, recorded on an `AbstractType` node's `source` so
    /// dispatch on SIG-declared members compares ids rather than scope pointers.
    pub id: ScopeId,
    /// Lexical classification, and with it every per-kind payload — the SIG slot collector, a
    /// `GROUP` body's operator record, a `RECURSIVE TYPES` block's open window. Every payload is
    /// reached through the walking accessors below, never off a scope reference directly.
    pub kind: ScopeKind<'a>,
    /// Set once the scope's defining block / frame finishes: no further bind is legal (rebinds are
    /// already rejected; this also rejects *new* binds). The seal point for its reach-set. `Cell`
    /// because it flips once, late, outside the bind hot path.
    closed: Cell<bool>,
}

/// A scope's binding storage. `Owned` is the default. `Borrowed` is the
/// `USING … SCOPE` transparent window: a read-only view onto another scope's
/// façade. Writes through a `Borrowed` window forward to `outer` (the call site),
/// so block-local binds persist after the block ends.
// Boxing `Owned` would add an allocation and an indirection on the hot `bindings()`
// read path; inlining the large variant is the deliberate trade.
#[allow(clippy::large_enum_variant)]
enum ScopeBindings<'a> {
    Owned(Bindings),
    /// The borrowed façade lives in the opened module's child-scope region; the
    /// `USING` builtin keeps that region alive by rooting the module value in the
    /// call-site region.
    Borrowed(&'a Bindings),
}

impl ScopeBindings<'_> {
    fn get(&self) -> &Bindings {
        match self {
            ScopeBindings::Owned(b) => b,
            ScopeBindings::Borrowed(b) => b,
        }
    }

    fn is_borrowed(&self) -> bool {
        matches!(self, ScopeBindings::Borrowed(_))
    }
}

/// Lexical classification for a [`Scope`], carrying each kind's own payload — so a payload exists
/// exactly when its kind does, by type rather than by prose. The SIG-body gate walks outward and
/// pivots on the first opaque variant: `Sig` admits VAL declarators and rejects LET-by-example;
/// `Module` is the opposite. The per-variant `name` field is the surface label for diagnostics.
///
/// `Root` marks the immutable run-global scope holding the builtins. It is transparent to the
/// SIG-body gate (like `Anonymous`); its distinct typing is the lever for routing builtin lookups
/// and the no-shadow consult through a genuinely run-lived scope.
///
/// Neither `Clone` nor `Debug`: `Sig`'s slot collector is a live `RefCell` that must not be
/// silently duplicated, and nothing prints a kind.
pub enum ScopeKind<'a> {
    Root,
    Anonymous,
    /// A SIG decl_scope. `slots` is its VAL slot collector: `VAL <name> :Type` records
    /// `name → declared type` here — a schema in progress, not a binding universe (nothing
    /// resolves names in it; no visibility index). The SIG finish projects it into the signature's
    /// stored `SigSchema`, and ATTR over the signature reads a slot's declared type back out of it.
    /// Plain `borrow_mut` inside the single write door is fine: the cell is never held across calls.
    Sig {
        name: String,
        slots: RefCell<HashMap<String, KType>>,
    },
    /// A MODULE body (also the per-ascription view minted by `:|`). `group` is `Some` for a `GROUP`
    /// body — a group *is* a module — naming the one [`OperatorGroup`] record its member `OP`
    /// declarations belong to, read through [`Scope::nearest_group_context`]. A bare
    /// `&'a OperatorGroup<'a>`: the record is same-region with this scope, so it rides the scope's
    /// own brand like every other `'a` field and the reader's answer cannot outlive its scope borrow
    /// by construction — no seal, no pin. The registry entries hold that same record as a sealed
    /// carrier, since a binding table is lifetime-free.
    Module {
        name: String,
        group: Option<&'a OperatorGroup<'a>>,
    },
    /// A `RECURSIVE TYPES` block body: transparent to the SIG-body gate like `Anonymous`, and
    /// carrying the open [`RecursiveGroupWindow`] whose members are co-declared and elaborate
    /// together. The elaborator lowers a bare leaf naming one of its members to that member's
    /// relative sibling back-edge, so cross-references inside the block resolve regardless of
    /// lexical order — the block is the one cross-order resolution that survives strict
    /// source-order type-name lookup. The window rides the scope rather than the registry because
    /// several can be open at once under the park-capable scheduler.
    RecursiveBlock {
        window: Rc<RecursiveGroupWindow>,
    },
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
            kind: ScopeKind::Root,
            closed: Cell::new(false),
        }
    }

    /// The storage pin [`CallFrame::new`](super::arena::CallFrame::new) chains for a frame whose
    /// child scope borrows into this scope's region: the region's owning storage — or no pin when
    /// that owner is at the eternal tier
    /// ([`is_eternal`](crate::witnessed::RegionHost::is_eternal)), whose region outlives everything
    /// that could retain it and must not be strong-chained (a root chain plus an escaping value's
    /// reach-set pin is the region↔value `Rc` cycle the frame design excludes). The owner answers
    /// its own tier, so the two outcomes stay distinct: the `expect` reports a **dead owner**, which
    /// is a bug, while `None` reports the eternal-tier **policy**.
    pub(crate) fn parent_frame_pin(&self) -> Option<Rc<FrameStorage>> {
        let owner = self
            .region_owner
            .upgrade()
            .expect("a live scope reference implies a live region owner");
        (!owner.is_eternal()).then_some(owner)
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
    /// scope's region routes (`scope.brand().alloc_scalar(…)`). Inherited unchanged by same-region
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

    pub fn child_for_call(&'a self) -> Scope<'a> {
        Self::child_under(self)
    }

    /// The mutable run scope: the direct child of the immutable run-global root. Unlike the
    /// generic [`Self::child_under`] — which copies the parent's *own* `root` handle — this stamps
    /// `root` to `run_root` itself, because the run-global root carries no `root` of its own
    /// (`root: None` marks "I am the root"). The only caller is `unseeded_scopes`, which holds the
    /// root as a genuine `&'a`.
    pub fn run_child(run_root: &'a Scope<'a>) -> Scope<'a> {
        let mut child = Self::child_under(run_root);
        child.root = Some(run_root);
        child
    }

    /// Shared skeleton for a **same-region** child of `outer`: inherits `outer`'s region, its
    /// `region_owner`, and its `root` handle, and takes a fresh id. The five public same-region
    /// constructors below differ only in what they pass here — the binding storage and the kind
    /// stamp (which carries its own payload) — so the inherit-from-`outer` field set lives in one
    /// place. (The two cross-region constructors, [`Self::run_root`] and
    /// [`Self::child_for_frame_witnessed`], do not route this: they set `root`/`region`/
    /// `region_owner` from a fresh frame, not from `outer`.)
    fn child_inheriting(
        outer: &'a Scope<'a>,
        bindings: ScopeBindings<'a>,
        kind: ScopeKind<'a>,
    ) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            root: outer.root,
            bindings,
            out: RefCell::new(None),
            brand: outer.brand,
            region_owner: outer.region_owner.clone(),
            id: ScopeId::next(),
            kind,
            closed: Cell::new(false),
        }
    }

    /// `outer` is the lexical parent — for FN bodies the captured definition scope,
    /// not the call site.
    pub fn child_under(outer: &'a Scope<'a>) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Owned(Bindings::new()),
            ScopeKind::Anonymous,
        )
    }

    /// Per-call frame child built **witnessed**, at the construction-door brand `'a`. The lexical
    /// parent and the fresh region arrive already coupled at one generative `'a` — the door
    /// ([`build_frame_child_witnessed`](crate::machine::core::arena::frame::build_frame_child_witnessed)) brands them
    /// together — so every field stores by plain coercion, honouring `Scope`'s invariance with no
    /// retype of its own. The door is the only caller; the brand `'a` is un-nameable and the result
    /// erases witness-less, so nothing at the brand escapes. The frame `Rc` pins the real parent (via
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
            kind: ScopeKind::Anonymous,
            closed: Cell::new(false),
        }
    }

    /// `child_under`, stamped as a SIG decl_scope with an empty VAL slot collector.
    pub fn child_under_sig(outer: &'a Scope<'a>, name: String) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Owned(Bindings::new()),
            ScopeKind::Sig {
                name,
                slots: RefCell::new(HashMap::new()),
            },
        )
    }

    /// `child_under`, stamped as a MODULE body (also used for the per-ascription view
    /// minted by `:|`).
    pub fn child_under_module(outer: &'a Scope<'a>, name: String) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Owned(Bindings::new()),
            ScopeKind::Module { name, group: None },
        )
    }

    /// `child_under_module`, carrying the [`OperatorGroup`] whose members the body declares —
    /// a `GROUP` body. The kind stays `Module` (a group *is* a module: it binds a module value
    /// and `USING` opens it), and the group record is what
    /// [`Self::nearest_group_context`] hands back to the `OP` declarations inside.
    pub fn child_under_group(
        outer: &'a Scope<'a>,
        name: String,
        group: &'a OperatorGroup<'a>,
    ) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Owned(Bindings::new()),
            ScopeKind::Module {
                name,
                group: Some(group),
            },
        )
    }

    /// Child scope for a `RECURSIVE TYPES` block body: carries the open
    /// [`RecursiveGroupWindow`] whose members are co-declared. Members dispatch against this
    /// scope, so the elaborator finds the window (a member name lowers to its sibling handle).
    /// `outer` is the lexical parent; the sealed members are mirrored up into it at the block's
    /// dep-finish, which is also the window's seal barrier.
    pub fn child_recursive_group(
        outer: &'a Scope<'a>,
        window: Rc<RecursiveGroupWindow>,
    ) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Owned(Bindings::new()),
            ScopeKind::RecursiveBlock { window },
        )
    }

    /// The open [`RecursiveGroupWindow`] of the nearest enclosing `RECURSIVE TYPES` block, if any.
    /// The elaborator consults this to decide whether a bare leaf is a co-declared member: only
    /// the *nearest* window is considered, so a reference to an outer block's member falls through
    /// to ordinary resolution (that member's sealed handle), not a back-edge into the inner
    /// window.
    pub fn nearest_recursive_window(&self) -> Option<Rc<RecursiveGroupWindow>> {
        self.ancestors().find_map(|s| match &s.kind {
            ScopeKind::RecursiveBlock { window } => Some(Rc::clone(window)),
            ScopeKind::Root
            | ScopeKind::Anonymous
            | ScopeKind::Sig { .. }
            | ScopeKind::Module { .. } => None,
        })
    }

    /// Transparent `USING … SCOPE` child scope. `outer` is the call site (the lexical
    /// parent, not the opened module's def site); bindings are a read-only window onto
    /// `module_bindings`. Reads consult the window first then walk `outer`; writes
    /// forward to `outer`. `region` is `outer.region` so block-body allocations outlive
    /// the block (forwarded binds are sound).
    pub fn child_transparent(outer: &'a Scope<'a>, module_bindings: &'a Bindings) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Borrowed(module_bindings),
            ScopeKind::Anonymous,
        )
    }

    pub fn bindings(&self) -> &Bindings {
        self.bindings.get()
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

    /// The nearest **opaque** scope — `self` or the first `Sig` / `Module` ancestor; `Root`,
    /// `Anonymous` and `RecursiveBlock` frames are transparent. The single home of the opacity
    /// classification: the SIG-body gate, the group-context read and the VAL-slot door
    /// ([`Self::write_sig_slot`]) all pivot on which opaque kind this finds, each reading the
    /// result's `kind` rather than re-walking.
    pub(crate) fn nearest_opaque(&self) -> Option<&Scope<'a>> {
        self.ancestors().find(|s| match &s.kind {
            ScopeKind::Sig { .. } | ScopeKind::Module { .. } => true,
            ScopeKind::Root | ScopeKind::Anonymous | ScopeKind::RecursiveBlock { .. } => false,
        })
    }

    /// True iff the nearest opaque enclosing scope is a SIG decl_scope.
    pub fn is_in_sig_body(&self) -> bool {
        matches!(
            self.nearest_opaque().map(|s| &s.kind),
            Some(ScopeKind::Sig { .. })
        )
    }

    /// The [`OperatorGroup`] whose body this scope sits in, if any — the context an `OP`
    /// declaration reads to know it is a group member (its registry write belongs to the
    /// group, and a heterogeneous `->` is admissible only under a pairwise mode). A group body
    /// *is* a `Module { group: Some }`, so it answers on the nearest opaque scope — a `Sig` or a
    /// plain `Module` (a group-less module nested inside a group body included) answers `None`.
    ///
    /// A bare field read: the payload is already at the scope's own `'a`, so the answer is confined
    /// to the region hosting the group scope by the borrow checker alone.
    pub fn nearest_group_context(&self) -> Option<&'a OperatorGroup<'a>> {
        match self.nearest_opaque().map(|s| &s.kind) {
            Some(ScopeKind::Module {
                group: Some(group), ..
            }) => Some(group),
            _ => None,
        }
    }

    /// Snapshot of every `(name, declared type)` slot pair — the schema projection's read. Empty
    /// for any scope that is not a SIG decl_scope, which is the only kind carrying slots.
    pub(crate) fn sig_value_slots(&self) -> Vec<(String, KType)> {
        match &self.kind {
            ScopeKind::Sig { slots, .. } => slots
                .borrow()
                .iter()
                .map(|(name, kt)| (name.clone(), *kt))
                .collect(),
            ScopeKind::Root
            | ScopeKind::Anonymous
            | ScopeKind::Module { .. }
            | ScopeKind::RecursiveBlock { .. } => Vec::new(),
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
}
