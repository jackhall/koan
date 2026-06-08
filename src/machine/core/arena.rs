use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::ptr::NonNull;
use std::rc::Rc;

use typed_arena::Arena;

use super::scope::Scope;
use super::scope_ptr::ScopePtr;
use crate::machine::core::kfunction::KFunction;
use crate::machine::model::operators::OperatorGroup;
use crate::machine::model::types::KType;
use crate::machine::model::values::{Held, KObject, Module, Signature};
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
    /// outer outlives this inner per the lexical-scoping invariant. `NonNull` because a
    /// `Some` escape is always a live arena address, never null.
    escape: Option<NonNull<RuntimeArena>>,
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

    /// `alloc` will redirect self-cyclic values to `escape`; see the `ArenaStored` engine.
    pub fn with_escape(escape: NonNull<RuntimeArena>) -> Self {
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

    /// Whether `ptr` was returned by a prior `alloc_object` on this arena.
    pub fn owns_object<'a>(&self, ptr: *const KObject<'a>) -> bool {
        // `KObject` is invariant in `'a`, so the through-`'static` cast is required despite
        // clippy's complaint.
        #[allow(clippy::unnecessary_cast)]
        let target = ptr as *const KObject<'static> as usize;
        self.allocated_objects.borrow().contains(&target)
    }

    /// Store a [`KObject`] into the run-lifetime arena, routing through the cycle gate (a
    /// self-anchoring value redirects to the escape arena; see the private `alloc` engine).
    pub fn alloc_object<'a>(&'a self, o: KObject<'a>) -> &'a KObject<'a> {
        self.alloc::<KObject<'static>>(o)
    }

    /// Store a [`KType`] into the run-lifetime arena, routing through the cycle gate (a
    /// `Module` frame anchoring back at `self` redirects to the escape arena; see the
    /// private `alloc` engine).
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

mod sealed {
    /// Sealed supertrait of [`super::ArenaStored`]: only the impls in this module can route a
    /// value through [`super::RuntimeArena::alloc`], so no out-of-module type can supply a
    /// bogus `anchors_to` and bypass the self-cycle redirect, nor a bogus `sub_arena` and
    /// store into the wrong layout family.
    pub trait Sealed {}
}

/// Per-family plumbing for the run-lifetime allocator, keyed on the stored type's `'static`
/// form (`Self::At<'static> == Self`). One trait carries every storage-safety answer for a
/// family — which sub-arena it lands in, whether it would self-cycle, and any post-store
/// side effect — so [`RuntimeArena::alloc`] reasons about the gate-erase-store sequence once
/// instead of forking it across six per-type methods. A new arena-stored type joins by
/// implementing this trait, not by copying a transmute pair, so the erasure cannot drift
/// between sites. Sealed: only the six in-module impls may supply these answers.
trait ArenaStored: Sized + 'static + sealed::Sealed {
    /// The lifetime family of the stored type. `At<'static>` is `Self`; a live value enters
    /// the engine as `At<'a>`. Because the engine keys on the `'static` form, the live and
    /// stored forms are both projections of this one GAT and cannot name different
    /// constructors — a wrong binding fails to compile in the safe wrapper.
    type At<'x>;
    /// The sub-arena this family stores into. This field type is the binding chokepoint:
    /// storing `At<'static>` into `Arena<Self::At<'static>>` only type-checks when the family
    /// is wired to the matching sub-arena.
    fn sub_arena(a: &RuntimeArena) -> &Arena<Self::At<'static>>;
    /// True iff any descendant of `value` carries an `Rc<CallArena>` whose backing
    /// `RuntimeArena` is `arena_ptr` — i.e. storing `value` there would form a
    /// self-referential cycle. Required (no default): the four non-cycling families return
    /// `false` as a deliberate declaration that they hold no arena anchors.
    fn anchors_to(value: &Self::At<'_>, arena_ptr: *const RuntimeArena) -> bool;
    /// Post-store hook, run inside the engine on the *final* storing arena (after any escape
    /// redirect). Default no-op; `KObject` overrides it to record the stored address for
    /// `owns_object` membership queries.
    fn record_local(_a: &RuntimeArena, _stored: &Self::At<'static>) {}
}

/// Lifetime-erase a stored value's live form to its `'static` form by moving it through a
/// union. A generic `mem::transmute::<K::At<'a>, K::At<'static>>` will not compile — the
/// compiler cannot prove the two GAT projections share a size — so the move-through-union
/// form stands in, with a `const` assert restoring the size check `transmute` would emit.
fn erase_store<'a, K: ArenaStored>(value: K::At<'a>) -> K::At<'static> {
    const { assert!(size_of::<K::At<'a>>() == size_of::<K::At<'static>>()) };
    union Erase<A, B> {
        live: ManuallyDrop<A>,
        stored: ManuallyDrop<B>,
    }
    let e = Erase::<K::At<'a>, K::At<'static>> {
        live: ManuallyDrop::new(value),
    };
    // SAFETY: `At<'a>` and `At<'static>` share layout — a lifetime never changes a type's
    // size or representation. The value is moved into the union once and exactly one
    // `ManuallyDrop` field is read out, so a single drop runs (no leak, no double-free).
    ManuallyDrop::into_inner(unsafe { std::ptr::read(&e.stored) })
}

impl sealed::Sealed for KObject<'_> {}
impl sealed::Sealed for KType<'_> {}
impl sealed::Sealed for KFunction<'_> {}
impl sealed::Sealed for Scope<'_> {}
impl sealed::Sealed for Module<'_> {}
impl sealed::Sealed for Signature<'_> {}

impl ArenaStored for KObject<'static> {
    type At<'x> = KObject<'x>;
    fn sub_arena(a: &RuntimeArena) -> &Arena<KObject<'static>> {
        &a.objects
    }
    fn anchors_to(value: &KObject<'_>, arena_ptr: *const RuntimeArena) -> bool {
        obj_anchors_to(value, arena_ptr)
    }
    fn record_local(a: &RuntimeArena, stored: &KObject<'static>) {
        a.allocated_objects
            .borrow_mut()
            .push(stored as *const _ as usize);
    }
}

impl ArenaStored for KType<'static> {
    type At<'x> = KType<'x>;
    fn sub_arena(a: &RuntimeArena) -> &Arena<KType<'static>> {
        &a.ktypes
    }
    fn anchors_to(value: &KType<'_>, arena_ptr: *const RuntimeArena) -> bool {
        ktype_anchors_to(value, arena_ptr)
    }
}

impl ArenaStored for KFunction<'static> {
    type At<'x> = KFunction<'x>;
    fn sub_arena(a: &RuntimeArena) -> &Arena<KFunction<'static>> {
        &a.functions
    }
    fn anchors_to(_value: &KFunction<'_>, _arena_ptr: *const RuntimeArena) -> bool {
        false
    }
}

impl ArenaStored for Scope<'static> {
    type At<'x> = Scope<'x>;
    fn sub_arena(a: &RuntimeArena) -> &Arena<Scope<'static>> {
        &a.scopes
    }
    fn anchors_to(_value: &Scope<'_>, _arena_ptr: *const RuntimeArena) -> bool {
        false
    }
}

impl ArenaStored for Module<'static> {
    type At<'x> = Module<'x>;
    fn sub_arena(a: &RuntimeArena) -> &Arena<Module<'static>> {
        &a.modules
    }
    fn anchors_to(_value: &Module<'_>, _arena_ptr: *const RuntimeArena) -> bool {
        false
    }
}

impl ArenaStored for Signature<'static> {
    type At<'x> = Signature<'x>;
    fn sub_arena(a: &RuntimeArena) -> &Arena<Signature<'static>> {
        &a.signatures
    }
    fn anchors_to(_value: &Signature<'_>, _arena_ptr: *const RuntimeArena) -> bool {
        false
    }
}

impl RuntimeArena {
    /// Single allocator engine for any `K: ArenaStored`. Runs the cycle gate — a value that
    /// would self-cycle (an `Rc<CallArena>` pointing back at `self`) redirects to the escape
    /// arena — then erases the live form to `'static`, stores it in the family's sub-arena,
    /// fires `record_local` on the final storing arena, and re-anchors the store to `'a`.
    /// Private: the only entry points are the named `alloc_*` wrappers.
    ///
    /// SAFETY of the `escape_ptr.as_ref()`: `escape_ptr` was set by `CallArena::new` to the
    /// outer scope's arena address. The outer arena outlives `self` per the lexical-scoping
    /// invariant (per-call frames nest inside their captured definition scope's arena);
    /// `Rc<CallArena>` keeps the chain pinned. So `'a` (bounded by `&self`) is a valid
    /// lifetime to attach to the dereferenced escape pointer.
    fn alloc<'a, K: ArenaStored>(&'a self, value: K::At<'a>) -> &'a K::At<'a> {
        if let Some(escape_ptr) = self.escape {
            if K::anchors_to(&value, self as *const RuntimeArena) {
                let escape_ref: &'a RuntimeArena = unsafe { escape_ptr.as_ref() };
                return escape_ref.alloc::<K>(value);
            }
        }
        let stored: &'a mut K::At<'static> = K::sub_arena(self).alloc(erase_store::<K>(value));
        let p: *const K::At<'static> = stored;
        // The post-store hook fires on the final storing arena (this one, after any redirect
        // above), so a `KObject`'s recorded address tracks its true owner.
        K::record_local(self, unsafe { &*p });
        // SAFETY: `At<'static>`/`At<'a>` share layout; re-anchor the `'static` store to the
        // arena-bounded `'a`. The returned `&'a` cannot outlive `&'a self`, so no
        // `'static`-claiming reference escapes the arena's own borrow.
        //
        // The `'static` → `'a` cast only changes the lifetime parameter, which clippy can't
        // see, so it reads as a no-op cast despite being load-bearing.
        #[allow(clippy::unnecessary_cast)]
        unsafe {
            &*(p as *const K::At<'a>)
        }
    }
}

#[cfg(test)]
impl RuntimeArena {
    /// Total number of values stored across all seven sub-arenas (test-only). Each `alloc_*`
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
    scope_ptr: Option<ScopePtr<'static>>,
    outer_frame: Option<Rc<CallArena>>,
}

impl CallArena {
    /// Build a fresh per-call frame whose child `Scope` uses `outer` as its `outer` link.
    /// `outer_frame` must hold the parent's Rc when the parent is per-call; `None` when
    /// the parent is run-root.
    pub fn new<'p>(outer: &'p Scope<'p>, outer_frame: Option<Rc<CallArena>>) -> Rc<CallArena> {
        let escape = NonNull::from(outer.arena);
        let mut rc = Rc::new(CallArena {
            arena: RuntimeArena::with_escape(escape),
            scope_ptr: None,
            outer_frame,
        });
        let arena_ptr: *const RuntimeArena = &rc.arena;
        // SAFETY: heap-pinning keeps `arena_ptr` valid for the Rc's lifetime, which exceeds
        // this function's duration; `outer` lives long enough by caller contract.
        let arena_ref: &'static RuntimeArena = unsafe { &*arena_ptr };
        // SAFETY: lexical-scoping invariant — `outer` (the captured definition scope, or a
        // longer-lived ancestor) outlives this frame, so erasing its lifetime to `'static`
        // for the child's `outer` link is sound; the child borrow is re-anchored on read.
        let outer_static: &Scope<'static> =
            unsafe { std::mem::transmute::<&Scope<'_>, &Scope<'static>>(outer) };
        let mut child = Scope::child_under(outer_static);
        // `child_under` defaults `arena` to `outer.arena`; override to the per-call arena.
        child.arena = arena_ref;
        // `arena_ref` is `&'static` (the `unsafe { &*arena_ptr }` above is where the `'static`
        // claim originates), so `alloc_scope` returns `&'static Scope<'static>` and the safe
        // `erase` yields a `ScopePtr<'static>` — no fabrication here.
        let allocated: &'static Scope<'static> = arena_ref.alloc_scope(child);
        Rc::get_mut(&mut rc)
            .expect("freshly-constructed Rc has unique ownership")
            .scope_ptr = Some(ScopePtr::erase(allocated));
        rc
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

    /// Scope handle bounded by `&'p Rc<Self>` — strictly shorter than the `&'a Scope<'a>`
    /// claim of [`CallArena::scope`]. Use this for local-bind plumbing (e.g.
    /// [`Scope::bind_value`]) that does not need to escape the `Rc`'s borrow, so the caller
    /// avoids an `unsafe` `'a`-anchoring transmute on the receiving end.
    ///
    /// SAFETY: `scope_ptr` stores a `ScopePtr<'static>`; the free-`'p` fabrication is
    /// concentrated here at the non-generic `CallArena` boundary. The pointer is stable for
    /// the `Rc`'s lifetime (heap-pinned by `Rc`), and the returned `'p` is bounded by the
    /// receiver so the borrow cannot outlive it. `'p` is driven by the return-type annotation
    /// — `reattach_unbounded`'s lifetime is late-bound, so it cannot be a turbofish argument.
    pub fn scope_for_bind<'p>(self: &'p Rc<Self>) -> &'p Scope<'p> {
        let scope: &'p Scope<'p> = unsafe { self.scope_ptr_set().reattach_unbounded() };
        scope
    }

    /// The child scope's `ScopePtr<'static>`, which is `Some` for the whole life of a
    /// constructed frame (`None` only transiently inside `new` / `try_reset_for_tail` before
    /// the child scope is allocated).
    fn scope_ptr_set(&self) -> &ScopePtr<'static> {
        self.scope_ptr
            .as_ref()
            .expect("scope_ptr is set after construction")
    }

    /// Re-anchor this frame's per-call arena and child scope to a free `'a` so the caller
    /// may move the frame into a `BodyResult::Tail` / slot `Node` while the borrows stay
    /// live. The single owner for the scattered `(inner_arena, child)` re-anchor performed
    /// by the MATCH / TRY-WITH builtins, [`KFunction::invoke`], and
    /// [`NodeStore::reinstall_with_frame`].
    ///
    /// SAFETY: the caller holds an `Rc<CallArena>` it is about to store in a payload whose
    /// lifetime is `'a`; that `Rc` heap-pins the arena (and its child scope) for as long as
    /// the payload lives, so claiming `'a` — unconstrained by the `&Rc` receiver — is the
    /// receiver-bound-borrow → storage-lifetime re-anchor. Caller obligation: an `Rc` clone
    /// of this frame survives in that payload for all of `'a`.
    pub fn anchored_parts<'a>(self: &Rc<Self>) -> (&'a RuntimeArena, &'a Scope<'a>) {
        unsafe {
            (
                std::mem::transmute::<&RuntimeArena, &'a RuntimeArena>(self.arena()),
                std::mem::transmute::<&Scope<'_>, &'a Scope<'a>>(self.scope()),
            )
        }
    }

    /// Run `f` with this frame's per-call arena and child scope re-anchored to a free `'a`.
    /// The single audited home for the *seed-side* re-anchor: the MATCH / TRY arm and
    /// `KFunction::invoke` body seeds bind their `it` / parameters (values whose type carries
    /// the caller's `'a`, allocated into this frame's arena) inside `f`, without each one
    /// restating the [`Self::anchored_parts`] fabrication. Sound on the same contract: the
    /// caller holds this frame's `Rc`, which heap-pins the arena and child scope for `'a`, and
    /// `f` only allocates into the arena / binds into the scope — work the frame outlives.
    pub fn with_anchored_child<'a, R>(
        self: &Rc<Self>,
        f: impl FnOnce(&'a RuntimeArena, &'a Scope<'a>) -> R,
    ) -> R {
        let (arena, child): (&'a RuntimeArena, &'a Scope<'a>) = self.anchored_parts();
        f(arena, child)
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
        let escape = NonNull::from(new_outer.arena);
        // SAFETY: lexical-scoping invariant — `new_outer.arena` outlives this frame
        // (it is the captured definition scope's arena, or a longer-lived ancestor).
        let outer_static: &Scope<'static> =
            unsafe { std::mem::transmute::<&Scope<'_>, &Scope<'static>>(new_outer) };
        let this = Rc::get_mut(self).expect("just-verified unique above");
        this.scope_ptr = None;
        this.outer_frame = None;
        this.arena = RuntimeArena::with_escape(escape);
        let arena_ptr: *const RuntimeArena = &this.arena;
        // SAFETY: heap-pinned via the `Rc` we hold; pointer is stable for the Rc's lifetime.
        let arena_ref: &'static RuntimeArena = unsafe { &*arena_ptr };
        let mut child = Scope::child_under(outer_static);
        child.arena = arena_ref;
        // `arena_ref` is `&'static` (the `unsafe { &*arena_ptr }` above is where the `'static`
        // claim originates), so `alloc_scope` returns `&'static Scope<'static>` and the safe
        // `erase` yields a `ScopePtr<'static>` — no fabrication here.
        let allocated: &'static Scope<'static> = arena_ref.alloc_scope(child);
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
                crate::machine::core::kfunction::Body::Builtin(|_, _, _| {
                    crate::machine::core::kfunction::BodyResult::value(null_singleton())
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
