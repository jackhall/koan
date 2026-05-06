use std::rc::Rc;

use crate::dispatch::runtime::{KError, KErrorKind};
use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, NoopResolver, SignatureElement};
use crate::dispatch::values::KObject;
use crate::dispatch::runtime::Scope;
use crate::dispatch::types::parse_typed_field_list;
use crate::parse::kexpression::{KExpression, TypeParams};

use super::{err, register_builtin};

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
    let name = match bundle.get("name") {
        Some(KObject::TypeExprValue(t)) => match &t.params {
            TypeParams::None => t.name.clone(),
            _ => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "STRUCT name must be a bare type name, got `{}`",
                    t.render(),
                ))));
            }
        },
        Some(other) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "name".to_string(),
                expected: "TypeExprRef".to_string(),
                got: other.ktype().name().to_string(),
            }));
        }
        None => return err(KError::new(KErrorKind::MissingArg("name".to_string()))),
    };
    let schema_expr = match extract_kexpression(&mut bundle, "schema") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "STRUCT schema slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    let fields = match parse_typed_field_list(&schema_expr, "STRUCT", &NoopResolver) {
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
    scope.add(name, struct_obj);
    BodyResult::Value(struct_obj)
}

/// Pull a `KExpression`-typed argument from the bundle. Mirrors the `Rc::try_unwrap` dance
/// used by [`union`](super::union) and [`fn_def`](super::fn_def).
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
    use crate::dispatch::types::KType;
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
    fn struct_named_registers_type_in_scope() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
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
        let data = scope.data.borrow();
        let entry = data.get("Point").expect("Point should be bound in scope");
        assert!(matches!(entry, KObject::StructType { .. }));
    }

    #[test]
    fn struct_returns_type_value() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let result = run_one(scope, parse_one("STRUCT Point = (x: Number, y: Number)"));
        assert_eq!(result.ktype(), KType::Type);
    }

    #[test]
    fn struct_preserves_field_order() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run_one(scope, parse_one("STRUCT Backwards = (b: Number, a: Number)"));
        let data = scope.data.borrow();
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
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let err = run_one_err(scope, parse_one("STRUCT Bad = (a: Bogus)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("Bogus")),
            "expected ShapeError mentioning Bogus, got {err}",
        );
    }

    #[test]
    fn struct_rejects_empty_schema() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let err = run_one_err(scope, parse_one("STRUCT Empty = ()"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("at least one field")),
            "expected ShapeError on empty schema, got {err}",
        );
    }

    #[test]
    fn struct_rejects_duplicate_field() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let err = run_one_err(scope, parse_one("STRUCT Pair = (x: Number, x: Str)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("duplicate") && msg.contains("`x`")),
            "expected ShapeError on duplicate field, got {err}",
        );
    }

    #[test]
    fn struct_rejects_missing_colon() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let err = run_one_err(scope, parse_one("STRUCT Pair = (x Number, y: Number)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`:`") || msg.contains("triple")),
            "expected ShapeError on missing colon, got {err}",
        );
    }
}
