use std::rc::Rc;

use crate::dispatch::runtime::{KError, KErrorKind};
use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, Parseable, SignatureElement};
use crate::dispatch::values::KObject;
use crate::dispatch::runtime::Scope;
use crate::dispatch::values::dispatch_constructor;
use crate::parse::kexpression::{KExpression, TypeParams};

use super::{err, register_builtin};

/// `<verb:TypeExprRef> <args:KExpression>` — the type-token construction path.
///
/// Mirrors [`call_by_name`](super::call_by_name) but for a leading type-token. Looks up
/// `verb` in scope and routes by the resolved `KObject` variant: `TaggedUnionType` hands
/// off to [`tagged_union::apply`] (constructs `(tag value)`-shaped tagged values);
/// `StructType` hands off to [`struct_value::apply`] (constructs positional struct values
/// from N field arguments). Anything else surfaces a `TypeMismatch`.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // The verb slot is `TypeExprRef`, so its resolved value is `KObject::TypeExprValue(t)`.
    // The name slot wants the bare type name; reject parameterized forms (`List<Number>` as
    // a constructor verb makes no sense here).
    let verb = match bundle.get("verb") {
        Some(KObject::TypeExprValue(t)) => match &t.params {
            TypeParams::None => t.name.clone(),
            _ => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "type-call verb must be a bare type name, got `{}`",
                    t.render(),
                ))));
            }
        },
        Some(other) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "TypeExprRef".to_string(),
                got: other.summarize(),
            }));
        }
        None => return err(KError::new(KErrorKind::MissingArg("verb".to_string()))),
    };
    let args_expr = match extract_kexpression(&mut bundle, "args") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "type-call args slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    match scope.lookup(&verb) {
        Some(obj) => match dispatch_constructor(obj, args_expr.parts) {
            Some(result) => result,
            None => err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "Type".to_string(),
                got: obj.ktype().name().to_string(),
            })),
        },
        None => err(KError::new(KErrorKind::UnboundName(verb))),
    }
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
        "type_call",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Argument(Argument { name: "verb".into(), ktype: KType::TypeExprRef }),
                SignatureElement::Argument(Argument { name: "args".into(), ktype: KType::KExpression }),
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

    use crate::dispatch::runtime::RuntimeArena;
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::runtime::KErrorKind;
    use crate::dispatch::values::KObject;
    use crate::dispatch::runtime::Scope;
    use crate::execute::scheduler::Scheduler;
    use crate::parse::expression_tree::parse;
    use crate::parse::kexpression::KExpression;

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

    fn parse_one(src: &str) -> KExpression<'static> {
        let mut exprs = parse(src).expect("parse should succeed");
        assert_eq!(exprs.len(), 1, "test helper expects a single expression");
        exprs.remove(0)
    }

    fn run<'a>(scope: &'a Scope<'a>, source: &str) {
        let exprs = parse(source).expect("parse should succeed");
        let mut sched = Scheduler::new();
        for expr in exprs {
            sched.add_dispatch(expr, scope);
        }
        sched.execute().expect("scheduler should succeed");
    }

    fn run_one<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> &'a KObject<'a> {
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().expect("scheduler should succeed");
        sched.read(id)
    }

    fn run_one_err<'a>(
        scope: &'a Scope<'a>,
        expr: KExpression<'a>,
    ) -> crate::dispatch::runtime::KError {
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().expect("scheduler should not surface errors directly");
        match sched.read_result(id) {
            Ok(_) => panic!("expected error"),
            Err(e) => e.clone(),
        }
    }

    #[test]
    fn type_token_calls_construct_tagged_value() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "UNION Maybe = (some: Number none: Null)");
        let result = run_one(scope, parse_one("Maybe (some 42)"));
        match result {
            KObject::Tagged { tag, value } => {
                assert_eq!(tag, "some");
                assert!(matches!(&**value, KObject::Number(n) if *n == 42.0));
            }
            other => panic!("expected Tagged, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn type_call_unbound_type_errors() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let err = run_one_err(scope, parse_one("Bogus (some 42)"));
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(name) if name == "Bogus"),
            "expected UnboundName(Bogus), got {err}",
        );
    }

    #[test]
    fn type_call_propagates_tag_validation_error() {
        // The synthesized TAG call surfaces the schema's tag check.
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "UNION Maybe = (some: Number none: Null)");
        let err = run_one_err(scope, parse_one("Maybe (other 42)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`other`")),
            "expected ShapeError mentioning `other`, got {err}",
        );
    }

    #[test]
    fn type_call_with_sub_expression_value() {
        // `(x)` parens-wrapping forces the value-side identifier to resolve via value_lookup
        // before TAG's typed-slot bind sees it.
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "UNION Maybe = (some: Number none: Null)\nLET x = 7");
        let result = run_one(scope, parse_one("Maybe (some (x))"));
        match result {
            KObject::Tagged { tag, value } => {
                assert_eq!(tag, "some");
                assert!(matches!(&**value, KObject::Number(n) if *n == 7.0));
            }
            other => panic!("expected Tagged, got {:?}", other.ktype()),
        }
    }
}
