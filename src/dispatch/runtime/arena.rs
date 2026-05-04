use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use typed_arena::Arena;

use crate::dispatch::kfunction::KFunction;
use crate::dispatch::values::KObject;
use super::scope::Scope;

/// Run-lifetime allocator. Constructed by `interpret`, lives for one program run, dropped at
/// run end.
///
/// **Lifetime erasure.** Internally the sub-arenas store `KObject<'static>` /
/// `KFunction<'static>` / `Scope<'static>`, but each `alloc_*` method takes input at the
/// caller's `'a` and returns a reference at the same `'a`. The 'static is a phantom — it lets
/// `RuntimeArena` itself have no lifetime parameter (which would otherwise force dropck to
/// keep the arena's borrow alive past its own drop), while the public API still tracks
/// lifetimes correctly. SAFETY of the transmutes:
/// - Lifetimes are zero-sized, so `KObject<'a>` and `KObject<'static>` have identical layout.
/// - Stored values cannot escape the arena: `alloc_*` returns `&'a` tied to the input borrow,
///   so the user can never observe a `'static` reference.
/// - When the arena drops, all stored values drop. None of them have user-defined `Drop`
///   impls that follow the lifetime-parameterized references; auto-derived drops only touch
///   *owned* contents (Strings, Vecs, HashMaps), never `&KFunction` borrows.
pub struct RuntimeArena {
    objects: Arena<KObject<'static>>,
    functions: Arena<KFunction<'static>>,
    scopes: Arena<Scope<'static>>,
}

impl RuntimeArena {
    pub fn new() -> Self {
        Self {
            objects: Arena::new(),
            functions: Arena::new(),
            scopes: Arena::new(),
        }
    }

    pub fn alloc_object<'a>(&'a self, obj: KObject<'a>) -> &'a KObject<'a> {
        let static_obj: KObject<'static> = unsafe {
            std::mem::transmute::<KObject<'a>, KObject<'static>>(obj)
        };
        let stored: &'a mut KObject<'static> = self.objects.alloc(static_obj);
        unsafe { std::mem::transmute::<&'a mut KObject<'static>, &'a KObject<'a>>(stored) }
    }

    /// INVARIANT: callers must allocate a `KFunction` into the same `RuntimeArena` that owns
    /// its `captured` scope. `lift_kobject`'s fast path in the scheduler relies on this: it
    /// skips the recursive Rc-attach walk when `functions_is_empty()` is true, on the
    /// reasoning that "no KFunction allocated here ⇒ no KFunction has captured_scope in this
    /// arena." If a future change ever allocates a KFunction into a different arena than its
    /// captured scope, that fast path will silently drop arenas out from under live
    /// `&KFunction` references and the invariant must be revisited. The `debug_assert!` below
    /// catches a violation at the allocation site rather than later as a use-after-free.
    pub fn alloc_function<'a>(&'a self, f: KFunction<'a>) -> &'a KFunction<'a> {
        debug_assert!(
            std::ptr::eq(self as *const RuntimeArena, f.captured_scope().arena as *const RuntimeArena),
            "alloc_function invariant: KFunction must be allocated into the same RuntimeArena \
             that owns its captured scope (lift_kobject's functions_is_empty fast path depends \
             on this)"
        );
        let static_f: KFunction<'static> = unsafe {
            std::mem::transmute::<KFunction<'a>, KFunction<'static>>(f)
        };
        let stored: &'a mut KFunction<'static> = self.functions.alloc(static_f);
        unsafe { std::mem::transmute::<&'a mut KFunction<'static>, &'a KFunction<'a>>(stored) }
    }

    pub fn alloc_scope<'a>(&'a self, s: Scope<'a>) -> &'a Scope<'a> {
        let static_s: Scope<'static> = unsafe {
            std::mem::transmute::<Scope<'a>, Scope<'static>>(s)
        };
        let stored: &'a mut Scope<'static> = self.scopes.alloc(static_s);
        unsafe { std::mem::transmute::<&'a mut Scope<'static>, &'a Scope<'a>>(stored) }
    }

    /// Whether the functions sub-arena holds zero `KFunction`s. Used by `lift_kobject`'s fast
    /// path: when true, no value can hold a `&KFunction` (whether directly via
    /// `KObject::KFunction` or indirectly via a `KFuture`'s `function` field) pointing into
    /// this arena, so the lift's recursive walk has nothing to attach an `Rc` to and can
    /// collapse to a plain `deep_clone`. `typed_arena::Arena::len()` is O(1).
    pub fn functions_is_empty(&self) -> bool { self.functions.len() == 0 }
}

impl Default for RuntimeArena {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
impl RuntimeArena {
    /// Total number of values stored across the three sub-arenas. Test-only — used by the
    /// per-call-arena leak regression test to prove the run-root arena's growth is bounded
    /// across many user-fn calls. `typed_arena::Arena::len()` is O(1) and counts allocations
    /// since arena construction.
    pub fn alloc_count(&self) -> usize {
        self.objects.len() + self.functions.len() + self.scopes.len()
    }
}

/// Wrapper that lets us put a `KObject` in a `static`. `KObject` isn't `Sync` because some
/// variants hold `Rc` / `Box<dyn Trait>`, but the only values we instantiate as statics are
/// `Null` and `Bool(_)` — both fully owned, no interior shared state, soundly shareable
/// across threads. The `unsafe impl Sync` is the explicit assertion of that fact.
struct StaticKObject(KObject<'static>);
unsafe impl Sync for StaticKObject {}

static NULL_HOLDER: StaticKObject = StaticKObject(KObject::Null);
static TRUE_HOLDER: StaticKObject = StaticKObject(KObject::Bool(true));
static FALSE_HOLDER: StaticKObject = StaticKObject(KObject::Bool(false));

/// Singleton `&KObject::Null`. Returned by `null_kobject()` for every type-mismatch /
/// missing-arg / lookup-miss path. `KObject<'a>` is invariant in `'a`, so the
/// `&'static`-typed singleton is reinterpreted to the caller's `'a` via `transmute`.
/// SAFETY: `KObject::Null` is a unit variant — no references inside, so the lifetime
/// parameter is purely phantom.
pub fn null_singleton<'a>() -> &'a KObject<'a> {
    unsafe { std::mem::transmute::<&'static KObject<'static>, &'a KObject<'a>>(&NULL_HOLDER.0) }
}

/// Singleton `&KObject::Bool(true)`. SAFETY: see `null_singleton` — `KObject::Bool` carries
/// only a `bool`, no references.
pub fn true_singleton<'a>() -> &'a KObject<'a> {
    unsafe { std::mem::transmute::<&'static KObject<'static>, &'a KObject<'a>>(&TRUE_HOLDER.0) }
}

/// Singleton `&KObject::Bool(false)`.
pub fn false_singleton<'a>() -> &'a KObject<'a> {
    unsafe { std::mem::transmute::<&'static KObject<'static>, &'a KObject<'a>>(&FALSE_HOLDER.0) }
}

/// One user-fn call's allocation frame. Owns its own `RuntimeArena` for the per-call child
/// `Scope`, parameter clones, and the substituted body's identifier→`Future` rewrites.
///
/// Reference-counted (`Rc<CallArena>`) so the arena's lifetime can be extended past slot
/// finalize when something else holds a reference — e.g., a closure that captured this
/// frame's scope and escaped via the body's return. The slot drops its Rc on finalize; if
/// no other Rc is held, the arena drops at that moment (matching the pre-Rc behavior). If
/// a closure carries a clone of the Rc, the arena lives until that closure is gone.
///
/// No `prev` chain: lexical scoping (closure capture in `KFunction::captured`) means each
/// per-call child's `outer` is the FN's *captured* scope (run-root for top-level FNs), not
/// the previous call's per-call scope. So previous frames hold no references the current
/// frame's child needs, and they can drop immediately on TCO replace.
///
/// SAFETY: `CallArena` is only ever heap-pinned via `Rc` (which boxes its inner). The
/// `arena` field's heap address is stable for the Rc's life, so the `scope_ptr` (which
/// points into `arena.scopes`) stays valid as long as the Rc is alive. Accessors `scope()`
/// and `arena()` re-attach lifetimes anchored to the borrow of `&CallArena`. Field
/// declaration order keeps `arena` first so the auto-derived `Drop` runs `arena`'s cleanup
/// before any later field — matches "inner allocations die before outer pointers."
pub struct CallArena {
    arena: RuntimeArena,
    scope_ptr: *const Scope<'static>,
}

impl CallArena {
    /// Build a fresh per-call frame whose child `Scope` uses `outer` as its `outer` link.
    /// `outer` is the FN's captured definition scope (or, for callers that don't have a
    /// captured scope, the call-site scope as a fallback). The returned `Rc` owns the arena
    /// and the child scope; `Node::frame` takes one clone, and any closure that escapes the
    /// call may take additional clones (Stages 2 & 3).
    pub fn new<'p>(outer: &'p Scope<'p>) -> Rc<CallArena> {
        // `Rc::new` heap-pins, so the inner `arena`'s address is stable for the Rc's
        // lifetime. Mutate `scope_ptr` after allocation via `Rc::get_mut` while we still
        // hold the unique reference (no clones yet).
        let mut rc = Rc::new(CallArena {
            arena: RuntimeArena::new(),
            scope_ptr: std::ptr::null(),
        });
        let arena_ptr: *const RuntimeArena = &rc.arena;
        // SAFETY: heap-pinning keeps `arena_ptr` valid for the Rc's lifetime, which exceeds
        // this function's duration; `outer` lives long enough by caller contract.
        let arena_ref: &'static RuntimeArena = unsafe { &*arena_ptr };
        let outer_static: &Scope<'static> = unsafe {
            std::mem::transmute::<&Scope<'_>, &Scope<'static>>(outer)
        };
        let child = Scope {
            outer: Some(outer_static),
            data: RefCell::new(HashMap::new()),
            functions: RefCell::new(HashMap::new()),
            out: RefCell::new(None),
            arena: arena_ref,
            pending: RefCell::new(Vec::new()),
        };
        let allocated: &Scope<'_> = arena_ref.alloc_scope(child);
        let scope_ptr = allocated as *const Scope<'_> as *const Scope<'static>;
        // Unique reference at this point — no clones exist yet. `get_mut` is safe.
        Rc::get_mut(&mut rc)
            .expect("freshly-constructed Rc has unique ownership")
            .scope_ptr = scope_ptr;
        rc
    }

    pub fn scope<'a>(&'a self) -> &'a Scope<'a> {
        unsafe {
            std::mem::transmute::<&Scope<'static>, &'a Scope<'a>>(&*self.scope_ptr)
        }
    }

    pub fn arena<'a>(&'a self) -> &'a RuntimeArena { &self.arena }
}
