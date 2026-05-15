use std::cell::RefCell;
use std::rc::Rc;

use typed_arena::Arena;

use crate::runtime::machine::core::kfunction::KFunction;
use crate::runtime::machine::model::types::KType;
use crate::runtime::machine::model::values::{KObject, Module, Signature};
use super::scope::Scope;
/// Run-lifetime allocator. Lives for one program run.
///
/// **Lifetime erasure.** Sub-arenas store `T<'static>`; each `alloc_*` takes input at the
/// caller's `'a` and returns `&'a T<'a>`. The `'static` is phantom so `RuntimeArena` itself
/// carries no lifetime parameter. SAFETY of the transmutes:
/// - Lifetimes are zero-sized, so `T<'a>` and `T<'static>` have identical layout.
/// - `alloc_*` returns `&'a` tied to the input borrow; no `'static` reference is observable.
/// - On drop, no stored value's `Drop` impl follows lifetime-parameterized references;
///   auto-derived drops only touch *owned* contents.
///
/// `escape` backs the cycle gate on `alloc_object`; see
/// [memory-model.md § Cycle gate](../../../../design/memory-model.md#cycle-gate-on-alloc_object).
pub struct RuntimeArena {
    objects: Arena<KObject<'static>>,
    functions: Arena<KFunction<'static>>,
    scopes: Arena<Scope<'static>>,
    modules: Arena<Module<'static>>,
    signatures: Arena<Signature<'static>>,
    /// `KType` has no lifetime parameter, so storage is direct — no `<'static>` erasure,
    /// no transmute apparatus in `alloc_ktype`. Backs the per-type identity binding storage
    /// (`Bindings::types` map) introduced in stage 1.2.
    ktypes: Arena<KType>,
    /// Stable addresses of every `KObject` allocated here. Backs `owns_object` membership
    /// queries via a linear scan (no deref, no borrow). `usize` rather than `*const _` keeps
    /// the field lifetime-erased and `Send`/`Sync`-neutral.
    allocated_objects: RefCell<Vec<usize>>,
    /// Redirect target for the `alloc_object` cycle gate. `None` on run-root.
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
            allocated_objects: RefCell::new(Vec::new()),
            escape: None,
        }
    }

    /// Construct a `RuntimeArena` whose `alloc_object` redirects self-cyclic values to
    /// `escape`.
    pub fn with_escape(escape: *const RuntimeArena) -> Self {
        Self {
            objects: Arena::new(),
            functions: Arena::new(),
            scopes: Arena::new(),
            modules: Arena::new(),
            signatures: Arena::new(),
            ktypes: Arena::new(),
            allocated_objects: RefCell::new(Vec::new()),
            escape: Some(escape),
        }
    }

    pub fn alloc_object<'a>(&'a self, obj: KObject<'a>) -> &'a KObject<'a> {
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
        self.allocated_objects
            .borrow_mut()
            .push(stored as *const _ as usize);
        unsafe { std::mem::transmute::<&'a mut KObject<'static>, &'a KObject<'a>>(stored) }
    }

    /// Whether `ptr` was returned by a prior `alloc_object` on this arena. Linear scan over
    /// `allocated_objects`.
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
            std::ptr::eq(self as *const RuntimeArena, f.captured_scope().arena as *const RuntimeArena),
            "alloc_function invariant :KFunction must be allocated into the same RuntimeArena \
             that owns its captured scope"
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

    pub fn alloc_module<'a>(&'a self, m: Module<'a>) -> &'a Module<'a> {
        let static_m: Module<'static> = unsafe {
            std::mem::transmute::<Module<'a>, Module<'static>>(m)
        };
        let stored: &'a mut Module<'static> = self.modules.alloc(static_m);
        unsafe { std::mem::transmute::<&'a mut Module<'static>, &'a Module<'a>>(stored) }
    }

    pub fn alloc_signature<'a>(&'a self, s: Signature<'a>) -> &'a Signature<'a> {
        let static_s: Signature<'static> = unsafe {
            std::mem::transmute::<Signature<'a>, Signature<'static>>(s)
        };
        let stored: &'a mut Signature<'static> = self.signatures.alloc(static_s);
        unsafe { std::mem::transmute::<&'a mut Signature<'static>, &'a Signature<'a>>(stored) }
    }

    /// Allocate a `KType` into the run-lifetime store. No lifetime erasure: `KType` carries
    /// no lifetime parameter, so storage is direct and the returned `&'a KType` is a plain
    /// coerce of `typed_arena::Arena::alloc`'s `&'a mut KType`. No `unsafe` is required.
    pub fn alloc_ktype(&self, t: KType) -> &KType {
        self.ktypes.alloc(t)
    }

    /// Whether the functions sub-arena holds zero `KFunction`s. When true, no value can hold
    /// a `&KFunction` pointing into this arena — see the `alloc_function` invariant.
    pub fn functions_is_empty(&self) -> bool { self.functions.len() == 0 }
}

impl Default for RuntimeArena {
    fn default() -> Self { Self::new() }
}

/// True iff any descendant of `obj` carries an `Rc<CallArena>` whose backing `RuntimeArena`
/// is `arena_ptr`. Walks the composite shapes mirrored from `KObject::deep_clone`
/// (`List`/`Dict`/`Tagged`/`Struct`) plus `KFunction`/`KFuture` anchors.
fn obj_anchors_to(obj: &KObject<'_>, arena_ptr: *const RuntimeArena) -> bool {
    fn rc_targets(rc: &Rc<CallArena>, arena_ptr: *const RuntimeArena) -> bool {
        std::ptr::eq(rc.arena() as *const RuntimeArena, arena_ptr)
    }
    match obj {
        KObject::KFunction(_, Some(rc)) => rc_targets(rc, arena_ptr),
        KObject::KFuture(_, Some(rc)) => rc_targets(rc, arena_ptr),
        KObject::KModule(_, Some(rc)) => rc_targets(rc, arena_ptr),
        KObject::List(items) => items.iter().any(|x| obj_anchors_to(x, arena_ptr)),
        KObject::Dict(entries) => entries.values().any(|x| obj_anchors_to(x, arena_ptr)),
        KObject::Tagged { value, .. } => obj_anchors_to(value, arena_ptr),
        KObject::Struct { fields, .. } => fields.values().any(|x| obj_anchors_to(x, arena_ptr)),
        _ => false,
    }
}

#[cfg(test)]
impl RuntimeArena {
    /// Total number of values stored across all six sub-arenas (test-only). Each `alloc_*`
    /// method writes to exactly one sub-arena, so this is the precise allocation count
    /// without double-counting — a `KObject::KModule(&Module, _)` value, for example, occupies
    /// one slot in `objects` and the referenced `&Module` occupies an independent slot in
    /// `modules`.
    pub fn alloc_count(&self) -> usize {
        self.objects.len()
            + self.functions.len()
            + self.scopes.len()
            + self.modules.len()
            + self.signatures.len()
            + self.ktypes.len()
    }
}

/// Static-singleton inhabitants. `KObject` itself isn't `Sync` (some variants carry `Rc` /
/// `Box<dyn Trait>`), but `Null` and `Bool(bool)` are unit-shaped — no references, no
/// interior shared state. Typing the static storage at this unit-only enum lets the
/// `NULL_HOLDER` / `TRUE_HOLDER` / `FALSE_HOLDER` statics derive `Sync` naturally, so no
/// dedicated `unsafe impl Sync` is needed for static storage.
///
/// The `Holder` statics are storage-only; the accessors below project the corresponding
/// `&KObject<'static>` from a `const KObject<'static>` item (a `const` is inlined per use
/// site and doesn't go through static storage, so `KObject: !Sync` is no obstacle). The
/// remaining `unsafe` in each accessor is the `'static → 'a` re-annotation, which is sound
/// because the carried variant holds no lifetime-parameterized data.
enum StaticKValue {
    Null,
    Bool(bool),
}

static NULL_HOLDER: StaticKValue = StaticKValue::Null;
static TRUE_HOLDER: StaticKValue = StaticKValue::Bool(true);
static FALSE_HOLDER: StaticKValue = StaticKValue::Bool(false);

/// Project the `KObject` view of a static `StaticKValue`. Lives at the boundary so the
/// `Holder` statics' typed inventory drives the accessor surface: any future addition to
/// `StaticKValue` is forced through here. `const` items inline at the use site, so the
/// returned reference is rvalue-promoted to `&'static KObject<'static>` without requiring
/// `KObject: Sync`.
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

/// Singleton `&KObject::Null`.
pub fn null_singleton<'a>() -> &'a KObject<'a> { project(&NULL_HOLDER) }

/// Singleton `&KObject::Bool(true)`.
pub fn true_singleton<'a>() -> &'a KObject<'a> { project(&TRUE_HOLDER) }

/// Singleton `&KObject::Bool(false)`.
pub fn false_singleton<'a>() -> &'a KObject<'a> { project(&FALSE_HOLDER) }

/// One user-fn call's allocation frame. Owns its own `RuntimeArena` for the per-call child
/// `Scope`, parameter clones, and substituted body rewrites. Reference-counted so an
/// escaping closure can extend the frame's life past slot finalize; with no extra Rc the
/// arena drops at finalize.
///
/// `outer_frame` keeps the parent frame's `Rc<CallArena>` alive when the child's `outer`
/// points into a per-call arena. `None` when the parent is run-root (which outlives every
/// per-call frame, so no chain Rc is needed and TCO recursion stays bounded).
///
/// SAFETY: `CallArena` is only heap-pinned via `Rc`, so `arena`'s heap address is stable
/// for the Rc's life and `scope_ptr` (into `arena.scopes`) stays valid alongside it.
/// Accessors re-attach lifetimes anchored to `&self`. Field declaration order keeps
/// `arena` before `outer_frame` so the auto-derived `Drop` tears down this arena's
/// allocations before releasing the parent Rc — inner pointers die before the outer
/// storage they may reference.
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
        let outer_static: &Scope<'static> = unsafe {
            std::mem::transmute::<&Scope<'_>, &Scope<'static>>(outer)
        };
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
        unsafe {
            std::mem::transmute::<&Scope<'static>, &'a Scope<'a>>(&*self.scope_ptr)
        }
    }

    /// Scope handle whose borrow lifetime is statically tied to `&self`. Use this when
    /// feeding the per-call scope into local-bind plumbing (e.g. [`Scope::bind_value`])
    /// that does not need to escape the `Rc`'s borrow.
    ///
    /// Unlike [`CallArena::scope`], the returned reference is bounded by `'p` (the
    /// `&'p Rc<Self>` receiver's borrow), so the caller does not need an `unsafe`
    /// `'a`-anchoring transmute to feed it into `'p`-lifetime APIs. The single internal
    /// transmute converts the `'static`-erased `scope_ptr` storage back to the receiver's
    /// borrow lifetime — strictly shorter than the broader `&'a Scope<'a>` claim that
    /// [`CallArena::scope`] makes.
    ///
    /// SAFETY: `scope_ptr` is stable for the `Rc`'s lifetime (heap-pinned by `Rc`); the
    /// returned `'p` is bounded by `&'p Rc<Self>` so the borrow cannot outlive the
    /// receiver.
    pub fn scope_for_bind<'p>(self: &'p Rc<Self>) -> &'p Scope<'p> {
        unsafe {
            std::mem::transmute::<&Scope<'static>, &'p Scope<'p>>(&*self.scope_ptr)
        }
    }

    pub fn arena(&self) -> &RuntimeArena { &self.arena }
}

#[cfg(test)]
mod tests {
    //! Targeted Miri coverage for the unsafe sites in this file. Each test pins down a
    //! specific aliasing/lifetime shape under tree borrows; logical assertions are minimal
    //! — these tests fail when Miri reports UB, not on values.

    use super::*;
    use crate::runtime::builtins::default_scope;
    use crate::runtime::machine::model::types::KType;

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
    /// sibling pointer afterward — `frame.scope()` and `frame.arena().alloc_object(...)`
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

    /// Raw-pointer roundtrip: extract `*const RuntimeArena` and `*const Scope<'_>` from a
    /// frame, transmute via `&*ptr` to lifetime-anchored refs, mutate the arena through
    /// one ref while the other is still live, then read through the held child ref.
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
        child.bind_value("it".to_string(), it_obj).unwrap();
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

    /// Re-anchor `frame.scope()` via transmute, move it into a struct alongside the
    /// frame's Rc, drop the local Rc handle, then read the re-anchored ref through the
    /// struct field — the in-struct Rc must keep the arena alive for `h.s`.
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

    /// `alloc_ktype` returns an arena-lifetime `&KType` and bumps `alloc_count` by one. Pins
    /// the new sub-arena's accounting alongside the no-`unsafe`, no-transmute storage path.
    #[test]
    fn alloc_ktype_returns_arena_lifetime_ref_and_counts() {
        let a = RuntimeArena::new();
        let baseline = a.alloc_count();
        let t: &KType = a.alloc_ktype(KType::Number);
        assert!(matches!(t, KType::Number));
        assert_eq!(a.alloc_count(), baseline + 1);
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
        let dummy_fn_obj = outer.alloc_object(KObject::KFunction(
            outer.alloc_function(crate::runtime::machine::core::kfunction::KFunction::new(
                crate::runtime::machine::model::types::ExpressionSignature {
                    return_type: crate::runtime::machine::model::types::ReturnType::Resolved(crate::runtime::machine::model::types::KType::Null),
                    elements: vec![crate::runtime::machine::model::types::SignatureElement::Keyword("DUMMY".into())],
                },
                crate::runtime::machine::core::kfunction::Body::Builtin(|_, _, _|
                    crate::runtime::machine::core::kfunction::BodyResult::Value(null_singleton())),
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
