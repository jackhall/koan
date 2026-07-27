use crate::machine::model::KKind;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{Argument, ExpressionSignature, KType, ReturnType, SignatureElement};
use crate::machine::{BindingIndex, FrameStorageExt, Scope, WriteGate};
use crate::machine::{Body, KFunction};

pub(crate) mod arithmetic;
#[cfg(feature = "ascription")]
mod ascribe;
mod attr;
mod await_body;
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
mod tests;

pub mod test_support;

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

/// Full-form builtin registration marking whether the builtin introduces a binder. The `body` is
/// an [`ActionFn`](crate::machine::ActionFn) (`fn(&BodyCtx) -> Action`) installed
/// as [`Body::Builtin`] — the builtin runs through `machine::execute::runtime::run_action`. The
/// binder's name/bucket extractors and chain-slot mask are the static spec table's business
/// ([`crate::machine::model::binder`]); `binder` is only the classification bit dispatch reads.
pub(crate) fn register_builtin_full<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature<'a>,
    body: crate::machine::ActionFn,
    binder: bool,
    types: &TypeRegistry,
    gate: &mut WriteGate,
) {
    let region = scope.brand();
    let f: &'a KFunction<'a> = region.alloc_function(KFunction::new(
        signature,
        Body::Builtin(body),
        scope,
        binder,
        types,
    ));
    let _ = scope.register_function_direct(name.into(), f, BindingIndex::BUILTIN, gate);
}

/// Common-case [`register_builtin_full`]: not a binder builtin.
pub(crate) fn register_builtin<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: ExpressionSignature<'a>,
    body: crate::machine::ActionFn,
    types: &TypeRegistry,
    gate: &mut WriteGate,
) {
    register_builtin_full(scope, name, signature, body, false, types, gate);
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
    types: &TypeRegistry,
    gate: &mut WriteGate,
) {
    let region = scope.brand();
    let f: &'a KFunction<'a> = region.alloc_function(KFunction::new(
        signature,
        Body::Builtin(body),
        scope,
        false,
        types,
    ));
    scope
        .register_function_direct(name.into(), f, index, gate)
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
pub(crate) fn seed_builtins<'a>(scope: &'a Scope<'a>, types: &TypeRegistry, gate: &mut WriteGate) {
    scope.register_builtin_type("Number".into(), KType::NUMBER, gate);
    scope.register_builtin_type("Str".into(), KType::STR, gate);
    scope.register_builtin_type("Bool".into(), KType::BOOL, gate);
    scope.register_builtin_type("Null".into(), KType::NULL, gate);
    scope.register_builtin_type("List".into(), KType::LIST_OF_ANY, gate);
    scope.register_builtin_type("Dict".into(), KType::DICT_ANY_ANY, gate);
    scope.register_builtin_type("KExpression".into(), KType::KEXPRESSION, gate);
    scope.register_builtin_type("Type".into(), KType::of_kind(KKind::AnyType), gate);
    scope.register_builtin_type("Module".into(), KType::EMPTY_SIGNATURE, gate);
    scope.register_builtin_type("Signature".into(), KType::of_kind(KKind::Signature), gate);
    scope.register_builtin_type("Any".into(), KType::ANY, gate);

    let_binding::register(scope, types, gate);
    print::register(scope, types, gate);
    fn_def::register(scope, types, gate);
    union::register(scope, types, gate);
    result::register(scope, types, gate);
    newtype_def::register(scope, types, gate);
    recursive_types::register(scope, types, gate);
    match_case::register(scope, types, gate);
    try_with::register(scope, types, gate);
    using_scope::register(scope, types, gate);
    catch::register(scope, types, gate);
    attr::register(scope, types, gate);
    eval::register(scope, types, gate);
    module_def::register(scope, types, gate);
    sig_def::register(scope, types, gate);
    val_decl::register(scope, types, gate);
    type_decl::register(scope, types, gate);
    #[cfg(feature = "ascription")]
    ascribe::register(scope, types, gate);
    record_projection::register(scope, types, gate);
    type_ops::register(scope, types, gate);
    parameterized_types::register(scope, types, gate);
    type_union::register(scope, types, gate);
    op_def::register(scope, types, gate);
    group_def::register(scope, types, gate);
    arithmetic::register(scope, types, gate);
    arithmetic::register_builtin_operator_groups(scope, types, gate);
    equality::register(scope, types, gate);
}
