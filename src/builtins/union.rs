use std::collections::HashMap;

use crate::machine::core::{PendingBinderGuard, PendingTypeEntry};
use crate::machine::model::types::{
    finalize_nominal_member, parse_typed_field_list_via_elaborator, seal_recursive_refs,
    Elaborator, FieldListOutcome, FieldNameKind, NominalKind, NominalSchema, SchemaSealResult,
    SealOutcome,
};
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CombineFinish, Frame, KError, KErrorKind, NodeId,
    SchedulerHandle, Scope,
};

use crate::machine::model::ast::KExpression;

use super::{arg, err, kw, register_builtin_with_binder, sig};
use crate::machine::core::kfunction::argument_bundle::{
    extract_bare_type_name, extract_kexpression,
};

/// `UNION <name:TypeExprRef> = (<schema>)` — declare a named tagged-union type.
///
/// The schema slot is a parens-wrapped expression of `<tag:Identifier> :<type:Type>`
/// pairs. Parens keep the parts from dispatching as their own expression so the
/// elaborator sees identifier/type pairs directly. Type-only: the variant schema rides
/// the sealed `RecursiveSet` member in `bindings.types`, and the declaration yields a
/// `KTypeValue(SetRef)` first-class type value — no value-side carrier.
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
    // Mark this binder in-flight so a consumer referencing it (an earlier sibling still
    // finalizing) can park on our producer node. The guard's Drop removes the entry; the
    // Park path moves it into the Combine-finish closure.
    let scope_id = scope.id;
    let pending_guard = scope.bindings().insert_pending_type(
        name.clone(),
        PendingTypeEntry {
            kind: NominalKind::Tagged,
            scope_id,
            schema_expr: schema_expr.clone(),
        },
    );
    // Seed the threaded set with this UNION's name so a self-recursive
    // `UNION List = (cons :List nil :Null)` resolves to the transient `RecursiveRef`
    // rather than parking on its own placeholder. The chain gates variant type names to
    // this binder's lexical position.
    let chain = sched.current_lexical_chain();
    let mut elaborator = Elaborator::new(scope)
        .with_threaded([name.clone()])
        .with_chain(chain.clone());
    let outcome = parse_typed_field_list_via_elaborator(
        &schema_expr,
        "UNION schema",
        FieldNameKind::Identifier,
        &mut elaborator,
    );
    // Non-nominal: the UNION name obeys source order like any other type name.
    let bind_index = chain
        .as_ref()
        .map(|c| BindingIndex::value(c.index))
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
            chain,
        ),
    }
}

/// Seal the elaborated variant schema into the UNION's [`RecursiveSet`] member and install
/// the `SetRef` identity into `bindings.types` — type-only, no value-side carrier.
/// Transient `RecursiveRef(name)` variant leaves seal to `SetLocal(index)`. Mirror of
/// [`super::struct_def::finalize_struct`].
fn finalize_union<'a>(
    scope: &'a Scope<'a>,
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
        NominalKind::Tagged,
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
        SealOutcome::Sealed(kt_ref) => BodyResult::Value(
            scope
                .arena
                .alloc_object(KObject::KTypeValue(kt_ref.clone())),
        ),
        SealOutcome::DanglingRef(missing) => err(KError::new(KErrorKind::ShapeError(format!(
            "UNION `{name}` schema references unsealed type `{missing}`",
        )))),
        SealOutcome::Rebind(e) => err(e),
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
    chain: Option<std::rc::Rc<crate::machine::core::LexicalFrame>>,
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
        let mut elaborator = Elaborator::new(scope)
            .with_threaded([name_for_finish.clone()])
            .with_chain(chain.clone());
        match parse_typed_field_list_via_elaborator(
            &spliced_schema,
            "UNION schema",
            FieldNameKind::Identifier,
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
    register_builtin_with_binder(
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
    use crate::machine::model::types::{NominalKind, ProjectedSchema, RecursiveSet};
    use crate::machine::model::{KObject, KType};
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
        let expr = parse_one("UNION Maybe = (some :Number, none :Null)");
        let name = super::binder_name(&expr);
        assert_eq!(name.as_deref(), Some("Maybe"));
    }

    #[test]
    fn union_named_registers_type_in_scope() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        // UNION is type-only: the declaration yields a `KTypeValue(SetRef)` whose Tagged
        // member carries the variant schema, registered into `types`.
        let result = run_one(scope, parse_one("UNION Maybe = (some :Number none :Null)"));
        match result {
            KObject::KTypeValue(KType::SetRef { set, index }) => {
                assert_eq!(set.member(*index).kind, NominalKind::Tagged);
            }
            other => panic!(
                "expected KTypeValue(SetRef) for Maybe, got {:?}",
                other.ktype()
            ),
        }
        let schema = tagged_schema(scope, "Maybe");
        assert_eq!(schema.get("some"), Some(&KType::Number));
        assert_eq!(schema.get("none"), Some(&KType::Null));
        assert!(
            scope.bindings().data().get("Maybe").is_none(),
            "UNION must not write a value-side carrier into data",
        );
    }

    /// No anonymous `UNION (...)` form: the bare two-part shape matches no UNION
    /// overload (the declarator is `UNION <name> = (<schema>)`, four elements), so
    /// dispatch fails cleanly with `DispatchFailed` rather than eagerly evaluating the
    /// `(ok …)` operand and leaking `UnboundName("ok")` — the relaxed admission pass
    /// keeps it a clean miss (see
    /// [scheduler.md § In-walk dispatch precedence](../../design/typing/scheduler.md#in-walk-dispatch-precedence)).
    #[test]
    fn anonymous_union_fails_dispatch() {
        use crate::machine::execute::Scheduler;

        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let mut sched = Scheduler::new();
        sched.add_dispatch(parse_one("UNION (ok :Number err :Str)"), scope);
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
            NominalKind::Tagged,
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
            vec![("some".into(), KType::Number)],
            BindingIndex::value(0),
        );
        assert!(matches!(first, crate::machine::BodyResult::Value(_)));
        // The member of the *pre-installed* set is now filled in place.
        assert!(pre_set.member(0).is_filled());
        let schema = tagged_schema(scope, "Maybe");
        assert_eq!(schema.get("some"), Some(&KType::Number));
        let second = super::finalize_union(
            scope,
            "Maybe".into(),
            vec![("some".into(), KType::Number)],
            BindingIndex::value(0),
        );
        match second {
            crate::machine::BodyResult::Value(KObject::KTypeValue(KType::SetRef {
                set,
                index,
            })) => {
                assert_eq!(set.member(*index).name, "Maybe");
            }
            _ => panic!("expected short-circuit Value(KTypeValue(SetRef)) from finalize_union"),
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
        let err = run_one_err(scope, parse_one("UNION Pair = (some :Number none)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("pair") || msg.contains("multiple of 2")),
            "expected ShapeError on odd part count, got {err}",
        );
    }
}
