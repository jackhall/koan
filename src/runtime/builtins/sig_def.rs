//! `SIG <name:TypeExprRef> = <body:KExpression>` — declare a module signature (an interface
//! a module can be ascribed to). See
//! [design/module-system.md](../../../design/module-system.md).
//!
//! Construction shape mirrors [`module_def`](super::module_def): body statements dispatch
//! against a fresh child scope on the OUTER scheduler, then a `Combine` over those slots
//! captures the populated scope into a [`Signature`] value, allocates it in the parent's
//! arena, and binds it under the signature's name. Body declarations are
//! `LET name = (FN <signature> -> <return> = ...)` for operations and `LET Type = TypeExpr`
//! for abstract type declarations (stage 4 will add `axiom`s here too).
//!
//! Stage 1 stores the raw scope; the ascription operators (`:|` / `:!`) iterate it at
//! ascription time. Stage 2 (functors) consumes signatures as parameter types; stage 4
//! attaches axioms.

use crate::runtime::model::{Argument, ExpressionSignature, KObject, KType, SignatureElement, ReturnType};
use crate::runtime::machine::{ArgumentBundle, BodyResult, CombineFinish, Frame, KError, KErrorKind, Scope, SchedulerHandle};
use crate::runtime::model::values::Signature;

use crate::ast::KExpression;

use crate::runtime::machine::kfunction::argument_bundle::{extract_bare_type_name, extract_kexpression};
use super::{err, register_builtin_with_pre_run};

pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let name = match extract_bare_type_name(&bundle, "name", "SIG") {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    let body_expr = match extract_kexpression(&mut bundle, "body") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "SIG body slot must be a parenthesized expression".to_string(),
            )));
        }
    };

    let arena = scope.arena;
    let decl_scope = arena.alloc_scope(Scope::child_under_named(
        scope,
        format!("SIG {}", name),
    ));

    let deps = sched.plan_body_statements(decl_scope, body_expr);

    let name_for_finish = name.clone();
    let finish: CombineFinish<'a> = Box::new(move |parent_scope, _sched, _results| {
        let arena = parent_scope.arena;
        let sig: &'a Signature<'a> =
            arena.alloc_signature(Signature::new(name_for_finish.clone(), decl_scope));
        let sig_obj: &'a KObject<'a> = arena.alloc_object(KObject::KSignature(sig));
        // SIG is not a `UserTypeKind`; the identity carrier stays `SignatureBound`.
        // Dual-write so type-name resolution finds the signature by name without
        // consulting `bindings.data` for the value-side carrier.
        let identity = KType::SignatureBound {
            sig_id: sig.sig_id(),
            sig_path: name_for_finish.clone(),
            // Unconstrained at the SIG-declaration site; `SIG_WITH` pins slots later.
            pinned_slots: Vec::new(),
        };
        match parent_scope.register_nominal(name_for_finish.clone(), identity, sig_obj) {
            Ok(obj) => BodyResult::Value(obj),
            Err(e) => BodyResult::Err(e.with_frame(Frame {
                function: "<signature>".to_string(),
                expression: format!("SIG {} body", name_for_finish),
            })),
        }
    });
    let combine_id = sched.add_combine(deps, scope, finish);
    BodyResult::DeferTo(combine_id)
}

/// Dispatch-time placeholder extractor for SIG. `parts[1]` is the `Type(t)` token of the
/// signature's name slot. Same shape as STRUCT / MODULE / named UNION.
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    expr.binder_name_from_type_part()
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin_with_pre_run(
        scope,
        "SIG",
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Signature),
            elements: vec![
                SignatureElement::Keyword("SIG".into()),
                SignatureElement::Argument(Argument {
                    name: "name".into(),
                    ktype: KType::TypeExprRef,
                }),
                SignatureElement::Keyword("=".into()),
                SignatureElement::Argument(Argument {
                    name: "body".into(),
                    ktype: KType::KExpression,
                }),
            ],
        },
        body,
        Some(pre_run),
    );
}

#[cfg(test)]
mod tests {
    use crate::runtime::builtins::test_support::{run, run_root_silent};
    use crate::runtime::model::KObject;
    use crate::runtime::machine::RuntimeArena;
    use crate::parse::parse;

    /// Smoke test for SIG's pre_run extractor: structural extraction of the `Type(_)`
    /// token at `parts[1]`.
    #[test]
    fn pre_run_extracts_sig_name() {
        let mut exprs = parse("SIG OrderedSig = (LET x = 1)").expect("parse should succeed");
        let expr = exprs.remove(0);
        let name = super::pre_run(&expr);
        assert_eq!(name.as_deref(), Some("OrderedSig"));
    }

    #[test]
    fn sig_binds_under_name_in_scope() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG OrderedSig = (LET x = 1)");
        let data = scope.bindings().data();
        assert!(matches!(data.get("OrderedSig"), Some(KObject::KSignature(_))));
    }

    #[test]
    fn sig_path_records_name() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG OrderedSig = (LET x = 1)");
        let data = scope.bindings().data();
        let sig = match data.get("OrderedSig") {
            Some(KObject::KSignature(s)) => *s,
            _ => panic!("OrderedSig should be a signature"),
        };
        assert_eq!(sig.path, "OrderedSig");
    }

    /// Body-statement forward-reference: SIG body's `LET x = y` references a sibling
    /// top-level binding. Mirrors `module_def::module_body_parks_on_outer_placeholder` —
    /// post-refactor the body statement parks on the outer placeholder.
    #[test]
    fn sig_body_parks_on_outer_placeholder() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET y = 7\nSIG Foo = (LET x = y)");
        let data = scope.bindings().data();
        let sig = match data.get("Foo") {
            Some(KObject::KSignature(s)) => *s,
            _ => panic!("Foo should be a signature"),
        };
        let inner = sig.decl_scope().bindings().data();
        assert!(matches!(inner.get("x"), Some(KObject::Number(n)) if *n == 7.0));
    }

    /// Failing body statement surfaces as the SIG node's error and must NOT bind `Foo` in
    /// the parent scope.
    #[test]
    fn sig_body_error_short_circuits_finalize() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG Foo = (LET x = nonexistent_name)");
        assert!(
            scope.bindings().data().get("Foo").is_none(),
            "Foo must not bind when its body errors",
        );
    }

    /// Stage 3.1: SIG finalize dual-writes a `KType::SignatureBound` into
    /// `bindings.types` next to the `KObject::KSignature` carrier in `bindings.data`.
    /// Without this, deleting `body_type_expr`'s `scope.lookup` fall-through would
    /// break every SIG-typed name lookup.
    #[test]
    fn sig_dual_writes_to_types_and_data() {
        use crate::runtime::model::types::KType;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG OrderedSig = (LET x = 1)");
        let types = scope.bindings().types();
        let kt = types
            .get("OrderedSig")
            .expect("OrderedSig should be in bindings.types");
        assert!(matches!(
            **kt,
            KType::SignatureBound { ref sig_path, .. } if sig_path == "OrderedSig"
        ));
        drop(types);
        let data = scope.bindings().data();
        let obj = data
            .get("OrderedSig")
            .expect("OrderedSig should be in bindings.data");
        assert!(matches!(obj, KObject::KSignature(_)));
    }
}
