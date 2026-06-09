use crate::machine::core::kfunction::{BinderNameFn, Body, BodyResult, BuiltinFn, KFunction};
use crate::machine::core::{BindingIndex, KError, Scope};
use crate::machine::model::types::KKind;
use crate::machine::model::types::{
    Argument, ExpressionSignature, KType, ReturnType, SignatureElement,
};
use crate::machine::model::values::KObject;

mod ascribe;
mod attr;
mod branch_walk;
mod catch;
mod eval;
mod fn_def;
mod functor_def;
mod let_binding;
mod match_case;
mod module_def;
pub(crate) mod newtype_def;
mod print;
mod quote;
mod record_projection;
mod recursive_types;
mod result;
mod sig_def;
mod try_with;
mod type_constructors;
mod type_ops;
mod union;
mod using_scope;
mod val_decl;

#[cfg(test)]
pub(crate) mod test_support;

pub(crate) fn err<'a>(e: KError) -> BodyResult<'a> {
    BodyResult::Err(e)
}

/// Signature-element constructor for a keyword slot.
pub(crate) fn kw<'a>(s: &str) -> SignatureElement<'a> {
    SignatureElement::Keyword(s.into())
}

/// Signature-element constructor for an argument slot.
pub(crate) fn arg<'a>(name: &str, ktype: KType<'a>) -> SignatureElement<'a> {
    SignatureElement::Argument(Argument {
        name: name.into(),
        ktype,
    })
}

/// Assemble an `ExpressionSignature` with `Resolved(return_type)`. Builtins needing
/// `Deferred(...)` build the `ExpressionSignature` directly.
pub(crate) fn sig<'a>(
    return_type: KType<'a>,
    elements: Vec<SignatureElement<'a>>,
) -> ExpressionSignature<'a> {
    ExpressionSignature {
        return_type: ReturnType::Resolved(return_type),
        elements,
    }
}

pub fn register_builtin<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature<'a>,
    body: BuiltinFn,
) {
    register_builtin_with_binder(scope, name, signature, body, None);
}

/// Shared [`BinderNameFn`] for typed-binder builtins (SIG / MODULE / UNION /
/// RECURSIVE TYPES / NEWTYPE): the binder name is `parts[1]`'s `Type(t)` token.
/// A free function (not the `KExpression::binder_name_from_type_part` method
/// reference) so the signature is higher-ranked over the expression lifetime, as
/// `BinderNameFn` requires.
pub(crate) fn type_part_binder_name(
    expr: &crate::machine::model::ast::KExpression<'_>,
) -> Option<String> {
    expr.binder_name_from_type_part()
}

/// Collisions from `register_function` are dropped: each builtin registers once at
/// run-root construction, so a collision would be a programming error.
pub(crate) fn register_builtin_with_binder<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature<'a>,
    body: BuiltinFn,
    binder_name: Option<BinderNameFn>,
) {
    register_builtin_full(scope, name, signature, body, binder_name, None, false);
}

/// Full-form builtin registration with both binder hooks and the `is_functor` flag.
/// `binder_bucket` lets FN / FUNCTOR key pending-overload entries by inner-call bucket.
#[allow(clippy::too_many_arguments)]
pub(crate) fn register_builtin_full<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature<'a>,
    body: BuiltinFn,
    binder_name: Option<BinderNameFn>,
    binder_bucket: Option<crate::machine::core::kfunction::BinderBucketFn>,
    is_functor: bool,
) {
    let arena = scope.arena;
    let f: &'a KFunction<'a> = arena.alloc_function(KFunction::with_binder_and_functor(
        signature,
        Body::Builtin(body),
        scope,
        binder_name,
        binder_bucket,
        is_functor,
    ));
    let obj: &'a KObject<'a> = arena.alloc_object(KObject::KFunction(f, None));
    let _ = scope.register_function(name.into(), f, obj, BindingIndex::BUILTIN);
}

/// Test-only: register one overload at an explicit [`BindingIndex`]. A test uses this to
/// place a *user*-position (non-`BUILTIN`) overload in a root-position scope, so dispatch
/// exercises the ordinary innermost-wins walk rather than the builtin root-first
/// short-circuit (which a `BUILTIN`-index entry in the root would trigger).
#[cfg(test)]
pub(crate) fn register_overload_at<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature<'a>,
    body: BuiltinFn,
    index: BindingIndex,
) {
    let arena = scope.arena;
    let f: &'a KFunction<'a> = arena.alloc_function(KFunction::with_binder_and_functor(
        signature,
        Body::Builtin(body),
        scope,
        None,
        None,
        false,
    ));
    let obj: &'a KObject<'a> = arena.alloc_object(KObject::KFunction(f, None));
    scope
        .register_function(name.into(), f, obj, index)
        .expect("register_overload_at: user-index overload should not collide with a builtin");
}

/// Build the run-global root populated with the language's builtin `KFunction`s, then
/// return a mutable `RunScope` child of it for top-level Koan bindings. The root stays
/// builtin-only and immutable; a top-level bind lands in the `RunScope`, leaving the
/// root binding-free. Builtins resolve from any scope by walking `outer` to the root
/// (the [`Scope::shadows_builtin_value`] no-shadow consult does the same).
///
/// Registration order does not affect dispatch — [`Scope::resolve_dispatch`] buckets by
/// untyped signature shape and picks overloads by `KType` specificity.
pub fn default_scope<'a>(
    arena: &'a crate::machine::core::RuntimeArena,
    out: Box<dyn std::io::Write + 'a>,
) -> &'a Scope<'a> {
    let scope = arena.alloc_scope(Scope::run_root(arena, None, out));

    scope.register_type("Number".into(), KType::Number, BindingIndex::BUILTIN);
    scope.register_type("Str".into(), KType::Str, BindingIndex::BUILTIN);
    scope.register_type("Bool".into(), KType::Bool, BindingIndex::BUILTIN);
    scope.register_type("Null".into(), KType::Null, BindingIndex::BUILTIN);
    scope.register_type(
        "List".into(),
        KType::List(Box::new(KType::Any)),
        BindingIndex::BUILTIN,
    );
    scope.register_type(
        "Dict".into(),
        KType::Dict(Box::new(KType::Any), Box::new(KType::Any)),
        BindingIndex::BUILTIN,
    );
    scope.register_type(
        "KExpression".into(),
        KType::KExpression,
        BindingIndex::BUILTIN,
    );
    scope.register_type(
        "Type".into(),
        KType::OfKind(KKind::Any),
        BindingIndex::BUILTIN,
    );
    scope.register_type(
        "Module".into(),
        KType::OfKind(KKind::Module),
        BindingIndex::BUILTIN,
    );
    scope.register_type(
        "Signature".into(),
        KType::OfKind(KKind::Signature),
        BindingIndex::BUILTIN,
    );
    scope.register_type("Any".into(), KType::Any, BindingIndex::BUILTIN);

    let_binding::register(scope);
    print::register(scope);
    fn_def::register(scope);
    functor_def::register(scope);
    union::register(scope);
    result::register(scope);
    newtype_def::register(scope);
    recursive_types::register(scope);
    match_case::register(scope);
    try_with::register(scope);
    using_scope::register(scope);
    catch::register(scope);
    attr::register(scope);
    quote::register(scope);
    eval::register(scope);
    module_def::register(scope);
    sig_def::register(scope);
    val_decl::register(scope);
    ascribe::register(scope);
    record_projection::register(scope);
    type_ops::register(scope);
    type_constructors::register(scope);

    arena.alloc_scope(Scope::child_under(scope))
}
