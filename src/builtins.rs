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
mod newtype_def;
mod print;
mod quote;
mod result;
mod sig_def;
mod struct_def;
pub(crate) mod struct_value;
pub(crate) mod tagged_union;
mod try_with;
mod type_call;
mod type_ops;
mod union;
mod using_scope;
mod val_decl;
pub(crate) mod value_lookup;
mod value_pass;

/// Route a resolved verb-object to its construction primitive's `apply` function. Returns
/// `Some(BodyResult)` for `TaggedUnionType` / `StructType`; `None` otherwise. Sole
/// remaining caller is [`type_call`] (the `call_by_name` builtin that previously
/// invoked this helper was deleted in Phase 1 of
/// `scratch/plan-fast-lane-subsume.md`; the dispatch scheduler's
/// `fast_lane_function_value_call` now calls `struct_value::apply` /
/// `tagged_union::apply` directly). Phase 2 of the same plan inlines or
/// relocates this helper alongside the fast lane once `type_call`'s constructor
/// arms also migrate.
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

/// Assemble an `ExpressionSignature` whose return type is `Resolved(return_type)`.
/// All shipped builtins resolve their return type at registration time; FN-bodies that
/// need `Deferred(...)` build the `ExpressionSignature` directly.
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

/// Errors from `register_function` are dropped: `default_scope` registers each builtin once
/// at run-root construction, so a collision is a programming error, not a runtime failure.
pub(crate) fn register_builtin_with_binder<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature<'a>,
    body: BuiltinFn,
    binder_name: Option<BinderNameFn>,
) {
    register_builtin_full(scope, name, signature, body, binder_name, None, false, false);
}

/// Like [`register_builtin_with_binder`] but stamps the registered overload as a
/// *nominal* binder (D7 carve-out). Used by STRUCT, named UNION, SIG, MODULE — the
/// forms whose placeholder must be visible to siblings on the same block regardless of
/// source order, so mutual recursion across sibling nominal binders elaborates.
/// FUNCTOR routes through [`register_builtin_full`] because it also needs
/// `binder_bucket`.
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
/// Used by FN / FUNCTOR to supply the [`BinderBucketFn`] that keys a pending-overload
/// entry by inner-call bucket — see [`crate::machine::core::kfunction::BinderBucketFn`].
/// Everything else routes through the simpler [`register_builtin_with_binder`].
///
/// `is_nominal_binder` flips the D7 carve-out so the submission-time placeholder install
/// in `submit::add_with_chain` stamps the [`BindingIndex`] with `nominal_binder: true`.
/// Used by STRUCT / named UNION / SIG / FUNCTOR / MODULE.
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
///
/// Registration order does not affect dispatch: [`Scope::resolve_dispatch`] buckets by
/// untyped signature shape and picks overloads by `KType` specificity. `value_lookup`
/// (single `Identifier` slot) and `value_pass` (single `Any` slot) share the bucket
/// `[Slot]`; `value_lookup` wins for inputs like `(some_var)` because `Identifier` is
/// more specific than `Any`.
pub fn default_scope<'a>(
    arena: &'a crate::machine::core::RuntimeArena,
    out: Box<dyn std::io::Write + 'a>,
) -> &'a Scope<'a> {
    let scope = arena.alloc_scope(Scope::run_root(arena, None, out));

    // Builtin type names — stored in `bindings.types` as arena-allocated `&KType`
    // via `Scope::register_type` (post-stage-1.4 storage flip). Reads go through
    // `Scope::resolve_type`; the sole `KObject::KTypeValue` synthesis site for
    // dispatch transport lives in `value_lookup::body_type_expr`.
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
    // carrier — matches `KType::from_name`'s mapping so type-name resolution through
    // the resolver and through the parser-side fast path agree. Per-declaration types
    // live as `KType::UserType` in `bindings.types`, dual-written by the finalize sites.
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
    // Post-collapse: `:Module` / `:Signature` slot wildcards have dedicated KType variants
    // (no more `UserTypeKind::Module` arm; `MetaSignature` retired in favor of `AnySignature`).
    scope.register_type("Module".into(), KType::AnyModule, BindingIndex::BUILTIN);
    scope.register_type("Signature".into(), KType::AnySignature, BindingIndex::BUILTIN);
    scope.register_type("Any".into(), KType::Any, BindingIndex::BUILTIN);

    let_binding::register(scope);
    print::register(scope);
    value_lookup::register(scope);
    value_pass::register(scope);
    fn_def::register(scope);
    functor_def::register(scope);
    // `call_by_name` was deleted in Phase 1 of the `unified-walk` follow-up
    // (`scratch/plan-fast-lane-subsume.md`); its Identifier-headed call
    // semantics are now served by `fast_lane_function_value_call` in the
    // dispatch scheduler.
    union::register(scope);
    result::register(scope);
    tagged_union::register(scope);
    struct_def::register(scope);
    struct_value::register(scope);
    newtype_def::register(scope);
    type_call::register(scope);
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

    scope
}
