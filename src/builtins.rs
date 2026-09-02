use crate::machine::model::RunRegistries;
use crate::machine::model::{
    Argument, BinderSymbol, KType, ReturnType, SignatureDraft, SignatureElement, StaticName,
    ValueSymbol, carrier_union_error,
};
use crate::machine::{BindingIndex, Scope, WriteGate};
use crate::machine::{Body, KFunction};

pub(crate) mod arithmetic;
mod ascribe;
mod attr;
mod await_body;
mod branch_walk;
mod catch;
mod close_over;
mod equality;
mod error_union;
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
pub(crate) mod union;
mod using_scope;
mod val_decl;

#[cfg(test)]
mod tests;

pub mod test_support;

/// Signature-element constructor for a keyword slot. The spelling is normalized, classified and
/// interned at this door, so the element is the token's symbol and that registration is what lets a
/// diagnostic naming this shape's bucket resolve the keyword back out of a symbol-only key.
pub(crate) fn kw(registries: &RunRegistries, s: &str) -> SignatureElement {
    SignatureElement::keyword(s, &registries.labels)
}

/// Signature-element constructor for an argument slot. The name arrives as a [`StaticName`] the
/// builtin declares beside its own body, so its symbol is already minted and registration only
/// records the spelling: the classified symbol is the whole argument, and diagnostics resolve the
/// text back through the same interner. Slots are value-class by declaration —
/// [`StaticName<ValueSymbol>`] is what the static carries — so the class is settled where the
/// spelling is written rather than probed here.
///
/// Every builtin slot passes through this door, so it is where a union carrier slot's
/// well-formedness is pinned ([`carrier_union_error`]): a union whose members disagree on which
/// raw part shape each claims would make admission and capture depend on member order, and
/// `Union` identity is order-blind. Only a builtin author can construct one, so the failure is a
/// seed-time panic rather than a user-facing diagnostic.
pub(crate) fn arg(
    registries: &RunRegistries,
    name: &StaticName<ValueSymbol>,
    ktype: KType,
) -> SignatureElement {
    if let Some(error) = carrier_union_error(ktype, registries) {
        panic!("builtin slot `{}`: {error}", name.text());
    }
    SignatureElement::Argument(Argument {
        name: BinderSymbol::Value(registries.labels.record(name)),
        ktype,
    })
}

/// Assemble a [`SignatureDraft`] with `Resolved(return_type)`. Builtins needing
/// `Deferred(...)` build the draft directly.
pub(crate) fn sig<'a>(return_type: KType, elements: Vec<SignatureElement>) -> SignatureDraft<'a> {
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
    signature: SignatureDraft<'a>,
    body: crate::machine::ActionFn,
    registries: &RunRegistries,
    gate: &mut WriteGate,
) {
    let cell = KFunction::alloc_captured(
        scope,
        signature.return_type,
        &signature.elements,
        Body::Builtin(body),
        registries,
    );
    let _ = scope.register_function_direct(&cell, BindingIndex::BUILTIN, registries, gate);
}

/// Test-only: register one overload at an explicit [`BindingIndex`]. A test uses this to
/// place a *user*-position (non-`BUILTIN`) overload in a root-position scope, so dispatch
/// exercises the ordinary innermost-wins walk rather than the builtin root-first
/// short-circuit (which a `BUILTIN`-index entry in the root would trigger).
#[cfg(test)]
pub(crate) fn register_overload_at<'a>(
    scope: &'a Scope<'a>,
    signature: SignatureDraft<'a>,
    body: crate::machine::ActionFn,
    index: BindingIndex,
    registries: &RunRegistries,
    gate: &mut WriteGate,
) {
    let cell = KFunction::alloc_captured(
        scope,
        signature.return_type,
        &signature.elements,
        Body::Builtin(body),
        registries,
    );
    scope
        .register_function_direct(&cell, index, registries, gate)
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
    for (name, ktype) in crate::machine::model::builtin_types() {
        scope.register_builtin_type(registries.labels.record(name), ktype, registries, gate);
    }

    let_binding::register(scope, registries, gate);
    print::register(scope, registries, gate);
    fn_def::register(scope, registries, gate);
    union::register(scope, registries, gate);
    result::register(scope, registries, gate);
    error_union::register(scope, registries, gate);
    newtype_def::register(scope, registries, gate);
    match_case::register(scope, registries, gate);
    try_with::register(scope, registries, gate);
    using_scope::register(scope, registries, gate);
    close_over::register(scope, registries, gate);
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
