use super::kfunction::{Body, BodyResult, BuiltinFn, KFunction};
use super::runtime::{KError, Scope};
use super::types::ExpressionSignature;
use super::values::KObject;

mod ascribe;
mod attr;
pub mod call_by_name;
mod eval;
mod fn_def;
mod let_binding;
mod match_case;
mod module_def;
mod print;
mod quote;
mod sig_def;
mod struct_def;
mod type_call;
mod type_ops;
mod union;
mod value_lookup;
mod value_pass;

/// `BodyResult::Err(e)` — the structured-error early-exit for builtins. The scheduler stores
/// the error on the producing slot and propagates it via the notify-walk; any dependent slot
/// short-circuits with the error frame appended.
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
/// // on a benign mismatch — currently no in-tree call site does this, but the
/// // override stays available.
/// try_args!(bundle, return BodyResult::Value(some_obj); name: KString);
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
                Some($crate::dispatch::values::KObject::$variant(v)) =>
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
                Some($crate::dispatch::values::KObject::$variant(v)) =>
                    $crate::try_args!(@extract $variant, v),
                other => return $crate::dispatch::builtins::err(
                    $crate::dispatch::runtime::KError::new(
                        $crate::dispatch::runtime::KErrorKind::TypeMismatch {
                            arg: stringify!($name).to_string(),
                            expected: stringify!($variant).to_string(),
                            got: match other {
                                Some(o) => {
                                    use $crate::dispatch::types::Parseable;
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
    arena: &'a super::runtime::RuntimeArena,
    out: Box<dyn std::io::Write + 'a>,
) -> &'a Scope<'a> {
    let scope = arena.alloc_scope(Scope::run_root(arena, None, out));

    let_binding::register(scope);
    print::register(scope);
    value_lookup::register(scope);
    value_pass::register(scope);
    fn_def::register(scope);
    call_by_name::register(scope);
    union::register(scope);
    super::values::tagged_union::register(scope);
    struct_def::register(scope);
    super::values::struct_value::register(scope);
    type_call::register(scope);
    match_case::register(scope);
    attr::register(scope);
    quote::register(scope);
    eval::register(scope);
    module_def::register(scope);
    sig_def::register(scope);
    ascribe::register(scope);
    type_ops::register(scope);

    scope
}
