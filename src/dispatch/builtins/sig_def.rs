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

use std::rc::Rc;

use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::dispatch::runtime::{KError, KErrorKind, Scope};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::values::{KObject, Signature};
use crate::execute::scheduler::Scheduler;
use crate::parse::kexpression::{ExpressionPart, KExpression, TypeParams};

use super::{err, register_builtin};

pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let name = match bundle.get("name") {
        Some(KObject::TypeExprValue(t)) => match &t.params {
            TypeParams::None => t.name.clone(),
            _ => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "SIG name must be a bare type name, got `{}`",
                    t.render(),
                ))));
            }
        },
        Some(other) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "name".to_string(),
                expected: "TypeExprRef".to_string(),
                got: other.ktype().name(),
            }));
        }
        None => return err(KError::new(KErrorKind::MissingArg("name".to_string()))),
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
    scope.add(name, sig_obj);
    BodyResult::Value(sig_obj)
}

fn run_body_statements<'a>(
    decl_scope: &'a Scope<'a>,
    body_expr: KExpression<'a>,
) -> Result<(), KError> {
    let is_multi_statement = !body_expr.parts.is_empty()
        && body_expr
            .parts
            .iter()
            .all(|p| matches!(p, ExpressionPart::Expression(_)));
    let mut sched = Scheduler::new();
    let ids: Vec<crate::dispatch::kfunction::NodeId> = if is_multi_statement {
        body_expr
            .parts
            .into_iter()
            .filter_map(|p| match p {
                ExpressionPart::Expression(e) => Some(sched.add_dispatch(*e, decl_scope)),
                _ => None,
            })
            .collect()
    } else {
        vec![sched.add_dispatch(body_expr, decl_scope)]
    };
    sched.execute()?;
    for id in ids {
        if let Err(e) = sched.read_result(id) {
            return Err(e.clone());
        }
    }
    Ok(())
}

fn extract_kexpression<'a>(
    bundle: &mut ArgumentBundle<'a>,
    name: &str,
) -> Option<KExpression<'a>> {
    let rc = bundle.args.remove(name)?;
    match Rc::try_unwrap(rc) {
        Ok(KObject::KExpression(e)) => Some(e),
        Ok(_) => None,
        Err(rc) => match &*rc {
            KObject::KExpression(e) => Some(e.clone()),
            _ => None,
        },
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
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
    );
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::runtime::{RuntimeArena, Scope};
    use crate::dispatch::values::KObject;
    use crate::execute::scheduler::Scheduler;
    use crate::parse::expression_tree::parse;

    struct SharedBuf(Rc<RefCell<Vec<u8>>>);
    impl Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    fn build_scope<'a>(arena: &'a RuntimeArena, captured: Rc<RefCell<Vec<u8>>>) -> &'a Scope<'a> {
        default_scope(arena, Box::new(SharedBuf(captured)))
    }

    fn run<'a>(scope: &'a Scope<'a>, source: &str) {
        let exprs = parse(source).expect("parse should succeed");
        let mut sched = Scheduler::new();
        for expr in exprs {
            sched.add_dispatch(expr, scope);
        }
        sched.execute().expect("scheduler should succeed");
    }

    #[test]
    fn sig_binds_under_name_in_scope() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "SIG OrderedSig = (LET x = 1)");
        let data = scope.data.borrow();
        assert!(matches!(data.get("OrderedSig"), Some(KObject::KSignature(_))));
    }

    #[test]
    fn sig_path_records_name() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "SIG OrderedSig = (LET x = 1)");
        let data = scope.data.borrow();
        let sig = match data.get("OrderedSig") {
            Some(KObject::KSignature(s)) => *s,
            _ => panic!("OrderedSig should be a signature"),
        };
        assert_eq!(sig.path, "OrderedSig");
    }
}
