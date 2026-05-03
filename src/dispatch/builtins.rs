use super::arena::null_singleton;
use super::kerror::KError;
use super::kfunction::{Body, BodyResult, BuiltinFn, ExpressionSignature, KFunction};
use super::kobject::KObject;
use super::scope::Scope;

pub mod call_by_name;
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
/// builtins. Used for *intentional* nulls only: an `IF false THEN x` skipping its lazy slot,
/// `PRINT`'s no-useful-return value. Failure paths (type mismatches, missing args, unbound
/// names, shape errors) return `err(...)` instead so the scheduler can short-circuit and the
/// CLI can report what went wrong.
pub(crate) fn null<'a>() -> BodyResult<'a> {
    BodyResult::Value(null_kobject())
}

/// `BodyResult::Err(e)` — the structured-error early-exit for builtins. Replaces the prior
/// pattern of returning `null()` from every failure path. The error propagates through the
/// scheduler's Forward chain and short-circuits any dependent node.
pub(crate) fn err<'a>(e: KError) -> BodyResult<'a> {
    BodyResult::Err(e)
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
    let arena = scope.arena;
    // Builtins capture the scope they're being registered into — typically run-root (set up
    // by `default_scope`). The captured scope's arena is the same arena the KFunction lives
    // in, so `lift_kobject`'s arena-pointer comparison correctly identifies builtins as
    // never-in-a-dying-frame.
    let f: &'a KFunction<'a> =
        arena.alloc_function(KFunction::new(signature, Body::Builtin(body), scope));
    // `frame: None` here — the lift-on-return logic in the scheduler doesn't need to attach
    // an Rc for builtins (their captured arena is run-root and never dies).
    let obj: &'a KObject<'a> = arena.alloc_object(KObject::KFunction(f, None));
    scope.add(name.into(), obj);
}

/// Pull typed arguments out of an `ArgumentBundle`. Two forms:
///
/// ```ignore
/// // Default form: a missing or mistyped arg returns BodyResult::Err with a structured
/// // KError::TypeMismatch identifying the offending argument and what was actually present.
/// try_args!(bundle; name: KString, predicate: Bool);
///
/// // Override form: the caller supplies the early-return expression. Used when the
/// // builtin wants to return something other than the structured TypeMismatch error
/// // (e.g., an intentional null on a benign mismatch — currently no in-tree call site
/// // does this, but the override stays available).
/// try_args!(bundle, return null(); name: KString);
/// ```
///
/// Each `name: Variant` pair becomes a `let name = ...` binding extracted from
/// `KObject::Variant`. Supported variants: `KString` (cloned to `String`), `Number`
/// (deref'd to `f64`), `Bool` (deref'd to `bool`).
///
/// The macro earns its keep by centralizing the "on failure, exit the caller" clause and
/// keeping each builtin's arg extraction to one line. It is not strictly necessary — a
/// `let Some(KObject::KString(name)) = bundle.get("name") else { return ... };` chain, or
/// a `bundle.try_get::<T>(name)` helper trait, would cover the same ground with a few more
/// lines per builtin and one less piece of project-specific syntax to learn. If new
/// `@extract` arms start piling up or the macro grows much beyond its current shape, that's
/// the signal it's outgrowing its weight; switch to the helper-trait version instead.
#[macro_export]
macro_rules! try_args {
    // Override form: caller supplies the early-return expression.
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
    // Default form: missing/mistyped → BodyResult::Err with structured TypeMismatch.
    (
        $bundle:expr;
        $( $name:ident : $variant:ident ),* $(,)?
    ) => {
        $(
            let $name = match $bundle.get(stringify!($name)) {
                Some($crate::dispatch::kobject::KObject::$variant(v)) =>
                    $crate::try_args!(@extract $variant, v),
                other => return $crate::dispatch::builtins::err(
                    $crate::dispatch::kerror::KError::new(
                        $crate::dispatch::kerror::KErrorKind::TypeMismatch {
                            arg: stringify!($name).to_string(),
                            expected: stringify!($variant).to_string(),
                            got: match other {
                                Some(o) => {
                                    use $crate::dispatch::ktraits::Parseable;
                                    o.summarize()
                                }
                                None => "(missing)".to_string(),
                            },
                        }
                    )
                ),
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
    call_by_name::register(scope);

    scope
}
