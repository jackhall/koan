use crate::machine::model::types::KKind;
use std::collections::HashMap;

use crate::machine::core::PendingTypeEntry;
use crate::machine::execute::defer_field_list_via_combine;
use crate::machine::model::types::{
    finalize_nominal_member, parse_typed_field_list_via_elaborator, seal_recursive_refs,
    Elaborator, FieldListOutcome, FieldNameKind, NominalSchema, SchemaSealResult, SealOutcome,
};
use crate::machine::model::KType;
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, Frame, KError, KErrorKind, SchedulerHandle, Scope,
};

use super::{arg, err, kw, sig};
#[cfg(not(feature = "action-harness"))]
use super::register_builtin_with_binder;
use crate::machine::core::kfunction::argument_bundle::{
    extract_bare_type_name, extract_kexpression,
};
#[cfg(feature = "action-harness")]
use crate::machine::execute::defer_field_list_action;

/// `UNION <name:TypeExprRef> = (<schema>)` — declare a named tagged-union type.
///
/// The schema slot is a parens-wrapped expression of `<tag:Type> :<type:Type>`
/// pairs — variant tags are capitalized type names. Parens keep the parts from
/// dispatching as their own expression so the elaborator sees tag/type pairs
/// directly. Type-only: the variant schema rides
/// the sealed `RecursiveSet` member in `bindings.types`, and the declaration yields a
/// `KTypeValue(SetRef)` first-class type value — no value-side carrier.
pub fn body<'a, 's>(
    sched: &mut dyn SchedulerHandle<'a, 's>,
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
    // Mark this binder in-flight so a consumer referencing it (an earlier sibling still
    // finalizing) can park on our producer node. The guard's Drop removes the entry; the
    // Park path moves it into the Combine-finish closure.
    let scope_id = sched.current_scope().id;
    let pending_guard = sched.current_scope().bindings().insert_pending_type(
        name.clone(),
        PendingTypeEntry {
            kind: KKind::Tagged,
            scope_id,
            schema_expr: schema_expr.clone(),
        },
    );
    // Seed the threaded set with this UNION's name so a self-recursive
    // `UNION List = (cons :List nil :Null)` resolves to the transient `RecursiveRef`
    // rather than parking on its own placeholder. The chain gates variant type names to
    // this binder's lexical position.
    let chain = sched.current_lexical_chain();
    let mut elaborator = Elaborator::new(sched.current_scope())
        .with_threaded([name.clone()])
        .with_chain(chain.clone());
    let outcome = parse_typed_field_list_via_elaborator(
        &schema_expr,
        "UNION schema",
        FieldNameKind::Type,
        &mut elaborator,
        None,
    );
    // Non-nominal: the UNION name obeys source order like any other type name.
    let bind_index = chain
        .as_ref()
        .map(|c| BindingIndex::value(c.index))
        .unwrap_or(BindingIndex::BUILTIN);
    match outcome {
        FieldListOutcome::Done(fields) => {
            finalize_union(sched.current_scope(), name, fields, bind_index)
        }
        FieldListOutcome::Err(msg) => err(KError::new(KErrorKind::ShapeError(msg))),
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => {
            let name_for_finish = name.clone();
            defer_field_list_via_combine(
                sched,
                schema_expr,
                park_producers,
                sub_dispatches,
                "UNION schema",
                FieldNameKind::Type,
                vec![name.clone()],
                chain,
                Some(pending_guard),
                Some(Frame::bare("<union>", format!("UNION {name} schema"))),
                Box::new(move |scope, fields| {
                    finalize_union(scope, name_for_finish, fields, bind_index)
                }),
            )
        }
    }
}

/// Seal the elaborated variant schema into the UNION's [`RecursiveSet`] member and install
/// the `SetRef` identity into `bindings.types` — type-only, no value-side carrier.
/// Transient `RecursiveRef(name)` variant leaves seal to `SetLocal(index)`. Mirror of
/// [`super::struct_def::finalize_struct`].
fn finalize_union<'a>(
    scope: &Scope<'a>,
    name: String,
    fields: Vec<(String, KType<'a>)>,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    if fields.is_empty() {
        return err(KError::new(KErrorKind::ShapeError(
            "UNION schema must have at least one tag".to_string(),
        )));
    }
    let scope_id = scope.id;
    let outcome = finalize_nominal_member(
        scope,
        &name,
        scope_id,
        KKind::Tagged,
        |set| {
            let missing = std::cell::RefCell::new(Vec::new());
            // UNION addresses by tag, not declaration order — flatten the ordered list
            // (shared shape with STRUCT) into a HashMap. Duplicates already rejected upstream.
            let schema: HashMap<String, KType<'a>> = fields
                .into_iter()
                .map(|(tag, kt)| (tag, seal_recursive_refs(set, &kt, &missing)))
                .collect();
            match missing.into_inner().into_iter().next() {
                Some(m) => SchemaSealResult::Dangling(m),
                None => SchemaSealResult::Ok(NominalSchema::Tagged(schema)),
            }
        },
        bind_index,
    );
    match outcome {
        SealOutcome::Sealed(kt_ref) => BodyResult::ktype(scope.arena.alloc_ktype(kt_ref.clone())),
        SealOutcome::DanglingRef(missing) => err(KError::new(KErrorKind::ShapeError(format!(
            "UNION `{name}` schema references unsealed type `{missing}`",
        )))),
        SealOutcome::Rebind(e) => err(e),
    }
}

/// `Action`-harness twin of [`body`]: elaborate the variant schema, folding synchronously via
/// [`finalize_union`] or deferring through [`defer_field_list_action`] (threading the binder name
/// and the in-flight pending guard), then install the sealed `SetRef` identity.
#[cfg(feature = "action-harness")]
pub fn body_action<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{
        arg_object, arg_type, body_result_to_action, Action,
    };
    use crate::machine::core::kfunction::argument_bundle::bare_type_name;
    use crate::machine::model::KObject;

    let name = match arg_type(ctx.args, "name") {
        Some(t) => match bare_type_name(t, "name", "UNION") {
            Ok(n) => n,
            Err(e) => return Action::Done(Err(e)),
        },
        None => return Action::Done(Err(KError::new(KErrorKind::MissingArg("name".to_string())))),
    };
    let schema_expr = match arg_object(ctx.args, "schema") {
        Some(KObject::KExpression(e)) => e.clone(),
        _ => {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(
                "UNION schema slot must be a parenthesized dict literal".to_string(),
            ))))
        }
    };
    let scope_id = ctx.scope.id;
    let pending_guard = ctx.scope.bindings().insert_pending_type(
        name.clone(),
        PendingTypeEntry {
            kind: KKind::Tagged,
            scope_id,
            schema_expr: schema_expr.clone(),
        },
    );
    let chain = ctx.chain.clone();
    let mut elaborator = Elaborator::new(ctx.scope)
        .with_threaded([name.clone()])
        .with_chain(chain.clone());
    let outcome = parse_typed_field_list_via_elaborator(
        &schema_expr,
        "UNION schema",
        FieldNameKind::Type,
        &mut elaborator,
        None,
    );
    let bind_index = chain
        .as_ref()
        .map(|c| BindingIndex::value(c.index))
        .unwrap_or(BindingIndex::BUILTIN);
    match outcome {
        FieldListOutcome::Done(fields) => {
            body_result_to_action(finalize_union(ctx.scope, name, fields, bind_index))
        }
        FieldListOutcome::Err(msg) => Action::Done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => {
            let name_for_finish = name.clone();
            defer_field_list_action(
                schema_expr,
                park_producers,
                sub_dispatches,
                "UNION schema",
                FieldNameKind::Type,
                vec![name.clone()],
                chain,
                Some(pending_guard),
                Some(Frame::bare("<union>", format!("UNION {name} schema"))),
                Box::new(move |scope, fields| {
                    finalize_union(scope, name_for_finish, fields, bind_index)
                }),
            )
        }
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(
        KType::OfKind(KKind::Any),
        vec![
            kw("UNION"),
            arg("name", KType::OfKind(KKind::Proper)),
            kw("="),
            arg("schema", KType::KExpression),
        ],
    );
    #[cfg(feature = "action-harness")]
    crate::builtins::register_action_builtin_full(
        scope,
        "UNION",
        signature,
        body_action,
        Some(super::type_part_binder_name),
        None,
        false,
    );
    #[cfg(not(feature = "action-harness"))]
    register_builtin_with_binder(
        scope,
        "UNION",
        signature,
        body,
        Some(super::type_part_binder_name),
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run_one_err, run_one_type, run_root_silent};
    use crate::machine::model::types::{KKind, ProjectedSchema, RecursiveSet};
    use crate::machine::model::KType;
    use crate::machine::{BindingIndex, KErrorKind, RuntimeArena, Scope};

    /// The projected (`SetLocal`s resolved) variant schema of a UNION member, by name.
    fn tagged_schema<'a>(
        scope: &'a Scope<'a>,
        name: &str,
    ) -> std::collections::HashMap<String, KType<'a>> {
        match scope.resolve_type(name) {
            Some(KType::SetRef { set, index }) => match RecursiveSet::projected_schema(set, *index)
            {
                ProjectedSchema::Tagged(schema) => schema,
                _ => panic!("expected {name} to project a Tagged schema, got a different kind"),
            },
            other => panic!("expected {name} to be a Tagged SetRef in types, got {other:?}"),
        }
    }

    #[test]
    fn binder_name_extracts_named_union_name() {
        let expr = parse_one("UNION Maybe = (Some :Number, None :Null)");
        let name = expr.binder_name_from_type_part();
        assert_eq!(name.as_deref(), Some("Maybe"));
    }

    #[test]
    fn union_named_registers_type_in_scope() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        // UNION is type-only: the declaration yields a `SetRef` type whose Tagged
        // member carries the variant schema, registered into `types`.
        let result = run_one_type(scope, parse_one("UNION Maybe = (Some :Number None :Null)"));
        match result {
            KType::SetRef { set, index } => {
                assert_eq!(set.member(*index).kind, KKind::Tagged);
            }
            other => panic!("expected SetRef type for Maybe, got {other:?}"),
        }
        let schema = tagged_schema(scope, "Maybe");
        assert_eq!(schema.get("Some"), Some(&KType::Number));
        assert_eq!(schema.get("None"), Some(&KType::Null));
        assert!(
            scope.bindings().data().get("Maybe").is_none(),
            "UNION must not write a value-side carrier into data",
        );
    }

    /// No anonymous `UNION (...)` form: the bare two-part shape matches no UNION
    /// overload (the declarator is `UNION <name> = (<schema>)`, four elements), so
    /// dispatch fails cleanly with `DispatchFailed` rather than eagerly evaluating the
    /// `(Ok …)` operand and leaking an unbound-name miss — the relaxed admission pass
    /// keeps it a clean miss (see
    /// [scheduler.md § In-walk dispatch precedence](../../design/typing/scheduler.md#in-walk-dispatch-precedence)).
    #[test]
    fn anonymous_union_fails_dispatch() {
        use crate::machine::execute::Scheduler;

        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let mut sched = Scheduler::new();
        sched.add_dispatch(parse_one("UNION (Ok :Number Err :Str)"), scope);
        let err = sched
            .execute()
            .expect_err("a bare anonymous UNION (...) must fail dispatch");
        assert!(
            matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed on bare UNION (...) (matches no UNION overload); got {err}",
        );
    }

    #[test]
    fn union_rejects_unknown_type_name() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("UNION Bad = (Some :Bogus)"));
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
        let err = run_one_err(scope, parse_one("UNION Dupe = (Some :Number Some :Str)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("duplicate") && msg.contains("`Some`")),
            "expected ShapeError on duplicate tag, got {err}",
        );
    }

    /// `finalize_union` fills the member of a pre-installed `SetRef` (the seal pre-install),
    /// then short-circuits on a second finalize once the member is filled — the type-only
    /// (no value-side carrier) idempotency net.
    #[test]
    fn finalize_union_idempotent_after_seal_pre_install() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let scope_id = scope.id;
        // Pre-install a `SetRef` to a pending (unfilled) member, as the RECURSIVE TYPES
        // block does for its co-declared members.
        let pending_member = crate::machine::model::types::NominalMember::pending(
            "Maybe".into(),
            scope_id,
            KKind::Tagged,
        );
        let pre_set = std::rc::Rc::new(RecursiveSet::new(vec![pending_member]));
        let pre_identity = KType::SetRef {
            set: std::rc::Rc::clone(&pre_set),
            index: 0,
        };
        scope.preinstall_identity("Maybe".into(), pre_identity, BindingIndex::value(0));
        let first = super::finalize_union(
            scope,
            "Maybe".into(),
            vec![("Some".into(), KType::Number)],
            BindingIndex::value(0),
        );
        assert!(matches!(first, crate::machine::BodyResult::Value(_)));
        // The member of the *pre-installed* set is now filled in place.
        assert!(pre_set.member(0).is_filled());
        let schema = tagged_schema(scope, "Maybe");
        assert_eq!(schema.get("Some"), Some(&KType::Number));
        let second = super::finalize_union(
            scope,
            "Maybe".into(),
            vec![("Some".into(), KType::Number)],
            BindingIndex::value(0),
        );
        match second {
            crate::machine::BodyResult::Value(crate::machine::model::values::Carried::Type(
                KType::SetRef { set, index },
            )) => {
                assert_eq!(set.member(*index).name, "Maybe");
            }
            _ => panic!("expected short-circuit Value(Type(SetRef)) from finalize_union"),
        }
        assert!(
            scope.bindings().data().get("Maybe").is_none(),
            "type-only finalize must not write a value-side carrier",
        );
    }

    #[test]
    fn union_rejects_odd_part_count() {
        // Typed variants parse as `[Identifier, Type]` pairs; odd-count parts are
        // rejected by the pair-list walker.
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("UNION Pair = (Some :Number None)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("pair") || msg.contains("multiple of 2")),
            "expected ShapeError on odd part count, got {err}",
        );
    }
}
