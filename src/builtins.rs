use crate::machine::core::kfunction::{BinderNameFn, Body, KFunction};
use crate::machine::core::{BindingIndex, FrameStorageExt, Scope};
use crate::machine::model::types::KKind;
use crate::machine::model::types::{
    Argument, ExpressionSignature, KType, ReturnType, SignatureElement,
};
use crate::machine::model::values::KObject;

pub(crate) mod arithmetic;
mod ascribe;
mod attr;
mod await_body;
mod block_tail;
mod branch_walk;
mod catch;
mod eval;
mod fn_def;
mod let_binding;
mod match_case;
mod module_def;
pub(crate) mod newtype_def;
mod nominal_schema;
mod parameterized_types;
mod print;
mod record_projection;
mod recursive_types;
mod resolve_or_await;
mod result;
mod sig_def;
mod try_with;
mod type_decl;
mod type_ops;
mod type_union;
mod union;
mod using_scope;
mod val_decl;

#[cfg(test)]
pub(crate) mod test_support;

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

/// Shared [`BinderNameFn`] for typed-binder builtins (SIG / UNION / RECURSIVE TYPES / NEWTYPE):
/// the binder name is `parts[1]`'s `Type(t)` token. A free function (not the
/// `KExpression::binder_name_from_type_part` method reference) so the signature is higher-ranked
/// over the expression lifetime, as `BinderNameFn` requires.
pub(crate) fn type_part_binder_name(
    expr: &crate::machine::model::ast::KExpression<'_>,
) -> Option<String> {
    expr.binder_name_from_type_part()
}

/// Shared [`BinderNameFn`] for value-binder builtins (`LET <name> = …`, `MODULE <name> = …`): the
/// binder name is `parts[1]`'s `Identifier` token. The Identifier-part twin of
/// [`type_part_binder_name`], so each overload's extractor matches exactly its own name-part kind
/// and the submit-time placeholder is tagged `Value` xor `Type` to match where the bind lands.
pub(crate) fn identifier_part_binder_name(
    expr: &crate::machine::model::ast::KExpression<'_>,
) -> Option<String> {
    match &expr.parts.get(1)?.value {
        crate::machine::model::ast::ExpressionPart::Identifier(s) => Some(s.clone()),
        _ => None,
    }
}

/// Full-form builtin registration with both binder hooks. The `body` is
/// an [`ActionFn`](crate::machine::core::kfunction::ActionFn) (`fn(&BodyCtx) -> Action`) installed
/// as [`Body::Builtin`] — the builtin runs through `machine::execute::runtime::run_action`.
/// `binder_bucket` lets FN key pending-overload entries by inner-call bucket.
pub(crate) fn register_builtin_full<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature<'a>,
    body: crate::machine::core::kfunction::ActionFn,
    binder_name: Option<(BinderNameFn, crate::machine::core::BindKind)>,
    binder_bucket: Option<crate::machine::core::kfunction::BinderBucketFn>,
) {
    let region = scope.brand();
    let f: &'a KFunction<'a> = region.alloc_function(KFunction::new(
        signature,
        Body::Builtin(body),
        scope,
        binder_name,
        binder_bucket,
    ));
    let obj: &'a KObject<'a> = region
        .alloc_object_checked(KObject::KFunction(f))
        .expect("f was just allocated into region's own region");
    let _ = scope.register_function(name.into(), f, obj, BindingIndex::BUILTIN);
}

/// Common-case [`register_builtin_full`]: no binder hooks.
pub(crate) fn register_builtin<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature<'a>,
    body: crate::machine::core::kfunction::ActionFn,
) {
    register_builtin_full(scope, name, signature, body, None, None);
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
    body: crate::machine::core::kfunction::ActionFn,
    index: BindingIndex,
) {
    let region = scope.brand();
    let f: &'a KFunction<'a> = region.alloc_function(KFunction::new(
        signature,
        Body::Builtin(body),
        scope,
        None,
        None,
    ));
    let obj: &'a KObject<'a> = region
        .alloc_object_checked(KObject::KFunction(f))
        .expect("f was just allocated into region's own region");
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
    run_storage: &'a std::rc::Rc<crate::machine::core::FrameStorage>,
    out: Box<dyn std::io::Write + 'a>,
) -> &'a Scope<'a> {
    let scope = run_storage
        .brand()
        .alloc_scope(Scope::run_root(run_storage, None, out));

    scope.register_builtin_type("Number".into(), KType::Number, BindingIndex::BUILTIN);
    scope.register_builtin_type("Str".into(), KType::Str, BindingIndex::BUILTIN);
    scope.register_builtin_type("Bool".into(), KType::Bool, BindingIndex::BUILTIN);
    scope.register_builtin_type("Null".into(), KType::Null, BindingIndex::BUILTIN);
    scope.register_builtin_type(
        "List".into(),
        KType::list(Box::new(KType::Any)),
        BindingIndex::BUILTIN,
    );
    scope.register_builtin_type(
        "Dict".into(),
        KType::dict(Box::new(KType::Any), Box::new(KType::Any)),
        BindingIndex::BUILTIN,
    );
    scope.register_builtin_type(
        "KExpression".into(),
        KType::KExpression,
        BindingIndex::BUILTIN,
    );
    scope.register_builtin_type(
        "Type".into(),
        KType::OfKind(KKind::AnyType),
        BindingIndex::BUILTIN,
    );
    scope.register_builtin_type(
        "Module".into(),
        KType::empty_signature(),
        BindingIndex::BUILTIN,
    );
    scope.register_builtin_type(
        "Signature".into(),
        KType::OfKind(KKind::Signature),
        BindingIndex::BUILTIN,
    );
    scope.register_builtin_type("Any".into(), KType::Any, BindingIndex::BUILTIN);

    let_binding::register(scope);
    print::register(scope);
    fn_def::register(scope);
    union::register(scope);
    result::register(scope);
    newtype_def::register(scope);
    recursive_types::register(scope);
    match_case::register(scope);
    try_with::register(scope);
    using_scope::register(scope);
    catch::register(scope);
    attr::register(scope);
    eval::register(scope);
    module_def::register(scope);
    sig_def::register(scope);
    val_decl::register(scope);
    type_decl::register(scope);
    ascribe::register(scope);
    record_projection::register(scope);
    type_ops::register(scope);
    parameterized_types::register(scope);
    type_union::register(scope);
    arithmetic::register(scope);
    arithmetic::register_builtin_operator_groups(scope);

    run_storage.brand().alloc_scope(Scope::run_child(scope))
}
