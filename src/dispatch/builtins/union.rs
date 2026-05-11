use std::collections::HashMap;
use std::rc::Rc;

use crate::dispatch::{
    Argument, ArgumentBundle, BodyResult, ExpressionSignature, KError, KErrorKind, KObject, KType,
    Scope, SchedulerHandle, SignatureElement,
};
use crate::dispatch::types::{parse_typed_field_list, ScopeResolver};

use crate::parse::kexpression::KExpression;

use crate::dispatch::kfunction::argument_bundle::{extract_bare_type_name, extract_kexpression};
use super::{err, register_builtin_with_pre_run};

/// `UNION <name:TypeExprRef> = (<schema>)` (named) or `UNION (<schema>)` (anonymous).
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
/// `KObject::TaggedUnionType` carrying the parsed schema; that value reports `KType::Type`
/// at runtime, sharing the meta-type with `STRUCT`-produced schemas.
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
    let resolver = ScopeResolver::new(scope);
    let fields = match parse_typed_field_list(&schema_expr, "UNION schema", &resolver) {
        Ok(f) => f,
        Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
    };
    if fields.is_empty() {
        return err(KError::new(KErrorKind::ShapeError(
            "UNION schema must have at least one tag".to_string(),
        )));
    }
    // UNION addresses by tag name and doesn't care about declaration order; flatten the
    // ordered field list (which `parse_typed_field_list` shares with `STRUCT`) into a
    // HashMap. Duplicate detection has already happened in the helper.
    let schema: HashMap<String, KType> = fields.into_iter().collect();
    let arena = scope.arena;
    let union_obj: &'a KObject<'a> =
        arena.alloc_object(KObject::TaggedUnionType(Rc::new(schema)));
    // The named form supplies a `name` slot; the anonymous form omits it. Only validate
    // the slot's shape (and bind into scope) when it's present — `extract_bare_type_name`
    // would otherwise treat absence as `MissingArg`, which is wrong here.
    if bundle.get("name").is_some() {
        match extract_bare_type_name(&bundle, "name", "UNION") {
            Ok(name) => {
                if let Err(e) = scope.bind_value(name, union_obj) {
                    return err(e);
                }
            }
            Err(e) => return err(e),
        }
    }
    BodyResult::Value(union_obj)
}

/// Dispatch-time placeholder extractor for the *named* UNION form (`UNION Foo = (...)`).
/// `parts[1]` is a `Type(t)` token. The anonymous form (`UNION (...)`, registered separately)
/// has no name slot and uses no pre_run.
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    expr.binder_name_from_type_part()
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Named form: `UNION Maybe = (some: Number none: Null)`
    register_builtin_with_pre_run(
        scope,
        "UNION",
        ExpressionSignature {
            return_type: KType::Type,
            elements: vec![
                SignatureElement::Keyword("UNION".into()),
                SignatureElement::Argument(Argument { name: "name".into(),   ktype: KType::TypeExprRef }),
                SignatureElement::Keyword("=".into()),
                SignatureElement::Argument(Argument { name: "schema".into(), ktype: KType::KExpression }),
            ],
        },
        body,
        Some(pre_run),
    );
    // Anonymous form: `LET maybe = (UNION (some: Number none: Null))` — no name slot to
    // pre-install. The wrapping LET (if any) installs its own placeholder via let's pre_run.
    register_builtin_with_pre_run(
        scope,
        "UNION",
        ExpressionSignature {
            return_type: KType::Type,
            elements: vec![
                SignatureElement::Keyword("UNION".into()),
                SignatureElement::Argument(Argument { name: "schema".into(), ktype: KType::KExpression }),
            ],
        },
        body,
        None,
    );
}

#[cfg(test)]
mod tests {
    use crate::dispatch::builtins::test_support::{parse_one, run_one, run_one_err, run_root_silent};
    use crate::dispatch::{KErrorKind, KObject, KType, RuntimeArena};

    /// Smoke test for the named-UNION pre_run extractor: structural extraction of the
    /// `Type(_)` token at `parts[1]` for the named form. The anonymous form has no
    /// pre_run.
    #[test]
    fn pre_run_extracts_named_union_name() {
        let expr = parse_one("UNION Maybe = (some: Number, none: Null)");
        let name = super::pre_run(&expr);
        assert_eq!(name.as_deref(), Some("Maybe"));
    }

    #[test]
    fn union_named_registers_type_in_scope() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let scope = run_root_silent(&arena);
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
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("UNION (some: Bogus)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("Bogus")),
            "expected ShapeError mentioning Bogus, got {err}",
        );
    }

    #[test]
    fn union_rejects_empty_schema() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("UNION ()"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("at least one tag")),
            "expected ShapeError on empty schema, got {err}",
        );
    }

    #[test]
    fn union_rejects_duplicate_tag() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("UNION (some: Number some: Str)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("duplicate") && msg.contains("`some`")),
            "expected ShapeError on duplicate tag, got {err}",
        );
    }

    #[test]
    fn union_rejects_missing_colon() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("UNION (some Number none: Null)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`:`") || msg.contains("triple")),
            "expected ShapeError on missing colon, got {err}",
        );
    }
}
