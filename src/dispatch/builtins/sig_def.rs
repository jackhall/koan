//! `SIG <name:TypeExprRef> = <body:KExpression>` — declare a module signature (an interface
//! a module can be ascribed to). See
//! [design/module-system.md](../../../design/module-system.md).
//!
//! Construction shape mirrors [`module_def`](super::module_def): the body is a parens-
//! wrapped KExpression dispatched against a fresh child scope. The body's declarations are
//! `LET name = (FN <signature> -> <return> = ...)` for operations and `LET Type = TypeExpr`
//! for abstract type declarations (stage 4 will add `axiom`s here too). The captured child
//! scope is wrapped in a [`Signature`] value, allocated in the parent's arena, and bound
//! under the signature's name.
//!
//! Stage 1 stores the raw scope; the ascription operators (`:|` / `:!`) iterate it at
//! ascription time. Stage 2 (functors) consumes signatures as parameter types; stage 4
//! attaches axioms.

use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::dispatch::runtime::{KError, KErrorKind, Scope};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::values::{KObject, Signature};

use crate::parse::kexpression::KExpression;

use super::helpers::{extract_bare_type_name, extract_kexpression, run_body_statements};
use super::{err, register_builtin_with_pre_run};

pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
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

    if let Err(e) = run_body_statements(decl_scope, body_expr) {
        return BodyResult::Err(e);
    }

    let sig: &'a Signature<'a> = arena.alloc_signature(Signature::new(name.clone(), decl_scope));
    let sig_obj: &'a KObject<'a> = arena.alloc_object(KObject::KSignature(sig));
    if let Err(e) = scope.bind_value(name, sig_obj) {
        return err(e);
    }
    BodyResult::Value(sig_obj)
}

/// Dispatch-time placeholder extractor for SIG. `parts[1]` is the `Type(t)` token of the
/// signature's name slot. Same shape as STRUCT / MODULE / named UNION.
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    super::helpers::binder_name_from_type_part(expr)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin_with_pre_run(
        scope,
        "SIG",
        ExpressionSignature {
            return_type: KType::Signature,
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
    use crate::dispatch::builtins::test_support::{run, run_root_silent};
    use crate::dispatch::runtime::RuntimeArena;
    use crate::dispatch::values::KObject;
    use crate::parse::expression_tree::parse;

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
        let data = scope.data.borrow();
        assert!(matches!(data.get("OrderedSig"), Some(KObject::KSignature(_))));
    }

    #[test]
    fn sig_path_records_name() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG OrderedSig = (LET x = 1)");
        let data = scope.data.borrow();
        let sig = match data.get("OrderedSig") {
            Some(KObject::KSignature(s)) => *s,
            _ => panic!("OrderedSig should be a signature"),
        };
        assert_eq!(sig.path, "OrderedSig");
    }
}
