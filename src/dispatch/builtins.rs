use super::arena::null_singleton;
use super::kfunction::{Body, BodyResult, BuiltinFn, ExpressionSignature, KFunction};
use super::kobject::KObject;
use super::scope::Scope;

mod fn_def;
mod if_then;
mod let_binding;
mod print;
mod value_lookup;
mod value_pass;

/// `&'static KObject::Null` singleton, for sites that need a literal value reference. Most
/// early-return sites want `null()` instead, which wraps this in `BodyResult::Value`. The
/// singleton lives in [`arena.rs`](super::arena) and is reused — no allocation per call.
pub(crate) fn null_kobject<'a>() -> &'a KObject<'a> {
    null_singleton()
}

/// `BodyResult::Value(null_kobject())` — the canonical "no useful return value" early-exit for
/// builtins. Pairs with `try_args!`'s `return $err;` clause so a typo or type mismatch produces
/// `Null` synchronously without further scheduler work.
pub(crate) fn null<'a>() -> BodyResult<'a> {
    BodyResult::Value(null_kobject())
}

/// Allocate a fresh `KFunction` + wrapping `KObject::KFunction` in `scope`'s arena, then add
/// the object to `scope` under `name`. Centralizes the per-builtin `register` boilerplate.
/// Allocations live for the run (the arena's lifetime) — fine for builtins because every run
/// rebuilds the default scope, and the per-builtin allocations are tiny.
pub(crate) fn register_builtin<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature,
    body: BuiltinFn,
) {
    let arena = scope
        .arena
        .expect("register_builtin requires an arena-backed scope");
    let f: &'a KFunction<'a> = arena.alloc_function(KFunction::new(signature, Body::Builtin(body)));
    let obj: &'a KObject<'a> = arena.alloc_object(KObject::KFunction(f));
    scope.add(name.into(), obj);
}

/// Pull typed arguments out of an `ArgumentBundle`, returning `$err` early on missing-or-mistyped
/// values. Each `name: Variant` pair becomes a `let name = ...` binding extracted from
/// `KObject::Variant`. Supported variants: `KString` (cloned to `String`), `Number` (deref'd to
/// `f64`), `Bool` (deref'd to `bool`).
///
/// ```ignore
/// try_args!(bundle, return null(); name: KString, predicate: Bool);
/// ```
///
/// The macro earns its keep by centralizing the "on failure, exit the caller" clause and
/// keeping each builtin's arg extraction to one line. It is not strictly necessary — a
/// `let Some(KObject::KString(name)) = bundle.get("name") else { return null() };` chain, or
/// a `bundle.try_get::<T>(name)` helper trait, would cover the same ground with a few more
/// lines per builtin and one less piece of project-specific syntax to learn. If new
/// `@extract` arms start piling up or the macro grows much beyond its current shape, that's
/// the signal it's outgrowing its weight; switch to the helper-trait version instead.
#[macro_export]
macro_rules! try_args {
    (
        $bundle:expr,
        return $err:expr;
        $( $name:ident : $variant:ident ),* $(,)?
    ) => {
        $(
            let $name = match $bundle.get(stringify!($name)) {
                Some($crate::dispatch::kobject::KObject::$variant(v)) =>
                    $crate::try_args!(@extract $variant, v),
                _ => return $err,
            };
        )*
    };
    (@extract KString, $v:ident) => { $v.clone() };
    (@extract Number,  $v:ident) => { *$v };
    (@extract Bool,    $v:ident) => { *$v };
}

/// Build a run-root scope populated with the language's builtin `KFunction`s, allocating them
/// in `arena`. The returned scope is owned by `arena` (via `alloc_scope`); callers chain
/// per-call child scopes off it via `Scope.outer`. Each `interpret` call constructs a fresh
/// default scope this way; per-builtin allocations are tiny and live only for the run.
///
/// Registration order does not affect dispatch. `Scope::dispatch` buckets registered functions
/// by their untyped signature shape and picks among overloads in the same bucket by `KType`
/// specificity. `value_lookup` (single `Identifier` slot) and `value_pass` (single `Any` slot)
/// share the bucket `[Slot]`; `value_lookup` wins for inputs like `(some_var)` because
/// `Identifier` is more specific than `Any`. Re-ordering the calls below should leave behavior
/// unchanged — the test suite is the authority.
pub fn default_scope<'a>(
    arena: &'a super::arena::RuntimeArena,
    out: Box<dyn std::io::Write + 'a>,
) -> &'a Scope<'a> {
    let scope = arena.alloc_scope(Scope::run_root(arena, None, out));

    let_binding::register(scope);
    print::register(scope);
    value_lookup::register(scope);
    value_pass::register(scope);
    if_then::register(scope);
    fn_def::register(scope);

    scope
}
