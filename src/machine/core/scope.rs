use std::cell::{Cell, RefCell};
use std::mem::ManuallyDrop;
use std::rc::{Rc, Weak};

use crate::machine::model::OperatorGroup;
use crate::machine::model::{AnnouncedData, AnnouncedWindow};
use crate::machine::model::{IdentityBuildHasher, KType, TypeSymbol, ValueSymbol};
use crate::witnessed::{And, RegionHandle, SealedExtern};

use super::arena::{FrameStorage, KoanRegion, RegionBrand};
use super::bindings::{Bindings, bump_table};
use super::ref_carriers::{BindingsReferenceFamily, ScopeRefFamily};
use super::scope_id::ScopeId;
use crate::witnessed::BumpBackedMap;

mod copy;
mod reach;
mod registry;
mod resolve;
#[cfg(test)]
mod tests;

pub(crate) use copy::consolidate_object;
pub(crate) use reach::AdoptSeam;
pub(crate) use resolve::HitTier;

/// Lexical environment, resident in its region's **bump**.
///
/// Every field is `Copy`, a [`Cell`] of a `Copy`, or a bump-backed table whose own destructor is
/// suppressed and whose elements are proved glue-free where they are named — so a `Scope` carries
/// **no drop glue at all**, which is what lets it live in a bump that runs no destructor. The
/// `reattachable!` declaration in
/// [`arena`](crate::machine::core::arena) states that as a compile-time assert; the bump doors
/// ([`BumpAllocator::in_place`](crate::witnessed::BumpAllocator::in_place),
/// [`RegionHandle::bump_born_with`]) restate it at each store.
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
    /// The region this scope lives in, held as its [`RegionBrand`] allocation capability — minted at
    /// region-open and inherited by same-region children. Allocation sites reach it through
    /// [`Self::brand`]; identity compares read the bare region through [`Self::region`]. Storing the
    /// brand (not a bare `&KoanRegion`) is what lets a scope hand out the alloc capability without a
    /// forgeable constructor: the no-forgeable-constructor rule is the library's — `RegionBrand` wraps a
    /// `RegionHandle`, whose only public minter is `RegionHandle::from_owner` and whose field and `new`
    /// are crate-private to `workgraph` — so nothing can turn the bare `region()` back into a brand.
    brand: RegionBrand<'a>,
    /// Position-independent origin id, recorded on an `AbstractType` node's `source` so
    /// dispatch on SIG-declared members compares ids rather than scope pointers.
    pub id: ScopeId,
    /// Lexical classification, and with it every per-kind payload — the SIG slot collector, a
    /// `GROUP` body's operator record, a module body's announced type window. Every payload is
    /// reached through the walking accessors below, never off a scope reference directly.
    pub kind: ScopeKind<'a>,
    /// Set once the scope's defining block / frame finishes: no further bind is legal (rebinds are
    /// already rejected; this also rejects *new* binds). The seal point for its reach-set. `Cell`
    /// because it flips once, late, outside the bind hot path.
    closed: Cell<bool>,
}

/// A scope's binding storage. `Owned` is the default. `Borrowed` is the
/// `USING … SCOPE` transparent window: a read-only view onto another scope's
/// façade. A `Borrowed` scope is never a write target — the block's statements run in the owned
/// child stacked inside it, so every write lands in a table its scope owns.
// Boxing `Owned` would add an allocation and an indirection on the hot `bindings()`
// read path; inlining the large variant is the deliberate trade.
#[allow(clippy::large_enum_variant)]
enum ScopeBindings<'a> {
    Owned(Bindings<'a>),
    /// The borrowed façade lives in the opened module's child-scope region;
    /// [`Scope::open_module_window`](crate::machine::core::Scope) keeps that region alive by
    /// minting the module's own delivery envelope's coverage into the call-site region before
    /// building the window that borrows into it, so a window and its root are one act over one
    /// operand.
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
    ///
    /// The collector is bump-backed like the four durable tables and keyed by the same classified
    /// vocabulary — a slot binds a value name, so [`ValueSymbol`] — with a `Copy` digest key and a
    /// `Copy` `KType` value, so its death frees nothing and walks no entry.
    /// It is wrapped exactly as [`Bindings`]' tables are: `ManuallyDrop` suppresses the map's own
    /// destructor, whose only act would be handing a bump-owned bucket array back to an allocator
    /// that frees nothing — and suppressing it is what makes this variant, and with it `Scope`,
    /// contribute no drop glue at all. The element proof the wrapper would otherwise swallow is
    /// stated below.
    Sig {
        name: TypeSymbol,
        slots: RefCell<ManuallyDrop<BumpBackedMap<'a, ValueSymbol, KType, IdentityBuildHasher>>>,
    },
    /// A MODULE body (also the per-ascription view minted by `:|`). `group` is `Some` for a `GROUP`
    /// body — a group *is* a module — naming the one [`OperatorGroup`] record its member `OP`
    /// declarations belong to, read through [`Scope::nearest_group_context`]. A bare
    /// `&'a OperatorGroup<'a>`: the record is same-region with this scope, so it rides the scope's
    /// own brand like every other `'a` field and the reader's answer cannot outlive its scope borrow
    /// by construction — no seal, no pin. The registry entries hold that same record as a sealed
    /// carrier, since a binding table is lifetime-free.
    ///
    /// `window` is the body's [`AnnouncedWindow`]: `Some` when the pre-scan found top-level
    /// `NEWTYPE` / `UNION` declarations, whose names are then mutually visible and order-independent
    /// throughout the body. It rides inline rather than behind a reference — its runs are bumped
    /// into this scope's own region and the record is `Drop`-free, so the scope allocation hosts it
    /// for free. Read through [`Scope::nearest_declaration_window`] (a consumer, walking out) and
    /// [`Scope::own_declaration_window`] (a declarator, this scope only).
    Module {
        group: Option<&'a OperatorGroup<'a>>,
        window: Option<AnnouncedWindow<'a>>,
    },
}

/// The slot collector's element proof, stated against the entry types directly — `needs_drop` is
/// false for *any* `ManuallyDrop<U>`, so the wrapper that suppresses the map's teardown also makes
/// the table's own entry assert say nothing about what it holds. A key is a bumped `&str` and a
/// value a `Copy` [`KType`] handle today; a variant that later brings a destructor back fails the
/// build here rather than leaking silently. The same statement [`Bindings`] makes for its buckets.
const _: () = assert!(!std::mem::needs_drop::<KType>());

impl<'a> Scope<'a> {
    /// The run-global root. Every field is already at the run region's own `'a`, so it is built
    /// there directly — no brand, no crossing operand, nothing to re-anchor.
    fn run_root(brand: RegionBrand<'a>) -> Self {
        Self {
            outer: None,
            root: None,
            bindings: ScopeBindings::Owned(Bindings::new(brand)),
            brand,
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
            .region_owner()
            .upgrade()
            .expect("a live scope reference implies a live region owner");
        (!owner.is_eternal()).then_some(owner)
    }

    /// The [`FrameStorage`] (a cloned `Weak`) whose region this scope lives in — read off the
    /// **region's own** host back-link rather than a copy carried here. A region is born naming its
    /// owner (`Rc::new_cyclic`), so the derivation is total and no constructor can wire it wrong;
    /// the link stays `Weak` because the storage owns the region owns this scope, and an `Rc`
    /// back-edge would leak. Upgrades whenever the region is live.
    pub(crate) fn region_owner(&self) -> Weak<FrameStorage> {
        self.brand.handle().host()
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

    /// The mutable run scope: the direct child of the immutable run-global root. Unlike the
    /// generic [`Self::child_under`] — which copies the parent's *own* `root` handle — this stamps
    /// `root` to `run_root` itself, because the run-global root carries no `root` of its own
    /// (`root: None` marks "I am the root").
    fn run_child(run_root: &'a Scope<'a>) -> Scope<'a> {
        let mut child = Self::child_under(run_root);
        child.root = Some(run_root);
        child
    }

    /// Shared skeleton for a **same-region** child of `outer`: inherits `outer`'s region brand and
    /// its `root` handle, and takes a fresh id. The five public same-region constructors below
    /// differ only in what they pass here — the binding storage and the kind stamp (which carries
    /// its own payload) — so the inherit-from-`outer` field set lives in one place. (The two
    /// cross-region constructors, [`Self::run_root`] and [`Self::child_for_frame_witnessed`], do not
    /// route this: they take their region from a fresh frame, not from `outer`.)
    fn child_inheriting(
        outer: &'a Scope<'a>,
        bindings: ScopeBindings<'a>,
        kind: ScopeKind<'a>,
    ) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            root: outer.root,
            bindings,
            brand: outer.brand,
            id: ScopeId::next(),
            kind,
            closed: Cell::new(false),
        }
    }

    /// `outer` is the lexical parent — for FN bodies the captured definition scope,
    /// not the call site.
    fn child_under(outer: &'a Scope<'a>) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Owned(Bindings::new(outer.brand)),
            ScopeKind::Anonymous,
        )
    }

    /// Per-call frame child built **witnessed**, at the construction-door brand `'a`. The lexical
    /// parent and the fresh region arrive already coupled at one generative `'a` — the door
    /// ([`build_frame_child_witnessed`](crate::machine::core::arena::frame::build_frame_child_witnessed)) brands them
    /// together — so every field stores by plain coercion, honouring `Scope`'s invariance with no
    /// retype of its own. The brand `'a` is un-nameable and the result erases witness-less, so
    /// nothing at the brand escapes. The frame `Rc` pins the real parent (via `FrameStorage.outer`)
    /// and the run-global root, so the coupled references never out-claim a live pointee.
    pub(crate) fn child_for_frame_witnessed(
        outer: &'a Scope<'a>,
        brand: RegionBrand<'a>,
    ) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            root: outer.root,
            bindings: ScopeBindings::Owned(Bindings::new(brand)),
            brand,
            id: ScopeId::next(),
            kind: ScopeKind::Anonymous,
            closed: Cell::new(false),
        }
    }

    /// `child_under`, stamped as a SIG decl_scope with an empty VAL slot collector.
    fn child_under_sig(outer: &'a Scope<'a>, name: TypeSymbol) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Owned(Bindings::new(outer.brand)),
            ScopeKind::Sig {
                name,
                slots: RefCell::new(ManuallyDrop::new(bump_table(outer.brand))),
            },
        )
    }

    /// `child_under`, stamped as a MODULE body (also used for the per-ascription view
    /// minted by `:|`). `announced` is the body's type-declaration pre-scan; its runs are bumped
    /// into this child's own region here, inside the construction door, so the window is live
    /// before any body statement can reach the scope.
    fn child_under_module(outer: &'a Scope<'a>, announced: Option<&AnnouncedData>) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Owned(Bindings::new(outer.brand)),
            ScopeKind::Module {
                group: None,
                window: announced.map(|data| AnnouncedWindow::bump(outer.brand(), data)),
            },
        )
    }

    /// `child_under_module`, carrying the [`OperatorGroup`] whose members the body declares —
    /// a `GROUP` body. The kind stays `Module` (a group *is* a module: it binds a module value
    /// and `USING` opens it), and the group record is what
    /// [`Self::nearest_group_context`] hands back to the `OP` declarations inside.
    fn child_under_group(
        outer: &'a Scope<'a>,
        group: &'a OperatorGroup<'a>,
        announced: Option<&AnnouncedData>,
    ) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Owned(Bindings::new(outer.brand)),
            ScopeKind::Module {
                group: Some(group),
                window: announced.map(|data| AnnouncedWindow::bump(outer.brand(), data)),
            },
        )
    }

    /// The nearest enclosing module body that announced type declarations, and its
    /// [`AnnouncedWindow`]. The elaborator consults this to decide whether a bare leaf names a
    /// co-declared type: only the *nearest* window is considered, so a reference to an outer
    /// module's member falls through to ordinary resolution (that member's sealed handle), not a
    /// back-edge into the inner window. The walk passes through window-less kinds, window-less
    /// modules included.
    ///
    /// The scope comes back beside the window because it is where the group's declaration
    /// placeholders live — what a consumer parked on the group waits for.
    pub fn nearest_declaration_window(&self) -> Option<(&Scope<'a>, &AnnouncedWindow<'a>)> {
        self.ancestors().find_map(|s| match &s.kind {
            ScopeKind::Module {
                window: Some(window),
                ..
            } => Some((s, window)),
            ScopeKind::Root
            | ScopeKind::Anonymous
            | ScopeKind::Sig { .. }
            | ScopeKind::Module { window: None, .. } => None,
        })
    }

    /// This scope's **own** [`AnnouncedWindow`], with no walk — what a declarator consults to
    /// decide whether it is filling an announced slot. Self-only is deliberate: a same-named
    /// declaration nested deeper in the body opens its own singleton instead of hijacking the
    /// announced slot.
    pub fn own_declaration_window(&self) -> Option<&AnnouncedWindow<'a>> {
        match &self.kind {
            ScopeKind::Module { window, .. } => window.as_ref(),
            _ => None,
        }
    }

    /// Transparent `USING … SCOPE` window scope. `outer` is the call site (the lexical
    /// parent, not the opened module's def site); bindings are a read-only window onto
    /// `module_bindings`. It is a middle link: the block's own owned scope stacks inside it, so a
    /// read walks block, then window, then `outer`, and no write ever targets the window. `region`
    /// is `outer.region` so block-body allocations outlive the block.
    fn child_transparent(outer: &'a Scope<'a>, module_bindings: &'a Bindings<'a>) -> Scope<'a> {
        Self::child_inheriting(
            outer,
            ScopeBindings::Borrowed(module_bindings),
            ScopeKind::Anonymous,
        )
    }

    /// Bump a **same-region** child into the region `outer` already lives in. There is no brand and
    /// nothing to re-anchor: every field the constructors inherit — the region brand, the `root`
    /// handle, the parent link — comes off `outer` at its own `'a`, so the child is built at `'a`
    /// and stored at `'a`. Residence is the borrow checker's: the only region reachable through
    /// `outer.brand()` is the one `outer` lives in, so pairing a scope with a foreign region is
    /// unrepresentable here.
    fn bump_child(outer: &'a Scope<'a>, child: Scope<'a>) -> &'a Scope<'a> {
        outer.brand().allocator().in_place(child)
    }

    /// Allocate the **run-global root** into `storage`'s region and hand back the resident scope.
    /// Built directly at `'a` like every same-region child — the run root has no parent to cross and
    /// nothing foreign to embed, so it needs no door at all.
    pub fn alloc_run_root(storage: &'a Rc<FrameStorage>) -> &'a Scope<'a> {
        let brand = RegionBrand(RegionHandle::from_owner(&**storage));
        brand.allocator().in_place(Scope::run_root(brand))
    }

    /// Allocate the mutable run scope — the direct child of the run-global root — into the root's own
    /// region. Unlike [`Self::alloc_child_under`] this stamps `root` to `run_root` itself, because
    /// the run-global root carries no `root` of its own.
    pub fn alloc_run_child(&'a self) -> &'a Scope<'a> {
        Self::bump_child(self, Scope::run_child(self))
    }

    /// Allocate an anonymous same-region child of `self` — the plain block / body scope.
    pub fn alloc_child_under(&'a self) -> &'a Scope<'a> {
        Self::bump_child(self, Scope::child_under(self))
    }

    /// Allocate a same-region child stamped as a SIG decl_scope with an empty VAL slot collector.
    /// `name` is the declaration's own token, so the kind owns no heap of its own.
    pub fn alloc_child_under_sig(&'a self, name: TypeSymbol) -> &'a Scope<'a> {
        Self::bump_child(self, Scope::child_under_sig(self, name))
    }

    /// Allocate a same-region child stamped as a MODULE body (also the per-ascription view `:|`
    /// mints, which announces nothing). `announced` is the body's type-declaration pre-scan, owned
    /// plain data whose runs are bumped into the child's own region here. A body's name is its
    /// declaration's — the binder holds the symbol and the module value carries the path — so the
    /// scope stamp re-homes no spelling of its own.
    pub fn alloc_child_under_module(&'a self, announced: Option<AnnouncedData>) -> &'a Scope<'a> {
        Self::bump_child(self, Scope::child_under_module(self, announced.as_ref()))
    }

    /// Allocate a `GROUP` body: a MODULE-kinded child carrying the [`OperatorGroup`] record its `OP`
    /// declarations belong to. The record is same-region with the scope — it arrives as a plain
    /// `&'a` — so this needs no crossing either.
    pub fn alloc_child_under_group(
        &'a self,
        group: &'a OperatorGroup<'a>,
        announced: Option<AnnouncedData>,
    ) -> &'a Scope<'a> {
        Self::bump_child(
            self,
            Scope::child_under_group(self, group, announced.as_ref()),
        )
    }

    /// Allocate one scope of a **copied environment** into `brand`'s region: a fresh id, empty
    /// owned bindings, and `outer` — the previously copied link, or (for the outermost link) the
    /// source chain's [`innermost_eternal_home`](Self::innermost_eternal_home) referenced verbatim.
    /// The copy engine's construction door ([`copy`](self::copy)).
    ///
    /// Built directly at `'a` like every same-region child, and for the same reason: `outer` and
    /// `brand` arrive already coupled at one lifetime — the engine runs at a relocation fold's own
    /// brand, where the source chain is the fold's operand view and `brand` is the destination — so
    /// there is nothing foreign to re-anchor. What makes the *outermost* link sound is the eternal
    /// tier, exactly as it is for a `CLOSE OVER` block scope: an eternal region outlives everything
    /// that could retain it, so an unpinned `&Scope` outer cannot dangle. The engine asserts that
    /// premise where it relies on it.
    ///
    /// `root` falls out of `outer` the way [`Self::run_child`]'s does: a parent carrying its own
    /// root handle passes it down, and a parent that *is* the root becomes the child's.
    ///
    /// Born **open**, not closed: the engine fills the tables and closes the scope after, so the
    /// bind door's own open-scope assertion holds throughout the fill.
    pub(in crate::machine::core::scope) fn alloc_copied_child(
        outer: &'a Scope<'a>,
        brand: RegionBrand<'a>,
    ) -> &'a Scope<'a> {
        brand.allocator().in_place(Scope {
            outer: Some(outer),
            root: outer.root.or(Some(outer)),
            bindings: ScopeBindings::Owned(Bindings::new(brand)),
            brand,
            id: ScopeId::next(),
            kind: ScopeKind::Anonymous,
            closed: Cell::new(false),
        })
    }

    /// Allocate a transparent `USING … SCOPE` child whose bindings are a read-only window onto
    /// `module_bindings`. The table lives in the **opened module's own region**, so this is one of
    /// the two scope stores with a genuinely foreign operand: it takes the bump's crossing door,
    /// which re-anchors the table to the same `'b` the parent crosses at (branding them
    /// independently is exactly what `Scope`'s invariance rejects). The pin is this scope's own
    /// region.
    ///
    /// Cluster-private (`pub(in crate::machine::core::scope)`): a bare window states no claim on
    /// the region its table lives in, so the visibility holds the call inside this cluster, where
    /// the table is read off a module's own delivery envelope with that envelope's coverage rooted
    /// first.
    pub(in crate::machine::core::scope) fn alloc_child_transparent(
        &'a self,
        module_bindings: SealedExtern<BindingsReferenceFamily>,
    ) -> &'a Scope<'a> {
        self.brand()
            .handle()
            .bump_born_with::<Scope<'static>, And<ScopeRefFamily, BindingsReferenceFamily>, _>(
                SealedExtern::<ScopeRefFamily>::erase(self).zip(module_bindings),
                self.region(),
                |_placement, (outer_b, bindings_b)| Scope::child_transparent(outer_b, bindings_b),
            )
    }

    /// Test fixture: a transparent window onto a bare scope's binding table, for a suite that builds
    /// the opened side as a plain scope rather than a module value in an envelope. The window door
    /// reads the table off the envelope whose coverage it roots; this door states no such claim, so
    /// a fixture reaching it mints the root itself.
    #[cfg(test)]
    pub(crate) fn alloc_transparent_window_for_test(
        &'a self,
        module_bindings: &'a Bindings<'a>,
    ) -> &'a Scope<'a> {
        self.alloc_child_transparent(SealedExtern::<BindingsReferenceFamily>::erase(
            module_bindings,
        ))
    }

    pub fn bindings(&self) -> &Bindings<'a> {
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

    /// The nearest **opaque** scope — `self` or the first `Sig` / `Module` ancestor; `Root` and
    /// `Anonymous` frames are transparent. The single home of the opacity
    /// classification: the SIG-body gate, the group-context read and the VAL-slot door
    /// ([`Self::write_sig_slot`]) all pivot on which opaque kind this finds, each reading the
    /// result's `kind` rather than re-walking.
    pub(crate) fn nearest_opaque(&self) -> Option<&Scope<'a>> {
        self.ancestors().find(|s| match &s.kind {
            ScopeKind::Sig { .. } | ScopeKind::Module { .. } => true,
            ScopeKind::Root | ScopeKind::Anonymous => false,
        })
    }

    /// The innermost enclosing scope whose region is at the **eternal tier** — `self` if it is one,
    /// else the first ancestor [`Self::parent_frame_pin`] declines to chain. Total: every chain ends
    /// at the run root, whose region outlives the run.
    ///
    /// This is the lexical outer a severed block scope takes ([`CLOSE OVER`](crate::builtins)):
    /// builtins and top-level definitions stay visible through it, and an eternal region contributes
    /// no reach, so a value homed in the block pins nothing through the link. Walked over
    /// [`Self::outer`] rather than [`Self::ancestors`] because the answer is stored at `'a` and a
    /// per-call frame is built from a parent at that same lifetime.
    pub(crate) fn innermost_eternal_home(&'a self) -> &'a Scope<'a> {
        let mut scope: &'a Scope<'a> = self;
        loop {
            if scope.parent_frame_pin().is_none() {
                return scope;
            }
            scope = scope
                .outer()
                .expect("a per-call-homed scope has a lexical parent; every chain ends eternal");
        }
    }

    /// Whether this scope's binding storage is a `USING … SCOPE` window rather than its own —
    /// a read-only façade onto another scope's tables, which the environment copy has no
    /// destination for (rebuilding the window would have to rebuild what it looks through, and the
    /// window's own scope binds nothing of its own). Not-ready is the answer here, so a captured
    /// chain holding one downgrades to a pin.
    fn borrows_its_bindings(&self) -> bool {
        self.bindings.is_borrowed()
    }

    /// Whether the environment copy can rebuild **this one scope** at a destination region — the
    /// readiness gate, read as stored facts with no walk over what the scope binds:
    ///
    /// - **Closed** ([`Self::is_closed`]): its defining block has finished, so no further bind is
    ///   legal and the copy cannot miss one. It also settles visibility: a closed scope is named by
    ///   no live call-site chain, so every entry in it reads as visible to every body that captured
    ///   it, and copying the table wholesale under a fresh id reproduces what the source answered.
    /// - **Owned bindings** ([`Self::borrows_its_bindings`]): a `USING` window has nothing of its
    ///   own to rebuild.
    /// - **No standing claim** ([`Bindings::has_no_claims`]): an in-flight binder is a binding that
    ///   does not exist yet, which is exactly the unfinalized binding the roadmap downgrades on.
    /// - **A kind the engine models**: `Anonymous` — the block and per-call frame scopes a closure
    ///   chain is made of — and `Root`, which is eternal and referenced verbatim rather than
    ///   copied. `Sig` carries a live slot collector and `Module` an announced window and group
    ///   record, neither of which the v1 engine rebuilds.
    ///
    /// Never a wait: an unready scope answers `false` and the caller pins. Nothing here can park,
    /// which is what makes the two-in-flight-environments deadlock unconstructible rather than
    /// merely handled.
    pub(crate) fn is_copy_ready(&self) -> bool {
        self.is_closed()
            && !self.borrows_its_bindings()
            && self.bindings().has_no_claims()
            && matches!(self.kind, ScopeKind::Root | ScopeKind::Anonymous)
    }

    /// The **per-call** portion of this scope's chain: `self` and its `outer` ancestors up to but
    /// excluding [`Self::innermost_eternal_home`]. Exactly the scopes an environment copy has to
    /// rebuild — an eternal-homed scope outlives everything that could retain it, so the copy
    /// references it verbatim and the walk stops there. Empty when `self` is itself eternal-homed.
    pub(crate) fn per_call_chain(&'a self) -> impl Iterator<Item = &'a Scope<'a>> {
        let eternal: *const Scope<'a> = self.innermost_eternal_home();
        std::iter::successors(Some(self), |scope| scope.outer())
            .take_while(move |scope| !std::ptr::eq(*scope as *const Scope<'a>, eternal))
    }

    /// Whether every per-call scope in this chain is [`copy-ready`](Self::is_copy_ready). The
    /// chain-level gate the callable escape seam asks before pricing anything: one unready link
    /// pins the whole crossing, because a rebuilt chain missing a link would have to point back
    /// into the source.
    pub(crate) fn chain_is_copy_ready(&'a self) -> bool {
        self.per_call_chain().all(Scope::is_copy_ready)
    }

    /// What rebuilding this chain's per-call portion would cost: the sum of each scope's monotone
    /// [`binding_copy_cost`](Bindings::binding_copy_cost) memo. O(chain depth), every term a
    /// stored read.
    pub(crate) fn chain_copy_cost(&'a self) -> u64 {
        self.per_call_chain()
            .map(|scope| scope.bindings().binding_copy_cost())
            .fold(0, u64::saturating_add)
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
    /// for any scope that is not a SIG decl_scope.
    pub(crate) fn sig_value_slots(&self) -> Vec<(ValueSymbol, KType)> {
        match &self.kind {
            ScopeKind::Sig { slots, .. } => slots
                .borrow()
                .iter()
                .map(|(name, kt)| (*name, *kt))
                .collect(),
            ScopeKind::Root | ScopeKind::Anonymous | ScopeKind::Module { .. } => Vec::new(),
        }
    }
}
