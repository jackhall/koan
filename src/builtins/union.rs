use crate::machine::model::KKind;
use std::rc::Rc;

use crate::machine::model::FieldListContext;
use crate::machine::model::KType;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{
    pair_list_names, FieldNameKind, RecursiveGroupWindow, RelativeSchema, TypeNode,
};
use crate::machine::FinishCtx;
use crate::machine::{seal_type_identity, StepCarried};
use crate::machine::{BindingIndex, KError, KErrorKind, Scope, TraceFrame};

use super::{arg, kw, sig};

/// What `finalize_union` recovers from `bindings.types[name]` before sealing.
enum UnionRecovery {
    /// A parallel finalize already sealed this union — return the bound union type unchanged (the
    /// idempotency net).
    Sealed(KType),
    /// No matching sealed identity — fill the declaration window and seal it.
    Fresh,
}

/// Recover a sealed union identity for `name`, distinguishing an idempotent re-finalize from a
/// fresh declaration. Declaration identity is the stored [`BindingIndex`]: a binding installed by
/// this same statement is this declaration's own seal, anything else is a genuine prior binding of
/// the name.
///
/// The structural check reads the *interned nodes*: a bound union every one of whose members is a
/// sealed group member, `n` of them, is this declaration's finished identity. Nothing pre-seal can
/// be bound at all — a member has no handle until its window seals — so there is no
/// partially-sealed state to recognize here.
fn recover_union(
    scope: &Scope<'_>,
    name: &str,
    bind_index: BindingIndex,
    n: usize,
    types: &TypeRegistry,
) -> UnionRecovery {
    let (bound, installed_at) = match scope.bindings().committed_type_binding(name) {
        Some(entry) => entry,
        None => return UnionRecovery::Fresh,
    };
    if installed_at != bind_index {
        return UnionRecovery::Fresh;
    }
    // `union_of` collapses a one-variant union to that member, so a single variant binds the
    // member handle directly rather than a `Union` node.
    let members: Vec<KType> = match types.node(*bound) {
        TypeNode::Union { members } => members,
        TypeNode::SetMember { .. } => vec![*bound],
        _ => return UnionRecovery::Fresh,
    };
    // A persistent-scope re-run whose source changed arity at the same statement position routes
    // onto the Fresh → `Rebind` path.
    if members.len() != n
        || !members
            .iter()
            .all(|m| matches!(types.node(*m), TypeNode::SetMember { .. }))
    {
        return UnionRecovery::Fresh;
    }
    UnionRecovery::Sealed(*bound)
}

/// Fill the elaborated variant schema into the declaration window and bind the union name to the
/// anonymous union of its sealed members. One member per variant (name = tag, [`KKind::NewType`],
/// schema [`RelativeSchema::NewType`]) in declaration order; the binder's own name already
/// elaborated to the union of all members (ruling F2) and variant-sibling references to relative
/// sibling handles, both through the window. `bindings.types[name]` binds the union of the sealed
/// member handles — type-only, no value-side carrier.
fn finalize_union<'a>(
    fctx: &FinishCtx<'a, '_>,
    name: String,
    window: Rc<RecursiveGroupWindow>,
    fields: Vec<(String, KType)>,
    bind_index: BindingIndex,
) -> Result<StepCarried<'a>, KError> {
    if fields.is_empty() {
        return Err(KError::new(KErrorKind::ShapeError(
            "UNION schema must have at least one tag".to_string(),
        )));
    }
    let scope = fctx.scope;
    let n = fields.len();

    if let UnionRecovery::Sealed(kt) = recover_union(scope, &name, bind_index, n, fctx.types) {
        // Idempotent re-finalize: the union is already bound. Allocate the recovered union into
        // this scope's own region and cross it as a declared operand, folding the carriers' reach
        // onto the placement — the same coverage the register-success path produces.
        let kt_ref = scope.brand().alloc_ktype(kt);
        return Ok(seal_type_identity(scope, kt_ref));
    }

    let mut sealed = None;
    for (tag, payload) in fields {
        let index = match window.index_of(&tag) {
            Some(index) => index,
            None => {
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "UNION `{name}`: variant `{tag}` is not one of the declared variants",
                ))))
            }
        };
        sealed = window.fill_member(index, RelativeSchema::NewType(payload), fctx.types);
    }
    // A window still open here holds a member the pre-scan announced or a reference announced —
    // either way a variant no declaration filled.
    let sealed = match sealed {
        Some(sealed) => sealed,
        None => {
            let missing = window.unfilled_member_names().join("`, `");
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "UNION `{name}` schema references unsealed type `{missing}`",
            ))));
        }
    };

    let union_ty = fctx.types.union_of(sealed.members);
    match scope.register_nominal_upsert(name.clone(), union_ty, bind_index) {
        // `register_nominal_upsert` hands back the region-allocated `&KType`. Cross it as a
        // declared operand and fold the variant carriers' reach onto the placement's witness,
        // rather than capturing the union type into a fold closure.
        Ok(kt_ref) => Ok(seal_type_identity(scope, kt_ref)),
        Err(e) => Err(e),
    }
}

/// Elaborate the variant schema, folding synchronously via [`finalize_union`] or deferring through
/// the shared `nominal_schema_action` field-list path (threading the binder name and the in-flight
/// pending guard), then install the sealed union identity over its member handles.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use super::nominal_schema::nominal_schema_action;
    use crate::machine::model::KObject;
    use crate::machine::{arg_object, require_bare_type_name, Action};

    let name = crate::try_action!(require_bare_type_name(ctx.args, "name", "UNION", ctx.types));
    let schema_expr = match arg_object(ctx.args, "schema") {
        Some(KObject::KExpression(e)) => e.clone(),
        _ => {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(
                "UNION schema slot must be a parenthesized dict literal".to_string(),
            ))))
        }
    };
    // Pre-scan the variant tags so every variant has a stable relative index before any payload
    // elaborates: a payload naming a later-declared sibling must mint the index that sibling will
    // fill. The binder itself is not a member — it denotes the union of them all.
    let tags = match pair_list_names(&schema_expr, "UNION schema", FieldNameKind::Type) {
        Ok(tags) => tags,
        Err(message) => return Action::Done(Err(KError::new(KErrorKind::ShapeError(message)))),
    };
    let window = RecursiveGroupWindow::new(
        tags.into_iter().map(|tag| (tag, KKind::NewType)).collect(),
        Some(name.clone()),
    );
    let error_frame = TraceFrame::bare("<union>", format!("UNION {name} schema"));
    nominal_schema_action(
        ctx,
        name,
        window,
        schema_expr,
        FieldListContext::UNION_SCHEMA,
        FieldNameKind::Type,
        error_frame,
        finalize_union,
    )
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry) {
    let signature = sig(
        KType::of_kind(KKind::AnyType),
        vec![
            kw("UNION"),
            arg("name", KType::of_kind(KKind::ProperType)),
            kw("="),
            arg("schema", KType::KEXPRESSION),
        ],
    );
    crate::builtins::register_builtin_full(
        scope,
        "UNION",
        signature,
        body,
        Some((super::type_part_binder_name, crate::machine::BindKind::Type)),
        None,
        types,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, TestRun};
    use crate::machine::model::Carried;
    use crate::machine::model::KType;
    use crate::machine::model::{KKind, NodeSchema, RecursiveGroupWindow, TypeNode, TypeRegistry};
    use crate::machine::run_root_storage;
    use crate::machine::{BindingIndex, KErrorKind, Scope};

    /// The newtype repr of union `name`'s `variant` member — each variant is a per-tag newtype
    /// `SetMember`, and its schema's `NewType` repr is the field type.
    fn variant_repr(scope: &Scope<'_>, name: &str, variant: &str, types: &TypeRegistry) -> KType {
        let handle = scope
            .resolve_type(name)
            .copied()
            .unwrap_or_else(|| panic!("expected {name} to be a type in scope"));
        let members = match types.node(handle) {
            TypeNode::Union { members } => members,
            _ => panic!("expected {name} to be a Union in types, got {handle:?}"),
        };
        for member in members {
            if let TypeNode::SetMember {
                name: member_name,
                schema,
                ..
            } = types.node(member)
            {
                if member_name == variant {
                    return match schema {
                        NodeSchema::NewType(repr) => repr,
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
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        // UNION is type-only: the declaration binds an anonymous `Union` node over one
        // per-variant newtype `SetMember` each, registered into `types`.
        let result = test_run.run_one_type(parse_one("UNION Maybe = (Some :Number None :Null)"));
        let types = test_run.types();
        match types.node(*result) {
            TypeNode::Union { members } => {
                assert_eq!(members.len(), 2, "one member per variant");
                for member in members {
                    match types.node(member) {
                        TypeNode::SetMember { kind, .. } => {
                            assert_eq!(kind, KKind::NewType);
                        }
                        _ => panic!("union member must be a newtype SetMember, got {member:?}"),
                    }
                }
            }
            _ => panic!("expected Union type for Maybe, got {result:?}"),
        }
        assert_eq!(
            variant_repr(scope, "Maybe", "Some", &test_run.types),
            KType::NUMBER
        );
        assert_eq!(
            variant_repr(scope, "Maybe", "None", &test_run.types),
            KType::NULL
        );
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
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        let runtime = &mut test_run.runtime;
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
        let mut test_run = TestRun::silent(&region);
        let err = test_run.run_one_err(parse_one("UNION Bad = (Some :Bogus)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("Bogus")),
            "expected ShapeError mentioning Bogus, got {err}",
        );
    }

    #[test]
    fn union_rejects_empty_schema() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let err = test_run.run_one_err(parse_one("UNION Empty = ()"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("at least one tag")),
            "expected ShapeError on empty schema, got {err}",
        );
    }

    #[test]
    fn union_rejects_duplicate_tag() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let err = test_run.run_one_err(parse_one("UNION Dupe = (Some :Number Some :Str)"));
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
        let test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        let types = test_run.types.clone();
        let fctx = crate::machine::FinishCtx::for_scope(scope, &types);
        let fields = || {
            vec![
                ("Some".to_string(), KType::NUMBER),
                ("None".to_string(), KType::NULL),
            ]
        };
        // Each declarator dispatch mints its own window (the union name is the binder, its variants
        // the members), exactly as the `nominal_schema_action` entry point does.
        let make_window = || {
            RecursiveGroupWindow::new(
                vec![
                    ("Some".to_string(), KKind::NewType),
                    ("None".to_string(), KKind::NewType),
                ],
                Some("Maybe".to_string()),
            )
        };
        // First finalize: no prior binding, so a fresh set of pending members is minted, sealed,
        // and registered.
        let first = super::finalize_union(
            &fctx,
            "Maybe".into(),
            make_window(),
            fields(),
            BindingIndex::value(0),
        );
        assert!(first.is_ok());
        assert_eq!(
            variant_repr(scope, "Maybe", "Some", &test_run.types),
            KType::NUMBER
        );
        assert_eq!(
            variant_repr(scope, "Maybe", "None", &test_run.types),
            KType::NULL
        );
        // Second finalize: every member is filled, so `recover_union` short-circuits, returning
        // the bound union type unchanged.
        let second = super::finalize_union(
            &fctx,
            "Maybe".into(),
            make_window(),
            fields(),
            BindingIndex::value(0),
        );
        let is_union = second.map(|carrier| {
            carrier.inspect_pinned(&crate::machine::FrameSet::empty(), |c| {
                matches!(c, Carried::Type(kt)
                    if matches!(types.node(**kt), TypeNode::Union { members } if members.len() == 2))
            })
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

    /// Two `UNION`s of one name in one scope are two declarations, not one, even at equal arity:
    /// `recover_union`'s identity check reads the stored `BindingIndex`, which belongs to the first
    /// statement, so the second re-mints Fresh and the install raises `Rebind`. `enter_block` is
    /// what gives the statements their distinct lexical indices.
    #[test]
    fn same_scope_union_redeclare_rebinds() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        let exprs = crate::parse::parse(
            "UNION Maybe = (Some :Number None :Null)\nUNION Maybe = (Some :Str None :Null)",
        )
        .expect("parse should succeed");
        let runtime = &mut test_run.runtime;
        let ids = runtime.enter_block(scope.id, exprs, scope);
        runtime
            .execute()
            .expect("execute does not surface per-slot errors");
        assert!(
            runtime.result_error(ids[0]).is_ok(),
            "the first declaration should succeed, got {:?}",
            runtime.result_error(ids[0]).err(),
        );
        let err = runtime
            .result_error(ids[1])
            .expect_err("redeclaring Maybe in the same scope should error");
        assert!(
            matches!(&err.kind, KErrorKind::Rebind { name } if name == "Maybe"),
            "expected Rebind naming Maybe, got {err}",
        );
    }

    #[test]
    fn union_rejects_odd_part_count() {
        // Typed variants parse as `[Identifier, Type]` pairs; odd-count parts are
        // rejected by the pair-list walker.
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let err = test_run.run_one_err(parse_one("UNION Pair = (Some :Number None)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("pair") || msg.contains("multiple of 2")),
            "expected ShapeError on odd part count, got {err}",
        );
    }
}
