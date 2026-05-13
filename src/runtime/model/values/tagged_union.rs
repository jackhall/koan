//! Tagged-union construction primitives, mirroring [`KFunction::apply`](super::kfunction)
//! in shape: the synthesis that turns a "call site" into a tail expression lives next to
//! the type, not inside the dispatch builtins that consume it.
//!
//! `apply` is the entry point both surface forms (type-token call via
//! [`type_call`](super::builtins::type_call) and identifier-bound type call via
//! [`call_by_name`](super::builtins::call_by_name)) call. It synthesizes a tail expression
//! that re-dispatches through the construction-primitive builtin defined here, whose
//! typed slots let the scheduler resolve sub-expression value-parts before construction
//! runs.
//!
//! The primitive builtin has no keyword in its signature — three typed slots
//! (`Type`, `Identifier`, `Any`) are specific enough to claim its dispatch bucket
//! unambiguously, and no user surface form spells the call directly. The user constructs
//! via the type token (`Maybe (some 42)`) or a LET-bound identifier; both routes funnel
//! through `apply`. The slot-0 `Type` is shared with the struct construction primitive
//! (`src/runtime/model/values/struct_value.rs`); they don't collide because struct construct is
//! 2-slot, not 3-slot — different dispatch bucket.

use std::collections::HashMap;
use std::rc::Rc;

use crate::runtime::builtins::register_builtin;
use crate::runtime::machine::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::runtime::machine::core::{KError, KErrorKind, Scope};
use crate::runtime::model::types::{Argument, ExpressionSignature, KType, SignatureElement};
use crate::runtime::model::values::KObject;
use crate::ast::{ExpressionPart, KExpression};

/// Mirror of [`KFunction::apply`](super::kfunction::KFunction::apply): take the args parts
/// captured at the call site and produce a `BodyResult::Tail` re-dispatching through the
/// construction primitive. `schema_obj` is the looked-up `&'a KObject<'a>` reference (must
/// be `KObject::TaggedUnionType(_)` — caller's responsibility).
///
/// Validates the args shape: exactly two parts, with the first an `Identifier` (the tag
/// name). The second part rides through unchanged so the scheduler resolves sub-expressions
/// (`(foo)`, list literals, etc.) before construction sees the value.
pub fn apply<'a>(
    schema_obj: &'a KObject<'a>,
    args_parts: Vec<ExpressionPart<'a>>,
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
    if !matches!(tag_part, ExpressionPart::Identifier(_)) {
        return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
            "tagged-union construction: first arg must be a bare-identifier tag, got {}",
            tag_part.summarize()
        ))));
    }
    let parts = vec![
        ExpressionPart::Future(schema_obj),
        tag_part,
        value_part,
    ];
    BodyResult::tail(KExpression { parts })
}

/// Validate `tag` against `schema` and `value` against the schema's expected type for that
/// tag, then build the `KObject::Tagged`. Pure logic — no scope, no scheduler. The
/// construction-primitive builtin's body is a thin shim around this.
pub fn construct<'a>(
    schema: &HashMap<String, KType>,
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
    })
}

/// Body of the construction-primitive builtin. Pulls the schema, tag, and value out of the
/// bundle, calls [`construct`], and arena-allocates the result. Registered with no keyword
/// in the signature — the typed-slot specificity is what claims its bucket.
fn primitive_body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let schema = match bundle.get("schema") {
        Some(KObject::TaggedUnionType(s)) => Rc::clone(s),
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
    // The `KType::Type` slot also accepts `KObject::StructType`; if a caller routed a
    // struct schema into this 3-slot path (e.g. via a hand-built dispatch), the
    // `KObject::TaggedUnionType` match above catches that — anything else falls into the
    // TypeMismatch arm.
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
    match construct(&schema, tag, value) {
        Ok(tagged) => BodyResult::Value(scope.arena.alloc_object(tagged)),
        Err(e) => BodyResult::Err(e),
    }
}

/// Register the construction primitive. No keyword in the signature — `Type` in slot 0
/// plus the 3-slot bucket `[Slot, Slot, Slot]` won't collide with other 3-arg signatures
/// via the specificity tiebreak. Called from [`default_scope`](super::builtins::default_scope).
pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "tagged_union_construct",
        ExpressionSignature {
            return_type: KType::Tagged,
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

    use crate::runtime::builtins::default_scope;
    use crate::runtime::machine::core::{KErrorKind, RuntimeArena, Scope};
    use crate::runtime::model::values::KObject;
    use crate::runtime::machine::execute::Scheduler;
    use crate::ast::KExpression;
    use crate::parse::parse;

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
    ) -> crate::runtime::machine::core::KError {
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().expect("scheduler should not surface errors directly");
        match sched.read_result(id) {
            Ok(_) => panic!("expected error"),
            Err(e) => e.clone(),
        }
    }

    /// The construction primitive can be reached by the only shape that produces it (a
    /// resolved `TaggedUnionType` in slot 0 plus a tag identifier and a value). Surface
    /// users go through `Maybe (some 42)`; this test exercises the primitive directly via
    /// the parens-wrapping that resolves `maybe` to a Future.
    #[test]
    fn primitive_constructs_tagged_value() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "LET maybe = (UNION (some: Number none: Null))");
        let result = run_one(scope, parse_one("(maybe) some 42"));
        match result {
            KObject::Tagged { tag, value } => {
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
        run(scope, "LET maybe = (UNION (some: Number none: Null))");
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
        run(scope, "LET maybe = (UNION (some: Number none: Null))");
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
}
