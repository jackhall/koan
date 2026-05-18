use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};

use crate::builtins::err;

/// `FUNCTION_OF <args:KExpression> -> <ret:TypeExprRef>` → `TypeExprRef` carrying
/// `Function<(args) -> ret>`. The `args` slot is captured raw as a `KExpression` whose
/// parts are bare `Type(_)` tokens; we re-extract and elaborate each into a `KType`.
/// Parameterized inner args (`List<Number>`) come through as `Future(KTypeValue(kt))` from
/// a prior sub-dispatch; leaf `Type(t)` tokens go through the resolver-free
/// [`KType::from_type_expr`] (builtin-table only) to handle nested-parameter shapes.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let args_expr = match bundle.require_kexpression("args") {
        Ok(e) => e.clone(),
        Err(e) => return err(e),
    };
    let ret = match bundle.require_ktype("ret") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    let mut args: Vec<KType> = Vec::with_capacity(args_expr.parts.len());
    for part in &args_expr.parts {
        match part {
            ExpressionPart::Type(t) => match KType::from_type_expr(t) {
                Ok(kt) => args.push(kt),
                Err(msg) => {
                    return err(KError::new(KErrorKind::ShapeError(format!(
                        "FUNCTION_OF args: {msg}"
                    ))));
                }
            },
            ExpressionPart::Future(KObject::KTypeValue(kt)) => args.push(kt.clone()),
            other => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "FUNCTION_OF args must be type names, got `{}`",
                    other.summarize()
                ))));
            }
        }
    }
    BodyResult::Value(
        scope.arena.alloc_object(KObject::KTypeValue(KType::KFunction {
            args,
            ret: Box::new(ret),
        })),
    )
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run_one, run_root_silent};
    use crate::machine::model::{KObject, KType};
    use crate::machine::RuntimeArena;

    /// `FUNCTION_OF (Number) -> Str` dispatches and produces a `KTypeValue` carrying
    /// `KFunction { args: [Number], ret: Str }`.
    #[test]
    fn function_of_unary_lowers_to_kfunction() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("FUNCTION_OF (Number) -> Str"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::KFunction {
                        args: vec![KType::Number],
                        ret: Box::new(KType::Str),
                    }
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// `FUNCTION_OF () -> Number` — empty args group, the parts list is empty.
    #[test]
    fn function_of_nullary_lowers_to_kfunction() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("FUNCTION_OF () -> Number"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::KFunction {
                        args: vec![],
                        ret: Box::new(KType::Number),
                    }
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// `FUNCTION_OF (Number Bool) -> Number` — multi-arg form, each bare `Type` token
    /// elaborates through `KType::from_type_expr`.
    #[test]
    fn function_of_multi_arg_lowers_to_kfunction() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("FUNCTION_OF (Number Bool) -> Number"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::KFunction {
                        args: vec![KType::Number, KType::Bool],
                        ret: Box::new(KType::Number),
                    }
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }
}
