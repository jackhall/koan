//! The Koan instantiation of the generic [`Region`](super::region::Region)
//! storage substrate: `KoanRegion = Region<KoanStorageProfile>`, the per-family
//! [`Stored`](super::region::Stored) impls (which sub-arena a family lands in, its cycle-gate
//! `anchors_to` answer), the cycle-gate walkers, and the Koan-typed `alloc_*` wrappers. `CallFrame`
//! — the per-call frame shell over a refcounted `FrameStorage` (the `KoanRegion` plus the ancestor
//! chain), holding the child `Scope` and resetting in place for TCO — also lives here.
//!
//! The generic erase-store engine and the cycle-redirect plumbing live in
//! [`super::region`]; this file supplies the Koan policy it runs.
//!
//! See [per-call-arena-protocol.md](../../../design/per-call-arena-protocol.md) for the carrier
//! set, lift-time anchor decision, cycle gate, ancestor chain, and TCO frame reuse;
//! [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
//! for the heap-pinning / drop-order invariants.

use std::ptr::NonNull;
use std::rc::Rc;

use typed_arena::Arena;

use super::reattach::pin_deref;
use super::scope::Scope;
use super::scope_ptr::{ScopeFamily, ScopePtr};
use super::region::{Region, StorageProfile, Stored};
use crate::machine::core::kfunction::KFunction;
use crate::machine::model::operators::OperatorGroup;
use crate::machine::model::types::KType;
use crate::machine::model::values::{Held, KObject, Module, ModuleSignature};
use crate::scheduler::{reattach_ref, Reattachable};

/// The Koan storage bundle: one typed sub-arena per stored family. Each sub-arena stores the
/// family's `'static` form (phantom); the [`Region`] engine re-anchors to the caller's `'a`
/// on the way out. The `KType` region backs per-type identity binding storage (`Bindings::types`);
/// the `OperatorGroup` region backs the per-scope operator registry (`Bindings::operators`).
#[derive(Default)]
pub struct KoanStorage {
    objects: Arena<KObject<'static>>,
    functions: Arena<KFunction<'static>>,
    scopes: Arena<Scope<'static>>,
    modules: Arena<Module<'static>>,
    signatures: Arena<ModuleSignature<'static>>,
    ktypes: Arena<KType<'static>>,
    operator_groups: Arena<OperatorGroup>,
}

/// The Koan workload: binds the generic [`Region`] to the Koan family set.
pub struct KoanStorageProfile;

impl StorageProfile for KoanStorageProfile {
    type Storage = KoanStorage;
}

/// Run-lifetime allocator. A [`Region`] carrying the Koan family set; lives for one program
/// run. The `KoanRegion` references across the tree and the `Rc<CallFrame>` back-edge ride this
/// alias unchanged.
pub type KoanRegion = Region<KoanStorageProfile>;

/// True iff any descendant of `obj` carries an `Rc<FrameStorage>` whose backing `KoanRegion`
/// is `region_ptr`. Walks the composite shapes mirrored from `KObject::deep_clone`
/// (`List`/`Dict`/`Tagged`/`Struct`) plus `KFunction`/`KFuture` anchors.
fn rc_targets(rc: &Rc<FrameStorage>, region_ptr: *const KoanRegion) -> bool {
    // `rc.region()` coerces `&KoanRegion → *const` for the address compare — no explicit raw
    // cast, so the borrow stays lifetime-bounded right up to the comparison.
    std::ptr::eq(rc.region(), region_ptr)
}

fn obj_anchors_to(obj: &KObject<'_>, region_ptr: *const KoanRegion) -> bool {
    match obj {
        KObject::KFunction(_, Some(rc)) => rc_targets(rc, region_ptr),
        KObject::KFuture(_, Some(rc)) => rc_targets(rc, region_ptr),
        KObject::List(items, _) => items.iter().any(|x| held_anchors_to(x, region_ptr)),
        KObject::Dict(entries, _, _) => entries.values().any(|x| held_anchors_to(x, region_ptr)),
        KObject::Tagged { value, .. } => obj_anchors_to(value, region_ptr),
        KObject::Wrapped { inner, .. } => obj_anchors_to(inner.get(), region_ptr),
        KObject::Record(values, _) => values.iter().any(|(_, x)| held_anchors_to(x, region_ptr)),
        _ => false,
    }
}

/// An aggregate cell anchors to `region_ptr` iff its `Object` arm does, or its `Type` arm is
/// a `Module` whose frame `Rc` backs that region.
fn held_anchors_to(cell: &Held<'_>, region_ptr: *const KoanRegion) -> bool {
    match cell {
        Held::Object(o) => obj_anchors_to(o, region_ptr),
        Held::Type(t) => ktype_anchors_to(t, region_ptr),
    }
}

fn ktype_anchors_to(t: &KType<'_>, region_ptr: *const KoanRegion) -> bool {
    match t {
        KType::Module {
            frame: Some(rc), ..
        } => rc_targets(rc, region_ptr),
        _ => false,
    }
}

// The lifetime family of each stored type, keyed on its `'static` form — the GAT the
// `Region` engine erases to `'static` for storage and re-anchors to the caller's `'a` on read.
// SAFETY: each family is one type generic only in a single lifetime, so its layout is identical for
// every choice of that lifetime; `OperatorGroup` is lifetime-free, trivially invariant.
unsafe impl Reattachable for KObject<'static> {
    type At<'r> = KObject<'r>;
}
unsafe impl Reattachable for KType<'static> {
    type At<'r> = KType<'r>;
}
unsafe impl Reattachable for KFunction<'static> {
    type At<'r> = KFunction<'r>;
}
unsafe impl Reattachable for Scope<'static> {
    type At<'r> = Scope<'r>;
}
unsafe impl Reattachable for Module<'static> {
    type At<'r> = Module<'r>;
}
unsafe impl Reattachable for ModuleSignature<'static> {
    type At<'r> = ModuleSignature<'r>;
}
unsafe impl Reattachable for OperatorGroup {
    type At<'r> = OperatorGroup;
}

// Per-family `Stored` policy. `KObject` and `KType` answer `anchors_to` by walking their composite
// tree; the families that cannot hold a self-targeting `Rc<FrameStorage>` declare `anchors_to => false`,
// so the cycle redirect is uniform across the whole allocation surface. `OperatorGroup` is
// lifetime-free and anchor-free, but routes the same engine for one uniform path.

impl Stored<KoanStorageProfile> for KObject<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<KObject<'static>> {
        &s.objects
    }
    fn anchors_to(value: &KObject<'_>, region_ptr: *const KoanRegion) -> bool {
        obj_anchors_to(value, region_ptr)
    }
    fn record_local(frame: &KoanRegion, stored: &KObject<'static>) {
        frame.record_addr(stored as *const _ as usize);
    }
}

impl Stored<KoanStorageProfile> for KType<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<KType<'static>> {
        &s.ktypes
    }
    fn anchors_to(value: &KType<'_>, region_ptr: *const KoanRegion) -> bool {
        ktype_anchors_to(value, region_ptr)
    }
}

impl Stored<KoanStorageProfile> for KFunction<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<KFunction<'static>> {
        &s.functions
    }
    fn anchors_to(_value: &KFunction<'_>, _region_ptr: *const KoanRegion) -> bool {
        false
    }
}

impl Stored<KoanStorageProfile> for Scope<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<Scope<'static>> {
        &s.scopes
    }
    fn anchors_to(_value: &Scope<'_>, _region_ptr: *const KoanRegion) -> bool {
        false
    }
}

impl Stored<KoanStorageProfile> for Module<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<Module<'static>> {
        &s.modules
    }
    fn anchors_to(_value: &Module<'_>, _region_ptr: *const KoanRegion) -> bool {
        false
    }
}

impl Stored<KoanStorageProfile> for ModuleSignature<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<ModuleSignature<'static>> {
        &s.signatures
    }
    fn anchors_to(_value: &ModuleSignature<'_>, _region_ptr: *const KoanRegion) -> bool {
        false
    }
}

impl Stored<KoanStorageProfile> for OperatorGroup {
    fn sub_arena(s: &KoanStorage) -> &Arena<OperatorGroup> {
        &s.operator_groups
    }
    fn anchors_to(_value: &OperatorGroup, _region_ptr: *const KoanRegion) -> bool {
        false
    }
}

/// Koan-typed allocation surface on the run-lifetime region. Each wrapper routes the single
/// [`Region::alloc`] engine, which runs the cycle gate; these named wrappers are the public
/// entry points.
impl Region<KoanStorageProfile> {
    /// Store a [`KObject`] into the run-lifetime region, routing through the cycle gate (a
    /// self-anchoring value redirects to the escape region).
    pub fn alloc_object<'a>(&'a self, o: KObject<'a>) -> &'a KObject<'a> {
        self.alloc::<KObject<'static>>(o)
    }

    /// Store a [`KType`] into the run-lifetime region, routing through the cycle gate (a `Module`
    /// frame anchoring back at `self` redirects to the escape region).
    pub fn alloc_ktype<'a>(&'a self, t: KType<'a>) -> &'a KType<'a> {
        self.alloc::<KType<'static>>(t)
    }

    /// INVARIANT: a `KFunction` must be allocated into the same `KoanRegion` that owns its
    /// captured scope. The `functions_is_empty` fast path relies on this — without the
    /// invariant, "no KFunction allocated here" no longer implies "no KFunction has
    /// `captured_scope` in this region," and the path silently drops arenas out from under
    /// live `&KFunction` references. The `debug_assert!` catches violations at the
    /// allocation site rather than later as use-after-free.
    pub fn alloc_function<'a>(&'a self, f: KFunction<'a>) -> &'a KFunction<'a> {
        debug_assert!(
            std::ptr::eq(
                self as *const KoanRegion,
                f.captured_scope().region as *const KoanRegion
            ),
            "alloc_function invariant :KFunction must be allocated into the same KoanRegion \
             that owns its captured scope"
        );
        self.alloc::<KFunction<'static>>(f)
    }

    pub fn alloc_scope<'a>(&'a self, s: Scope<'a>) -> &'a Scope<'a> {
        self.alloc::<Scope<'static>>(s)
    }

    pub fn alloc_module<'a>(&'a self, m: Module<'a>) -> &'a Module<'a> {
        self.alloc::<Module<'static>>(m)
    }

    pub fn alloc_signature<'a>(&'a self, s: ModuleSignature<'a>) -> &'a ModuleSignature<'a> {
        self.alloc::<ModuleSignature<'static>>(s)
    }

    /// Allocate an [`OperatorGroup`]. Lifetime-free and anchor-free, so the gate is a no-op, but it
    /// routes the same engine for a single uniform allocation path.
    pub fn alloc_operator_group(&self, g: OperatorGroup) -> &OperatorGroup {
        self.alloc::<OperatorGroup>(g)
    }

    /// Whether `ptr` was returned by a prior `alloc_object` on this region.
    pub fn owns_object<'a>(&self, ptr: *const KObject<'a>) -> bool {
        // `KObject` is invariant in `'a`, so the through-`'static` cast is required despite
        // clippy's complaint.
        #[allow(clippy::unnecessary_cast)]
        let target = ptr as *const KObject<'static> as usize;
        self.owns_addr(target)
    }

    /// When true, no value can hold a `&KFunction` pointing into this region — see the
    /// `alloc_function` invariant.
    pub fn functions_is_empty(&self) -> bool {
        self.family_len::<KFunction<'static>>() == 0
    }
}

#[cfg(test)]
impl Region<KoanStorageProfile> {
    /// Total number of values stored across all seven sub-arenas (test-only). Each `alloc_*`
    /// writes to exactly one sub-arena, so this is the precise allocation count without
    /// double-counting.
    pub fn alloc_count(&self) -> usize {
        self.family_len::<KObject<'static>>()
            + self.family_len::<KFunction<'static>>()
            + self.family_len::<Scope<'static>>()
            + self.family_len::<Module<'static>>()
            + self.family_len::<ModuleSignature<'static>>()
            + self.family_len::<KType<'static>>()
            + self.family_len::<OperatorGroup>()
    }
}

/// A frame's refcounted storage: the per-call `KoanRegion` plus the `outer` link that keeps
/// the lexical-ancestor frames' storage alive. An escaping value (a returned closure, a module
/// frame) pins *this* — not the [`CallFrame`] shell — so the shell stays uniquely owned and the
/// scheduler can reuse it for the next tail iteration while the escapee's captured environment
/// rides the old `FrameStorage` it still holds. Field order is load-bearing: `region` drops
/// before `outer`, so inner pointers die before the outer storage they may reference.
pub struct FrameStorage {
    region: KoanRegion,
    /// Liveness pin only — held so the ancestor frames' storage outlives this child's `outer`
    /// scope pointer, dropped (never read) when this storage drops. The drop *is* the use.
    #[allow(dead_code)]
    outer: Option<Rc<FrameStorage>>,
}

impl FrameStorage {
    /// The backing `KoanRegion`. Used for cycle-gate / lift identity comparisons by holders
    /// that pin storage but never name a `CallFrame`.
    pub(crate) fn region(&self) -> &KoanRegion {
        &self.region
    }
}

/// One user-fn call's allocation frame: a thin shell over a refcounted [`FrameStorage`]. `Rc`-pinned
/// so the scheduler manages the frame by `Rc<CallFrame>`; an escaping closure extends only the
/// *storage* (via [`Self::storage_rc`]), not the shell, so tail reuse can reset the shell's storage
/// without foreclosing on the escapee. Field order is load-bearing: `storage` drops before
/// `scope_ptr`, so the region tears down before the now-dangling child pointer.
///
/// See [per-call-arena-protocol.md](../../../design/per-call-arena-protocol.md) for the
/// carrier set, lift-time anchor decision, cycle gate, ancestor chain, and TCO
/// frame reuse; [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
/// for the heap-pinning / drop-order invariants.
pub struct CallFrame {
    storage: Rc<FrameStorage>,
    scope_ptr: Option<ScopePtr<'static>>,
    /// True only for the scheduler-owned run frame, which carries the top-level run scope and
    /// never drops mid-run. Its `region` is empty (top-level values live in the externally-owned
    /// run region, reached via `scope.region`), so there is nothing to lift out of it: the Done
    /// boundary skips the lift for a non-dying frame (lift exists to rescue values from a *dying*
    /// per-call region). Every per-call frame is `false`.
    non_dying: bool,
}

impl CallFrame {
    /// Build a fresh per-call frame whose child `Scope` uses `outer` as its `outer` link.
    /// `outer_frame` must hold the parent frame's `FrameStorage` Rc when the parent is per-call;
    /// `None` when the parent is run-root.
    pub fn new(outer: &Scope<'_>, outer_frame: Option<Rc<FrameStorage>>) -> Rc<CallFrame> {
        let escape = NonNull::from(outer.region);
        // The region is born inside its own `Rc<FrameStorage>`, heap-pinned from this point on, so
        // the child-scope pointer below stays valid as the storage Rc moves into the shell.
        let storage = Rc::new(FrameStorage {
            region: KoanRegion::with_escape(escape),
            outer: outer_frame,
        });
        let region_ptr: *const KoanRegion = &storage.region;
        // SAFETY: heap-pinning keeps `region_ptr` valid for the storage Rc's lifetime, which exceeds
        // this function's duration; `outer` lives long enough by caller contract.
        let region_ref: &'static KoanRegion = unsafe { pin_deref(region_ptr) };
        // SAFETY: lexical-scoping invariant — `outer` (the captured definition scope, or a
        // longer-lived ancestor) outlives this frame, so erasing its lifetime to `'static`
        // for the child's `outer` link is sound; the child borrow is re-anchored on read.
        let outer_static: &Scope<'static> = unsafe { reattach_ref::<ScopeFamily>(outer) };
        let mut child = Scope::child_under(outer_static);
        // `child_under` defaults `region` to `outer.region`; override to the per-call region.
        child.region = region_ref;
        // `region_ref` is `&'static` (the `pin_deref` above is where the `'static` claim
        // originates), so `alloc_scope` returns `&'static Scope<'static>` and the safe `erase`
        // yields a `ScopePtr<'static>` — no fabrication here.
        let allocated: &'static Scope<'static> = region_ref.alloc_scope(child);
        Rc::new(CallFrame {
            storage,
            scope_ptr: Some(ScopePtr::erase(allocated)),
            non_dying: false,
        })
    }

    /// The scheduler-owned **run frame**: a frame that *carries an already-built run scope*
    /// rather than minting a child. Top-level execution runs against this frame so `active_frame`
    /// is never `None`, which makes a body's re-dispatch-against-its-own-scope uniformly framed
    /// (Yoked) at every depth — top level included. The run scope keeps its own (run) region, so
    /// this frame's `region` stays empty and unused; `escape` is `None` (a non-dying top frame has
    /// nothing to redirect into). Marked `non_dying` so the Done boundary skips the (pointless)
    /// self-lift of top-level results.
    ///
    /// SAFETY: the adopted run scope lives in the externally-owned run region, which outlives this
    /// scheduler-owned frame; erasing its borrow to `'static` for storage in `scope_ptr` is the
    /// same re-anchored-on-read erasure every [`ScopePtr`] carries.
    pub fn adopting(scope: &Scope<'_>) -> Rc<CallFrame> {
        let scope_static: &'static Scope<'static> = unsafe { reattach_ref::<ScopeFamily>(scope) };
        Rc::new(CallFrame {
            storage: Rc::new(FrameStorage {
                region: KoanRegion::new(),
                outer: None,
            }),
            scope_ptr: Some(ScopePtr::erase(scope_static)),
            non_dying: true,
        })
    }

    /// True only for the scheduler-owned run frame (see [`Self::adopting`]). The Done boundary
    /// reads this to skip the self-lift that a never-dying frame would otherwise perform.
    pub fn non_dying(&self) -> bool {
        self.non_dying
    }

    pub fn scope<'a>(&'a self) -> &'a Scope<'a> {
        // SAFETY: `scope_ptr` stores a `ScopePtr<'static>`; the free-`'a` fabrication is
        // concentrated here at the non-generic `CallFrame` boundary. `scope_ptr` is `Some`
        // after construction and stable for the `Rc`'s lifetime (heap-pinned), and the
        // returned `'a` is bounded by `&self`, so the fabricated lifetime cannot outlive the
        // pointee. `'a` is driven by the return-type annotation — `reattach_unbounded`'s
        // lifetime is late-bound, so it cannot be a turbofish argument.
        let scope: &'a Scope<'a> = unsafe { self.scope_ptr_set().reattach_unbounded() };
        scope
    }

    /// Scope handle bounded by `&'step Rc<Self>` — strictly shorter than the `&'a Scope<'a>`
    /// claim of [`CallFrame::scope`]. Use this for local-bind plumbing (e.g.
    /// [`Scope::bind_value`]) that does not need to escape the `Rc`'s borrow, so the caller
    /// avoids an `unsafe` `'a`-anchoring transmute on the receiving end.
    ///
    /// SAFETY: `scope_ptr` stores a `ScopePtr<'static>`; the free-`'step` fabrication is
    /// concentrated here at the non-generic `CallFrame` boundary. The pointer is stable for
    /// the `Rc`'s lifetime (heap-pinned by `Rc`), and the returned `'step` is bounded by the
    /// receiver so the borrow cannot outlive it. `'step` is driven by the return-type annotation
    /// — `reattach_unbounded`'s lifetime is late-bound, so it cannot be a turbofish argument.
    pub fn scope_for_bind<'step>(self: &'step Rc<Self>) -> &'step Scope<'step> {
        let scope: &'step Scope<'step> = unsafe { self.scope_ptr_set().reattach_unbounded() };
        scope
    }

    /// The child scope re-handed with a **witness-bounded** borrow: the borrow `'step` is bounded by
    /// the `&'step Rc<Self>` receiver (the frame `Rc` witness), while the scope content `'a` is free
    /// (`'a: 'step`). This is the read boundary: it hands back a reference that *cannot outlive the
    /// `Rc` it borrows from*, so storing it past the frame is a compile error rather than a
    /// fabrication. Invariance in `'a` rides structurally on the returned `Scope<'a>` (`Scope` is
    /// invariant), so this ephemeral form needs no separate brand struct. Reached through the
    /// scheduler's workload-side scope re-anchor (`reattach_node_scope`, `Yoked` slots) and
    /// [`Self::with_frame_interior`] (the seed binds).
    ///
    /// SAFETY: delegates to [`ScopePtr::reattach_bounded`]; the `&'step Rc<Self>` receiver pins
    /// the region and child scope for all of `'step`, so the `'step`-bounded borrow cannot dangle.
    pub fn scope_bounded<'step, 'a: 'step>(self: &'step Rc<Self>) -> &'step Scope<'a> {
        unsafe { self.scope_ptr_set().reattach_bounded() }
    }

    /// The child scope's `ScopePtr<'static>`, which is `Some` for the whole life of a
    /// constructed frame (`None` only transiently inside `new` / `try_reset_for_tail` before
    /// the child scope is allocated).
    fn scope_ptr_set(&self) -> &ScopePtr<'static> {
        self.scope_ptr
            .as_ref()
            .expect("scope_ptr is set after construction")
    }

    /// Run `f` with this frame's per-call region re-exposed at a free `'a` and its child scope
    /// re-handed at a bounded borrow. The single audited home for the *seed-side* re-anchor: the
    /// MATCH / TRY arm and `KFunction::invoke` body seeds bind their `it` / parameters — values
    /// whose type carries the caller's `'a`, deep-cloned into this frame's region — into the child
    /// scope inside `f`.
    ///
    /// The **region** is re-exposed at a free `'a`: this is the inherent region re-exposure the C0
    /// verdict keeps (an `'a`-typed value must land in an `'a`-typed region, and no lifetime scheme
    /// closes that — the frame `Rc` the caller holds heap-pins the region, so the seed's binds
    /// outlive `f`). The **child scope** rides the bounded `scope_bounded` brand — borrow capped at
    /// the `&Rc` receiver, content `'a` — so it is *not* fabricated free; `bind_value` matches on
    /// the `'a` content.
    ///
    /// SAFETY (region re-borrow): the caller holds this frame's `Rc`, which heap-pins the region for
    /// as long as any value `f` binds into the scope lives.
    pub fn with_frame_interior<'a, R>(
        self: &Rc<Self>,
        f: impl FnOnce(&'a KoanRegion, &Scope<'a>) -> R,
    ) -> R {
        // SAFETY: the held frame `Rc` heap-pins the region for all of `'a`, so re-borrowing the
        // stable region pointer at `'a` is sound. This is a pointer re-borrow, not a value retype, so
        // it routes the audited `pin_deref` rather than a bespoke `transmute`. `'a` is driven by the
        // closure's parameter type, not a turbofish argument.
        let region: &'a KoanRegion = unsafe { pin_deref(self.region() as *const KoanRegion) };
        f(region, self.scope_bounded())
    }

    pub fn region(&self) -> &KoanRegion {
        &self.storage.region
    }

    /// Clone this frame's `FrameStorage` Rc — the handle an escaping value (a returned closure, a
    /// module frame) pins to keep its captured environment alive *without* pinning the shell, so
    /// tail reuse stays free to reset the shell.
    pub fn storage_rc(&self) -> Rc<FrameStorage> {
        Rc::clone(&self.storage)
    }

    /// Reset this frame for a tail-call iteration: install a fresh `FrameStorage` (a new
    /// `KoanRegion` escaping into `new_outer.region`, no `outer` link) and re-allocate the child
    /// `Scope` under `new_outer`. The old `FrameStorage` is dropped here — and its region with it —
    /// *unless* an escaped value still holds it, in which case that snapshot lives on independently
    /// while the shell reuses. Returns `false` (untouched) only when `Rc::get_mut` fails — another
    /// live `Rc<CallFrame>` (a shell clone, never an escape) foreclosing in-place reuse. See
    /// [per-call-arena-protocol.md § TCO frame reuse](../../../design/per-call-arena-protocol.md#tco-frame-reuse).
    pub fn try_reset_for_tail(self: &mut Rc<Self>, new_outer: &Scope<'_>) -> bool {
        if Rc::get_mut(self).is_none() {
            return false;
        }
        let escape = NonNull::from(new_outer.region);
        // SAFETY: lexical-scoping invariant — `new_outer.region` outlives this frame
        // (it is the captured definition scope's region, or a longer-lived ancestor).
        let outer_static: &Scope<'static> = unsafe { reattach_ref::<ScopeFamily>(new_outer) };
        // Build the fresh storage and its child scope before touching the shell, so the
        // region pointer is heap-pinned by the new storage Rc when it lands in the shell.
        let storage = Rc::new(FrameStorage {
            region: KoanRegion::with_escape(escape),
            outer: None,
        });
        let region_ptr: *const KoanRegion = &storage.region;
        // SAFETY: heap-pinned via the storage Rc; pointer is stable for its lifetime.
        let region_ref: &'static KoanRegion = unsafe { pin_deref(region_ptr) };
        let mut child = Scope::child_under(outer_static);
        child.region = region_ref;
        // `region_ref` is `&'static` (the `pin_deref` above is where the `'static` claim
        // originates), so `alloc_scope` returns `&'static Scope<'static>` and the safe `erase`
        // yields a `ScopePtr<'static>` — no fabrication here.
        let allocated: &'static Scope<'static> = region_ref.alloc_scope(child);
        let this = Rc::get_mut(self).expect("just-verified unique above");
        // Drops the old storage (and its region) unless an escapee still holds it.
        this.storage = storage;
        this.scope_ptr = Some(ScopePtr::erase(allocated));
        true
    }
}

#[cfg(test)]
mod tests;
