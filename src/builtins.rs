use crate::machine::model::KKind;
use crate::machine::model::KObject;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{Argument, ExpressionSignature, KType, ReturnType, SignatureElement};
use crate::machine::{BinderNameFn, Body, KFunction};
use crate::machine::{BindingIndex, FrameStorageExt, Scope};

pub(crate) mod arithmetic;
mod ascribe;
mod attr;
mod await_body;
mod block_tail;
mod branch_walk;
mod catch;
mod equality;
mod eval;
mod fn_def;
mod group_def;
mod let_binding;
mod match_case;
mod module_def;
pub(crate) mod newtype_def;
mod nominal_schema;
mod op_def;
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
pub(crate) fn kw(s: &str) -> SignatureElement {
    SignatureElement::Keyword(s.into())
}

/// Signature-element constructor for an argument slot.
pub(crate) fn arg(name: &str, ktype: KType) -> SignatureElement {
    SignatureElement::Argument(Argument {
        name: name.into(),
        ktype,
    })
}

/// Assemble an `ExpressionSignature` with `Resolved(return_type)`. Builtins needing
/// `Deferred(...)` build the `ExpressionSignature` directly.
pub(crate) fn sig<'a>(
    return_type: KType,
    elements: Vec<SignatureElement>,
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
    expr: &crate::machine::model::KExpression<'_>,
) -> Option<String> {
    expr.binder_name_from_type_part()
}

/// Shared [`BinderNameFn`] for value-binder builtins (`LET <name> = …`, `MODULE <name> = …`): the
/// binder name is `parts[1]`'s `Identifier` token. The Identifier-part twin of
/// [`type_part_binder_name`], so each overload's extractor matches exactly its own name-part kind
/// and the submit-time placeholder is tagged `Value` xor `Type` to match where the bind lands.
pub(crate) fn identifier_part_binder_name(
    expr: &crate::machine::model::KExpression<'_>,
) -> Option<String> {
    match &expr.parts.get(1)?.value {
        crate::machine::model::ExpressionPart::Identifier(s) => Some(s.clone()),
        _ => None,
    }
}

/// Full-form builtin registration with both binder hooks. The `body` is
/// an [`ActionFn`](crate::machine::ActionFn) (`fn(&BodyCtx) -> Action`) installed
/// as [`Body::Builtin`] — the builtin runs through `machine::execute::runtime::run_action`.
/// `binder_bucket` lets FN key pending-overload entries by inner-call bucket.
pub(crate) fn register_builtin_full<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature<'a>,
    body: crate::machine::ActionFn,
    binder_name: Option<(BinderNameFn, crate::machine::BindKind)>,
    binder_bucket: Option<crate::machine::BinderBucketFn>,
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
    body: crate::machine::ActionFn,
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
    body: crate::machine::ActionFn,
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

/// Allocate the run-global root scope and the mutable `RunScope` child of it that carries
/// top-level Koan bindings. Neither is seeded — [`seed_builtins`] populates the root. The
/// root stays builtin-only and immutable; a top-level bind lands in the `RunScope`, leaving
/// the root binding-free. Builtins resolve from any scope by walking `outer` to the root
/// (the [`Scope::shadows_builtin_value`] no-shadow consult does the same).
pub fn unseeded_scopes<'a>(
    run_storage: &'a std::rc::Rc<crate::machine::FrameStorage>,
    out: Box<dyn std::io::Write + 'a>,
) -> (&'a Scope<'a>, &'a Scope<'a>) {
    let root = run_storage
        .brand()
        .alloc_scope(Scope::run_root(run_storage, None, out));
    let child = run_storage.brand().alloc_scope(Scope::run_child(root));
    (root, child)
}

/// Register every builtin type and `KFunction` onto the run root. `types` is the run
/// frame's registry, the home the seeded types answer from.
///
/// Registration order does not affect dispatch — [`Scope::resolve_dispatch`] buckets by
/// untyped signature shape and picks overloads by `KType` specificity.
pub fn seed_builtins<'a>(scope: &'a Scope<'a>, _types: &TypeRegistry) {
    // TODO(interned-type-content): `_types` is the interning operand for seeding's own
    // KType construction; the per-module `register` fan-out lands with that item.
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
    op_def::register(scope);
    group_def::register(scope);
    arithmetic::register(scope);
    arithmetic::register_builtin_operator_groups(scope);
    equality::register(scope);
}

/// One-call constructor for tests and integration tests: allocate the scope pair, seed the
/// root, and hand back the `RunScope` child. Production ([`crate::machine::execute`]'s
/// `interpret`) sequences [`unseeded_scopes`] and [`seed_builtins`] itself so seeding
/// receives the run frame's own registry rather than a cold one.
pub fn default_scope<'a>(
    run_storage: &'a std::rc::Rc<crate::machine::FrameStorage>,
    out: Box<dyn std::io::Write + 'a>,
) -> &'a Scope<'a> {
    let (root, child) = unseeded_scopes(run_storage, out);
    seed_builtins(root, &TypeRegistry::new());
    child
}
