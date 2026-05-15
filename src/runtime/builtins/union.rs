use std::collections::HashMap;
use std::rc::Rc;

use crate::runtime::machine::core::PendingTypeEntry;
use crate::runtime::model::{Argument, ExpressionSignature, KObject, KType, SignatureElement};
use crate::runtime::model::types::UserTypeKind;
use crate::runtime::machine::{
    ArgumentBundle, BodyResult, CombineFinish, Frame, KError, KErrorKind, NodeId, Scope,
    SchedulerHandle,
};
use crate::runtime::model::types::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListOutcome, ReturnType,
};

use crate::ast::KExpression;

use crate::runtime::machine::kfunction::argument_bundle::{extract_bare_type_name, extract_kexpression};
use super::{err, register_builtin_with_pre_run};

/// `UNION <name:TypeExprRef> = (<schema>)` — declare a named tagged-union type.
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
/// etc.) all surface as `ShapeError` with the offending position called out. The named
/// form registers the type token (`Maybe`) in the current scope so it can be used as a
/// constructor downstream. Returns a `KObject::TaggedUnionType` carrying the parsed
/// schema; that value reports `KType::Type` at runtime, sharing the meta-type with
/// `STRUCT`-produced schemas. Stage 3.2 removed the anonymous `UNION (...)` form —
/// every tagged value now carries a real per-declaration identity.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let name = match extract_bare_type_name(&bundle, "name", "UNION") {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    let schema_expr = match extract_kexpression(&mut bundle, "schema") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "UNION schema slot must be a parenthesized dict literal".to_string(),
            )));
        }
    };
    // Stage-3.2 SCC: install the binder's pending-type entry before launching the
    // elaborator so a fellow in-flight binder parking on this name closes the cycle
    // via DFS on `pending_types`.
    let scope_id = scope as *const _ as usize;
    scope.bindings().insert_pending_type(
        name.clone(),
        PendingTypeEntry {
            kind: UserTypeKind::Tagged,
            scope_id,
            schema_expr: schema_expr.clone(),
            edges: Vec::new(),
        },
    );
    // Seeds the threaded set with this UNION's binder name so a self-recursive
    // `UNION List = (cons: List, nil: Null)` resolves to `RecursiveRef` rather than
    // parking on its own placeholder. `with_current_decl` arms the SCC edge-recording
    // / cycle-detection arm.
    let mut elaborator = Elaborator::new(scope)
        .with_threaded([name.clone()])
        .with_current_decl(name.clone(), UserTypeKind::Tagged, scope_id);
    let outcome =
        parse_typed_field_list_via_elaborator(&schema_expr, "UNION schema", &mut elaborator);
    match outcome {
        FieldListOutcome::Done(fields) => finalize_union(scope, name, fields),
        FieldListOutcome::Err(msg) => {
            scope.bindings().remove_pending_type(&name);
            err(KError::new(KErrorKind::ShapeError(msg)))
        }
        FieldListOutcome::Park(producers) => {
            defer_union_via_combine(scope, sched, name, schema_expr, producers)
        }
    }
}

fn finalize_union<'a>(
    scope: &'a Scope<'a>,
    name: String,
    fields: Vec<(String, KType)>,
) -> BodyResult<'a> {
    // Stage-3.2 cleanup + idempotent-finalize guard. See `finalize_struct` for the
    // symmetric rationale.
    scope.bindings().remove_pending_type(&name);
    let bindings = scope.bindings();
    if bindings.types().get(&name).is_some() {
        if let Some(existing) = bindings.data().get(&name).copied() {
            return BodyResult::Value(existing);
        }
    }
    if fields.is_empty() {
        return err(KError::new(KErrorKind::ShapeError(
            "UNION schema must have at least one tag".to_string(),
        )));
    }
    // UNION addresses by tag name and doesn't care about declaration order; flatten the
    // ordered field list (which `parse_typed_field_list_via_elaborator` shares with
    // `STRUCT`) into a HashMap. Duplicate detection has already happened in the helper.
    let schema: HashMap<String, KType> = fields.into_iter().collect();
    let arena = scope.arena;
    // Per-declaration identity: same `*const _ as usize` scheme `finalize_struct` and
    // `Module::scope_id()` use. Dual-write the identity into `bindings.types` next to
    // the schema carrier in `bindings.data` so type-name resolution finds the union by
    // name and dispatch on `(PICK x: Maybe)` lowers to the same `KType::UserType` the
    // carrier's `ktype()` reports.
    let scope_id = scope as *const _ as usize;
    let union_obj: &'a KObject<'a> = arena.alloc_object(KObject::TaggedUnionType {
        schema: Rc::new(schema),
        name: name.clone(),
        scope_id,
    });
    let identity = KType::UserType {
        kind: UserTypeKind::Tagged,
        scope_id,
        name: name.clone(),
    };
    match scope.register_nominal(name, identity, union_obj) {
        Ok(obj) => BodyResult::Value(obj),
        Err(e) => err(e),
    }
}

fn defer_union_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    name: String,
    schema_expr: KExpression<'a>,
    producers: Vec<NodeId>,
) -> BodyResult<'a> {
    let name_for_finish = name.clone();
    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, _results| {
        let mut elaborator = Elaborator::new(scope).with_threaded([name_for_finish.clone()]);
        match parse_typed_field_list_via_elaborator(
            &schema_expr,
            "UNION schema",
            &mut elaborator,
        ) {
            FieldListOutcome::Done(fields) => finalize_union(scope, name_for_finish.clone(), fields),
            FieldListOutcome::Err(msg) => {
                scope.bindings().remove_pending_type(&name_for_finish);
                BodyResult::Err(
                    KError::new(KErrorKind::ShapeError(msg)).with_frame(Frame {
                        function: "<union>".to_string(),
                        expression: format!("UNION {} schema", name_for_finish),
                    }),
                )
            }
            FieldListOutcome::Park(_) => {
                scope.bindings().remove_pending_type(&name_for_finish);
                BodyResult::Err(KError::new(KErrorKind::ShapeError(
                    "UNION schema elaboration parked again after Combine wake".to_string(),
                )))
            }
        }
    });
    let combine_id = sched.add_combine(producers, scope, finish);
    BodyResult::DeferTo(combine_id)
}

/// Dispatch-time placeholder extractor for UNION. `parts[1]` is a `Type(t)` token —
/// the binder name slot. Same shape as STRUCT / MODULE / SIG.
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    expr.binder_name_from_type_part()
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin_with_pre_run(
        scope,
        "UNION",
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Type),
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
}

#[cfg(test)]
mod tests {
    use crate::runtime::builtins::test_support::{parse_one, run_one, run_one_err, run_root_silent};
    use crate::runtime::model::{KObject, KType};
    use crate::runtime::machine::{KErrorKind, RuntimeArena};

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
        assert!(matches!(result, KObject::TaggedUnionType { .. }));
        let data = scope.bindings().data();
        let entry = data.get("Maybe").expect("Maybe should be bound in scope");
        match entry {
            KObject::TaggedUnionType { schema, .. } => {
                assert_eq!(schema.get("some"), Some(&KType::Number));
                assert_eq!(schema.get("none"), Some(&KType::Null));
            }
            other => panic!("expected TaggedUnionType, got {:?}", other.ktype()),
        }
    }

    /// Stage 3.2 removed the anonymous `UNION (...)` form. The bare parens shape no
    /// longer matches any registered overload — `DispatchFailed` surfaces out of
    /// `Scheduler::execute()` (the failure is structural, not a node-level result).
    #[test]
    fn anonymous_union_fails_dispatch() {
        use crate::runtime::machine::execute::Scheduler;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let mut sched = Scheduler::new();
        sched.add_dispatch(parse_one("UNION (ok: Number err: Str)"), scope);
        let err = sched.execute().expect_err("bare UNION should fail dispatch");
        assert!(
            matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed on bare UNION (...), got {err}",
        );
    }

    #[test]
    fn union_rejects_unknown_type_name() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("UNION Bad = (some: Bogus)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("Bogus")),
            "expected ShapeError mentioning Bogus, got {err}",
        );
    }

    #[test]
    fn union_rejects_empty_schema() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("UNION Empty = ()"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("at least one tag")),
            "expected ShapeError on empty schema, got {err}",
        );
    }

    #[test]
    fn union_rejects_duplicate_tag() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("UNION Dupe = (some: Number some: Str)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("duplicate") && msg.contains("`some`")),
            "expected ShapeError on duplicate tag, got {err}",
        );
    }

    /// `finalize_union` is idempotent for a *named* form when both `bindings.types[name]`
    /// and `bindings.data[name]` are already populated. Pins the defensive guard.
    #[test]
    fn finalize_union_is_idempotent_when_both_maps_populated() {
        use crate::runtime::model::types::UserTypeKind;
        use std::collections::HashMap;
        use std::rc::Rc;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let scope_id = scope as *const _ as usize;
        let mut schema: HashMap<String, KType> = HashMap::new();
        schema.insert("some".into(), KType::Number);
        let pre_carrier: &KObject<'_> = arena.alloc_object(KObject::TaggedUnionType {
            name: "Maybe".into(),
            scope_id,
            schema: Rc::new(schema),
        });
        let pre_identity = KType::UserType {
            kind: UserTypeKind::Tagged,
            scope_id,
            name: "Maybe".into(),
        };
        scope
            .register_nominal("Maybe".into(), pre_identity, pre_carrier)
            .unwrap();
        let outcome = super::finalize_union(
            scope,
            "Maybe".into(),
            vec![("some".into(), KType::Number)],
        );
        match outcome {
            crate::runtime::machine::BodyResult::Value(obj) => {
                assert!(std::ptr::eq(obj, pre_carrier),
                    "finalize_union must return the pre-installed carrier pointer");
            }
            _ => panic!("expected Value variant from finalize_union"),
        }
    }

    /// Mutually recursive STRUCT ↔ UNION pair: `STRUCT Wrap = (m: Maybe)` with
    /// `UNION Maybe = (just: Wrap, none: Null)`. Both binders' bodies park on each
    /// other; cycle-close pre-installs identities for both kinds, both finalizes
    /// run, the field types carry `UserType` references.
    #[test]
    fn struct_union_mutual_recursion() {
        use crate::runtime::model::types::UserTypeKind;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        use crate::runtime::machine::execute::Scheduler;
        use crate::parse::parse;
        let mut sched = Scheduler::new();
        for e in parse(
            "STRUCT Wrap = (m: Maybe)\nUNION Maybe = (just: Wrap, none: Null)",
        )
        .unwrap()
        {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        let data = scope.bindings().data();
        let wrap_fields = match data.get("Wrap") {
            Some(KObject::StructType { fields, .. }) => fields.clone(),
            other => panic!("expected Wrap StructType, got {:?}", other.map(|o| o.ktype())),
        };
        assert!(
            matches!(&wrap_fields[0].1, KType::UserType { kind: UserTypeKind::Tagged, name, .. } if name == "Maybe"),
            "Wrap.m expected UserType{{Tagged Maybe}}, got {:?}",
            wrap_fields[0].1,
        );
        let maybe_schema = match data.get("Maybe") {
            Some(KObject::TaggedUnionType { schema, .. }) => schema.clone(),
            other => panic!("expected Maybe TaggedUnionType, got {:?}", other.map(|o| o.ktype())),
        };
        let just_kt = maybe_schema.get("just").expect("just tag");
        assert!(
            matches!(just_kt, KType::UserType { kind: UserTypeKind::Struct, name, .. } if name == "Wrap"),
            "Maybe.just expected UserType{{Struct Wrap}}, got {just_kt:?}",
        );
    }

    #[test]
    fn union_rejects_missing_colon() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("UNION Pair = (some Number none: Null)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`:`") || msg.contains("triple")),
            "expected ShapeError on missing colon, got {err}",
        );
    }
}
