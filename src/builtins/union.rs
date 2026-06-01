use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::{PendingBinderGuard, PendingTypeEntry};
use crate::machine::model::types::UserTypeKind;
use crate::machine::model::types::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListOutcome,
};
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CombineFinish, Frame, KError, KErrorKind, NodeId,
    SchedulerHandle, Scope,
};

use crate::machine::model::ast::KExpression;

use super::{arg, err, kw, register_nominal_binder, sig};
use crate::machine::core::kfunction::argument_bundle::{
    extract_bare_type_name, extract_kexpression,
};

/// `UNION <name:TypeExprRef> = (<schema>)` — declare a named tagged-union type.
///
/// The schema slot is a parens-wrapped expression of `<tag:Identifier> :<type:Type>`
/// pairs. Parens keep the parts from dispatching as their own expression so the
/// elaborator sees identifier/type pairs directly. Type-only: the variant schema rides
/// the `UserType { Tagged { schema } }` identity in `bindings.types`, and the declaration
/// yields a `KTypeValue(UserType)` first-class type value — no value-side carrier.
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
    // Install the pending-type entry before launching the elaborator so a fellow
    // in-flight binder parking on this name can close the cycle via DFS. The guard's
    // Drop removes the entry; the Park path moves it into the Combine-finish closure.
    let scope_id = scope.id;
    let pending_guard = scope.bindings().insert_pending_type(
        name.clone(),
        PendingTypeEntry {
            kind: UserTypeKind::tagged_sentinel(),
            scope_id,
            schema_expr: schema_expr.clone(),
            edges: Vec::new(),
        },
    );
    // Seed the threaded set with this UNION's name so a self-recursive
    // `UNION List = (cons :List nil :Null)` resolves to `RecursiveRef` rather than
    // parking on its own placeholder.
    let mut elaborator = Elaborator::new(scope)
        .with_threaded([name.clone()])
        .with_current_decl(name.clone(), UserTypeKind::tagged_sentinel(), scope_id);
    let outcome =
        parse_typed_field_list_via_elaborator(&schema_expr, "UNION schema", &mut elaborator);
    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::nominal(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);
    match outcome {
        FieldListOutcome::Done(fields) => finalize_union(scope, name, fields, bind_index),
        FieldListOutcome::Err(msg) => err(KError::new(KErrorKind::ShapeError(msg))),
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => defer_union_via_combine(
            scope,
            sched,
            name,
            schema_expr,
            park_producers,
            sub_dispatches,
            pending_guard,
            bind_index,
        ),
    }
}

/// Fold the elaborated variant schema into the `UserType { Tagged { schema } }` identity
/// and upsert it into `bindings.types` — type-only, no value-side carrier. Mirror of
/// [`super::struct_def::finalize_struct`].
fn finalize_union<'a>(
    scope: &'a Scope<'a>,
    name: String,
    fields: Vec<(String, KType<'a>)>,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    // Idempotent-finalize guard: short-circuit only on a populated `Tagged { schema }`
    // payload, distinguishing it from the cycle-close payload-empty pre-install.
    let bindings = scope.bindings();
    if let Some(KType::UserType {
        kind: UserTypeKind::Tagged { schema },
        ..
    }) = bindings.lookup_type(&name, None)
    {
        if !schema.is_empty() {
            return BodyResult::Value(scope.arena.alloc(KObject::KTypeValue(
                bindings.lookup_type(&name, None).unwrap().clone(),
            )));
        }
    }
    if fields.is_empty() {
        return err(KError::new(KErrorKind::ShapeError(
            "UNION schema must have at least one tag".to_string(),
        )));
    }
    // UNION addresses by tag, not by declaration order — flatten the ordered list
    // (shared shape with `STRUCT`) into a HashMap. Duplicates already rejected upstream.
    let schema: HashMap<String, KType<'a>> = fields.into_iter().collect();
    let scope_id = scope.id;
    let identity = KType::UserType {
        kind: UserTypeKind::Tagged {
            schema: Rc::new(schema),
        },
        scope_id,
        name: name.clone(),
    };
    match scope.register_type_upsert(name, identity, bind_index) {
        Ok(kt_ref) => BodyResult::Value(scope.arena.alloc(KObject::KTypeValue(kt_ref.clone()))),
        Err(e) => err(e),
    }
}

#[allow(clippy::too_many_arguments)]
fn defer_union_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    name: String,
    schema_expr: KExpression<'a>,
    park_producers: Vec<NodeId>,
    sub_dispatches: Vec<(usize, KExpression<'a>)>,
    pending_guard: PendingBinderGuard<'a>,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    use crate::machine::model::ast::ExpressionPart;
    let name_for_finish = name.clone();
    let park_count = park_producers.len();
    let mut owned_subs: Vec<NodeId> = Vec::with_capacity(sub_dispatches.len());
    let mut splice_layout: Vec<(usize, usize)> = Vec::with_capacity(sub_dispatches.len());
    for (slot_idx, sub_expr) in sub_dispatches {
        let id = sched.add_dispatch(sub_expr, scope);
        splice_layout.push((slot_idx, park_count + owned_subs.len()));
        owned_subs.push(id);
    }
    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
        let _pending_guard = pending_guard;
        let mut spliced_parts = schema_expr.parts.clone();
        for &(slot_idx, results_pos) in &splice_layout {
            let obj = results[results_pos];
            if !matches!(obj, KObject::KTypeValue(_)) {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                    "UNION schema slot at part-index {slot_idx} expected a type expression, \
                     got a {} value",
                    obj.ktype().name(),
                ))));
            }
            spliced_parts[slot_idx].value = ExpressionPart::Future(obj);
        }
        let spliced_schema = KExpression::new(spliced_parts);
        let mut elaborator = Elaborator::new(scope).with_threaded([name_for_finish.clone()]);
        match parse_typed_field_list_via_elaborator(
            &spliced_schema,
            "UNION schema",
            &mut elaborator,
        ) {
            FieldListOutcome::Done(fields) => {
                finalize_union(scope, name_for_finish.clone(), fields, bind_index)
            }
            FieldListOutcome::Err(msg) => BodyResult::Err(
                KError::new(KErrorKind::ShapeError(msg)).with_frame(Frame::bare(
                    "<union>",
                    format!("UNION {} schema", name_for_finish),
                )),
            ),
            FieldListOutcome::Pending { .. } => {
                BodyResult::Err(KError::new(KErrorKind::ShapeError(
                    "UNION schema elaboration parked again after Combine wake".to_string(),
                )))
            }
        }
    });
    let combine_id = sched.add_combine(owned_subs, park_producers, scope, finish);
    BodyResult::DeferTo(combine_id)
}

/// Dispatch-time placeholder extractor: pulls the binder name from `parts[1]`'s
/// `Type(t)` token. Same shape as STRUCT / MODULE / SIG.
pub(crate) fn binder_name(expr: &KExpression<'_>) -> Option<String> {
    expr.binder_name_from_type_part()
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_nominal_binder(
        scope,
        "UNION",
        sig(
            KType::Type,
            vec![
                kw("UNION"),
                arg("name", KType::TypeExprRef),
                kw("="),
                arg("schema", KType::KExpression),
            ],
        ),
        body,
        Some(binder_name),
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run_one, run_one_err, run_root_silent};
    use crate::machine::model::{KObject, KType};
    use crate::machine::{BindingIndex, KErrorKind, RuntimeArena};

    #[test]
    fn binder_name_extracts_named_union_name() {
        let expr = parse_one("UNION Maybe = (some :Number, none :Null)");
        let name = super::binder_name(&expr);
        assert_eq!(name.as_deref(), Some("Maybe"));
    }

    #[test]
    fn union_named_registers_type_in_scope() {
        use crate::machine::model::types::UserTypeKind;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        // UNION is type-only: the declaration yields a `KTypeValue(UserType)` whose
        // `Tagged { schema }` payload carries the variant schema, registered into `types`.
        let result = run_one(scope, parse_one("UNION Maybe = (some :Number none :Null)"));
        assert!(matches!(
            result,
            KObject::KTypeValue(KType::UserType {
                kind: UserTypeKind::Tagged { .. },
                ..
            })
        ));
        match scope.resolve_type("Maybe") {
            Some(KType::UserType {
                kind: UserTypeKind::Tagged { schema },
                ..
            }) => {
                assert_eq!(schema.get("some"), Some(&KType::Number));
                assert_eq!(schema.get("none"), Some(&KType::Null));
            }
            other => panic!("expected Tagged identity for Maybe in types, got {other:?}"),
        }
        assert!(
            scope.bindings().data().get("Maybe").is_none(),
            "UNION must not write a value-side carrier into data",
        );
    }

    /// No anonymous `UNION (...)` form: the inner sub-expression classifies as a
    /// `FunctionValueCall` with bare identifier `ok` as head, surfacing `UnboundName`
    /// on the slot rather than a scheduler-level `DispatchFailed`.
    #[test]
    fn anonymous_union_fails_dispatch() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("UNION (ok :Number err :Str)"));
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(name) if name == "ok"),
            "expected UnboundName(\"ok\") on bare UNION (...) (sub-expression `ok` \
             is unbound in the fast lane); got {err}",
        );
    }

    #[test]
    fn union_rejects_unknown_type_name() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("UNION Bad = (some :Bogus)"));
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
        let err = run_one_err(scope, parse_one("UNION Dupe = (some :Number some :Str)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("duplicate") && msg.contains("`some`")),
            "expected ShapeError on duplicate tag, got {err}",
        );
    }

    /// `finalize_union` upserts the schema-bearing identity over a cycle-close
    /// payload-empty pre-install, then short-circuits on a second finalize once the
    /// payload is populated — the type-only (no value-side carrier) idempotency net.
    #[test]
    fn finalize_union_idempotent_after_cycle_close_pre_install() {
        use crate::machine::model::types::UserTypeKind;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let scope_id = scope.id;
        let pre_identity = KType::UserType {
            kind: UserTypeKind::tagged_sentinel(),
            scope_id,
            name: "Maybe".into(),
        };
        scope.cycle_close_install_identity("Maybe".into(), pre_identity, BindingIndex::nominal(0));
        let first = super::finalize_union(
            scope,
            "Maybe".into(),
            vec![("some".into(), KType::Number)],
            BindingIndex::nominal(0),
        );
        assert!(matches!(first, crate::machine::BodyResult::Value(_)));
        match scope.resolve_type("Maybe") {
            Some(KType::UserType {
                kind: UserTypeKind::Tagged { schema },
                ..
            }) => {
                assert_eq!(schema.get("some"), Some(&KType::Number));
            }
            other => panic!("expected populated Tagged identity, got {other:?}"),
        }
        let second = super::finalize_union(
            scope,
            "Maybe".into(),
            vec![("some".into(), KType::Number)],
            BindingIndex::nominal(0),
        );
        match second {
            crate::machine::BodyResult::Value(KObject::KTypeValue(KType::UserType {
                name,
                ..
            })) => {
                assert_eq!(name, "Maybe");
            }
            _ => panic!("expected short-circuit Value(KTypeValue(UserType)) from finalize_union"),
        }
        assert!(
            scope.bindings().data().get("Maybe").is_none(),
            "type-only finalize must not write a value-side carrier",
        );
    }

    /// Mutually recursive STRUCT ↔ UNION pair: each binder parks on the other,
    /// cycle-close pre-installs identities for both kinds, and field types end up
    /// carrying `UserType` references to the partner.
    #[test]
    fn struct_union_mutual_recursion() {
        use crate::machine::model::types::UserTypeKind;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        use crate::machine::execute::Scheduler;
        use crate::parse::parse;
        let mut sched = Scheduler::new();
        for e in parse("STRUCT Wrap = (m :Maybe)\nUNION Maybe = (just :Wrap, none :Null)").unwrap()
        {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        // Both are type-only — read schemas off the type-side identities.
        let wrap_fields = match scope.resolve_type("Wrap") {
            Some(KType::UserType {
                kind: UserTypeKind::Struct { fields },
                ..
            }) => fields.clone(),
            other => panic!("expected Wrap Struct identity, got {other:?}"),
        };
        assert!(
            matches!(&wrap_fields[0].1, KType::UserType { kind: UserTypeKind::Tagged { .. }, name, .. } if name == "Maybe"),
            "Wrap.m expected UserType{{Tagged Maybe}}, got {:?}",
            wrap_fields[0].1,
        );
        let maybe_schema = match scope.resolve_type("Maybe") {
            Some(KType::UserType {
                kind: UserTypeKind::Tagged { schema },
                ..
            }) => schema.clone(),
            other => panic!("expected Maybe Tagged identity, got {other:?}"),
        };
        let just_kt = maybe_schema.get("just").expect("just tag");
        assert!(
            matches!(just_kt, KType::UserType { kind: UserTypeKind::Struct { .. }, name, .. } if name == "Wrap"),
            "Maybe.just expected UserType{{Struct Wrap}}, got {just_kt:?}",
        );
    }

    #[test]
    fn union_rejects_odd_part_count() {
        // Typed variants parse as `[Identifier, Type]` pairs; odd-count parts are
        // rejected by the pair-list walker.
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("UNION Pair = (some :Number none)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("pair") || msg.contains("multiple of 2")),
            "expected ShapeError on odd part count, got {err}",
        );
    }
}
