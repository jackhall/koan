use typed_arena::Arena;

use super::kfunction::KFunction;
use super::kobject::KObject;
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

    pub fn alloc_function<'a>(&'a self, f: KFunction<'a>) -> &'a KFunction<'a> {
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
}

impl Default for RuntimeArena {
    fn default() -> Self { Self::new() }
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
/// missing-arg / lookup-miss path; previously each such site `Box::leak`'d a fresh `Null`.
/// `KObject<'a>` is invariant in `'a`, so the `&'static`-typed singleton is reinterpreted to
/// the caller's `'a` via `transmute`. SAFETY: `KObject::Null` is a unit variant — no
/// references inside, so the lifetime parameter is purely phantom.
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
