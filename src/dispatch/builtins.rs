use std::collections::HashMap;

use super::kfunction::{BuiltinFn, ExpressionSignature, KFunction};
use super::kobject::KObject;
use super::scope::Scope;

mod if_then;
mod let_binding;
mod print;
mod value_lookup;
mod value_pass;

/// Returns a freshly leaked `KObject::Null`, used by builtins as their "no-op / type mismatch"
/// return so they always satisfy the `&'a KObject<'a>` signature without threading lifetimes.
pub(crate) fn null<'a>() -> &'a KObject<'a> {
    Box::leak(Box::new(KObject::Null))
}

/// `Box::leak` a fresh `KFunction` + wrapping `KObject::KFunction`, then add the leaked object
/// to `scope` under `name`. Centralizes the static-lifetime wrapping each per-builtin `register`
/// fn would otherwise duplicate.
pub(crate) fn register_builtin(
    scope: &mut Scope<'static>,
    name: &str,
    signature: ExpressionSignature,
    body: BuiltinFn,
) {
    let f: &'static KFunction<'static> =
        Box::leak(Box::new(KFunction::new(None, signature, body)));
    let obj: &'static KObject<'static> = Box::leak(Box::new(KObject::KFunction(f)));
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

/// Build a fresh root scope populated with the language's builtin `KFunction`s. Each call
/// `Box::leak`s its own function and object boxes, so the returned scope is `'static` and child
/// scopes can chain off it via `Scope.outer` to inherit the builtins.
///
/// Registration order does not affect dispatch. `Scope::dispatch` buckets registered functions
/// by their untyped signature shape and picks among overloads in the same bucket by `KType`
/// specificity. `value_lookup` (single `Identifier` slot) and `value_pass` (single `Any` slot)
/// share the bucket `[Slot]`; `value_lookup` wins for inputs like `(some_var)` because
/// `Identifier` is more specific than `Any`. Re-ordering the calls below should leave behavior
/// unchanged — the test suite is the authority.
pub fn default_scope() -> Scope<'static> {
    let mut scope = Scope {
        outer: None,
        data: HashMap::new(),
        functions: HashMap::new(),
        out: Box::new(std::io::stdout()),
    };

    let_binding::register(&mut scope);
    print::register(&mut scope);
    value_lookup::register(&mut scope);
    value_pass::register(&mut scope);
    if_then::register(&mut scope);

    scope
}
