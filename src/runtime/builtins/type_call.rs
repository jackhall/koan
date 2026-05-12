use crate::runtime::model::{Argument, ExpressionSignature, KType, SignatureElement};
use crate::runtime::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};
use crate::runtime::model::values::dispatch_constructor;

use crate::runtime::machine::kfunction::argument_bundle::{extract_bare_type_name, extract_kexpression};
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
    // a constructor verb makes no sense here). The shared helper reports
    // `other.ktype().name()` for the non-`TypeExprValue` branch — slightly different from
    // the previous `other.summarize()` here, but the value is debug-only (no test asserts
    // on this `got:` field) and consolidating beats a one-off divergence.
    let verb = match extract_bare_type_name(&bundle, "verb", "type-call") {
        Ok(n) => n,
        Err(e) => return err(e),
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
    use crate::runtime::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::runtime::model::KObject;
    use crate::runtime::machine::{KErrorKind, RuntimeArena};

    #[test]
    fn type_token_calls_construct_tagged_value() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let scope = run_root_silent(&arena);
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
        let scope = run_root_silent(&arena);
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
        let scope = run_root_silent(&arena);
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
