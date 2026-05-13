use std::rc::Rc;

use crate::runtime::model::{Argument, ExpressionSignature, KObject, KType, SignatureElement};
use crate::runtime::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};
use crate::runtime::model::types::{parse_typed_field_list, ScopeResolver};

use crate::ast::KExpression;

use crate::runtime::machine::kfunction::argument_bundle::{extract_bare_type_name, extract_kexpression};
use super::{err, register_builtin_with_pre_run};

/// `STRUCT <name:TypeExprRef> = (<schema>)` — declare a named record type.
///
/// The schema slot is `KType::KExpression`: the user writes a parens-wrapped expression of
/// repeated `<field:Identifier> : <type:Type>` triples (`STRUCT Point = (x: Number, y: Number)`).
/// Same triple shape as `UNION` — both delegate to [`parse_typed_field_list`] so the parsing
/// logic and error messages stay consistent.
///
/// Unlike `UNION`, struct schemas preserve declaration order so [`struct_value::apply`]
/// (super::struct_value::apply) can reorder the user's named-arg pairs (`Point (x: 3, y: 4)`
/// or `Point (y: 4, x: 3)`) into a stable canonical order before the construction primitive
/// runs. The registered schema is therefore an ordered `Vec<(String, KType)>` rather than a
/// `HashMap`.
///
/// Empty schemas, unknown type names, duplicate field names, and malformed triples all
/// surface as `ShapeError` with the offending position called out. The named form
/// registers the type token (`Point`) in the current scope so it can be used as a
/// constructor downstream via the type-call dispatch path.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // `TypeExprRef`-typed slot resolves to `KObject::TypeExprValue(t)`. The name slot wants
    // a bare leaf — reject parameterized forms like `Point<X>` at definition time.
    let name = match extract_bare_type_name(&bundle, "name", "STRUCT") {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    let schema_expr = match extract_kexpression(&mut bundle, "schema") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "STRUCT schema slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    let resolver = ScopeResolver::new(scope);
    let fields = match parse_typed_field_list(&schema_expr, "STRUCT schema", &resolver) {
        Ok(f) => f,
        Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
    };
    if fields.is_empty() {
        return err(KError::new(KErrorKind::ShapeError(
            "STRUCT schema must have at least one field".to_string(),
        )));
    }
    let arena = scope.arena;
    let struct_obj: &'a KObject<'a> = arena.alloc_object(KObject::StructType {
        name: name.clone(),
        fields: Rc::new(fields),
    });
    if let Err(e) = scope.bind_value(name, struct_obj) {
        return err(e);
    }
    BodyResult::Value(struct_obj)
}

/// Dispatch-time placeholder extractor for STRUCT. The name slot at `parts[1]` is a
/// `Type(t)` token (the `TypeExprRef`-typed `name` argument). Only fires for bare leaves —
/// parameterized forms (`STRUCT Foo<X> = ...`) aren't supported until functors land.
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    expr.binder_name_from_type_part()
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin_with_pre_run(
        scope,
        "STRUCT",
        ExpressionSignature {
            return_type: KType::Type,
            elements: vec![
                SignatureElement::Keyword("STRUCT".into()),
                SignatureElement::Argument(Argument { name: "name".into(),   ktype: KType::TypeExprRef }),
                SignatureElement::Keyword("=".into()),
                SignatureElement::Argument(Argument { name: "schema".into(), ktype: KType::KExpression }),
            ],
        },
        body,
        Some(pre_run),
    );
}

#[cfg(test)]
mod tests {
    use crate::runtime::builtins::test_support::{parse_one, run_one, run_one_err, run_root_silent};
    use crate::runtime::model::{KObject, KType};
    use crate::runtime::machine::{KErrorKind, RuntimeArena};

    /// Smoke test for STRUCT's pre_run extractor: structural extraction of the `Type(_)`
    /// token at `parts[1]`.
    #[test]
    fn pre_run_extracts_struct_name() {
        let expr = parse_one("STRUCT Point = (x: Number, y: Number)");
        let name = super::pre_run(&expr);
        assert_eq!(name.as_deref(), Some("Point"));
    }

    #[test]
    fn struct_named_registers_type_in_scope() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(
            scope,
            parse_one("STRUCT Point = (x: Number, y: Number)"),
        );
        match result {
            KObject::StructType { name, fields } => {
                assert_eq!(name, "Point");
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0], ("x".to_string(), KType::Number));
                assert_eq!(fields[1], ("y".to_string(), KType::Number));
            }
            other => panic!("expected StructType, got {:?}", other.ktype()),
        }
        let data = scope.bindings().data();
        let entry = data.get("Point").expect("Point should be bound in scope");
        assert!(matches!(entry, KObject::StructType { .. }));
    }

    #[test]
    fn struct_returns_type_value() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("STRUCT Point = (x: Number, y: Number)"));
        assert_eq!(result.ktype(), KType::Type);
    }

    #[test]
    fn struct_preserves_field_order() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run_one(scope, parse_one("STRUCT Backwards = (b: Number, a: Number)"));
        let data = scope.bindings().data();
        match data.get("Backwards").unwrap() {
            KObject::StructType { fields, .. } => {
                assert_eq!(fields[0].0, "b", "first field should be `b` (declaration order)");
                assert_eq!(fields[1].0, "a");
            }
            _ => panic!("expected StructType"),
        }
    }

    #[test]
    fn struct_rejects_unknown_type_name() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("STRUCT Bad = (a: Bogus)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("Bogus")),
            "expected ShapeError mentioning Bogus, got {err}",
        );
    }

    #[test]
    fn struct_rejects_empty_schema() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("STRUCT Empty = ()"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("at least one field")),
            "expected ShapeError on empty schema, got {err}",
        );
    }

    #[test]
    fn struct_rejects_duplicate_field() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("STRUCT Pair = (x: Number, x: Str)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("duplicate") && msg.contains("`x`")),
            "expected ShapeError on duplicate field, got {err}",
        );
    }

    #[test]
    fn struct_rejects_missing_colon() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("STRUCT Pair = (x Number, y: Number)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`:`") || msg.contains("triple")),
            "expected ShapeError on missing colon, got {err}",
        );
    }
}
