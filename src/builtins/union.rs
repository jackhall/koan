use crate::machine::model::types::KKind;
use std::rc::Rc;

use crate::machine::core::kfunction::action::FinishCtx;
use crate::machine::core::{NameLookup, ScopeId};
use crate::machine::execute::{seal_type_operand, StepCarried};
use crate::machine::model::types::{
    seal_union_refs, FieldNameKind, NominalMember, NominalSchema, RecursiveSet,
};
use crate::machine::model::KType;
use crate::machine::{BindingIndex, KError, KErrorKind, Scope, TraceFrame};

use super::{arg, kw, sig};
use crate::machine::DeliveredCarried;

/// What `finalize_union` recovers from `bindings.types[name]` before sealing.
enum UnionRecovery<'a> {
    /// A parallel finalize already sealed this union (its members are filled) — return the
    /// bound union type unchanged (the idempotency net).
    Sealed(KType<'a>),
    /// No matching sealed identity — mint a fresh set of `n` pending members.
    Fresh,
}

/// Recover a sealed union identity for `name`, distinguishing an idempotent re-finalize (a bound
/// `KType::Union` whose members are all filled) from a fresh declaration. A pre-seal placeholder
/// is never reused in place: under content-addressed identity its transient digest no longer
/// stands in for the sealed result, so a partially-filled match re-mints Fresh rather than
/// upserting the placeholder.
fn recover_union<'a>(
    scope: &Scope<'a>,
    name: &str,
    scope_id: ScopeId,
    n: usize,
) -> UnionRecovery<'a> {
    let bound = scope
        .bindings()
        .lookup_type(name, None)
        .and_then(NameLookup::bound);
    let members = match bound {
        Some(KType::Union { members, .. }) => members,
        _ => return UnionRecovery::Fresh,
    };
    let set = match members.first() {
        Some(KType::SetRef { set, .. }) => set,
        _ => return UnionRecovery::Fresh,
    };
    let member0 = set.member(0);
    if set.len() == n
        && member0.scope_id == scope_id
        && member0.kind == KKind::NewType
        && set.members().iter().all(NominalMember::is_filled)
    {
        return UnionRecovery::Sealed(KType::union_of(members.clone()));
    }
    UnionRecovery::Fresh
}

/// Seal the elaborated variant schema into a per-variant [`RecursiveSet`] and bind the union
/// name to the anonymous union of its members. One member per variant (name = tag,
/// [`KKind::NewType`], schema [`NominalSchema::NewType`]) in declaration order; the binder's own
/// name seals to the union of all members (ruling F2), variant-sibling references to `SetLocal`
/// indices. `bindings.types[name]` binds `KType::Union { members: [SetRef{set,0}, …], .. }` through
/// [`KType::union_of`] — type-only, no value-side carrier.
fn finalize_union<'a>(
    fctx: &FinishCtx<'a>,
    name: String,
    fields: Vec<(String, KType<'a>)>,
    bind_index: BindingIndex,
    carriers: &[&DeliveredCarried],
) -> Result<StepCarried<'a>, KError> {
    if fields.is_empty() {
        return Err(KError::new(KErrorKind::ShapeError(
            "UNION schema must have at least one tag".to_string(),
        )));
    }
    let scope = fctx.scope;
    let scope_id = scope.id;
    let n = fields.len();

    let set = match recover_union(scope, &name, scope_id, n) {
        // Idempotent re-finalize: the union is already bound. Cross its identity as a declared
        // operand — move the recovered union's reference into this region under a checked audit that
        // derives its stored reach, and fold the carriers' reach onto the placement, the same
        // coverage the register-success path produces.
        UnionRecovery::Sealed(kt) => {
            let (kt_ref, stored) = scope.alloc_ktype_checked_stored(kt)?;
            return Ok(seal_type_operand(
                scope,
                fctx.ctx.frame(),
                kt_ref,
                stored,
                carriers,
            ));
        }
        UnionRecovery::Fresh => Rc::new(RecursiveSet::new(
            fields
                .iter()
                .map(|(tag, _)| NominalMember::pending(tag.clone(), scope_id, KKind::NewType))
                .collect(),
        )),
    };

    // Ruling F2: the declaring name maps to the union of every member; `union_of` collapses a
    // one-variant union to that member. Variant-sibling references seal via `index_of`.
    let binder_union = KType::union_of((0..n).map(KType::SetLocal).collect());
    let missing = std::cell::RefCell::new(Vec::new());
    let sealed: Vec<(usize, KType<'a>)> = fields
        .into_iter()
        .enumerate()
        .map(|(index, (_tag, payload))| {
            (
                index,
                seal_union_refs(&set, &name, &binder_union, &payload, &missing),
            )
        })
        .collect();
    if let Some(m) = missing.into_inner().into_iter().next() {
        return Err(KError::new(KErrorKind::ShapeError(format!(
            "UNION `{name}` schema references unsealed type `{m}`",
        ))));
    }
    for (index, payload) in sealed {
        set.fill_member(index, NominalSchema::NewType(Box::new(payload)));
    }

    let union_ty = KType::union_of(
        (0..n)
            .map(|index| KType::SetRef {
                set: Rc::clone(&set),
                index,
            })
            .collect(),
    );
    match scope.register_nominal_upsert(name.clone(), union_ty, bind_index) {
        // `register_type_upsert` hands back the region-allocated `&KType`. Cross it as a declared
        // operand and fold the variant carriers' reach onto the placement's witness, rather than
        // capturing the union type into a fold closure.
        Ok(kt_ref) => Ok(seal_type_operand(
            scope,
            fctx.ctx.frame(),
            kt_ref,
            scope.checked_reach_of_type(kt_ref),
            carriers,
        )),
        Err(e) => Err(e),
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
        KKind::NewType,
        "UNION schema",
        FieldNameKind::Type,
        error_frame,
        finalize_union,
    )
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(
        KType::OfKind(KKind::AnyType),
        vec![
            kw("UNION"),
            arg("name", KType::OfKind(KKind::ProperType)),
            kw("="),
            arg("schema", KType::KExpression),
        ],
    );
    crate::builtins::register_builtin_full(
        scope,
        "UNION",
        signature,
        body,
        Some((super::type_part_binder_name, crate::machine::BindKind::Type)),
        None,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run_one_err, run_one_type, run_root_silent};
    use crate::machine::core::run_root_storage;
    use crate::machine::model::types::{KKind, ProjectedSchema, RecursiveSet};
    use crate::machine::model::values::Carried;
    use crate::machine::model::KType;
    use crate::machine::{BindingIndex, KErrorKind, Scope};

    /// The projected (`SetLocal`s resolved) newtype repr of union `name`'s `variant` member —
    /// each variant is a per-tag newtype under the dissolved model.
    fn variant_repr<'a>(scope: &'a Scope<'a>, name: &str, variant: &str) -> KType<'a> {
        let members = match scope.resolve_type(name) {
            Some(KType::Union { members, .. }) => members,
            other => panic!("expected {name} to be a Union in types, got {other:?}"),
        };
        for member in members {
            if let KType::SetRef { set, index } = member {
                if set.member(*index).name == variant {
                    return match RecursiveSet::projected_schema(set, *index) {
                        ProjectedSchema::NewType(repr) => repr,
                        _ => panic!("variant `{variant}` must project a NewType repr"),
                    };
                }
            }
        }
        panic!("union `{name}` has no variant `{variant}`");
    }

    #[test]
    fn binder_name_extracts_named_union_name() {
        let expr = parse_one("UNION Maybe = (Some :Number, None :Null)");
        let name = expr.binder_name_from_type_part();
        assert_eq!(name.as_deref(), Some("Maybe"));
    }

    #[test]
    fn union_named_registers_type_in_scope() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        // UNION is type-only: the declaration binds an anonymous `KType::Union` over one
        // per-variant newtype `SetRef` each, registered into `types`.
        let result = run_one_type(scope, parse_one("UNION Maybe = (Some :Number None :Null)"));
        match result {
            KType::Union { members, .. } => {
                assert_eq!(members.len(), 2, "one member per variant");
                for member in members {
                    match member {
                        KType::SetRef { set, index } => {
                            assert_eq!(set.member(*index).kind, KKind::NewType);
                        }
                        other => panic!("union member must be a newtype SetRef, got {other:?}"),
                    }
                }
            }
            other => panic!("expected Union type for Maybe, got {other:?}"),
        }
        assert_eq!(variant_repr(scope, "Maybe", "Some"), KType::Number);
        assert_eq!(variant_repr(scope, "Maybe", "None"), KType::Null);
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

        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let mut runtime = KoanRuntime::new();
        let root = runtime.dispatch_in_scope(parse_one("UNION (Ok :Number Err :Str)"), scope);
        runtime
            .execute()
            .expect("a dispatch failure is slot-terminal, not a fatal execute error");
        let err = runtime
            .result_error(root)
            .expect_err("a bare anonymous UNION (...) must fail dispatch");
        assert!(
            matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed on bare UNION (...) (matches no UNION overload); got {err}",
        );
    }

    #[test]
    fn union_rejects_unknown_type_name() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let err = run_one_err(scope, parse_one("UNION Bad = (Some :Bogus)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("Bogus")),
            "expected ShapeError mentioning Bogus, got {err}",
        );
    }

    #[test]
    fn union_rejects_empty_schema() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let err = run_one_err(scope, parse_one("UNION Empty = ()"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("at least one tag")),
            "expected ShapeError on empty schema, got {err}",
        );
    }

    #[test]
    fn union_rejects_duplicate_tag() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let err = run_one_err(scope, parse_one("UNION Dupe = (Some :Number Some :Str)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("duplicate") && msg.contains("`Some`")),
            "expected ShapeError on duplicate tag, got {err}",
        );
    }

    /// `finalize_union` mints and seals a fresh union's members on first finalize, then
    /// short-circuits on a second finalize once every member is filled — the type-only
    /// (no value-side carrier) idempotency net (`recover_union`'s `Sealed` arm).
    ///
    /// `recover_union` has no in-place reuse arm: under content-addressed identity a pre-seal
    /// composite carries a transient digest that no longer stands in for the sealed result, so a
    /// partially-filled prior binding re-mints Fresh rather than upserting the placeholder. Only a
    /// fully-sealed match short-circuits. See
    /// [design/typing/type-identity.md](../../design/typing/type-identity.md).
    #[test]
    fn finalize_union_seals_then_is_idempotent() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let fctx = crate::machine::core::kfunction::action::FinishCtx::for_scope(scope);
        let fields = || {
            vec![
                ("Some".to_string(), KType::Number),
                ("None".to_string(), KType::Null),
            ]
        };
        // First finalize: no prior binding, so a fresh set of pending members is minted, sealed,
        // and registered.
        let first =
            super::finalize_union(&fctx, "Maybe".into(), fields(), BindingIndex::value(0), &[]);
        assert!(first.is_ok());
        assert_eq!(variant_repr(scope, "Maybe", "Some"), KType::Number);
        assert_eq!(variant_repr(scope, "Maybe", "None"), KType::Null);
        // Second finalize: every member is filled, so `recover_union` short-circuits, returning
        // the bound union type unchanged.
        let second =
            super::finalize_union(&fctx, "Maybe".into(), fields(), BindingIndex::value(0), &[]);
        let is_union = second.map(|carrier| {
            carrier.inspect_pinned(
                &crate::machine::FrameSet::empty(),
                |c| matches!(c, Carried::Type(KType::Union { members, .. }) if members.len() == 2),
            )
        });
        assert_eq!(
            is_union.ok(),
            Some(true),
            "expected short-circuit Ok(Type(Union)) from finalize_union",
        );
        assert!(
            scope.bindings().data().get("Maybe").is_none(),
            "type-only finalize must not write a value-side carrier",
        );
    }

    #[test]
    fn union_rejects_odd_part_count() {
        // Typed variants parse as `[Identifier, Type]` pairs; odd-count parts are
        // rejected by the pair-list walker.
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let err = run_one_err(scope, parse_one("UNION Pair = (Some :Number None)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("pair") || msg.contains("multiple of 2")),
            "expected ShapeError on odd part count, got {err}",
        );
    }
}
