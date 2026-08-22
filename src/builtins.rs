use crate::machine::model::KKind;
use crate::machine::model::RunRegistries;
use crate::machine::model::{
    Argument, BinderSymbol, KType, ReturnType, SignatureDraft, SignatureElement,
};
use crate::machine::{BindingIndex, Scope, WriteGate};
use crate::machine::{Body, KFunction};

pub(crate) mod arithmetic;
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

/// Signature-element constructor for a keyword slot. The text rides as a borrow at whatever
/// lifetime the caller has — a `&'static` literal for a builtin, an operator symbol at the defining
/// scope's — since the mint door re-homes every signature name at the function's own region.
pub(crate) fn kw(s: &str) -> SignatureElement<'_> {
    SignatureElement::keyword(s)
}

/// Signature-element constructor for an argument slot. The name is syntactic, so it classifies and
/// interns here: the classified symbol it becomes is the whole argument, and diagnostics resolve
/// the text back through the same interner. A builtin's parameter spelling is programmer-controlled
/// literal text, so a keyword-class name is a build-time mistake, not a runtime disposition.
pub(crate) fn arg<'a>(
    registries: &RunRegistries,
    name: &str,
    ktype: KType,
) -> SignatureElement<'a> {
    SignatureElement::Argument(Argument {
        name: BinderSymbol::declared(name, &registries.labels)
            .expect("a builtin parameter name is a value or Type token"),
        ktype,
    })
}

/// Assemble a [`SignatureDraft`] with `Resolved(return_type)`. Builtins needing
/// `Deferred(...)` build the draft directly.
pub(crate) fn sig<'a>(
    return_type: KType,
    elements: Vec<SignatureElement<'a>>,
) -> SignatureDraft<'a> {
    SignatureDraft {
        return_type: ReturnType::Resolved(return_type),
        elements,
    }
}

/// Builtin registration. The `body` is an [`ActionFn`](crate::machine::ActionFn)
/// (`fn(&BodyCtx) -> Action`) installed as [`Body::Builtin`] — the builtin runs through
/// `machine::execute`'s action harness (`run_action`).
///
/// Whether the registered form introduces a binder is not declared here: binder-ness, the
/// name/bucket extractors it implies, and the chain-slot mask all live once in the static spec
/// table ([`crate::machine::model::binder`]), keyed by untyped signature shape, and dispatch reads
/// them off the expression's cached plan.
pub(crate) fn register_builtin<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: SignatureDraft<'a>,
    body: crate::machine::ActionFn,
    registries: &RunRegistries,
    gate: &mut WriteGate,
) {
    let cell = KFunction::alloc_captured(scope, signature, Body::Builtin(body), registries);
    let _ =
        scope.register_function_direct(name.into(), &cell, BindingIndex::BUILTIN, registries, gate);
}

/// Test-only: register one overload at an explicit [`BindingIndex`]. A test uses this to
/// place a *user*-position (non-`BUILTIN`) overload in a root-position scope, so dispatch
/// exercises the ordinary innermost-wins walk rather than the builtin root-first
/// short-circuit (which a `BUILTIN`-index entry in the root would trigger).
#[cfg(test)]
pub(crate) fn register_overload_at<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    signature: SignatureDraft<'a>,
    body: crate::machine::ActionFn,
    index: BindingIndex,
    registries: &RunRegistries,
    gate: &mut WriteGate,
) {
    let cell = KFunction::alloc_captured(scope, signature, Body::Builtin(body), registries);
    scope
        .register_function_direct(name.into(), &cell, index, registries, gate)
        .expect("register_overload_at: user-index overload should not collide with a builtin");
}

/// Allocate the run-global root scope and the mutable `RunScope` child of it that carries
/// top-level Koan bindings. Neither is seeded — [`seed_builtins`] populates the root. The
/// root stays builtin-only and immutable; a top-level bind lands in the `RunScope`, leaving
/// the root binding-free. Builtins resolve from any scope by walking `outer` to the root
/// (the [`Scope::shadows_builtin_value`] no-shadow consult does the same).
pub fn unseeded_scopes<'a>(
    run_storage: &'a std::rc::Rc<crate::machine::FrameStorage>,
) -> (&'a Scope<'a>, &'a Scope<'a>) {
    let root = Scope::alloc_run_root(run_storage);
    let child = root.alloc_run_child();
    (root, child)
}

/// A builtin type's name as a [`TypeSymbol`]. Builtin registration text is programmer-controlled
/// and every entry below is a Type token, so a classification miss is a build-time typo rather
/// than a runtime disposition.
fn builtin_type_name(name: &str, registries: &RunRegistries) -> crate::machine::model::TypeSymbol {
    crate::machine::model::TypeSymbol::declared(name, &registries.labels)
        .expect("a builtin type name is a Type token")
}

/// Register every builtin type and `KFunction` onto the run root. `types` is the run
/// frame's registry, the home the seeded types answer from.
///
/// Registration order does not affect dispatch — [`Scope::resolve_dispatch`] buckets by
/// untyped signature shape and picks overloads by `KType` specificity.
pub(crate) fn seed_builtins<'a>(
    scope: &'a Scope<'a>,
    registries: &RunRegistries,
    gate: &mut WriteGate,
) {
    scope.register_builtin_type(
        builtin_type_name("Number", registries),
        KType::NUMBER,
        registries,
        gate,
    );
    scope.register_builtin_type(
        builtin_type_name("Str", registries),
        KType::STR,
        registries,
        gate,
    );
    scope.register_builtin_type(
        builtin_type_name("Bool", registries),
        KType::BOOL,
        registries,
        gate,
    );
    scope.register_builtin_type(
        builtin_type_name("Null", registries),
        KType::NULL,
        registries,
        gate,
    );
    scope.register_builtin_type(
        builtin_type_name("List", registries),
        KType::LIST_OF_ANY,
        registries,
        gate,
    );
    scope.register_builtin_type(
        builtin_type_name("Dict", registries),
        KType::DICT_ANY_ANY,
        registries,
        gate,
    );
    scope.register_builtin_type(
        builtin_type_name("KExpression", registries),
        KType::KEXPRESSION,
        registries,
        gate,
    );
    scope.register_builtin_type(
        builtin_type_name("Type", registries),
        KType::of_kind(KKind::AnyType),
        registries,
        gate,
    );
    scope.register_builtin_type(
        builtin_type_name("Module", registries),
        KType::EMPTY_SIGNATURE,
        registries,
        gate,
    );
    scope.register_builtin_type(
        builtin_type_name("Signature", registries),
        KType::of_kind(KKind::Signature),
        registries,
        gate,
    );
    scope.register_builtin_type(
        builtin_type_name("Any", registries),
        KType::ANY,
        registries,
        gate,
    );

    let_binding::register(scope, registries, gate);
    print::register(scope, registries, gate);
    fn_def::register(scope, registries, gate);
    union::register(scope, registries, gate);
    result::register(scope, registries, gate);
    newtype_def::register(scope, registries, gate);
    match_case::register(scope, registries, gate);
    try_with::register(scope, registries, gate);
    using_scope::register(scope, registries, gate);
    catch::register(scope, registries, gate);
    attr::register(scope, registries, gate);
    eval::register(scope, registries, gate);
    module_def::register(scope, registries, gate);
    sig_def::register(scope, registries, gate);
    val_decl::register(scope, registries, gate);
    type_decl::register(scope, registries, gate);
    ascribe::register(scope, registries, gate);
    record_projection::register(scope, registries, gate);
    type_ops::register(scope, registries, gate);
    parameterized_types::register(scope, registries, gate);
    type_union::register(scope, registries, gate);
    op_def::register(scope, registries, gate);
    group_def::register(scope, registries, gate);
    arithmetic::register(scope, registries, gate);
    arithmetic::register_builtin_operator_groups(scope, registries, gate);
    equality::register(scope, registries, gate);
}
