use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use typed_arena::Arena;

use crate::dispatch::kfunction::KFunction;
use crate::dispatch::values::{KObject, Module, Signature};
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
///
/// **Escape arena.** `escape` is the address of the next-outer arena (run-root or another
/// per-call arena's). `alloc_object` redirects to `escape`'s `alloc_object` whenever the
/// incoming value carries an `Rc<CallArena>` whose `arena()` is `self` — that combination
/// would otherwise produce a self-referential Rc cycle (the in-arena `KObject` keeps the
/// arena alive via the Rc, the arena keeps the `KObject` alive via its storage, neither
/// can drop). Set by `CallArena::new` to the outer scope's arena; `None` for run-root,
/// which has no outer to escape to. The cycle case shows up when a body returns a
/// composite (List/Dict/Tagged/Struct) holding an escaping closure: the lift-on-return
/// machinery attaches the per-call frame's Rc to the closure, then a re-allocation of the
/// composite (via `value_pass`, `Aggregate`, etc.) lands the composite back in the per-call
/// arena. The redirect short-circuits that landing into the outer arena, where the Rc is
/// no longer self-referential.
pub struct RuntimeArena {
    objects: Arena<KObject<'static>>,
    functions: Arena<KFunction<'static>>,
    scopes: Arena<Scope<'static>>,
    modules: Arena<Module<'static>>,
    signatures: Arena<Signature<'static>>,
    /// Addresses (as `usize`) of every `KObject` ever allocated into `objects`. Used by
    /// `owns_object` so `lift_kobject`'s KFuture arm can ask "does this `&KObject` borrow
    /// point into my arena?" without a full traversal — answer is a single linear scan over
    /// addresses (no deref, no borrow). Addresses go in once at `alloc_object` and never
    /// move (typed-arena allocations are stable for the arena's life), so the membership
    /// check is sound for the arena's lifetime. Stored as `usize` rather than `*const _` so
    /// the field is `Send`/`Sync`-neutral and lifetime-erased like the rest of the arena.
    allocated_objects: RefCell<Vec<usize>>,
    /// Outer arena address used as the redirect target by `alloc_object` whenever the
    /// incoming value carries an `Rc<CallArena>` pointing at `self` (a self-referential
    /// cycle). `None` on run-root; `Some(outer.arena)` on per-call arenas constructed via
    /// `CallArena::new`. Stored as a raw pointer so `RuntimeArena` stays lifetime-erased;
    /// the address is stable because `CallArena::new` heap-pins the outer arena via `Rc`,
    /// and the outer always outlives this inner per the lexical-scoping invariant.
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
            allocated_objects: RefCell::new(Vec::new()),
            escape: None,
        }
    }

    /// Construct a `RuntimeArena` whose `alloc_object` redirects self-cyclic values to
    /// `escape`. Used by `CallArena::new` to build the per-call arena that escapes into the
    /// outer scope's arena.
    pub fn with_escape(escape: *const RuntimeArena) -> Self {
        Self {
            objects: Arena::new(),
            functions: Arena::new(),
            scopes: Arena::new(),
            modules: Arena::new(),
            signatures: Arena::new(),
            allocated_objects: RefCell::new(Vec::new()),
            escape: Some(escape),
        }
    }

    pub fn alloc_object<'a>(&'a self, obj: KObject<'a>) -> &'a KObject<'a> {
        // Cycle gate: if `obj` carries an `Rc<CallArena>` pointing at `self` (anywhere in
        // its tree), storing it here would create a self-referential cycle the typed-arena
        // can't break — neither the arena nor the value can drop. Redirect to the escape
        // arena (the outer scope's arena), where the Rc is no longer self-referential.
        // Run-root has `escape: None` and the value would be cycle-free at run-root by
        // construction (the `Rc<CallArena>`s the cycle gate looks for can only point at
        // per-call arenas), so the gate doesn't fire there.
        if let Some(escape_ptr) = self.escape {
            let self_ptr = self as *const RuntimeArena;
            if obj_anchors_to(&obj, self_ptr) {
                // SAFETY: `escape_ptr` was set by `CallArena::new` to the outer scope's arena
                // address. The outer arena outlives `self` per the lexical-scoping invariant
                // (per-call frames are nested inside their captured definition scope's arena);
                // `Rc<CallArena>` keeps the chain pinned. So `'a` (bounded by `&self`) is a
                // valid lifetime to attach to `&*escape_ptr`.
                let escape_ref: &'a RuntimeArena = unsafe { &*escape_ptr };
                return escape_ref.alloc_object(obj);
            }
        }
        let static_obj: KObject<'static> = unsafe {
            std::mem::transmute::<KObject<'a>, KObject<'static>>(obj)
        };
        let stored: &'a mut KObject<'static> = self.objects.alloc(static_obj);
        // Record the stable address so `owns_object` can answer membership queries from
        // `lift_kobject`'s KFuture arm. Address is stable for the arena's life because
        // `typed_arena::Arena` never moves an allocated value.
        self.allocated_objects
            .borrow_mut()
            .push(stored as *const _ as usize);
        unsafe { std::mem::transmute::<&'a mut KObject<'static>, &'a KObject<'a>>(stored) }
    }

    /// Whether `ptr` was returned by a prior `alloc_object` on this arena. Used by
    /// `lift_kobject`'s KFuture arm to decide whether an unanchored KFuture's bundle/parsed
    /// borrows reach into the dying arena: if any embedded `Future(&KObject)` answers `true`
    /// here, the lift attaches a chain Rc; otherwise it leaves `frame: None`. Linear scan
    /// over `allocated_objects` — typically tens to hundreds of entries per per-call arena,
    /// dwarfed by the lift's recursion cost.
    pub fn owns_object<'a>(&self, ptr: *const KObject<'a>) -> bool {
        // Lifetime-erased identity comparison — `allocated_objects` stores raw addresses, so
        // we cast through `*const KObject<'static>` to match. `KObject` is invariant in `'a`,
        // so the through-`'static` cast is required despite clippy's complaint.
        #[allow(clippy::unnecessary_cast)]
        let target = ptr as *const KObject<'static> as usize;
        self.allocated_objects.borrow().contains(&target)
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

    /// Arena-allocate a [`Module`] (module-system stage 1). Same lifetime-erasure shape as
    /// `alloc_function`/`alloc_scope`: the public API tracks `'a`; internal storage is
    /// `'static`. The returned reference is stable for the arena's lifetime — `KObject::KModule`
    /// captures it directly and shares it cheaply across clones.
    pub fn alloc_module<'a>(&'a self, m: Module<'a>) -> &'a Module<'a> {
        let static_m: Module<'static> = unsafe {
            std::mem::transmute::<Module<'a>, Module<'static>>(m)
        };
        let stored: &'a mut Module<'static> = self.modules.alloc(static_m);
        unsafe { std::mem::transmute::<&'a mut Module<'static>, &'a Module<'a>>(stored) }
    }

    /// Arena-allocate a [`Signature`] (module-system stage 1). Same lifetime-erasure shape
    /// as `alloc_module`.
    pub fn alloc_signature<'a>(&'a self, s: Signature<'a>) -> &'a Signature<'a> {
        let static_s: Signature<'static> = unsafe {
            std::mem::transmute::<Signature<'a>, Signature<'static>>(s)
        };
        let stored: &'a mut Signature<'static> = self.signatures.alloc(static_s);
        unsafe { std::mem::transmute::<&'a mut Signature<'static>, &'a Signature<'a>>(stored) }
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

/// True iff any descendant of `obj` carries an `Rc<CallArena>` whose backing `RuntimeArena`
/// is `arena_ptr`. The cycle gate in `RuntimeArena::alloc_object` uses this to decide
/// whether the incoming value would land back in the arena it already anchors to — the
/// classic self-referential Rc-cycle shape. Walks the same composite shapes
/// `KObject::deep_clone` does (`List`/`Dict`/`Tagged`/`Struct`) plus `KFuture`'s anchor.
/// Bottoms out on the first hit (`any`-style).
fn obj_anchors_to(obj: &KObject<'_>, arena_ptr: *const RuntimeArena) -> bool {
    fn rc_targets(rc: &Rc<CallArena>, arena_ptr: *const RuntimeArena) -> bool {
        std::ptr::eq(rc.arena() as *const RuntimeArena, arena_ptr)
    }
    match obj {
        KObject::KFunction(_, Some(rc)) => rc_targets(rc, arena_ptr),
        KObject::KFuture(_, Some(rc)) => rc_targets(rc, arena_ptr),
        KObject::List(items) => items.iter().any(|x| obj_anchors_to(x, arena_ptr)),
        KObject::Dict(entries) => entries.values().any(|x| obj_anchors_to(x, arena_ptr)),
        KObject::Tagged { value, .. } => obj_anchors_to(value, arena_ptr),
        KObject::Struct { fields, .. } => fields.values().any(|x| obj_anchors_to(x, arena_ptr)),
        _ => false,
    }
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
/// `outer_frame` keeps the parent frame's `Rc<CallArena>` alive when the child scope's
/// `outer` points into a per-call arena (rather than run-root). User-fn invokes whose
/// captured scope is run-root pass `None` here — the captured chain ends in run-root, which
/// outlives the run, so no chain Rc is needed and TCO recursion stays bounded. MATCH-style
/// builtins whose new frame's outer is the call-site (per-call) scope MUST pass the call-
/// site frame's Rc here, so the call-site arena stays alive while the new frame's `outer`
/// pointer is in use. Closure-captured scopes that themselves live in a per-call arena will
/// pass that arena's Rc here when the planned closure-escape support lands.
///
/// SAFETY: `CallArena` is only ever heap-pinned via `Rc` (which boxes its inner). The
/// `arena` field's heap address is stable for the Rc's life, so the `scope_ptr` (which
/// points into `arena.scopes`) stays valid as long as the Rc is alive. Accessors `scope()`
/// and `arena()` re-attach lifetimes anchored to the borrow of `&CallArena`. Field
/// declaration order keeps `arena` first so the auto-derived `Drop` runs `arena`'s cleanup
/// before any later field — matches "inner allocations die before outer pointers." The
/// `outer_frame` Rc declared last drops after `arena`, releasing the parent only after this
/// arena (and any scopes it owns) are already torn down.
pub struct CallArena {
    arena: RuntimeArena,
    scope_ptr: *const Scope<'static>,
    outer_frame: Option<Rc<CallArena>>,
}

impl CallArena {
    /// Build a fresh per-call frame whose child `Scope` uses `outer` as its `outer` link.
    /// `outer` is the FN's captured definition scope (or, for callers that don't have a
    /// captured scope, the call-site scope as a fallback). `outer_frame` is the parent
    /// frame's `Rc<CallArena>` when the parent is per-call (must keep its arena alive while
    /// this child's `outer` pointer is in use); `None` when the parent is run-root (which
    /// outlives every per-call frame). The returned `Rc` owns the arena and the child
    /// scope; `Node::frame` takes one clone, and any closure that escapes the call may take
    /// additional clones (Stages 2 & 3).
    pub fn new<'p>(outer: &'p Scope<'p>, outer_frame: Option<Rc<CallArena>>) -> Rc<CallArena> {
        // `Rc::new` heap-pins, so the inner `arena`'s address is stable for the Rc's
        // lifetime. Mutate `scope_ptr` after allocation via `Rc::get_mut` while we still
        // hold the unique reference (no clones yet). The inner arena's `escape` is the
        // outer scope's arena: `alloc_object` redirects self-cyclic values up the chain so
        // an escaping closure stored in a composite doesn't form an Rc<-->arena loop.
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
            name: String::new(),
        };
        let allocated: &Scope<'_> = arena_ref.alloc_scope(child);
        // `Scope` is invariant in `'a`, so the through-`'static` cast is required to match
        // `scope_ptr`'s `*const Scope<'static>` field type — clippy's "unnecessary cast"
        // complaint is wrong.
        #[allow(clippy::unnecessary_cast)]
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

    pub fn arena(&self) -> &RuntimeArena { &self.arena }
}

#[cfg(test)]
mod tests {
    //! Targeted Miri coverage for arena.rs unsafe sites. Each test pins down a specific
    //! shape (singleton transmute, `CallArena::scope` re-borrow, interleaved alloc) under
    //! `MIRIFLAGS="-Zmiri-tree-borrows"`. Logical assertions are deliberately minimal —
    //! the values are deterministic; the tests fail when Miri reports UB, not on values.

    use super::*;
    use crate::dispatch::builtins::default_scope;

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
    /// sibling pointer afterward. The frame's `arena` field is heap-pinned by the Rc, so
    /// `frame.scope()` and `frame.arena().alloc_object(...)` must coexist soundly under
    /// tree borrows.
    #[test]
    fn call_arena_scope_survives_subsequent_alloc() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let frame = CallArena::new(scope, None);
        let s = frame.scope();
        let _new = frame.arena().alloc_object(KObject::Number(1.0));
        assert!(std::ptr::eq(s.arena, frame.arena()));
    }

    /// Mirror of match_case.rs:83-94: `*const RuntimeArena` and `*const Scope<'_>` are
    /// extracted via `frame.arena()` / `frame.scope()`, transmuted via `&*ptr` to
    /// lifetime-anchored refs, then `inner_arena.alloc_object(...)` mutates while the
    /// child scope ref is still held; afterwards the child is read. This is the strict
    /// shape the indirect MATCH program tests already exercise — pinned in isolation here.
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
        child.add("it".to_string(), it_obj);
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

    /// Two-deep `outer_frame` chain. Drop the local `outer` Rc handle before reading
    /// through `inner.scope().outer.unwrap()` — at that point only `inner.outer_frame`
    /// keeps the outer arena alive.
    #[test]
    fn call_arena_chained_outer_frame_walkable() {
        let arena = RuntimeArena::new();
        let run_scope = default_scope(&arena, Box::new(std::io::sink()));
        let outer = CallArena::new(run_scope, None);
        let inner = CallArena::new(outer.scope(), Some(outer.clone()));
        drop(outer);
        let outer_scope = inner.scope().outer.expect("inner.scope().outer must be Some");
        assert!(std::ptr::eq(outer_scope.arena, inner.scope().outer.unwrap().arena));
        assert!(outer_scope.outer.is_some());
    }

    /// Mirror of scheduler.rs:251-263: re-anchor `frame.scope()` via transmute, move it
    /// into a struct alongside the frame's Rc, drop the local Rc handle, then read the
    /// re-anchored ref through the struct field. The Rc inside `Holder` keeps the arena
    /// alive; `h.s` is anchored to the holder's lifetime.
    #[test]
    fn call_arena_scope_re_anchored_into_struct_alongside_rc() {
        struct Holder<'a> { s: &'a Scope<'a>, _f: Rc<CallArena> }

        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let h = {
            let f = CallArena::new(scope, None);
            let s: &Scope<'_> = unsafe {
                std::mem::transmute::<&Scope<'_>, &Scope<'_>>(f.scope())
            };
            Holder { s, _f: f }
        };
        assert!(h.s.outer.is_some());
    }

    /// `RuntimeArena::alloc_object` does `RefCell::borrow_mut` on `allocated_objects`
    /// while a prior `&KObject` from the same arena is shared-borrowed. Typed-arena
    /// promises stable addresses, but tree borrows is sensitive to interleaved mutation
    /// under live shared borrows — pin the shape down.
    #[test]
    fn runtime_arena_alloc_while_prior_ref_live() {
        let a = RuntimeArena::new();
        let r1 = a.alloc_object(KObject::Number(1.0));
        let r2 = a.alloc_object(KObject::Number(2.0));
        assert!(matches!(r1, KObject::Number(n) if *n == 1.0));
        assert!(matches!(r2, KObject::Number(n) if *n == 2.0));
    }

    /// Cycle gate: alloc'ing a value that anchors back at the receiving arena via an
    /// `Rc<CallArena>` redirects to the escape arena (the outer scope's arena). The leak
    /// audit pinned this as the only cycle shape that closure-bearing-composite returns
    /// can produce; without the redirect the per-call arena's storage would hold an Rc
    /// to itself and never drop.
    #[test]
    fn alloc_object_redirects_self_anchored_value_to_escape_arena() {
        let outer = RuntimeArena::new();
        let scope = default_scope(&outer, Box::new(std::io::sink()));
        let frame: Rc<CallArena> = CallArena::new(scope, None);
        // Build a List whose only element is a `KFunction` carrying an `Rc<CallArena>`
        // pointing at `frame.arena()`. Use an arbitrary `&KFunction` reference — the
        // redirect logic only inspects the carried `Rc`, not the function itself.
        let dummy_fn_obj = outer.alloc_object(KObject::KFunction(
            // Allocate a placeholder KFunction in `outer` so we have a `&'a KFunction<'a>`.
            // Body content is irrelevant — the cycle gate only inspects `Rc<CallArena>`.
            outer.alloc_function(crate::dispatch::kfunction::KFunction::new(
                crate::dispatch::types::ExpressionSignature {
                    return_type: crate::dispatch::types::KType::Null,
                    elements: vec![crate::dispatch::types::SignatureElement::Keyword("DUMMY".into())],
                },
                crate::dispatch::kfunction::Body::Builtin(|_, _, _|
                    crate::dispatch::kfunction::BodyResult::Value(null_singleton())),
                scope,
            )),
            None,
        ));
        let f_ref = match dummy_fn_obj {
            KObject::KFunction(f, _) => *f,
            _ => unreachable!(),
        };
        let cyclic_kfn = KObject::KFunction(f_ref, Some(Rc::clone(&frame)));
        let list = KObject::List(std::rc::Rc::new(vec![cyclic_kfn]));

        // Allocating `list` into `frame.arena()` (the escape-aware arena) must redirect
        // to `outer`. Use `owns_object` to verify the resulting reference's address.
        let stored = frame.arena().alloc_object(list);
        let stored_ptr = stored as *const KObject<'_>;
        assert!(
            outer.owns_object(stored_ptr),
            "self-anchored alloc_object should redirect to the escape arena (outer)",
        );
        assert!(
            !frame.arena().owns_object(stored_ptr),
            "self-anchored value must not land in the per-call arena",
        );
    }
}
