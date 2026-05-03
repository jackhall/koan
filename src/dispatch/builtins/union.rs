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

use super::{err, register_builtin};

/// `UNION <name:TypeRef> = (<schema>)` (named) or `UNION (<schema>)` (anonymous).
///
/// The schema slot is `KType::KExpression` — the user writes a parens-wrapped expression
/// of repeated `<tag:Identifier> : <type:Type>` triples
/// (`UNION Maybe = (some: Number none: Null)`). The parens prevent the parts from being
/// dispatched as their own expression, so identifier tag names ride through as
/// `Identifier` parts and type tokens as `Type` parts. Same type-annotation shape that
/// function-signature parameter declarations will use later.
///
/// Type names must resolve via `KType::from_name`. Empty schemas are rejected with
/// `ShapeError`; malformed shapes (parts not in groups of 3, missing `:`, non-Type RHS,
/// etc.) all surface as `ShapeError` with the offending position called out.
///
/// The named form additionally registers the type in the current scope so the type token
/// (`Maybe`) can be used as a constructor downstream. Both forms return a
/// `KObject::TaggedUnionType` carrying the parsed schema.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let schema_expr = match extract_kexpression(&mut bundle, "schema") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "UNION schema slot must be a parenthesized dict literal".to_string(),
            )));
        }
    };
    let schema = match extract_arrow_triples(&schema_expr) {
        Ok(s) => s,
        Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
    };
    if schema.is_empty() {
        return err(KError::new(KErrorKind::ShapeError(
            "UNION schema must have at least one tag".to_string(),
        )));
    }
    let arena = scope.arena;
    let union_obj: &'a KObject<'a> =
        arena.alloc_object(KObject::TaggedUnionType(Rc::new(schema)));
    if let Some(name_obj) = bundle.get("name") {
        let name = match name_obj {
            KObject::KString(s) => s.clone(),
            other => {
                return err(KError::new(KErrorKind::TypeMismatch {
                    arg: "name".to_string(),
                    expected: "TypeRef".to_string(),
                    got: other.ktype().name().to_string(),
                }));
            }
        };
        scope.add(name, union_obj);
    }
    BodyResult::Value(union_obj)
}

/// Extract a `KExpression`-typed argument from the bundle. Mirrors the `Rc::try_unwrap`
/// dance used by [`if_then`](super::if_then) and [`fn_def`](super::fn_def).
fn extract_kexpression<'a>(
    bundle: &mut ArgumentBundle<'a>,
    name: &str,
) -> Option<KExpression<'a>> {
    let rc = bundle.args.remove(name)?;
    match std::rc::Rc::try_unwrap(rc) {
        Ok(KObject::KExpression(e)) => Some(e),
        Ok(_) => None,
        Err(rc) => match &*rc {
            KObject::KExpression(e) => Some(e.clone()),
            _ => None,
        },
    }
}

/// Walk the schema KExpression's parts as repeated `<Identifier(tag)> <Keyword(":")>
/// <Type(name)>` triples and assemble the resulting `tag -> KType` map. Errors with a
/// `ShapeError`-string on any malformed triple, unknown type name, or duplicate tag.
fn extract_arrow_triples<'a>(
    expr: &KExpression<'a>,
) -> Result<HashMap<String, KType>, String> {
    let parts = &expr.parts;
    if parts.len() % 3 != 0 {
        return Err(format!(
            "UNION schema must be `<tag>: <Type>` triples; got {} parts (not a multiple of 3)",
            parts.len()
        ));
    }
    let mut schema: HashMap<String, KType> = HashMap::with_capacity(parts.len() / 3);
    let mut i = 0;
    while i < parts.len() {
        let tag = match &parts[i] {
            ExpressionPart::Identifier(s) => s.clone(),
            other => {
                return Err(format!(
                    "UNION schema tag must be a bare identifier, got {}",
                    other.summarize()
                ));
            }
        };
        match &parts[i + 1] {
            ExpressionPart::Keyword(k) if k == ":" => {}
            other => {
                return Err(format!(
                    "UNION schema separator must be `:`, got {}",
                    other.summarize()
                ));
            }
        }
        let type_name = match &parts[i + 2] {
            ExpressionPart::Type(s) => s.clone(),
            other => {
                return Err(format!(
                    "UNION schema type for tag `{}` must be a type name token, got {}",
                    tag,
                    other.summarize()
                ));
            }
        };
        let ktype = KType::from_name(&type_name).ok_or_else(|| {
            format!(
                "unknown type name `{}` in UNION schema for tag `{}`",
                type_name, tag
            )
        })?;
        if schema.insert(tag.clone(), ktype).is_some() {
            return Err(format!("duplicate tag `{}` in UNION schema", tag));
        }
        i += 3;
    }
    Ok(schema)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Named form: `UNION Maybe = (some: Number none: Null)`
    register_builtin(
        scope,
        "UNION",
        ExpressionSignature {
            return_type: KType::TaggedUnionType,
            elements: vec![
                SignatureElement::Keyword("UNION".into()),
                SignatureElement::Argument(Argument { name: "name".into(),   ktype: KType::TypeRef }),
                SignatureElement::Keyword("=".into()),
                SignatureElement::Argument(Argument { name: "schema".into(), ktype: KType::KExpression }),
            ],
        },
        body,
    );
    // Anonymous form: `LET maybe = (UNION (some: Number none: Null))`
    register_builtin(
        scope,
        "UNION",
        ExpressionSignature {
            return_type: KType::TaggedUnionType,
            elements: vec![
                SignatureElement::Keyword("UNION".into()),
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

    use crate::dispatch::arena::RuntimeArena;
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::kerror::KErrorKind;
    use crate::dispatch::kfunction::KType;
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
    fn union_named_registers_type_in_scope() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let result = run_one(
            scope,
            parse_one("UNION Maybe = (some: Number none: Null)"),
        );
        assert!(matches!(result, KObject::TaggedUnionType(_)));
        let data = scope.data.borrow();
        let entry = data.get("Maybe").expect("Maybe should be bound in scope");
        match entry {
            KObject::TaggedUnionType(schema) => {
                assert_eq!(schema.get("some"), Some(&KType::Number));
                assert_eq!(schema.get("none"), Some(&KType::Null));
            }
            other => panic!("expected TaggedUnionType, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn union_anonymous_returns_type_value() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let result = run_one(scope, parse_one("UNION (ok: Number err: Str)"));
        match result {
            KObject::TaggedUnionType(schema) => {
                assert_eq!(schema.get("ok"), Some(&KType::Number));
                assert_eq!(schema.get("err"), Some(&KType::Str));
            }
            other => panic!("expected TaggedUnionType, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn union_rejects_unknown_type_name() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let err = run_one_err(scope, parse_one("UNION (some: Bogus)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("Bogus")),
            "expected ShapeError mentioning Bogus, got {err}",
        );
    }

    #[test]
    fn union_rejects_empty_schema() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let err = run_one_err(scope, parse_one("UNION ()"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("at least one tag")),
            "expected ShapeError on empty schema, got {err}",
        );
    }

    #[test]
    fn union_rejects_duplicate_tag() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let err = run_one_err(scope, parse_one("UNION (some: Number some: Str)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("duplicate") && msg.contains("`some`")),
            "expected ShapeError on duplicate tag, got {err}",
        );
    }

    #[test]
    fn union_rejects_missing_colon() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let err = run_one_err(scope, parse_one("UNION (some Number none: Null)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`:`") || msg.contains("triple")),
            "expected ShapeError on missing colon, got {err}",
        );
    }
}
