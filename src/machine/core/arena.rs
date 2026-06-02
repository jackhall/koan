use std::cell::RefCell;
use std::rc::Rc;

use typed_arena::Arena;

use super::scope::Scope;
use crate::machine::core::kfunction::KFunction;
use crate::machine::model::operators::OperatorGroup;
use crate::machine::model::types::KType;
use crate::machine::model::values::{KObject, Module, Signature};
/// Run-lifetime allocator. Lives for one program run. Sub-arenas store `T<'static>`
/// (phantom); each `alloc*` re-anchors to the caller's `'a` on the way out.
///
/// See [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#arena-lifetime-erasure)
/// for the transmute soundness argument and
/// [per-call-arena-protocol.md § Cycle gate](../../../design/per-call-arena-protocol.md#cycle-gate-on-alloc_object)
/// for the `Rc<CallArena>` redirect that `alloc` enforces.
pub struct RuntimeArena {
    objects: Arena<KObject<'static>>,
    functions: Arena<KFunction<'static>>,
    scopes: Arena<Scope<'static>>,
    modules: Arena<Module<'static>>,
    signatures: Arena<Signature<'static>>,
    /// Backs per-type identity binding storage (`Bindings::types`). Same erasure /
    /// SAFETY argument as the other sub-arenas.
    ktypes: Arena<KType<'static>>,
    /// Backs the per-scope operator registry (`Bindings::operators`). `OperatorGroup`
    /// is lifetime-free (owns only `String`/`HashMap`/scalars) and holds no arena
    /// anchors, so it needs no cycle gate or `'static` erasure.
    operator_groups: Arena<OperatorGroup>,
    /// Stable addresses of every `KObject` allocated here. Backs `owns_object` membership
    /// queries via a linear scan (no deref, no borrow). `usize` rather than `*const _` keeps
    /// the field lifetime-erased and `Send`/`Sync`-neutral.
    allocated_objects: RefCell<Vec<usize>>,
    /// Redirect target for the `alloc` cycle gate. `None` on run-root.
    /// Stable for `self`'s lifetime: `CallArena::new` heap-pins the outer via `Rc` and the
    /// outer outlives this inner per the lexical-scoping invariant.
    escape: Option<*const RuntimeArena>,
}

impl RuntimeArena {
    pub fn new() -> Self {
        Self {
            objects: Arena::new(),
            functions: Arena::new(),
            scopes: Arena::new(),
            modules: Arena::new(),
            signatures: Arena::new(),
            ktypes: Arena::new(),
            operator_groups: Arena::new(),
            allocated_objects: RefCell::new(Vec::new()),
            escape: None,
        }
    }

    /// `alloc` will redirect self-cyclic values to `escape`; see [`CycleGated`].
    pub fn with_escape(escape: *const RuntimeArena) -> Self {
        Self {
            objects: Arena::new(),
            functions: Arena::new(),
            scopes: Arena::new(),
            modules: Arena::new(),
            signatures: Arena::new(),
            ktypes: Arena::new(),
            operator_groups: Arena::new(),
            allocated_objects: RefCell::new(Vec::new()),
            escape: Some(escape),
        }
    }

    /// Whether `ptr` was returned by a prior `alloc::<KObject<_>>` on this arena.
    pub fn owns_object<'a>(&self, ptr: *const KObject<'a>) -> bool {
        // `KObject` is invariant in `'a`, so the through-`'static` cast is required despite
        // clippy's complaint.
        #[allow(clippy::unnecessary_cast)]
        let target = ptr as *const KObject<'static> as usize;
        self.allocated_objects.borrow().contains(&target)
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
        let static_f: KFunction<'static> =
            unsafe { std::mem::transmute::<KFunction<'a>, KFunction<'static>>(f) };
        let stored: &'a mut KFunction<'static> = self.functions.alloc(static_f);
        unsafe { std::mem::transmute::<&'a mut KFunction<'static>, &'a KFunction<'a>>(stored) }
    }

    pub fn alloc_scope<'a>(&'a self, s: Scope<'a>) -> &'a Scope<'a> {
        let static_s: Scope<'static> =
            unsafe { std::mem::transmute::<Scope<'a>, Scope<'static>>(s) };
        let stored: &'a mut Scope<'static> = self.scopes.alloc(static_s);
        unsafe { std::mem::transmute::<&'a mut Scope<'static>, &'a Scope<'a>>(stored) }
    }

    pub fn alloc_module<'a>(&'a self, m: Module<'a>) -> &'a Module<'a> {
        let static_m: Module<'static> =
            unsafe { std::mem::transmute::<Module<'a>, Module<'static>>(m) };
        let stored: &'a mut Module<'static> = self.modules.alloc(static_m);
        unsafe { std::mem::transmute::<&'a mut Module<'static>, &'a Module<'a>>(stored) }
    }

    pub fn alloc_signature<'a>(&'a self, s: Signature<'a>) -> &'a Signature<'a> {
        let static_s: Signature<'static> =
            unsafe { std::mem::transmute::<Signature<'a>, Signature<'static>>(s) };
        let stored: &'a mut Signature<'static> = self.signatures.alloc(static_s);
        unsafe { std::mem::transmute::<&'a mut Signature<'static>, &'a Signature<'a>>(stored) }
    }

    /// Allocate an [`OperatorGroup`] into the run-lifetime arena. No lifetime erasure
    /// (the type carries none) and no cycle gate (it holds no arena anchors).
    pub fn alloc_operator_group(&self, g: OperatorGroup) -> &OperatorGroup {
        self.operator_groups.alloc(g)
    }

    /// When true, no value can hold a `&KFunction` pointing into this arena — see the
    /// `alloc_function` invariant.
    pub fn functions_is_empty(&self) -> bool {
        self.functions.len() == 0
    }
}

impl Default for RuntimeArena {
    fn default() -> Self {
        Self::new()
    }
}

/// True iff any descendant of `obj` carries an `Rc<CallArena>` whose backing `RuntimeArena`
/// is `arena_ptr`. Walks the composite shapes mirrored from `KObject::deep_clone`
/// (`List`/`Dict`/`Tagged`/`Struct`) plus `KFunction`/`KFuture` anchors.
fn rc_targets(rc: &Rc<CallArena>, arena_ptr: *const RuntimeArena) -> bool {
    std::ptr::eq(rc.arena() as *const RuntimeArena, arena_ptr)
}

fn obj_anchors_to(obj: &KObject<'_>, arena_ptr: *const RuntimeArena) -> bool {
    match obj {
        KObject::KFunction(_, Some(rc)) => rc_targets(rc, arena_ptr),
        KObject::KFuture(_, Some(rc)) => rc_targets(rc, arena_ptr),
        // A module value rides `KTypeValue(KType::Module { frame, .. })`.
        KObject::KTypeValue(kt) => ktype_anchors_to(kt, arena_ptr),
        KObject::List(items, _) => items.iter().any(|x| obj_anchors_to(x, arena_ptr)),
        KObject::Dict(entries, _, _) => entries.values().any(|x| obj_anchors_to(x, arena_ptr)),
        KObject::Tagged { value, .. } => obj_anchors_to(value, arena_ptr),
        KObject::Struct { fields, .. } => fields.values().any(|x| obj_anchors_to(x, arena_ptr)),
        _ => false,
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

/// Per-type plumbing for the cycle-gated allocator. Sharing this through one trait keeps
/// the gate-and-recurse logic in [`RuntimeArena::alloc`] rather than forked across per-type
/// `alloc_*` methods — a future variant gaining a frame anchor adds one `impl CycleGated`,
/// not another copy of the gate.
pub trait CycleGated<'a>: Sized {
    /// True iff any descendant carries an `Rc<CallArena>` whose backing `RuntimeArena` is
    /// `arena_ptr` — i.e. allocating `self` into that arena would form a self-referential
    /// cycle.
    fn anchors_to(&self, arena_ptr: *const RuntimeArena) -> bool;
    /// Lifetime-erase and stash `self` in the appropriate sub-arena. The cycle gate has
    /// already redirected to the escape arena if needed; this is the local-store step.
    fn alloc_local(self, arena: &'a RuntimeArena) -> &'a Self;
}

impl<'a> CycleGated<'a> for KObject<'a> {
    fn anchors_to(&self, arena_ptr: *const RuntimeArena) -> bool {
        obj_anchors_to(self, arena_ptr)
    }
    fn alloc_local(self, arena: &'a RuntimeArena) -> &'a Self {
        let static_obj: KObject<'static> =
            unsafe { std::mem::transmute::<KObject<'a>, KObject<'static>>(self) };
        let stored: &'a mut KObject<'static> = arena.objects.alloc(static_obj);
        arena
            .allocated_objects
            .borrow_mut()
            .push(stored as *const _ as usize);
        unsafe { std::mem::transmute::<&'a mut KObject<'static>, &'a KObject<'a>>(stored) }
    }
}

impl<'a> CycleGated<'a> for KType<'a> {
    fn anchors_to(&self, arena_ptr: *const RuntimeArena) -> bool {
        ktype_anchors_to(self, arena_ptr)
    }
    fn alloc_local(self, arena: &'a RuntimeArena) -> &'a Self {
        let static_t: KType<'static> =
            unsafe { std::mem::transmute::<KType<'a>, KType<'static>>(self) };
        let stored: &'a mut KType<'static> = arena.ktypes.alloc(static_t);
        unsafe { std::mem::transmute::<&'a mut KType<'static>, &'a KType<'a>>(stored) }
    }
}

impl RuntimeArena {
    /// Single allocator entry for any `T: CycleGated`. Walks the escape chain when the
    /// value would self-cycle (an `Rc<CallArena>` pointing back at `self`), then hands off
    /// to the type's `alloc_local`.
    ///
    /// SAFETY of the `&*escape_ptr`: `escape_ptr` was set by `CallArena::new` to the outer
    /// scope's arena address. The outer arena outlives `self` per the lexical-scoping
    /// invariant (per-call frames nest inside their captured definition scope's arena);
    /// `Rc<CallArena>` keeps the chain pinned. So `'a` (bounded by `&self`) is a valid
    /// lifetime to attach to the dereferenced escape pointer.
    pub fn alloc<'a, T: CycleGated<'a>>(&'a self, value: T) -> &'a T {
        if let Some(escape_ptr) = self.escape {
            let self_ptr = self as *const RuntimeArena;
            if value.anchors_to(self_ptr) {
                let escape_ref: &'a RuntimeArena = unsafe { &*escape_ptr };
                return escape_ref.alloc(value);
            }
        }
        value.alloc_local(self)
    }
}

#[cfg(test)]
impl RuntimeArena {
    /// Total number of values stored across all six sub-arenas (test-only). Each `alloc_*`
    /// writes to exactly one sub-arena, so this is the precise allocation count without
    /// double-counting.
    pub fn alloc_count(&self) -> usize {
        self.objects.len()
            + self.functions.len()
            + self.scopes.len()
            + self.modules.len()
            + self.signatures.len()
            + self.ktypes.len()
            + self.operator_groups.len()
    }
}

/// Static-singleton inhabitants. `KObject` itself isn't `Sync` (some variants carry `Rc` /
/// `Box<dyn Trait>`), but `Null` and `Bool(bool)` are unit-shaped. Typing the static
/// storage at this unit-only enum lets the `*_HOLDER` statics derive `Sync` naturally —
/// no `unsafe impl Sync` needed. The accessors then project a `const KObject` (inlined
/// per use site, sidestepping `!Sync`) and re-annotate `'static → 'a`, sound because the
/// projected variants carry no lifetime-parameterized data.
enum StaticKValue {
    Null,
    Bool(bool),
}

static NULL_HOLDER: StaticKValue = StaticKValue::Null;
static TRUE_HOLDER: StaticKValue = StaticKValue::Bool(true);
static FALSE_HOLDER: StaticKValue = StaticKValue::Bool(false);

/// Project the `KObject` view of a static `StaticKValue`. Lives at the boundary so any
/// future addition to `StaticKValue` is forced through here. `const` items inline at the
/// use site, so the returned reference is rvalue-promoted without requiring `KObject: Sync`.
fn project<'a>(v: &'static StaticKValue) -> &'a KObject<'a> {
    const NULL: KObject<'static> = KObject::Null;
    const TRUE: KObject<'static> = KObject::Bool(true);
    const FALSE: KObject<'static> = KObject::Bool(false);
    let r: &'static KObject<'static> = match v {
        StaticKValue::Null => &NULL,
        StaticKValue::Bool(true) => &TRUE,
        StaticKValue::Bool(false) => &FALSE,
    };
    // SAFETY: the projected `KObject` is `Null` or `Bool(_)` — both unit-shaped, carrying
    // no references — so the `'static` lifetime parameter is purely phantom and `'static`
    // → `'a` is sound.
    unsafe { std::mem::transmute::<&'static KObject<'static>, &'a KObject<'a>>(r) }
}

pub fn null_singleton<'a>() -> &'a KObject<'a> {
    project(&NULL_HOLDER)
}

pub fn true_singleton<'a>() -> &'a KObject<'a> {
    project(&TRUE_HOLDER)
}

pub fn false_singleton<'a>() -> &'a KObject<'a> {
    project(&FALSE_HOLDER)
}

/// One user-fn call's allocation frame. `Rc`-pinned so an escaping closure can extend
/// the frame's life past slot finalize. Field order is load-bearing: `arena` drops before
/// `outer_frame`, so inner pointers die before the outer storage they may reference.
///
/// See [per-call-arena-protocol.md](../../../design/per-call-arena-protocol.md) for the
/// carrier set, lift-time anchor decision, cycle gate, `outer_frame` chain, and TCO
/// frame reuse; [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#arena-lifetime-erasure)
/// for the heap-pinning / drop-order invariants.
pub struct CallArena {
    arena: RuntimeArena,
    scope_ptr: *const Scope<'static>,
    outer_frame: Option<Rc<CallArena>>,
}

impl CallArena {
    /// Build a fresh per-call frame whose child `Scope` uses `outer` as its `outer` link.
    /// `outer_frame` must hold the parent's Rc when the parent is per-call; `None` when
    /// the parent is run-root.
    pub fn new<'p>(outer: &'p Scope<'p>, outer_frame: Option<Rc<CallArena>>) -> Rc<CallArena> {
        let escape: *const RuntimeArena = outer.arena;
        let mut rc = Rc::new(CallArena {
            arena: RuntimeArena::with_escape(escape),
            scope_ptr: std::ptr::null(),
            outer_frame,
        });
        let arena_ptr: *const RuntimeArena = &rc.arena;
        // SAFETY: heap-pinning keeps `arena_ptr` valid for the Rc's lifetime, which exceeds
        // this function's duration; `outer` lives long enough by caller contract.
        let arena_ref: &'static RuntimeArena = unsafe { &*arena_ptr };
        let outer_static: &Scope<'static> =
            unsafe { std::mem::transmute::<&Scope<'_>, &Scope<'static>>(outer) };
        let mut child = Scope::child_under(outer_static);
        // `child_under` defaults `arena` to `outer.arena`; override to the per-call arena.
        child.arena = arena_ref;
        let allocated: &Scope<'_> = arena_ref.alloc_scope(child);
        // `Scope` is invariant in `'a`, so the through-`'static` cast is required.
        #[allow(clippy::unnecessary_cast)]
        let scope_ptr = allocated as *const Scope<'_> as *const Scope<'static>;
        Rc::get_mut(&mut rc)
            .expect("freshly-constructed Rc has unique ownership")
            .scope_ptr = scope_ptr;
        rc
    }

    pub fn scope<'a>(&'a self) -> &'a Scope<'a> {
        unsafe { std::mem::transmute::<&Scope<'static>, &'a Scope<'a>>(&*self.scope_ptr) }
    }

    /// Scope handle bounded by `&'p Rc<Self>` — strictly shorter than the `&'a Scope<'a>`
    /// claim of [`CallArena::scope`]. Use this for local-bind plumbing (e.g.
    /// [`Scope::bind_value`]) that does not need to escape the `Rc`'s borrow, so the caller
    /// avoids an `unsafe` `'a`-anchoring transmute on the receiving end.
    ///
    /// SAFETY: `scope_ptr` is stable for the `Rc`'s lifetime (heap-pinned by `Rc`); the
    /// returned `'p` is bounded by the receiver so the borrow cannot outlive it.
    pub fn scope_for_bind<'p>(self: &'p Rc<Self>) -> &'p Scope<'p> {
        unsafe { std::mem::transmute::<&Scope<'static>, &'p Scope<'p>>(&*self.scope_ptr) }
    }

    pub fn arena(&self) -> &RuntimeArena {
        &self.arena
    }

    /// Reset this frame in place for a tail-call iteration: drop the old arena storage,
    /// install a fresh `RuntimeArena` escaping into `new_outer.arena`, re-allocate the
    /// child `Scope` under `new_outer`. Returns `false` (untouched) when `Rc::get_mut`
    /// fails — any other live `Rc` foreclosing in-place reuse. See
    /// [per-call-arena-protocol.md § TCO frame reuse](../../../design/per-call-arena-protocol.md#tco-frame-reuse).
    pub fn try_reset_for_tail<'p>(self: &mut Rc<Self>, new_outer: &'p Scope<'p>) -> bool {
        if Rc::get_mut(self).is_none() {
            return false;
        }
        let escape: *const RuntimeArena = new_outer.arena;
        // SAFETY: lexical-scoping invariant — `new_outer.arena` outlives this frame
        // (it is the captured definition scope's arena, or a longer-lived ancestor).
        let outer_static: &Scope<'static> =
            unsafe { std::mem::transmute::<&Scope<'_>, &Scope<'static>>(new_outer) };
        let this = Rc::get_mut(self).expect("just-verified unique above");
        this.scope_ptr = std::ptr::null();
        this.outer_frame = None;
        this.arena = RuntimeArena::with_escape(escape);
        let arena_ptr: *const RuntimeArena = &this.arena;
        // SAFETY: heap-pinned via the `Rc` we hold; pointer is stable for the Rc's lifetime.
        let arena_ref: &'static RuntimeArena = unsafe { &*arena_ptr };
        let mut child = Scope::child_under(outer_static);
        child.arena = arena_ref;
        let allocated: &Scope<'_> = arena_ref.alloc_scope(child);
        #[allow(clippy::unnecessary_cast)]
        let scope_ptr = allocated as *const Scope<'_> as *const Scope<'static>;
        this.scope_ptr = scope_ptr;
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

    #[test]
    fn null_singleton_returns_null_kobject() {
        let n = null_singleton();
        assert!(matches!(n, KObject::Null));
    }

    #[test]
    fn bool_singletons_return_correct_values() {
        let t = true_singleton();
        let f = false_singleton();
        assert!(matches!(t, KObject::Bool(true)));
        assert!(matches!(f, KObject::Bool(false)));
    }

    /// The unsafe `'static`→`'a` re-annotation must be sound on its own, with no
    /// `RuntimeArena` in scope at all — the singleton's storage is the static `NULL_HOLDER`.
    #[test]
    fn singleton_ref_independent_of_arena_lifetime() {
        let n: &KObject<'_> = null_singleton();
        assert!(matches!(n, KObject::Null));
    }

    /// Tree-borrows shared-read aliasing check: two simultaneous `&KObject` refs from
    /// the same singleton, both readable.
    #[test]
    fn singletons_aliasable() {
        let a = null_singleton();
        let b = null_singleton();
        assert!(matches!(a, KObject::Null));
        assert!(matches!(b, KObject::Null));
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
        let _new = frame.arena().alloc(KObject::Number(1.0));
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
        let it_obj: &KObject<'_> = inner_arena.alloc(KObject::Number(42.0));
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
        assert!(s1.outer.is_some());
    }

    /// Two-deep chain: dropping the local `outer` handle leaves only `inner.outer_frame`
    /// keeping the outer arena alive while we read through `inner.scope().outer`.
    #[test]
    fn call_arena_chained_outer_frame_walkable() {
        let arena = RuntimeArena::new();
        let run_scope = default_scope(&arena, Box::new(std::io::sink()));
        let outer = CallArena::new(run_scope, None);
        let inner = CallArena::new(outer.scope(), Some(outer.clone()));
        drop(outer);
        let outer_scope = inner
            .scope()
            .outer
            .expect("inner.scope().outer must be Some");
        assert!(std::ptr::eq(
            outer_scope.arena,
            inner.scope().outer.unwrap().arena
        ));
        assert!(outer_scope.outer.is_some());
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
        assert!(h.s.outer.is_some());
    }

    /// Allocating mutates `allocated_objects` via `RefCell::borrow_mut` while a prior
    /// `&KObject` from the same arena is shared-borrowed. Pins that tree-borrows shape.
    #[test]
    fn runtime_arena_alloc_while_prior_ref_live() {
        let a = RuntimeArena::new();
        let r1 = a.alloc(KObject::Number(1.0));
        let r2 = a.alloc(KObject::Number(2.0));
        assert!(matches!(r1, KObject::Number(n) if *n == 1.0));
        assert!(matches!(r2, KObject::Number(n) if *n == 2.0));
    }

    /// `alloc::<KType>` returns an arena-lifetime `&KType` and bumps `alloc_count` by one.
    #[test]
    fn alloc_ktype_returns_arena_lifetime_ref_and_counts() {
        let a = RuntimeArena::new();
        let baseline = a.alloc_count();
        let t: &KType = a.alloc(KType::Number);
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
        let _pre = frame.arena().alloc(KObject::Number(1.0));
        assert!(frame.arena().alloc_count() >= 1);

        let did_reset = frame.try_reset_for_tail(outer_scope);
        assert!(did_reset, "Rc was unique, reset must succeed");

        // Fresh arena: only the new child scope remains.
        assert_eq!(frame.arena().alloc_count(), 1);

        let v = frame.arena().alloc(KObject::Number(42.0));
        frame
            .scope()
            .bind_value("k".to_string(), v, BindingIndex::BUILTIN)
            .unwrap();
        assert!(matches!(frame.scope().lookup("k"), Some(KObject::Number(n)) if *n == 42.0));
        assert!(frame.scope().outer.is_some());
    }

    /// `try_reset_for_tail` must refuse when any other `Rc` to this frame
    /// exists (closure escape, sub-Dispatch clone, etc.). The escape-detection
    /// gate keeps reuse semantically equivalent to drop-and-alloc; without it
    /// a later tail step would mutate the captured arena under a live alias.
    #[test]
    fn call_arena_try_reset_for_tail_refuses_when_aliased() {
        let outer_arena = RuntimeArena::new();
        let outer_scope = default_scope(&outer_arena, Box::new(std::io::sink()));
        let mut frame: Rc<CallArena> = CallArena::new(outer_scope, None);
        let pre_arena_addr = frame.arena() as *const RuntimeArena as usize;

        // Simulate a closure escape: clone the Rc so strong_count > 1.
        let _alias = Rc::clone(&frame);

        let did_reset = frame.try_reset_for_tail(outer_scope);
        assert!(!did_reset, "aliased frame must refuse reset");

        assert_eq!(
            frame.arena() as *const RuntimeArena as usize,
            pre_arena_addr,
            "refused reset must leave arena pointer unchanged",
        );
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
        // `Rc<CallArena>` pointing at `frame.arena()`. The cycle gate only inspects the
        // carried `Rc`, so the placeholder `KFunction` body is irrelevant.
        let dummy_fn_obj = outer.alloc(KObject::KFunction(
            outer.alloc_function(crate::machine::core::kfunction::KFunction::new(
                crate::machine::model::types::ExpressionSignature {
                    return_type: crate::machine::model::types::ReturnType::Resolved(
                        crate::machine::model::types::KType::Null,
                    ),
                    elements: vec![crate::machine::model::types::SignatureElement::Keyword(
                        "DUMMY".into(),
                    )],
                },
                crate::machine::core::kfunction::Body::Builtin(|_, _, _| {
                    crate::machine::core::kfunction::BodyResult::Value(null_singleton())
                }),
                scope,
            )),
            None,
        ));
        let f_ref = match dummy_fn_obj {
            KObject::KFunction(f, _) => *f,
            _ => unreachable!(),
        };
        let cyclic_kfn = KObject::KFunction(f_ref, Some(Rc::clone(&frame)));
        let list = KObject::list(vec![cyclic_kfn]);

        let stored = frame.arena().alloc(list);
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
