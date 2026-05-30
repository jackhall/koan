use crate::machine::core::source::Spanned;
use crate::machine::model::ast::ExpressionPart;
use crate::machine::core::kfunction::{Body, BodyResult, BuiltinFn, KFunction, BinderNameFn};
use crate::machine::core::{BindingIndex, KError, Scope};
use crate::machine::model::types::{
    Argument, ExpressionSignature, KType, ReturnType, SignatureElement, UserTypeKind,
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
mod result;
mod sig_def;
mod struct_def;
pub(crate) mod struct_value;
pub(crate) mod tagged_union;
mod try_with;
mod type_constructors;
mod type_ops;
mod union;
mod using_scope;
mod val_decl;

/// Route a resolved verb-object to its construction primitive's `apply`. Returns
/// `Some` for `TaggedUnionType` / `StructType`; `None` otherwise.
pub(crate) fn dispatch_constructor<'a>(
    verb_obj: &'a KObject<'a>,
    args_parts: Vec<Spanned<ExpressionPart<'a>>>,
) -> Option<BodyResult<'a>> {
    match verb_obj {
        KObject::TaggedUnionType { .. } => Some(tagged_union::apply(verb_obj, args_parts)),
        KObject::StructType { .. } => Some(struct_value::apply(verb_obj, args_parts)),
        _ => None,
    }
}

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
    SignatureElement::Argument(Argument { name: name.into(), ktype })
}

/// Assemble an `ExpressionSignature` with `Resolved(return_type)`. Builtins needing
/// `Deferred(...)` build the `ExpressionSignature` directly.
pub(crate) fn sig<'a>(return_type: KType<'a>, elements: Vec<SignatureElement<'a>>) -> ExpressionSignature<'a> {
    ExpressionSignature { return_type: ReturnType::Resolved(return_type), elements }
}

pub fn register_builtin<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature<'a>,
    body: BuiltinFn,
) {
    register_builtin_with_binder(scope, name, signature, body, None);
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
    register_builtin_full(scope, name, signature, body, binder_name, None, false, false);
}

/// Like [`register_builtin_with_binder`] but stamps the overload as a *nominal* binder
/// (D7 carve-out) so its placeholder is visible to siblings on the same block regardless
/// of source order, enabling mutual recursion across sibling nominal binders.
pub(crate) fn register_nominal_binder<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature<'a>,
    body: BuiltinFn,
    binder_name: Option<BinderNameFn>,
) {
    register_builtin_full(scope, name, signature, body, binder_name, None, false, true);
}

/// Full-form builtin registration with both binder hooks and the `is_functor` flag.
/// `binder_bucket` lets FN / FUNCTOR key pending-overload entries by inner-call bucket.
/// `is_nominal_binder` flips the D7 carve-out so the placeholder is stamped with
/// `nominal_binder: true`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn register_builtin_full<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature<'a>,
    body: BuiltinFn,
    binder_name: Option<BinderNameFn>,
    binder_bucket: Option<crate::machine::core::kfunction::BinderBucketFn>,
    is_functor: bool,
    is_nominal_binder: bool,
) {
    let arena = scope.arena;
    let f: &'a KFunction<'a> = arena.alloc_function(KFunction::with_binder_and_functor(
        signature,
        Body::Builtin(body),
        scope,
        binder_name,
        binder_bucket,
        is_functor,
        is_nominal_binder,
    ));
    let obj: &'a KObject<'a> = arena.alloc(KObject::KFunction(f, None));
    let _ = scope.register_function(name.into(), f, obj, BindingIndex::BUILTIN);
}

/// Build a run-root scope populated with the language's builtin `KFunction`s.
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
    scope.register_type("List".into(), KType::List(Box::new(KType::Any)), BindingIndex::BUILTIN);
    scope.register_type(
        "Dict".into(),
        KType::Dict(Box::new(KType::Any), Box::new(KType::Any)),
        BindingIndex::BUILTIN,
    );
    scope.register_type("KExpression".into(), KType::KExpression, BindingIndex::BUILTIN);
    scope.register_type("Type".into(), KType::Type, BindingIndex::BUILTIN);
    // User-declared-type surface names lower to the wildcard `AnyUserType { kind }`
    // carrier so the resolver and the parser-side fast path agree.
    scope.register_type(
        "Tagged".into(),
        KType::AnyUserType { kind: UserTypeKind::Tagged },
        BindingIndex::BUILTIN,
    );
    scope.register_type(
        "Struct".into(),
        KType::AnyUserType { kind: UserTypeKind::Struct },
        BindingIndex::BUILTIN,
    );
    scope.register_type("Module".into(), KType::AnyModule, BindingIndex::BUILTIN);
    scope.register_type("Signature".into(), KType::AnySignature, BindingIndex::BUILTIN);
    scope.register_type("Any".into(), KType::Any, BindingIndex::BUILTIN);

    let_binding::register(scope);
    print::register(scope);
    fn_def::register(scope);
    functor_def::register(scope);
    union::register(scope);
    result::register(scope);
    tagged_union::register(scope);
    struct_def::register(scope);
    struct_value::register(scope);
    newtype_def::register(scope);
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
    type_ops::register(scope);
    type_constructors::register(scope);

    scope
}
