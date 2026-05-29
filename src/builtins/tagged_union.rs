//! Tagged-union construction primitives. `apply` is the entry point both surface
//! forms invoke — the type-token call via the `ConstructorCall` fast lane and the
//! identifier-bound LET-alias call via the `FunctionValueCall` fast lane — and
//! synthesizes a tail expression that re-dispatches through the construction-primitive
//! builtin defined here. The primitive has no keyword; three typed slots
//! (`Type`, `Identifier`, `Any`) claim its dispatch bucket unambiguously. Slot-0
//! `Type` is shared with [`super::struct_value`] but the arity differs (2 vs 3 slots).

use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::core::{KError, KErrorKind, Scope, ScopeId};
use crate::machine::core::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::machine::model::types::{
    Argument, ExpressionSignature, KType, ReturnType, SignatureElement, UserTypeKind,
};
use crate::machine::model::values::KObject;

use super::register_builtin;

/// Take the args parts captured at the call site and produce a `BodyResult::Tail`
/// re-dispatching through the construction primitive. `schema_obj` must be a
/// `KObject::TaggedUnionType` (caller-enforced). Validates the args shape: exactly
/// two parts, the first an `Identifier` tag. The second rides through unchanged so
/// the scheduler can resolve sub-expressions before construction sees the value.
pub fn apply<'a>(
    schema_obj: &'a KObject<'a>,
    args_parts: Vec<Spanned<ExpressionPart<'a>>>,
) -> BodyResult<'a> {
    debug_assert!(
        schema_obj.as_tagged_union_type().is_some(),
        "tagged_union::apply called on non-TaggedUnionType",
    );
    if args_parts.len() != 2 {
        return BodyResult::Err(KError::new(KErrorKind::ArityMismatch {
            expected: 2,
            got: args_parts.len(),
        }));
    }
    let mut iter = args_parts.into_iter();
    let tag_part = iter.next().unwrap();
    let value_part = iter.next().unwrap();
    if !matches!(tag_part.value, ExpressionPart::Identifier(_)) {
        return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
            "tagged-union construction = first arg must be a bare-identifier tag, got {}",
            tag_part.value.summarize()
        ))));
    }
    let parts = vec![
        Spanned::bare(ExpressionPart::Future(schema_obj)),
        tag_part,
        value_part,
    ];
    BodyResult::tail(KExpression::new(parts))
}

/// Validate `tag` against `schema` and `value` against the schema's expected type for that
/// tag, then build the `KObject::Tagged`. Pure logic — no scope, no scheduler. The
/// construction-primitive builtin's body is a thin shim around this.
pub fn construct<'a>(
    schema: &HashMap<String, KType<'a>>,
    schema_name: &str,
    schema_scope_id: ScopeId,
    tag: String,
    value: &KObject<'a>,
) -> Result<KObject<'a>, KError> {
    let expected = match schema.get(&tag) {
        Some(t) => t.clone(),
        None => {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "tag `{}` not in union (known: {})",
                tag,
                schema.keys().cloned().collect::<Vec<_>>().join(", ")
            ))));
        }
    };
    if !expected.matches_value(value) {
        return Err(KError::new(KErrorKind::TypeMismatch {
            arg: "value".to_string(),
            expected: expected.name().to_string(),
            got: value.ktype().name().to_string(),
        }));
    }
    Ok(KObject::Tagged {
        tag,
        value: Rc::new(value.deep_clone()),
        scope_id: schema_scope_id,
        name: schema_name.to_string(),
        // Type args are stamped by ascription at an annotated boundary, not here.
        type_args: Rc::new(vec![]),
    })
}

/// Body of the construction-primitive builtin. Pulls the schema, tag, and value out
/// of the bundle, calls [`construct`], and arena-allocates the result.
fn primitive_body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // Carry `(schema, name, scope_id)` so the produced `Tagged` value points back at
    // the declaring schema's identity.
    let (schema, schema_name, schema_scope_id) = match bundle.get("schema") {
        Some(KObject::TaggedUnionType { schema, name, scope_id }) => {
            (Rc::clone(schema), name.clone(), *scope_id)
        }
        Some(other) => {
            return BodyResult::Err(KError::new(KErrorKind::TypeMismatch {
                arg: "schema".to_string(),
                expected: "TaggedUnionType".to_string(),
                got: other.ktype().name().to_string(),
            }));
        }
        None => {
            return BodyResult::Err(KError::new(KErrorKind::MissingArg("schema".to_string())));
        }
    };
    // `KType::Type` also accepts `KObject::StructType`; the match above forces
    // TaggedUnionType so a struct routed through this 3-slot path errors cleanly.
    let tag = match bundle.get("tag") {
        Some(KObject::KString(s)) => s.clone(),
        Some(other) => {
            return BodyResult::Err(KError::new(KErrorKind::TypeMismatch {
                arg: "tag".to_string(),
                expected: "Identifier".to_string(),
                got: other.ktype().name().to_string(),
            }));
        }
        None => return BodyResult::Err(KError::new(KErrorKind::MissingArg("tag".to_string()))),
    };
    let value = match bundle.get("value") {
        Some(v) => v,
        None => {
            return BodyResult::Err(KError::new(KErrorKind::MissingArg("value".to_string())));
        }
    };
    match construct(&schema, &schema_name, schema_scope_id, tag, value) {
        Ok(tagged) => BodyResult::Value(scope.arena.alloc(tagged)),
        Err(e) => BodyResult::Err(e),
    }
}

/// Register the construction primitive. No keyword; the `[Type, Identifier, Any]`
/// slot triple claims its bucket via the specificity tiebreak.
pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "tagged_union_construct",
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::AnyUserType { kind: UserTypeKind::Tagged }),
            elements: vec![
                SignatureElement::Argument(Argument { name: "schema".into(), ktype: KType::Type }),
                SignatureElement::Argument(Argument { name: "tag".into(),    ktype: KType::Identifier }),
                SignatureElement::Argument(Argument { name: "value".into(),  ktype: KType::Any }),
            ],
        },
        primitive_body,
    );
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    use crate::machine::model::ast::KExpression;
    use crate::parse::parse;
    use crate::builtins::default_scope;
    use crate::machine::core::{KErrorKind, RuntimeArena, Scope};
    use crate::machine::execute::Scheduler;
    use crate::machine::model::values::KObject;

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

    fn parse_one<'a>(src: &str) -> KExpression<'a> {
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
    ) -> crate::machine::core::KError {
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().expect("scheduler should not surface errors directly");
        match sched.read_result(id) {
            Ok(_) => panic!("expected error"),
            Err(e) => e.clone(),
        }
    }

    /// Exercises the primitive directly via the parens-wrapping that resolves
    /// `maybe` to a Future before the slot-0 type bind.
    #[test]
    fn primitive_constructs_tagged_value() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "UNION Maybe = (some :Number none :Null)\nLET maybe = Maybe");
        let result = run_one(scope, parse_one("(maybe) some 42"));
        match result {
            KObject::Tagged { tag, value, .. } => {
                assert_eq!(tag, "some");
                assert!(matches!(&**value, KObject::Number(n) if *n == 42.0));
            }
            other => panic!("expected Tagged, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn primitive_rejects_unknown_tag() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "UNION Maybe = (some :Number none :Null)\nLET maybe = Maybe");
        let err = run_one_err(scope, parse_one("(maybe) other 42"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`other`")),
            "expected ShapeError mentioning `other`, got {err}",
        );
    }

    #[test]
    fn primitive_rejects_value_of_wrong_type() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "UNION Maybe = (some :Number none :Null)\nLET maybe = Maybe");
        let err = run_one_err(scope, parse_one("(maybe) some \"oops\""));
        match &err.kind {
            KErrorKind::TypeMismatch { arg, expected, got } => {
                assert_eq!(arg, "value");
                assert_eq!(expected, "Number");
                assert_eq!(got, "Str");
            }
            _ => panic!("expected TypeMismatch on value, got {err}"),
        }
    }

    /// `ConstructorCall` fast lane (leaf-Type head) propagates the schema's tag check —
    /// companion to `primitive_rejects_unknown_tag`'s `(maybe) other 42` shape.
    #[test]
    fn ctor_fast_lane_propagates_tag_validation_error() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "UNION Maybe = (some :Number none :Null)");
        let err = run_one_err(scope, parse_one("Maybe (other 42)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`other`")),
            "expected ShapeError mentioning `other`, got {err}",
        );
    }

    /// Value-cell sub-expression `(x)` rides the `BareIdentifier` fast lane to resolve
    /// `x` before the synthesized TAG call sees the typed-slot bind.
    #[test]
    fn ctor_fast_lane_with_sub_expression_value() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "UNION Maybe = (some :Number none :Null)\nLET x = 7");
        let result = run_one(scope, parse_one("Maybe (some (x))"));
        match result {
            KObject::Tagged { tag, value, .. } => {
                assert_eq!(tag, "some");
                assert!(matches!(&**value, KObject::Number(n) if *n == 7.0));
            }
            other => panic!("expected Tagged, got {:?}", other.ktype()),
        }
    }
}
