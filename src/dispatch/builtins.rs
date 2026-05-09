use super::kfunction::{Body, BodyResult, BuiltinFn, KFunction, PreRunFn};
use super::runtime::{KError, Scope};
use super::types::ExpressionSignature;
use super::values::KObject;

mod ascribe;
mod attr;
pub mod call_by_name;
mod eval;
mod fn_def;
mod helpers;
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

#[cfg(test)]
pub(crate) mod test_support;

pub(crate) fn err<'a>(e: KError) -> BodyResult<'a> {
    BodyResult::Err(e)
}

pub(crate) fn register_builtin<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature,
    body: BuiltinFn,
) {
    register_builtin_with_pre_run(scope, name, signature, body, None);
}

/// Errors from `register_function` are dropped: `default_scope` registers each builtin once
/// at run-root construction, so a collision is a programming error, not a runtime failure.
pub(crate) fn register_builtin_with_pre_run<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature,
    body: BuiltinFn,
    pre_run: Option<PreRunFn>,
) {
    let arena = scope.arena;
    // The captured scope's arena must be the same arena the KFunction lives in, so
    // `lift_kobject`'s arena-pointer comparison identifies builtins as never-in-a-dying-frame.
    let f: &'a KFunction<'a> =
        arena.alloc_function(KFunction::with_pre_run(signature, Body::Builtin(body), scope, pre_run));
    let obj: &'a KObject<'a> = arena.alloc_object(KObject::KFunction(f, None));
    let _ = scope.register_function(name.into(), f, obj);
}

/// Build a run-root scope populated with the language's builtin `KFunction`s.
///
/// Registration order does not affect dispatch: `Scope::dispatch` buckets by untyped signature
/// shape and picks overloads by `KType` specificity. `value_lookup` (single `Identifier` slot)
/// and `value_pass` (single `Any` slot) share the bucket `[Slot]`; `value_lookup` wins for
/// inputs like `(some_var)` because `Identifier` is more specific than `Any`.
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
