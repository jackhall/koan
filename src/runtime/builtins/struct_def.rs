use std::rc::Rc;

use crate::runtime::model::{Argument, ExpressionSignature, KObject, KType, SignatureElement};
use crate::runtime::machine::{
    ArgumentBundle, BodyResult, CombineFinish, Frame, KError, KErrorKind, NodeId, Scope,
    SchedulerHandle,
};
use crate::runtime::model::types::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListOutcome,
};

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
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // `TypeExprRef`-typed slot resolves to `KObject::KTypeValue(kt)`. The name slot wants
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
    // Phase-3 elaborator: seeds the threaded set with this STRUCT's binder name so a
    // self-reference (`STRUCT Tree { children: List<Tree> }`) resolves to
    // `KType::RecursiveRef("Tree")` rather than parking on the binder's own placeholder.
    let mut elaborator = Elaborator::new(scope).with_threaded([name.clone()]);
    let outcome = parse_typed_field_list_via_elaborator(
        &schema_expr,
        "STRUCT schema",
        &mut elaborator,
    );
    match outcome {
        FieldListOutcome::Done(fields) => finalize_struct(scope, name, fields),
        FieldListOutcome::Err(msg) => err(KError::new(KErrorKind::ShapeError(msg))),
        FieldListOutcome::Park(producers) => defer_struct_via_combine(
            scope,
            sched,
            name,
            schema_expr,
            producers,
        ),
    }
}

/// Build and bind the `KObject::StructType` once every field type has elaborated.
/// Shared between the synchronous (no-park) path and the Combine-finish path.
fn finalize_struct<'a>(
    scope: &'a Scope<'a>,
    name: String,
    fields: Vec<(String, KType)>,
) -> BodyResult<'a> {
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

/// Schedule a `Combine` over `producers` and re-run the schema elaboration in the finish
/// closure. Same shape MODULE / SIG / FN-def use post-phase-3.
fn defer_struct_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    name: String,
    schema_expr: KExpression<'a>,
    producers: Vec<NodeId>,
) -> BodyResult<'a> {
    let name_for_finish = name.clone();
    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, _results| {
        // Producers terminalized — re-elaborate against the now-final scope.
        let mut elaborator = Elaborator::new(scope).with_threaded([name_for_finish.clone()]);
        match parse_typed_field_list_via_elaborator(
            &schema_expr,
            "STRUCT schema",
            &mut elaborator,
        ) {
            FieldListOutcome::Done(fields) => {
                finalize_struct(scope, name_for_finish.clone(), fields)
            }
            FieldListOutcome::Err(msg) => BodyResult::Err(
                KError::new(KErrorKind::ShapeError(msg)).with_frame(Frame {
                    function: "<struct>".to_string(),
                    expression: format!("STRUCT {} schema", name_for_finish),
                }),
            ),
            FieldListOutcome::Park(_) => BodyResult::Err(KError::new(KErrorKind::ShapeError(
                "STRUCT schema elaboration parked again after Combine wake".to_string(),
            ))),
        }
    });
    let combine_id = sched.add_combine(producers, scope, finish);
    BodyResult::DeferTo(combine_id)
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

    /// Phase 3 — self-recursive STRUCT: `STRUCT Tree = (children: List<Tree>)` elaborates
    /// with the field type carrying `KType::RecursiveRef("Tree")` inside `KType::List(...)`.
    /// The elaborator's threaded set seeded with the binder's own name short-circuits the
    /// self-reference to `RecursiveRef` rather than parking on the binder's placeholder.
    #[test]
    fn recursive_struct_tree_elaborates_with_recursive_ref_on_field() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run_one(scope, parse_one("STRUCT Tree = (children: List<Tree>)"));
        let data = scope.bindings().data();
        match data.get("Tree").expect("Tree should be bound") {
            KObject::StructType { name, fields } => {
                assert_eq!(name, "Tree");
                assert_eq!(fields.len(), 1);
                assert_eq!(fields[0].0, "children");
                assert_eq!(
                    fields[0].1,
                    KType::List(Box::new(KType::RecursiveRef("Tree".into()))),
                );
            }
            other => panic!("expected StructType, got {:?}", other.ktype()),
        }
    }

    /// Mutually recursive STRUCTs. `STRUCT TreeA = (b: TreeB)` and
    /// `STRUCT TreeB = (a: TreeA)` submitted in the same batch must both finalize with
    /// their schemas carrying `RecursiveRef` to each other. The current implementation
    /// only handles single-binder self-recursion via the elaborator's threaded set; mutual
    /// recursion deadlocks because each binder's body parks on the other's placeholder
    /// and neither can ever finalize. Marked `#[ignore]` until batch SCC pre-registration
    /// lands; that work is tracked under
    /// [per-declaration type identity](../../../roadmap/per-declaration-type-identity.md).
    /// Sanity check that two unrelated STRUCTs in the same batch don't
    /// spuriously cross-pollinate `RecursiveRef`. `STRUCT A = (x: Number)`,
    /// `STRUCT B = (y: A)` — B's field references A, which is non-recursive; B's schema
    /// must record `KType::Struct` (or the StructType-shaped reference) for `y`, never a
    /// `RecursiveRef`. Per-binder threaded-set seeding handles this — only the binder's
    /// own name is in its threaded set.
    #[test]
    fn mutual_non_recursive_pair_does_not_wrap_either() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        use crate::runtime::machine::execute::Scheduler;
        use crate::parse::parse;
        let mut sched = Scheduler::new();
        for e in parse("STRUCT Aa = (x: Number)\nSTRUCT Bb = (y: Aa)").unwrap() {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        let data = scope.bindings().data();
        let b_fields = match data.get("Bb") {
            Some(KObject::StructType { fields, .. }) => fields.clone(),
            other => panic!("expected Bb to be a StructType, got {:?}", other.map(|o| o.ktype())),
        };
        // `y`'s recorded KType is whatever the elaborator pulls out of `Aa`'s binding —
        // which is `KObject::StructType` (matches `KType::Struct`) — not `RecursiveRef`.
        assert_eq!(b_fields[0].0, "y");
        assert!(
            !matches!(b_fields[0].1, KType::RecursiveRef(_)),
            "Bb's `y` field must not be wrapped in RecursiveRef, got {:?}",
            b_fields[0].1,
        );
    }

    #[test]
    #[ignore]
    fn mutually_recursive_struct_pair() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        use crate::runtime::machine::execute::Scheduler;
        use crate::parse::parse;
        let mut sched = Scheduler::new();
        for e in parse("STRUCT TreeA = (b: TreeB)\nSTRUCT TreeB = (a: TreeA)").unwrap() {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        let data = scope.bindings().data();
        assert!(matches!(data.get("TreeA"), Some(KObject::StructType { .. })));
        assert!(matches!(data.get("TreeB"), Some(KObject::StructType { .. })));
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
