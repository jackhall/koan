//! The Koan instantiation of the generic [`StorageFrame`](super::storage_frame::StorageFrame)
//! storage substrate: `RuntimeArena = StorageFrame<KoanStorageProfile>`, the per-family
//! [`Stored`](super::storage_frame::Stored) impls (which sub-arena a family lands in, its cycle-gate
//! `anchors_to` answer), the cycle-gate walkers, and the Koan-typed `alloc_*` wrappers. `CallArena`
//! — the per-call frame shell over a refcounted `FrameStorage` (the `RuntimeArena` plus the ancestor
//! chain), holding the child `Scope` and resetting in place for TCO — also lives here.
//!
//! The generic erase-store engine and the cycle-redirect plumbing live in
//! [`super::storage_frame`]; this file supplies the Koan policy it runs.
//!
//! See [per-call-arena-protocol.md](../../../design/per-call-arena-protocol.md) for the carrier
//! set, lift-time anchor decision, cycle gate, ancestor chain, and TCO frame reuse;
//! [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#arena-lifetime-erasure)
//! for the heap-pinning / drop-order invariants.

use std::ptr::NonNull;
use std::rc::Rc;

use typed_arena::Arena;

use super::reattach::pin_deref;
use super::scope::Scope;
use super::scope_ptr::{ScopeFamily, ScopePtr};
use super::storage_frame::{StorageFrame, StorageProfile, Stored};
use crate::machine::core::kfunction::KFunction;
use crate::machine::model::operators::OperatorGroup;
use crate::machine::model::types::KType;
use crate::machine::model::values::{Held, KObject, Module, Signature};
use crate::scheduler::{reattach_ref, Reattachable};

/// The Koan storage bundle: one typed sub-arena per stored family. Each sub-arena stores the
/// family's `'static` form (phantom); the [`StorageFrame`] engine re-anchors to the caller's `'a`
/// on the way out. The `KType` arena backs per-type identity binding storage (`Bindings::types`);
/// the `OperatorGroup` arena backs the per-scope operator registry (`Bindings::operators`).
#[derive(Default)]
pub struct KoanStorage {
    objects: Arena<KObject<'static>>,
    functions: Arena<KFunction<'static>>,
    scopes: Arena<Scope<'static>>,
    modules: Arena<Module<'static>>,
    signatures: Arena<Signature<'static>>,
    ktypes: Arena<KType<'static>>,
    operator_groups: Arena<OperatorGroup>,
}

/// The Koan workload: binds the generic [`StorageFrame`] to the Koan family set.
pub struct KoanStorageProfile;

impl StorageProfile for KoanStorageProfile {
    type Storage = KoanStorage;
}

/// Run-lifetime allocator. A [`StorageFrame`] carrying the Koan family set; lives for one program
/// run. The `RuntimeArena` references across the tree and the `Rc<CallArena>` back-edge ride this
/// alias unchanged.
pub type RuntimeArena = StorageFrame<KoanStorageProfile>;

/// True iff any descendant of `obj` carries an `Rc<FrameStorage>` whose backing `RuntimeArena`
/// is `arena_ptr`. Walks the composite shapes mirrored from `KObject::deep_clone`
/// (`List`/`Dict`/`Tagged`/`Struct`) plus `KFunction`/`KFuture` anchors.
fn rc_targets(rc: &Rc<FrameStorage>, arena_ptr: *const RuntimeArena) -> bool {
    // `rc.arena()` coerces `&RuntimeArena → *const` for the address compare — no explicit raw
    // cast, so the borrow stays lifetime-bounded right up to the comparison.
    std::ptr::eq(rc.arena(), arena_ptr)
}

fn obj_anchors_to(obj: &KObject<'_>, arena_ptr: *const RuntimeArena) -> bool {
    match obj {
        KObject::KFunction(_, Some(rc)) => rc_targets(rc, arena_ptr),
        KObject::KFuture(_, Some(rc)) => rc_targets(rc, arena_ptr),
        KObject::List(items, _) => items.iter().any(|x| held_anchors_to(x, arena_ptr)),
        KObject::Dict(entries, _, _) => entries.values().any(|x| held_anchors_to(x, arena_ptr)),
        KObject::Tagged { value, .. } => obj_anchors_to(value, arena_ptr),
        KObject::Wrapped { inner, .. } => obj_anchors_to(inner.get(), arena_ptr),
        KObject::Record(values, _) => values.iter().any(|(_, x)| held_anchors_to(x, arena_ptr)),
        _ => false,
    }
}

/// An aggregate cell anchors to `arena_ptr` iff its `Object` arm does, or its `Type` arm is
/// a `Module` whose frame `Rc` backs that arena.
fn held_anchors_to(cell: &Held<'_>, arena_ptr: *const RuntimeArena) -> bool {
    match cell {
        Held::Object(o) => obj_anchors_to(o, arena_ptr),
        Held::Type(t) => ktype_anchors_to(t, arena_ptr),
    }
}

fn ktype_anchors_to(t: &KType<'_>, arena_ptr: *const RuntimeArena) -> bool {
    match t {
        KType::Module {
            frame: Some(rc), ..
        } => rc_targets(rc, arena_ptr),
        _ => false,
    }
}

// The lifetime family of each stored type, keyed on its `'static` form — the GAT the
// `StorageFrame` engine erases to `'static` for storage and re-anchors to the caller's `'a` on read.
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
unsafe impl Reattachable for Signature<'static> {
    type At<'r> = Signature<'r>;
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
    fn anchors_to(value: &KObject<'_>, arena_ptr: *const RuntimeArena) -> bool {
        obj_anchors_to(value, arena_ptr)
    }
    fn record_local(frame: &RuntimeArena, stored: &KObject<'static>) {
        frame.record_addr(stored as *const _ as usize);
    }
}

impl Stored<KoanStorageProfile> for KType<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<KType<'static>> {
        &s.ktypes
    }
    fn anchors_to(value: &KType<'_>, arena_ptr: *const RuntimeArena) -> bool {
        ktype_anchors_to(value, arena_ptr)
    }
}

impl Stored<KoanStorageProfile> for KFunction<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<KFunction<'static>> {
        &s.functions
    }
    fn anchors_to(_value: &KFunction<'_>, _arena_ptr: *const RuntimeArena) -> bool {
        false
    }
}

impl Stored<KoanStorageProfile> for Scope<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<Scope<'static>> {
        &s.scopes
    }
    fn anchors_to(_value: &Scope<'_>, _arena_ptr: *const RuntimeArena) -> bool {
        false
    }
}

impl Stored<KoanStorageProfile> for Module<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<Module<'static>> {
        &s.modules
    }
    fn anchors_to(_value: &Module<'_>, _arena_ptr: *const RuntimeArena) -> bool {
        false
    }
}

impl Stored<KoanStorageProfile> for Signature<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<Signature<'static>> {
        &s.signatures
    }
    fn anchors_to(_value: &Signature<'_>, _arena_ptr: *const RuntimeArena) -> bool {
        false
    }
}

impl Stored<KoanStorageProfile> for OperatorGroup {
    fn sub_arena(s: &KoanStorage) -> &Arena<OperatorGroup> {
        &s.operator_groups
    }
    fn anchors_to(_value: &OperatorGroup, _arena_ptr: *const RuntimeArena) -> bool {
        false
    }
}

/// Koan-typed allocation surface on the run-lifetime arena. Each wrapper routes the single
/// [`StorageFrame::alloc`] engine, which runs the cycle gate; these named wrappers are the public
/// entry points.
impl StorageFrame<KoanStorageProfile> {
    /// Store a [`KObject`] into the run-lifetime arena, routing through the cycle gate (a
    /// self-anchoring value redirects to the escape arena).
    pub fn alloc_object<'a>(&'a self, o: KObject<'a>) -> &'a KObject<'a> {
        self.alloc::<KObject<'static>>(o)
    }

    /// Store a [`KType`] into the run-lifetime arena, routing through the cycle gate (a `Module`
    /// frame anchoring back at `self` redirects to the escape arena).
    pub fn alloc_ktype<'a>(&'a self, t: KType<'a>) -> &'a KType<'a> {
        self.alloc::<KType<'static>>(t)
    }

    /// INVARIANT: a `KFunction` must be allocated into the same `RuntimeArena` that owns its
    /// captured scope. The `functions_is_empty` fast path relies on this — without the
    /// invariant, "no KFunction allocated here" no longer implies "no KFunction has
    /// `captured_scope` in this arena," and the path silently drops arenas out from under
    /// live `&KFunction` references. The `debug_assert!` catches violations at the
    /// allocation site rather than later as use-after-free.
    pub fn alloc_function<'a>(&'a self, f: KFunction<'a>) -> &'a KFunction<'a> {
        debug_assert!(
            std::ptr::eq(
                self as *const RuntimeArena,
                f.captured_scope().arena as *const RuntimeArena
            ),
            "alloc_function invariant :KFunction must be allocated into the same RuntimeArena \
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

    pub fn alloc_signature<'a>(&'a self, s: Signature<'a>) -> &'a Signature<'a> {
        self.alloc::<Signature<'static>>(s)
    }

    /// Allocate an [`OperatorGroup`]. Lifetime-free and anchor-free, so the gate is a no-op, but it
    /// routes the same engine for a single uniform allocation path.
    pub fn alloc_operator_group(&self, g: OperatorGroup) -> &OperatorGroup {
        self.alloc::<OperatorGroup>(g)
    }

    /// Whether `ptr` was returned by a prior `alloc_object` on this arena.
    pub fn owns_object<'a>(&self, ptr: *const KObject<'a>) -> bool {
        // `KObject` is invariant in `'a`, so the through-`'static` cast is required despite
        // clippy's complaint.
        #[allow(clippy::unnecessary_cast)]
        let target = ptr as *const KObject<'static> as usize;
        self.owns_addr(target)
    }

    /// When true, no value can hold a `&KFunction` pointing into this arena — see the
    /// `alloc_function` invariant.
    pub fn functions_is_empty(&self) -> bool {
        self.family_len::<KFunction<'static>>() == 0
    }
}

#[cfg(test)]
impl StorageFrame<KoanStorageProfile> {
    /// Total number of values stored across all seven sub-arenas (test-only). Each `alloc_*`
    /// writes to exactly one sub-arena, so this is the precise allocation count without
    /// double-counting.
    pub fn alloc_count(&self) -> usize {
        self.family_len::<KObject<'static>>()
            + self.family_len::<KFunction<'static>>()
            + self.family_len::<Scope<'static>>()
            + self.family_len::<Module<'static>>()
            + self.family_len::<Signature<'static>>()
            + self.family_len::<KType<'static>>()
            + self.family_len::<OperatorGroup>()
    }
}

/// A frame's refcounted storage: the per-call `RuntimeArena` plus the `outer` link that keeps
/// the lexical-ancestor frames' storage alive. An escaping value (a returned closure, a module
/// frame) pins *this* — not the [`CallArena`] shell — so the shell stays uniquely owned and the
/// scheduler can reuse it for the next tail iteration while the escapee's captured environment
/// rides the old `FrameStorage` it still holds. Field order is load-bearing: `arena` drops
/// before `outer`, so inner pointers die before the outer storage they may reference.
pub struct FrameStorage {
    arena: RuntimeArena,
    /// Liveness pin only — held so the ancestor frames' storage outlives this child's `outer`
    /// scope pointer, dropped (never read) when this storage drops. The drop *is* the use.
    #[allow(dead_code)]
    outer: Option<Rc<FrameStorage>>,
}

impl FrameStorage {
    /// The backing `RuntimeArena`. Used for cycle-gate / lift identity comparisons by holders
    /// that pin storage but never name a `CallArena`.
    pub(crate) fn arena(&self) -> &RuntimeArena {
        &self.arena
    }
}

/// One user-fn call's allocation frame: a thin shell over a refcounted [`FrameStorage`]. `Rc`-pinned
/// so the scheduler manages the frame by `Rc<CallArena>`; an escaping closure extends only the
/// *storage* (via [`Self::storage_rc`]), not the shell, so tail reuse can reset the shell's storage
/// without foreclosing on the escapee. Field order is load-bearing: `storage` drops before
/// `scope_ptr`, so the arena tears down before the now-dangling child pointer.
///
/// See [per-call-arena-protocol.md](../../../design/per-call-arena-protocol.md) for the
/// carrier set, lift-time anchor decision, cycle gate, ancestor chain, and TCO
/// frame reuse; [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#arena-lifetime-erasure)
/// for the heap-pinning / drop-order invariants.
pub struct CallArena {
    storage: Rc<FrameStorage>,
    scope_ptr: Option<ScopePtr<'static>>,
    /// True only for the scheduler-owned run frame, which carries the top-level run scope and
    /// never drops mid-run. Its `arena` is empty (top-level values live in the externally-owned
    /// run arena, reached via `scope.arena`), so there is nothing to lift out of it: the Done
    /// boundary skips the lift for a non-dying frame (lift exists to rescue values from a *dying*
    /// per-call arena). Every per-call frame is `false`.
    non_dying: bool,
}

impl CallArena {
    /// Build a fresh per-call frame whose child `Scope` uses `outer` as its `outer` link.
    /// `outer_frame` must hold the parent frame's `FrameStorage` Rc when the parent is per-call;
    /// `None` when the parent is run-root.
    pub fn new(outer: &Scope<'_>, outer_frame: Option<Rc<FrameStorage>>) -> Rc<CallArena> {
        let escape = NonNull::from(outer.arena);
        // The arena is born inside its own `Rc<FrameStorage>`, heap-pinned from this point on, so
        // the child-scope pointer below stays valid as the storage Rc moves into the shell.
        let storage = Rc::new(FrameStorage {
            arena: RuntimeArena::with_escape(escape),
            outer: outer_frame,
        });
        let arena_ptr: *const RuntimeArena = &storage.arena;
        // SAFETY: heap-pinning keeps `arena_ptr` valid for the storage Rc's lifetime, which exceeds
        // this function's duration; `outer` lives long enough by caller contract.
        let arena_ref: &'static RuntimeArena = unsafe { pin_deref(arena_ptr) };
        // SAFETY: lexical-scoping invariant — `outer` (the captured definition scope, or a
        // longer-lived ancestor) outlives this frame, so erasing its lifetime to `'static`
        // for the child's `outer` link is sound; the child borrow is re-anchored on read.
        let outer_static: &Scope<'static> = unsafe { reattach_ref::<ScopeFamily>(outer) };
        let mut child = Scope::child_under(outer_static);
        // `child_under` defaults `arena` to `outer.arena`; override to the per-call arena.
        child.arena = arena_ref;
        // `arena_ref` is `&'static` (the `pin_deref` above is where the `'static` claim
        // originates), so `alloc_scope` returns `&'static Scope<'static>` and the safe `erase`
        // yields a `ScopePtr<'static>` — no fabrication here.
        let allocated: &'static Scope<'static> = arena_ref.alloc_scope(child);
        Rc::new(CallArena {
            storage,
            scope_ptr: Some(ScopePtr::erase(allocated)),
            non_dying: false,
        })
    }

    /// The scheduler-owned **run frame**: a frame that *carries an already-built run scope*
    /// rather than minting a child. Top-level execution runs against this frame so `active_frame`
    /// is never `None`, which makes a body's re-dispatch-against-its-own-scope uniformly framed
    /// (Yoked) at every depth — top level included. The run scope keeps its own (run) arena, so
    /// this frame's `arena` stays empty and unused; `escape` is `None` (a non-dying top frame has
    /// nothing to redirect into). Marked `non_dying` so the Done boundary skips the (pointless)
    /// self-lift of top-level results.
    ///
    /// SAFETY: the adopted run scope lives in the externally-owned run arena, which outlives this
    /// scheduler-owned frame; erasing its borrow to `'static` for storage in `scope_ptr` is the
    /// same re-anchored-on-read erasure every [`ScopePtr`] carries.
    pub fn adopting(scope: &Scope<'_>) -> Rc<CallArena> {
        let scope_static: &'static Scope<'static> = unsafe { reattach_ref::<ScopeFamily>(scope) };
        Rc::new(CallArena {
            storage: Rc::new(FrameStorage {
                arena: RuntimeArena::new(),
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
        // concentrated here at the non-generic `CallArena` boundary. `scope_ptr` is `Some`
        // after construction and stable for the `Rc`'s lifetime (heap-pinned), and the
        // returned `'a` is bounded by `&self`, so the fabricated lifetime cannot outlive the
        // pointee. `'a` is driven by the return-type annotation — `reattach_unbounded`'s
        // lifetime is late-bound, so it cannot be a turbofish argument.
        let scope: &'a Scope<'a> = unsafe { self.scope_ptr_set().reattach_unbounded() };
        scope
    }

    /// Scope handle bounded by `&'step Rc<Self>` — strictly shorter than the `&'a Scope<'a>`
    /// claim of [`CallArena::scope`]. Use this for local-bind plumbing (e.g.
    /// [`Scope::bind_value`]) that does not need to escape the `Rc`'s borrow, so the caller
    /// avoids an `unsafe` `'a`-anchoring transmute on the receiving end.
    ///
    /// SAFETY: `scope_ptr` stores a `ScopePtr<'static>`; the free-`'step` fabrication is
    /// concentrated here at the non-generic `CallArena` boundary. The pointer is stable for
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
    /// the arena and child scope for all of `'step`, so the `'step`-bounded borrow cannot dangle.
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

    /// Run `f` with this frame's per-call arena re-exposed at a free `'a` and its child scope
    /// re-handed at a bounded borrow. The single audited home for the *seed-side* re-anchor: the
    /// MATCH / TRY arm and `KFunction::invoke` body seeds bind their `it` / parameters — values
    /// whose type carries the caller's `'a`, deep-cloned into this frame's arena — into the child
    /// scope inside `f`.
    ///
    /// The **arena** is re-exposed at a free `'a`: this is the inherent arena re-exposure the C0
    /// verdict keeps (an `'a`-typed value must land in an `'a`-typed arena, and no lifetime scheme
    /// closes that — the frame `Rc` the caller holds heap-pins the arena, so the seed's binds
    /// outlive `f`). The **child scope** rides the bounded `scope_bounded` brand — borrow capped at
    /// the `&Rc` receiver, content `'a` — so it is *not* fabricated free; `bind_value` matches on
    /// the `'a` content.
    ///
    /// SAFETY (arena re-borrow): the caller holds this frame's `Rc`, which heap-pins the arena for
    /// as long as any value `f` binds into the scope lives.
    pub fn with_frame_interior<'a, R>(
        self: &Rc<Self>,
        f: impl FnOnce(&'a RuntimeArena, &Scope<'a>) -> R,
    ) -> R {
        // SAFETY: the held frame `Rc` heap-pins the arena for all of `'a`, so re-borrowing the
        // stable arena pointer at `'a` is sound. This is a pointer re-borrow, not a value retype, so
        // it routes the audited `pin_deref` rather than a bespoke `transmute`. `'a` is driven by the
        // closure's parameter type, not a turbofish argument.
        let arena: &'a RuntimeArena = unsafe { pin_deref(self.arena() as *const RuntimeArena) };
        f(arena, self.scope_bounded())
    }

    pub fn arena(&self) -> &RuntimeArena {
        &self.storage.arena
    }

    /// Clone this frame's `FrameStorage` Rc — the handle an escaping value (a returned closure, a
    /// module frame) pins to keep its captured environment alive *without* pinning the shell, so
    /// tail reuse stays free to reset the shell.
    pub fn storage_rc(&self) -> Rc<FrameStorage> {
        Rc::clone(&self.storage)
    }

    /// Reset this frame for a tail-call iteration: install a fresh `FrameStorage` (a new
    /// `RuntimeArena` escaping into `new_outer.arena`, no `outer` link) and re-allocate the child
    /// `Scope` under `new_outer`. The old `FrameStorage` is dropped here — and its arena with it —
    /// *unless* an escaped value still holds it, in which case that snapshot lives on independently
    /// while the shell reuses. Returns `false` (untouched) only when `Rc::get_mut` fails — another
    /// live `Rc<CallArena>` (a shell clone, never an escape) foreclosing in-place reuse. See
    /// [per-call-arena-protocol.md § TCO frame reuse](../../../design/per-call-arena-protocol.md#tco-frame-reuse).
    pub fn try_reset_for_tail(self: &mut Rc<Self>, new_outer: &Scope<'_>) -> bool {
        if Rc::get_mut(self).is_none() {
            return false;
        }
        let escape = NonNull::from(new_outer.arena);
        // SAFETY: lexical-scoping invariant — `new_outer.arena` outlives this frame
        // (it is the captured definition scope's arena, or a longer-lived ancestor).
        let outer_static: &Scope<'static> = unsafe { reattach_ref::<ScopeFamily>(new_outer) };
        // Build the fresh storage and its child scope before touching the shell, so the
        // arena pointer is heap-pinned by the new storage Rc when it lands in the shell.
        let storage = Rc::new(FrameStorage {
            arena: RuntimeArena::with_escape(escape),
            outer: None,
        });
        let arena_ptr: *const RuntimeArena = &storage.arena;
        // SAFETY: heap-pinned via the storage Rc; pointer is stable for its lifetime.
        let arena_ref: &'static RuntimeArena = unsafe { pin_deref(arena_ptr) };
        let mut child = Scope::child_under(outer_static);
        child.arena = arena_ref;
        // `arena_ref` is `&'static` (the `pin_deref` above is where the `'static` claim
        // originates), so `alloc_scope` returns `&'static Scope<'static>` and the safe `erase`
        // yields a `ScopePtr<'static>` — no fabrication here.
        let allocated: &'static Scope<'static> = arena_ref.alloc_scope(child);
        let this = Rc::get_mut(self).expect("just-verified unique above");
        // Drops the old storage (and its arena) unless an escapee still holds it.
        this.storage = storage;
        this.scope_ptr = Some(ScopePtr::erase(allocated));
        true
    }
}

#[cfg(test)]
mod tests {
    //! Targeted Miri coverage for the unsafe sites in this file. Each test pins down a
    //! specific aliasing/lifetime shape under tree borrows; logical assertions are minimal
    //! — these tests fail when Miri reports UB, not on values.

    use super::*;
    use crate::builtins::default_scope;
    use crate::machine::model::types::KType;
    use crate::machine::BindingIndex;

    /// `scope_bounded` re-anchors the child scope with a borrow bounded by the `&Rc` witness.
    /// The good path: read it within the witness borrow. The over-anchor and covariance
    /// compile-error properties were confirmed by the C0 spike (see
    /// scratch/type-enforced-frame-reanchor-plan.md § C0 verdict); they are structural —
    /// `scope_bounded`'s `'step` borrow cannot widen to a free `'a`, and `Scope<'a>` is invariant.
    #[test]
    fn scope_bounded_reanchors_within_witness_borrow() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let frame: Rc<CallArena> = CallArena::new(scope, None);
        let bounded: &Scope<'_> = frame.scope_bounded();
        // Same underlying child scope as the unbounded accessors, just a shorter borrow.
        assert_eq!(bounded.id, frame.scope().id);
        assert_eq!(bounded.id, frame.scope_for_bind().id);
    }

    /// `CallArena::scope`'s re-borrow stays valid when the arena is mutated through a
    /// sibling pointer afterward — `frame.scope()` and `frame.arena().alloc(...)`
    /// must coexist soundly under tree borrows.
    #[test]
    fn call_arena_scope_survives_subsequent_alloc() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let frame = CallArena::new(scope, None);
        let s = frame.scope();
        let _new = frame.arena().alloc_object(KObject::Number(1.0));
        assert!(std::ptr::eq(s.arena, frame.arena()));
    }

    /// Raw-pointer roundtrip: lifetime-anchor an extracted `*const RuntimeArena` and
    /// `*const Scope<'_>` from the same frame, then mutate via one ref while the other
    /// stays live.
    #[test]
    fn call_arena_scope_survives_subsequent_alloc_via_raw_ptr_roundtrip() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let frame: Rc<CallArena> = CallArena::new(scope, None);
        let arena_ptr: *const RuntimeArena = frame.arena();
        let scope_ptr: *const Scope<'_> = frame.scope();
        let inner_arena: &RuntimeArena = unsafe { &*(arena_ptr as *const _) };
        let child: &Scope<'_> = unsafe { &*(scope_ptr as *const _) };
        let it_obj: &KObject<'_> = inner_arena.alloc_object(KObject::Number(42.0));
        child
            .bind_value("it".to_string(), it_obj, BindingIndex::BUILTIN)
            .unwrap();
        assert!(matches!(child.lookup("it"), Some(KObject::Number(n)) if *n == 42.0));
    }

    /// Repeated `frame.scope()` calls produce aliasing shared refs that must be
    /// concurrently readable.
    #[test]
    fn call_arena_scope_repeated_calls_alias() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let frame = CallArena::new(scope, None);
        let s1 = frame.scope();
        let s2 = frame.scope();
        let s3 = frame.scope();
        assert!(std::ptr::eq(s1, s2));
        assert!(std::ptr::eq(s2, s3));
        assert!(s1.outer().is_some());
    }

    /// Two-deep chain: dropping the local `outer` handle leaves only `inner`'s `FrameStorage.outer`
    /// keeping the outer arena alive while we read through `inner.scope().outer`.
    #[test]
    fn call_arena_chained_outer_frame_walkable() {
        let arena = RuntimeArena::new();
        let run_scope = default_scope(&arena, Box::new(std::io::sink()));
        let outer = CallArena::new(run_scope, None);
        let inner = CallArena::new(outer.scope(), Some(outer.storage_rc()));
        drop(outer);
        let outer_scope = inner
            .scope()
            .outer()
            .expect("inner.scope().outer must be Some");
        assert!(std::ptr::eq(
            outer_scope.arena,
            inner.scope().outer().unwrap().arena
        ));
        assert!(outer_scope.outer().is_some());
    }

    /// In-struct Rc must keep the arena alive for a re-anchored `&Scope` stored alongside
    /// it once the local Rc handle is dropped.
    #[test]
    fn call_arena_scope_re_anchored_into_struct_alongside_rc() {
        struct Holder<'a> {
            s: &'a Scope<'a>,
            _f: Rc<CallArena>,
        }

        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let h = {
            let f = CallArena::new(scope, None);
            let s: &Scope<'_> = unsafe { std::mem::transmute::<&Scope<'_>, &Scope<'_>>(f.scope()) };
            Holder { s, _f: f }
        };
        assert!(h.s.outer().is_some());
    }

    /// Allocating records the stored address into the `membership` side-table via
    /// `RefCell::borrow_mut` while a prior `&KObject` from the same arena is shared-borrowed.
    /// Pins that tree-borrows shape.
    #[test]
    fn runtime_arena_alloc_while_prior_ref_live() {
        let a = RuntimeArena::new();
        let r1 = a.alloc_object(KObject::Number(1.0));
        let r2 = a.alloc_object(KObject::Number(2.0));
        assert!(matches!(r1, KObject::Number(n) if *n == 1.0));
        assert!(matches!(r2, KObject::Number(n) if *n == 2.0));
    }

    /// `alloc_ktype` returns an arena-lifetime `&KType` and bumps `alloc_count` by one.
    #[test]
    fn alloc_ktype_returns_arena_lifetime_ref_and_counts() {
        let a = RuntimeArena::new();
        let baseline = a.alloc_count();
        let t: &KType = a.alloc_ktype(KType::Number);
        assert!(matches!(t, KType::Number));
        assert_eq!(a.alloc_count(), baseline + 1);
    }

    /// Pins the reset transmute pair (`&Scope<'_> → &Scope<'static>` outer cast plus the
    /// raw-arena-ptr re-anchor) under tree borrows: after reset, a fresh alloc via
    /// `arena()` and a `bind_value` on `scope()` must coexist.
    #[test]
    fn call_arena_try_reset_for_tail_round_trip() {
        let outer_arena = RuntimeArena::new();
        let outer_scope = default_scope(&outer_arena, Box::new(std::io::sink()));
        let mut frame: Rc<CallArena> = CallArena::new(outer_scope, None);
        let _pre = frame.arena().alloc_object(KObject::Number(1.0));
        assert!(frame.arena().alloc_count() >= 1);

        let did_reset = frame.try_reset_for_tail(outer_scope);
        assert!(did_reset, "Rc was unique, reset must succeed");

        // Fresh arena: only the new child scope remains.
        assert_eq!(frame.arena().alloc_count(), 1);

        let v = frame.arena().alloc_object(KObject::Number(42.0));
        frame
            .scope()
            .bind_value("k".to_string(), v, BindingIndex::BUILTIN)
            .unwrap();
        assert!(matches!(frame.scope().lookup("k"), Some(KObject::Number(n)) if *n == 42.0));
        assert!(frame.scope().outer().is_some());
    }

    /// `try_reset_for_tail` refuses when another `Rc<CallArena>` *shell* clone exists — a
    /// transient holder still naming the frame, for which in-place reset would mutate the shell
    /// under a live alias. (An escaped value pins `FrameStorage`, not the shell — see
    /// [`call_arena_try_reset_for_tail_allows_reset_under_escaped_storage`].)
    #[test]
    fn call_arena_try_reset_for_tail_refuses_when_aliased() {
        let outer_arena = RuntimeArena::new();
        let outer_scope = default_scope(&outer_arena, Box::new(std::io::sink()));
        let mut frame: Rc<CallArena> = CallArena::new(outer_scope, None);
        let pre_arena_addr = frame.arena() as *const RuntimeArena as usize;

        // A second shell holder (not an escape): clone the `Rc<CallArena>` so strong_count > 1.
        let _alias = Rc::clone(&frame);

        let did_reset = frame.try_reset_for_tail(outer_scope);
        assert!(!did_reset, "aliased frame must refuse reset");

        assert_eq!(
            frame.arena() as *const RuntimeArena as usize,
            pre_arena_addr,
            "refused reset must leave arena pointer unchanged",
        );
    }

    /// An escaped value pins the frame's `FrameStorage`, not its shell, so the shell stays uniquely
    /// owned and `try_reset_for_tail` *succeeds*: the escapee's snapshot rides the `FrameStorage` it
    /// still holds while the shell installs fresh storage. A gate keyed on the shell's `Rc` count
    /// could not distinguish this from a live shell alias and would refuse it.
    #[test]
    fn call_arena_try_reset_for_tail_allows_reset_under_escaped_storage() {
        let outer_arena = RuntimeArena::new();
        let outer_scope = default_scope(&outer_arena, Box::new(std::io::sink()));
        let mut frame: Rc<CallArena> = CallArena::new(outer_scope, None);
        let _escaped = frame.arena().alloc_object(KObject::Number(7.0));
        let pre_alloc_count = frame.arena().alloc_count();
        let pre_storage_addr = frame.arena() as *const RuntimeArena as usize;

        // Simulate a closure escape: hold the frame's storage Rc (what an anchored value carries).
        let escaped_storage = frame.storage_rc();

        let did_reset = frame.try_reset_for_tail(outer_scope);
        assert!(
            did_reset,
            "an escaped *storage* hold must not foreclose reuse"
        );

        // The shell reset to a fresh arena, distinct from the snapshot the escapee still holds.
        assert_ne!(
            frame.arena() as *const RuntimeArena as usize,
            pre_storage_addr,
            "reuse installed fresh storage",
        );
        // The escaped snapshot is still alive (its retained storage Rc still owns the pre-reset
        // arena, allocations intact) — the reset dropped only the shell's reference to it.
        assert!(std::ptr::eq(
            escaped_storage.arena() as *const RuntimeArena,
            pre_storage_addr as *const RuntimeArena
        ));
        assert_eq!(escaped_storage.arena().alloc_count(), pre_alloc_count);
    }

    /// Cycle gate: alloc'ing a value that anchors back at the receiving arena via an
    /// `Rc<CallArena>` redirects to the escape arena. Without the redirect the per-call
    /// arena's storage would hold an Rc to itself and never drop.
    #[test]
    fn alloc_object_redirects_self_anchored_value_to_escape_arena() {
        let outer = RuntimeArena::new();
        let scope = default_scope(&outer, Box::new(std::io::sink()));
        let frame: Rc<CallArena> = CallArena::new(scope, None);
        // Build a List whose only element is a `KFunction` carrying an
        // `Rc<FrameStorage>` pointing at `frame.arena()`. The cycle gate only inspects the
        // carried `Rc`, so the placeholder `KFunction` body is irrelevant.
        let dummy_fn_obj = outer.alloc_object(KObject::KFunction(
            outer.alloc_function(crate::machine::core::kfunction::KFunction::new(
                crate::machine::model::types::ExpressionSignature {
                    return_type: crate::machine::model::types::ReturnType::Resolved(
                        crate::machine::model::types::KType::Null,
                    ),
                    elements: vec![crate::machine::model::types::SignatureElement::Keyword(
                        "DUMMY".into(),
                    )],
                },
                crate::machine::core::kfunction::Body::Builtin(|ctx| {
                    crate::machine::core::kfunction::action::Action::Done(Ok(
                        crate::machine::model::Carried::Object(
                            ctx.scope
                                .arena
                                .alloc_object(crate::machine::model::KObject::Null),
                        ),
                    ))
                }),
                scope,
            )),
            None,
        ));
        let f_ref = match dummy_fn_obj {
            KObject::KFunction(f, _) => *f,
            _ => unreachable!(),
        };
        let cyclic_kfn = KObject::KFunction(f_ref, Some(frame.storage_rc()));
        let list = KObject::list(vec![cyclic_kfn]);

        let stored = frame.arena().alloc_object(list);
        let stored_ptr = stored as *const KObject<'_>;
        assert!(
            outer.owns_object(stored_ptr),
            "self-anchored alloc should redirect to the escape arena (outer)",
        );
        assert!(
            !frame.arena().owns_object(stored_ptr),
            "self-anchored value must not land in the per-call arena",
        );
    }
}
