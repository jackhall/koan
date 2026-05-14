use crate::runtime::machine::kfunction::{Body, BodyResult, BuiltinFn, KFunction, PreRunFn};
use crate::runtime::machine::core::{KError, Scope};
use crate::runtime::model::types::{ExpressionSignature, KType, UserTypeKind};
use crate::runtime::model::values::KObject;

mod ascribe;
mod attr;
pub mod call_by_name;
mod cons;
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

#[cfg(test)]
pub(crate) mod test_support;

pub(crate) fn err<'a>(e: KError) -> BodyResult<'a> {
    BodyResult::Err(e)
}

pub fn register_builtin<'a>(
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
/// Registration order does not affect dispatch: [`Scope::resolve_dispatch`] buckets by
/// untyped signature shape and picks overloads by `KType` specificity. `value_lookup`
/// (single `Identifier` slot) and `value_pass` (single `Any` slot) share the bucket
/// `[Slot]`; `value_lookup` wins for inputs like `(some_var)` because `Identifier` is
/// more specific than `Any`.
pub fn default_scope<'a>(
    arena: &'a crate::runtime::machine::core::RuntimeArena,
    out: Box<dyn std::io::Write + 'a>,
) -> &'a Scope<'a> {
    let scope = arena.alloc_scope(Scope::run_root(arena, None, out));

    // Builtin type names — stored in `bindings.types` as arena-allocated `&KType`
    // via `Scope::register_type` (post-stage-1.4 storage flip). Reads go through
    // `Scope::resolve_type`; the sole `KObject::KTypeValue` synthesis site for
    // dispatch transport lives in `value_lookup::body_type_expr`.
    scope.register_type("Number".into(), KType::Number);
    scope.register_type("Str".into(), KType::Str);
    scope.register_type("Bool".into(), KType::Bool);
    scope.register_type("Null".into(), KType::Null);
    scope.register_type("List".into(), KType::List(Box::new(KType::Any)));
    scope.register_type(
        "Dict".into(),
        KType::Dict(Box::new(KType::Any), Box::new(KType::Any)),
    );
    scope.register_type("KExpression".into(), KType::KExpression);
    scope.register_type("Type".into(), KType::Type);
    // Stage 3.0b: agree with `KType::from_name` so the type-resolver path produces the
    // same wildcard carriers a parser-side `from_name` would. Stage 3.1 will delete the
    // old singletons entirely; until then the singletons remain valid `KType` values
    // for the carriers' `ktype()` arms — they're just not what the user types resolve
    // to anymore.
    scope.register_type("Tagged".into(), KType::AnyUserType { kind: UserTypeKind::Tagged });
    scope.register_type("Struct".into(), KType::AnyUserType { kind: UserTypeKind::Struct });
    scope.register_type("Module".into(), KType::AnyUserType { kind: UserTypeKind::Module });
    scope.register_type("Signature".into(), KType::Signature);
    scope.register_type("Any".into(), KType::Any);

    let_binding::register(scope);
    print::register(scope);
    value_lookup::register(scope);
    value_pass::register(scope);
    fn_def::register(scope);
    call_by_name::register(scope);
    union::register(scope);
    crate::runtime::model::values::tagged_union::register(scope);
    struct_def::register(scope);
    crate::runtime::model::values::struct_value::register(scope);
    type_call::register(scope);
    match_case::register(scope);
    attr::register(scope);
    quote::register(scope);
    eval::register(scope);
    module_def::register(scope);
    sig_def::register(scope);
    ascribe::register(scope);
    type_ops::register(scope);
    cons::register(scope);

    scope
}
