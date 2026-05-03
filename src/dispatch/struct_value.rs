//! Struct construction primitives, paralleling [`tagged_union`](super::tagged_union) for
//! product types. `apply` is the entry point both surface forms (type-token call via
//! [`type_call`](super::builtins::type_call) and identifier-bound type call via
//! [`call_by_name`](super::builtins::call_by_name)) call. It synthesizes a tail expression
//! that re-dispatches through the construction-primitive builtin defined here.
//!
//! Unlike the tagged-union primitive (3 fixed slots: schema/tag/value), struct construction
//! is variable-arity — a `Point` schema declares 2 fields, a `User` schema might declare 5.
//! To keep the primitive's signature fixed, `apply` wraps each value-part in an
//! `ExpressionPart::Expression` (single-part KExpression) inside a `ListLiteral`. The
//! scheduler aggregates the list, dispatching each wrapped sub-expression through
//! `value_lookup`/`value_pass` so identifiers and literals both resolve to their values
//! before the primitive sees the assembled `KObject::List`. The primitive then validates
//! arity and per-field types against the schema and emits a `KObject::Struct`.

use std::collections::HashMap;
use std::rc::Rc;

use crate::dispatch::kerror::{KError, KErrorKind};
use crate::dispatch::kfunction::{
    Argument, ArgumentBundle, BodyResult, ExpressionSignature, KType, SchedulerHandle,
    SignatureElement,
};
use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::Scope;
use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::builtins::register_builtin;

/// Synthesize a tail that re-dispatches through the struct construction primitive once
/// each value-part has been resolved. Each arg part is wrapped in a single-part
/// sub-expression so bare identifiers route through `value_lookup` (returning the bound
/// value) and bare literals route through `value_pass` — uniform handling regardless of
/// the surface form. The wrapped parts are bundled into an `ExpressionPart::ListLiteral`,
/// which the scheduler aggregates into a `KObject::List` before the primitive runs.
pub fn apply<'a>(
    schema_obj: &'a KObject<'a>,
    args_parts: Vec<ExpressionPart<'a>>,
) -> BodyResult<'a> {
    debug_assert!(
        matches!(schema_obj, KObject::StructType { .. }),
        "struct_value::apply called on non-StructType",
    );
    let wrapped: Vec<ExpressionPart<'a>> = args_parts
        .into_iter()
        .map(|p| ExpressionPart::expression(vec![p]))
        .collect();
    let parts = vec![
        ExpressionPart::Future(schema_obj),
        ExpressionPart::ListLiteral(wrapped),
    ];
    BodyResult::tail(KExpression { parts })
}

/// Validate `values` against `fields` (matching length and per-position types) and build
/// the `KObject::Struct`. Pure logic — no scope, no scheduler. The construction-primitive
/// builtin's body is a thin shim around this.
pub fn construct<'a>(
    type_name: &str,
    fields: &[(String, KType)],
    values: &[KObject<'a>],
) -> Result<KObject<'a>, KError> {
    if values.len() != fields.len() {
        return Err(KError::new(KErrorKind::ArityMismatch {
            expected: fields.len(),
            got: values.len(),
        }));
    }
    let mut map: HashMap<String, KObject<'a>> = HashMap::with_capacity(fields.len());
    for ((field_name, expected), value) in fields.iter().zip(values.iter()) {
        if !expected.matches_value(value) {
            return Err(KError::new(KErrorKind::TypeMismatch {
                arg: field_name.clone(),
                expected: expected.name().to_string(),
                got: value.ktype().name().to_string(),
            }));
        }
        map.insert(field_name.clone(), value.deep_clone());
    }
    Ok(KObject::Struct {
        type_name: type_name.to_string(),
        fields: Rc::new(map),
    })
}

/// Body of the construction-primitive builtin. Pulls the struct schema and the assembled
/// values list out of the bundle, calls [`construct`], and arena-allocates the result.
fn primitive_body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let (type_name, fields) = match bundle.get("schema") {
        Some(KObject::StructType { name, fields }) => (name.clone(), Rc::clone(fields)),
        Some(other) => {
            return BodyResult::Err(KError::new(KErrorKind::TypeMismatch {
                arg: "schema".to_string(),
                expected: "StructType".to_string(),
                got: other.ktype().name().to_string(),
            }));
        }
        None => {
            return BodyResult::Err(KError::new(KErrorKind::MissingArg("schema".to_string())));
        }
    };
    let values = match bundle.get("values") {
        Some(KObject::List(items)) => Rc::clone(items),
        Some(other) => {
            return BodyResult::Err(KError::new(KErrorKind::TypeMismatch {
                arg: "values".to_string(),
                expected: "List".to_string(),
                got: other.ktype().name().to_string(),
            }));
        }
        None => {
            return BodyResult::Err(KError::new(KErrorKind::MissingArg("values".to_string())));
        }
    };
    match construct(&type_name, &fields, &values) {
        Ok(struct_value) => BodyResult::Value(scope.arena.alloc_object(struct_value)),
        Err(e) => BodyResult::Err(e),
    }
}

/// Register the struct construction primitive. No keyword in the signature — slot 0 is
/// `Type` (matches both `StructType` and `TaggedUnionType`, but the union construct
/// primitive is 3-slot so the bucket keys differ) and slot 1 is `List`. The `[Slot, Slot]`
/// bucket is shared with other 2-arg signatures; specificity ranks our `Type+List` slots
/// above more permissive ones.
pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "struct_construct",
        ExpressionSignature {
            return_type: KType::Struct,
            elements: vec![
                SignatureElement::Argument(Argument { name: "schema".into(), ktype: KType::Type }),
                SignatureElement::Argument(Argument { name: "values".into(), ktype: KType::List }),
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

    use crate::dispatch::arena::RuntimeArena;
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::kerror::KErrorKind;
    use crate::dispatch::kobject::KObject;
    use crate::dispatch::scope::Scope;
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
    ) -> crate::dispatch::kerror::KError {
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().expect("scheduler should not surface errors directly");
        match sched.read_result(id) {
            Ok(_) => panic!("expected error"),
            Err(e) => e.clone(),
        }
    }

    #[test]
    fn struct_construction_via_type_token() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Point = (x: Number, y: Number)");
        let result = run_one(scope, parse_one("Point (3 4)"));
        match result {
            KObject::Struct { type_name, fields } => {
                assert_eq!(type_name, "Point");
                assert_eq!(fields.len(), 2);
                assert!(matches!(fields.get("x"), Some(KObject::Number(n)) if *n == 3.0));
                assert!(matches!(fields.get("y"), Some(KObject::Number(n)) if *n == 4.0));
            }
            other => panic!("expected Struct, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn struct_construction_arity_too_few() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Point = (x: Number, y: Number)");
        let err = run_one_err(scope, parse_one("Point (3)"));
        assert!(
            matches!(err.kind, KErrorKind::ArityMismatch { expected: 2, got: 1 }),
            "expected ArityMismatch{{2, 1}}, got {err}",
        );
    }

    #[test]
    fn struct_construction_arity_too_many() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Point = (x: Number, y: Number)");
        let err = run_one_err(scope, parse_one("Point (3 4 5)"));
        assert!(
            matches!(err.kind, KErrorKind::ArityMismatch { expected: 2, got: 3 }),
            "expected ArityMismatch{{2, 3}}, got {err}",
        );
    }

    #[test]
    fn struct_construction_value_type_mismatch() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Point = (x: Number, y: Number)");
        let err = run_one_err(scope, parse_one("Point (3 \"oops\")"));
        match &err.kind {
            KErrorKind::TypeMismatch { arg, expected, got } => {
                assert_eq!(arg, "y");
                assert_eq!(expected, "Number");
                assert_eq!(got, "Str");
            }
            _ => panic!("expected TypeMismatch on field y, got {err}"),
        }
    }

    #[test]
    fn struct_construction_with_identifier_arg() {
        // Bare identifiers in the args list resolve through value_lookup because `apply`
        // wraps each part in a single-part sub-expression. The user does not need to
        // parens-wrap individually.
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Point = (x: Number, y: Number)\nLET ax = 7\nLET ay = 9");
        let result = run_one(scope, parse_one("Point (ax ay)"));
        match result {
            KObject::Struct { fields, .. } => {
                assert!(matches!(fields.get("x"), Some(KObject::Number(n)) if *n == 7.0));
                assert!(matches!(fields.get("y"), Some(KObject::Number(n)) if *n == 9.0));
            }
            other => panic!("expected Struct, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn struct_construction_unbound_type_token_errors() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let err = run_one_err(scope, parse_one("Bogus (1 2)"));
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(name) if name == "Bogus"),
            "expected UnboundName(\"Bogus\"), got {err}",
        );
    }

    #[test]
    fn struct_value_summarizes_with_type_name_and_fields() {
        // Smoke-tests the `summarize` format so PRINT downstream doesn't surprise users.
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Point = (x: Number, y: Number)");
        let result = run_one(scope, parse_one("Point (3 4)"));
        let summary = crate::dispatch::ktraits::Parseable::summarize(result);
        assert!(summary.starts_with("Point("), "summary should start with Point(, got {summary}");
        assert!(summary.contains("x: 3"), "summary should include x: 3, got {summary}");
        assert!(summary.contains("y: 4"), "summary should include y: 4, got {summary}");
    }
}
