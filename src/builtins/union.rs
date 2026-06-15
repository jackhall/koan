use crate::machine::model::types::KKind;
use std::collections::HashMap;

use crate::machine::model::types::{
    finalize_nominal_member, seal_recursive_refs, FieldNameKind, NominalSchema, SchemaSealResult,
    SealOutcome,
};
use crate::machine::model::values::Carried;
use crate::machine::model::KType;
use crate::machine::{BindingIndex, KError, KErrorKind, Scope, TraceFrame};

use super::{arg, kw, sig};

/// Seal the elaborated variant schema into the UNION's [`RecursiveSet`] member and install
/// the `SetRef` identity into `bindings.types` — type-only, no value-side carrier.
/// Transient `RecursiveRef(name)` variant leaves seal to `SetLocal(index)`. Mirror of
/// [`super::struct_def::finalize_struct`].
fn finalize_union<'a>(
    scope: &Scope<'a>,
    name: String,
    fields: Vec<(String, KType<'a>)>,
    bind_index: BindingIndex,
) -> Result<Carried<'a>, KError> {
    if fields.is_empty() {
        return Err(KError::new(KErrorKind::ShapeError(
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
        SealOutcome::Sealed(kt_ref) => Ok(Carried::Type(scope.arena.alloc_ktype(kt_ref.clone()))),
        SealOutcome::DanglingRef(missing) => Err(KError::new(KErrorKind::ShapeError(format!(
            "UNION `{name}` schema references unsealed type `{missing}`",
        )))),
        SealOutcome::Rebind(e) => Err(e),
    }
}

/// Elaborate the variant schema, folding synchronously via [`finalize_union`] or deferring through
/// the shared `nominal_schema_action` field-list path (threading the binder name and the in-flight
/// pending guard), then install the sealed `SetRef` identity.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use super::nominal_schema::nominal_schema_action;
    use crate::machine::core::kfunction::action::{arg_object, require_bare_type_name, Action};
    use crate::machine::model::KObject;

    let name = crate::try_action!(require_bare_type_name(ctx.args, "name", "UNION"));
    let schema_expr = match arg_object(ctx.args, "schema") {
        Some(KObject::KExpression(e)) => e.clone(),
        _ => {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(
                "UNION schema slot must be a parenthesized dict literal".to_string(),
            ))))
        }
    };
    let error_frame = TraceFrame::bare("<union>", format!("UNION {name} schema"));
    nominal_schema_action(
        ctx,
        name,
        schema_expr,
        KKind::Tagged,
        "UNION schema",
        FieldNameKind::Type,
        error_frame,
        finalize_union,
    )
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
    crate::builtins::register_builtin_full(
        scope,
        "UNION",
        signature,
        body,
        Some(super::type_part_binder_name),
        None,
        false,
    );
}

#[cfg(test)]
mod tests {
    use super::Carried;
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
        use crate::machine::execute::KoanRuntime;

        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let mut sched = KoanRuntime::new();
        let root = sched.dispatch_in_scope(parse_one("UNION (Ok :Number Err :Str)"), scope);
        sched
            .execute()
            .expect("a dispatch failure is slot-terminal, not a fatal execute error");
        let err = sched
            .read_result(root)
            .err()
            .expect("a bare anonymous UNION (...) must fail dispatch");
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
        assert!(first.is_ok());
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
            Ok(Carried::Type(KType::SetRef { set, index })) => {
                assert_eq!(set.member(*index).name, "Maybe");
            }
            _ => panic!("expected short-circuit Ok(Type(SetRef)) from finalize_union"),
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
