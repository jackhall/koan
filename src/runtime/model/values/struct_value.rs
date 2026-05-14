//! Struct construction primitives, paralleling [`tagged_union`](super::tagged_union) for
//! product types. `apply` is the entry point both surface forms (type-token call via
//! [`type_call`](super::builtins::type_call) and identifier-bound type call via
//! [`call_by_name`](super::builtins::call_by_name)) call. It synthesizes a tail expression
//! that re-dispatches through the construction-primitive builtin defined here.
//!
//! Unlike the tagged-union primitive (3 fixed slots: schema/tag/value), struct construction
//! is variable-arity — a `Point` schema declares 2 fields, a `User` schema might declare 5.
//! Construction is **named-only**: the user writes `Point (x: 3, y: 4)` and `apply` parses
//! the inner expression as `<name>: <value>` triples (via
//! [`parse_named_value_pairs`](super::named_pairs::parse_named_value_pairs)), validates
//! against the declared schema, and reorders the values to match schema declaration order.
//! Reordered value-parts are then wrapped in single-part sub-expressions inside a
//! `ListLiteral`. The scheduler aggregates the list, dispatching each wrapped sub-expression
//! through `value_lookup`/`value_pass` so identifiers and literals both resolve to their
//! values before the primitive sees the assembled `KObject::List`. The primitive then
//! validates per-field types against the schema and emits a `KObject::Struct`.

use std::rc::Rc;

use indexmap::IndexMap;

use crate::runtime::builtins::register_builtin;
use crate::runtime::machine::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::runtime::machine::core::{KError, KErrorKind, Scope};
use crate::runtime::model::types::{Argument, ExpressionSignature, KType, SignatureElement};
use crate::runtime::model::values::{KObject, parse_named_value_pairs};
use crate::ast::{ExpressionPart, KExpression};

/// Parse the inner expression of a `Point (x: 3, y: 4)` form as named pairs, validate the
/// names match the schema, reorder the values into schema declaration order, and synthesize
/// a tail that re-dispatches through the construction primitive.
///
/// Validation precedence (when both fire, the first wins): missing field → unknown field →
/// arity. Missing-first because telling the user "you forgot `y`" is more actionable than
/// "you have a stray `z`" — adding the missing field is what they need either way.
///
/// After reordering, each value-part is wrapped in a single-part sub-expression so bare
/// identifiers route through `value_lookup` and bare literals through `value_pass` —
/// uniform handling regardless of surface form. The wrapped parts are bundled into an
/// `ExpressionPart::ListLiteral`, which the scheduler aggregates into a `KObject::List`
/// before the construction primitive runs.
pub fn apply<'a>(
    schema_obj: &'a KObject<'a>,
    args_parts: Vec<ExpressionPart<'a>>,
) -> BodyResult<'a> {
    let fields = match schema_obj.as_struct_type() {
        Some((_, fields)) => Rc::clone(fields),
        None => {
            debug_assert!(false, "struct_value::apply called on non-StructType");
            return BodyResult::Err(KError::new(KErrorKind::ShapeError(
                "struct_value::apply called on non-StructType".to_string(),
            )));
        }
    };
    let tmp_expr = KExpression { parts: args_parts };
    let pairs = match parse_named_value_pairs(&tmp_expr, "struct construction") {
        Ok(p) => p,
        Err(msg) => return BodyResult::Err(KError::new(KErrorKind::ShapeError(msg))),
    };
    // Missing-first error precedence: any missing field shadows arity / unknown checks.
    for (field_name, _) in fields.iter() {
        if !pairs.iter().any(|(n, _)| n == field_name) {
            return BodyResult::Err(KError::new(KErrorKind::MissingArg(field_name.clone())));
        }
    }
    for (pair_name, _) in pairs.iter() {
        if !fields.iter().any(|(n, _)| n == pair_name) {
            return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                "unknown field `{}` in struct construction",
                pair_name
            ))));
        }
    }
    if pairs.len() != fields.len() {
        return BodyResult::Err(KError::new(KErrorKind::ArityMismatch {
            expected: fields.len(),
            got: pairs.len(),
        }));
    }
    let mut wrapped: Vec<ExpressionPart<'a>> = Vec::with_capacity(fields.len());
    for (field_name, _) in fields.iter() {
        let value_part = pairs
            .iter()
            .find(|(n, _)| n == field_name)
            .map(|(_, v)| v.clone())
            .expect("missing-field check above guarantees presence");
        wrapped.push(ExpressionPart::expression(vec![value_part]));
    }
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
    scope_id: usize,
    fields: &[(String, KType)],
    values: &[KObject<'a>],
) -> Result<KObject<'a>, KError> {
    if values.len() != fields.len() {
        return Err(KError::new(KErrorKind::ArityMismatch {
            expected: fields.len(),
            got: values.len(),
        }));
    }
    // Insert in declaration order so iteration (via PRINT / summarize) matches the schema.
    // IndexMap preserves insertion order while keeping O(1) keyed lookup.
    let mut map: IndexMap<String, KObject<'a>> = IndexMap::with_capacity(fields.len());
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
    // Stage 3.0c: copy `(scope_id, name)` off the schema's `StructType` so the value
    // carries the declaring schema's identity. Stage 3.1 reads these in `ktype()`.
    Ok(KObject::Struct {
        name: type_name.to_string(),
        scope_id,
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
    // Pull `(scope_id, name)` off the schema so the produced `Struct` value carries
    // the declaring schema's identity — stage 3.0c made this load-bearing for 3.1's
    // `ktype()` flip.
    let (type_name, scope_id, fields) = match bundle.get("schema") {
        Some(KObject::StructType { name, scope_id, fields }) => {
            (name.clone(), *scope_id, Rc::clone(fields))
        }
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
    match construct(&type_name, scope_id, &fields, &values) {
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
                SignatureElement::Argument(Argument { name: "values".into(), ktype: KType::List(Box::new(KType::Any)) }),
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

    #[test]
    fn struct_construction_via_type_token() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Point = (x: Number, y: Number)");
        let result = run_one(scope, parse_one("Point (x: 3, y: 4)"));
        match result {
            KObject::Struct { name: type_name, fields, .. } => {
                assert_eq!(type_name, "Point");
                assert_eq!(fields.len(), 2);
                assert!(matches!(fields.get("x"), Some(KObject::Number(n)) if *n == 3.0));
                assert!(matches!(fields.get("y"), Some(KObject::Number(n)) if *n == 4.0));
            }
            other => panic!("expected Struct, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn struct_construction_missing_field_errors() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Point = (x: Number, y: Number)");
        let err = run_one_err(scope, parse_one("Point (x: 3)"));
        assert!(
            matches!(&err.kind, KErrorKind::MissingArg(name) if name == "y"),
            "expected MissingArg(\"y\"), got {err}",
        );
    }

    #[test]
    fn struct_construction_unknown_field_errors() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Point = (x: Number, y: Number)");
        let err = run_one_err(scope, parse_one("Point (x: 3, y: 4, z: 5)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("unknown field") && msg.contains("`z`")),
            "expected ShapeError on unknown field z, got {err}",
        );
    }

    #[test]
    fn struct_construction_value_type_mismatch() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Point = (x: Number, y: Number)");
        let err = run_one_err(scope, parse_one("Point (x: 3, y: \"oops\")"));
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
        // Bare identifiers on the value side resolve through value_lookup because `apply`
        // wraps each value-part in a single-part sub-expression after reordering.
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Point = (x: Number, y: Number)\nLET ax = 7\nLET ay = 9");
        let result = run_one(scope, parse_one("Point (x: ax, y: ay)"));
        match result {
            KObject::Struct { fields, .. } => {
                assert!(matches!(fields.get("x"), Some(KObject::Number(n)) if *n == 7.0));
                assert!(matches!(fields.get("y"), Some(KObject::Number(n)) if *n == 9.0));
            }
            other => panic!("expected Struct, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn struct_construction_order_independent() {
        // The user can write fields in any order; `apply` reorders to schema declaration order
        // before construction. Result is identical regardless of source order.
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Point = (x: Number, y: Number)");
        let result = run_one(scope, parse_one("Point (y: 4, x: 3)"));
        match result {
            KObject::Struct { fields, .. } => {
                assert!(matches!(fields.get("x"), Some(KObject::Number(n)) if *n == 3.0));
                assert!(matches!(fields.get("y"), Some(KObject::Number(n)) if *n == 4.0));
            }
            other => panic!("expected Struct, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn struct_construction_missing_colon_errors() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Point = (x: Number, y: Number)");
        let err = run_one_err(scope, parse_one("Point (x 3, y 4)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`:`") || msg.contains("separator") || msg.contains("triples")),
            "expected ShapeError on missing colon, got {err}",
        );
    }

    #[test]
    fn struct_construction_duplicate_name_errors() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Point = (x: Number, y: Number)");
        let err = run_one_err(scope, parse_one("Point (x: 1, x: 2)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("duplicate") && msg.contains("`x`")),
            "expected ShapeError on duplicate name, got {err}",
        );
    }

    #[test]
    fn struct_construction_unbound_type_token_errors() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let err = run_one_err(scope, parse_one("Bogus (x: 1, y: 2)"));
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(name) if name == "Bogus"),
            "expected UnboundName(\"Bogus\"), got {err}",
        );
    }

    /// Regression: struct values iterate (and therefore PRINT/`summarize` render) in
    /// declaration order. Pre-Phase-1 this used a `HashMap`, so the surface output sat at
    /// hash-iteration order — which differed from the schema and surprised users. The order
    /// `z, a, m` is chosen to differ from any alphabetical / hash-stable ordering on a small
    /// set of single-letter keys.
    #[test]
    fn struct_value_iterates_in_declaration_order() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Triple = (z: Number, a: Number, m: Number)");
        let result = run_one(scope, parse_one("Triple (a: 2, m: 3, z: 1)"));
        match result {
            KObject::Struct { fields, .. } => {
                let keys: Vec<&str> = fields.keys().map(|s| s.as_str()).collect();
                assert_eq!(
                    keys,
                    vec!["z", "a", "m"],
                    "struct fields should iterate in declaration order, not call-site order",
                );
            }
            other => panic!("expected Struct, got {:?}", other.ktype()),
        }
        let summary = crate::runtime::model::types::Parseable::summarize(result);
        assert_eq!(
            summary, "Triple(z: 1, a: 2, m: 3)",
            "summary must emit fields in declaration order"
        );
    }

    #[test]
    fn struct_value_summarizes_with_type_name_and_fields() {
        // Smoke-tests the `summarize` format so PRINT downstream doesn't surprise users.
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "STRUCT Point = (x: Number, y: Number)");
        let result = run_one(scope, parse_one("Point (x: 3, y: 4)"));
        let summary = crate::runtime::model::types::Parseable::summarize(result);
        assert!(summary.starts_with("Point("), "summary should start with Point(, got {summary}");
        assert!(summary.contains("x: 3"), "summary should include x: 3, got {summary}");
        assert!(summary.contains("y: 4"), "summary should include y: 4, got {summary}");
    }
}
