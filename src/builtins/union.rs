use crate::machine::WriteGate;
use crate::machine::model::KKind;

use crate::machine::FinishCtx;
use crate::machine::core::bindings::WriteOp;
use crate::machine::model::FieldListContext;
use crate::machine::model::KType;
use crate::machine::model::{DeclWindow, RecursiveGroupWindow};
use crate::machine::model::{FieldNameKind, pair_list_names, seal_writes};
use crate::machine::{DeclarationSite, KError, KErrorKind, Scope, TraceFrame};
use crate::machine::{StepCarried, seal_type_identity};

use super::{arg, kw, sig};
use crate::machine::model::RunRegistries;
use crate::machine::model::render_label;
use crate::machine::model::{Symbol, TypeSymbol, type_binder};

/// Fill the elaborated variant payloads into the declaration window's owned members and bind the
/// union name to the anonymous union of the sealed variants. Every variant is one member of the
/// window (name = tag, [`KKind::NewType`], owner = this binder); the binder's own name already
/// elaborated to the union of the members it owns, and variant-sibling references to relative
/// sibling handles, both through the window.
///
/// The binder is not itself a member, so this drives the fills directly rather than through
/// `finalize_nominal_member`. A declaration whose window is announced by the enclosing module body
/// may still be `Deferred` here: the group seals at its last member's fill, which may be another
/// statement's, and that statement carries the writes.
fn finalize_union<'a>(
    fctx: &FinishCtx<'a, '_>,
    name: String,
    window: &DeclWindow<'a>,
    fields: Vec<(Symbol, KType)>,
    site: DeclarationSite,
) -> Result<(StepCarried<'a>, Vec<WriteOp<'a>>), KError> {
    if fields.is_empty() {
        return Err(KError::new(KErrorKind::ShapeError(
            "UNION schema must have at least one tag".to_string(),
        )));
    }
    let scope = fctx.scope;
    let brand = scope.brand();

    let binder = type_binder(&name, fctx.registries)?;
    let mut sealed = false;
    for (tag, payload) in fields {
        // A variant tag arrives as the record literal's bare field symbol. It probes the window's
        // member list by those bits and the stored `TypeSymbol` the declaration minted is the hit —
        // no class predicate, and text only where the miss is reported.
        let index = match window.view().variant_index(binder, tag) {
            Some(index) => index,
            None => {
                let tag = render_label(tag, fctx.registries);
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "UNION `{name}`: variant `{tag}` is not one of the declared variants",
                ))));
            }
        };
        sealed = window.fill(index, payload, brand, fctx.types());
    }
    let view = window.view();
    if !sealed {
        // The announced group this union belongs to still holds unfilled members, so no member has
        // an identity yet: the fill that closes the group installs every name at once, and this
        // per-statement result is discarded. A benign `Null` stands in without fabricating a handle.
        return Ok((
            fctx.ctx
                .alloc_object_scalar(&crate::machine::model::KObject::Null)
                .expect("Null is a shallow scalar carrier"),
            Vec::new(),
        ));
    }
    let union_ty = match view.sealed_binder(binder) {
        Some(kt) => kt,
        None => {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "UNION `{name}` did not seal",
            ))));
        }
    };
    // The union type is a `Copy` handle: cross it as a declared operand and fold the variant
    // carriers' reach onto the placement's witness, rather than capturing it into a fold closure.
    // The `types` writes installing every name the seal settles ride the outcome.
    Ok((seal_type_identity(scope, union_ty), seal_writes(view, site)))
}

/// Elaborate the variant schema, folding synchronously via [`finalize_union`] or deferring through
/// the shared `nominal_schema_action` field-list path (threading the binder name and the in-flight
/// pending guard), then install the sealed union identity over its member handles.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use super::nominal_schema::nominal_schema_action;
    use crate::machine::model::KObject;
    use crate::machine::{Action, require_bare_type_name};

    let name = crate::try_action!(require_bare_type_name(
        ctx.args,
        "name",
        "UNION",
        ctx.registries
    ));
    let schema_expr = match ctx.args.object("schema") {
        Some(KObject::KExpression(e)) => e.node(),
        _ => {
            return Action::done(Err(KError::new(KErrorKind::ShapeError(
                "UNION schema slot must be a parenthesized dict literal".to_string(),
            ))));
        }
    };
    // Pre-scan the variant tags so every variant has a stable relative index before any payload
    // elaborates: a payload naming a later-declared sibling must mint the index that sibling will
    // fill. The binder itself is not a member — it denotes the union of them all.
    let tags = match pair_list_names(&schema_expr, "UNION schema", FieldNameKind::Type) {
        Ok(tags) => tags,
        Err(message) => return Action::done(Err(KError::new(KErrorKind::ShapeError(message)))),
    };
    // The tags classify and intern here, at the declaration that mints them, so the window, the
    // sealed member nodes and every later diagnostic share one classified currency.
    let binder = crate::try_action!(type_binder(&name, ctx.registries));
    let tags: Vec<TypeSymbol> = crate::try_action!(
        tags.iter()
            .map(|tag| type_binder(tag, ctx.registries))
            .collect::<Result<Vec<_>, _>>()
    );
    // The window this union's variants fill: the enclosing module body's, when it announced this
    // binder, else one this declaration owns — the one-binder special case of the same machinery.
    let window = match ctx.scope.own_declaration_window() {
        Some(ambient) if ambient.binds(binder) => {
            // The scan read the same statement, so a disagreement is a scan/dispatch wiring bug.
            if tags
                .iter()
                .any(|tag| ambient.variant_index(binder, tag.symbol()).is_none())
            {
                return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "UNION `{name}`: its announced variants differ from its declared ones",
                )))));
            }
            DeclWindow::Ambient(ambient)
        }
        _ => DeclWindow::Owned(RecursiveGroupWindow::for_binder(binder, tags)),
    };
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

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let signature = sig(
        KType::of_kind(KKind::AnyType),
        vec![
            kw("UNION"),
            arg(registries, "name", KType::of_kind(KKind::ProperType)),
            kw("="),
            arg(registries, "schema", KType::KEXPRESSION),
        ],
    );
    crate::builtins::register_builtin(scope, "UNION", signature, body, registries, gate);
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{TestRun, mock_declaration_site, parse_one, type_name};
    use crate::machine::model::Carried;
    use crate::machine::model::KType;
    use crate::machine::model::{KKind, NodeSchema, RecursiveGroupWindow, TypeNode, TypeRegistry};
    use crate::machine::{KErrorKind, Scope};
    use crate::machine::{program_storage, run_root_storage};

    /// The newtype repr of union `name`'s `variant` member — each variant is a per-tag newtype
    /// `SetMember`, and its schema's `NewType` repr is the field type.
    fn variant_repr(scope: &Scope<'_>, name: &str, variant: &str, types: &TypeRegistry) -> KType {
        let handle = scope
            .resolve_type(name)
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
                && member_name.symbol() == crate::machine::model::Symbol::of(variant)
            {
                return match schema {
                    NodeSchema::NewType(repr) => repr,
                    _ => panic!("variant `{variant}` must project a NewType repr"),
                };
            }
        }
        panic!("union `{name}` has no variant `{variant}`");
    }

    #[test]
    fn binder_name_extracts_named_union_name() {
        let program = program_storage();
        let expr = parse_one(&program, "UNION Maybe = (Some :Number, None :Null)");
        let name = expr.binder_name_from_type_part();
        assert_eq!(name, Some("Maybe"));
    }

    #[test]
    fn union_named_registers_type_in_scope() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        // UNION is type-only: the declaration binds an anonymous `Union` node over one
        // per-variant newtype `SetMember` each, registered into `types`.
        let result = test_run.run_one_type(parse_one(
            &program,
            "UNION Maybe = (Some :Number None :Null)",
        ));
        let types = test_run.types();
        match types.node(result) {
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
            variant_repr(scope, "Maybe", "Some", test_run.types()),
            KType::NUMBER
        );
        assert_eq!(
            variant_repr(scope, "Maybe", "None", test_run.types()),
            KType::NULL
        );
        // `Maybe` is a Type token and `data` is keyed by `ValueSymbol`, so there is no key to
        // probe under; the containing fact is that the value table stayed empty.
        assert!(
            scope.bindings().data().is_empty(),
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
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        let root = test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(
                scope.brand(),
                parse_one(&program, "UNION (Ok :Number Err :Str)"),
            ),
            scope,
        );
        let root = test_run.runtime.install_edge_for_test(root, scope);
        test_run
            .runtime
            .execute()
            .expect("a dispatch failure is slot-terminal, not a fatal execute error");
        let err = test_run
            .runtime
            .edge_result_error(root)
            .expect_err("a bare anonymous UNION (...) must fail dispatch");
        assert!(
            matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed on bare UNION (...) (matches no UNION overload); got {err}",
        );
    }

    #[test]
    fn union_rejects_unknown_type_name() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run.run_one_err(parse_one(&program, "UNION Bad = (Some :Bogus)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("Bogus")),
            "expected ShapeError mentioning Bogus, got {err}",
        );
    }

    #[test]
    fn union_rejects_empty_schema() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run.run_one_err(parse_one(&program, "UNION Empty = ()"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("at least one tag")),
            "expected ShapeError on empty schema, got {err}",
        );
    }

    #[test]
    fn union_rejects_duplicate_tag() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err =
            test_run.run_one_err(parse_one(&program, "UNION Dupe = (Some :Number Some :Str)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("duplicate") && msg.contains("`Some`")),
            "expected ShapeError on duplicate tag, got {err}",
        );
    }

    /// `finalize_union` mints and seals a fresh union's members on first finalize, then a second
    /// finalize of the same declaration refills the already-sealed window and reaches the upsert
    /// under the same installing statement, so the overwrite is idempotent — the type-only (no
    /// value-side carrier) identity net.
    ///
    /// Both calls pass the same `site`, simulating one declaration's parallel finalize: installer
    /// equality is what makes the second install idempotent rather than a `Rebind`. See
    /// [design/typing/type-identity.md](../../design/typing/type-identity.md).
    #[test]
    fn finalize_union_seals_then_is_idempotent() {
        let program = program_storage();
        let region = run_root_storage();
        let test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        let types = test_run.registry_handle();
        let fctx = crate::machine::FinishCtx::for_scope(scope, types.registries());
        let fields = || {
            vec![
                (types.registries().labels.intern("Some"), KType::NUMBER),
                (types.registries().labels.intern("None"), KType::NULL),
            ]
        };
        // Each declarator dispatch mints its own window (the union name is the binder, its variants
        // the members), exactly as the `nominal_schema_action` entry point does.
        let make_window = || {
            crate::machine::model::DeclWindow::Owned(RecursiveGroupWindow::for_binder(
                type_name("Maybe", types.registries()),
                vec![
                    type_name("Some", types.registries()),
                    type_name("None", types.registries()),
                ],
            ))
        };
        // One declaration's identity: both finalize calls simulate a parallel finalize of the
        // same statement, so they share one site.
        let site = mock_declaration_site(0);
        // First finalize: no prior binding, so a fresh set of pending members is minted and
        // sealed. The finalize writes nothing itself — the ops it hands back are what install the
        // identity, exactly as the run loop applies them after the declaring step returns.
        let (_, writes) =
            super::finalize_union(&fctx, "Maybe".into(), &make_window(), fields(), site)
                .expect("the first finalize seals");
        assert!(
            scope
                .bindings()
                .types()
                .get(&type_name("Maybe", test_run.registries()))
                .is_none()
        );
        for write in writes {
            write
                .apply(
                    scope,
                    test_run.registries(),
                    &mut crate::machine::WriteGate::for_test(),
                )
                .expect("the first install lands");
        }
        assert_eq!(
            variant_repr(scope, "Maybe", "Some", test_run.types()),
            KType::NUMBER
        );
        assert_eq!(
            variant_repr(scope, "Maybe", "None", test_run.types()),
            KType::NULL
        );
        // Second finalize: the sealed window refills to the same handles and the upsert sees the
        // same installing handle, so it overwrites idempotently and returns the bound union type.
        let second = super::finalize_union(&fctx, "Maybe".into(), &make_window(), fields(), site);
        let is_union = second.map(|(carrier, writes)| {
            for write in writes {
                write
                    .apply(
                        scope,
                        test_run.registries(),
                        &mut crate::machine::WriteGate::for_test(),
                    )
                    .expect("a same-handle re-install overwrites idempotently");
            }
            carrier.inspect_at(std::rc::Rc::clone(&region), |c| {
                matches!(c, Carried::Type(kt)
                    if matches!(types.node(*kt), TypeNode::Union { members } if members.len() == 2))
            })
        });
        assert_eq!(
            is_union.ok(),
            Some(true),
            "expected short-circuit Ok(Type(Union)) from finalize_union",
        );
        assert!(
            scope.bindings().data().is_empty(),
            "type-only finalize must not write a value-side carrier",
        );
    }

    /// Two `UNION`s of one name in one **block** are two declarations, not one, even at equal
    /// arity — and the block rules on it at fan-out, where both declaring statements are in hand,
    /// so the error names both positions and the first declaration still lands.
    #[test]
    fn same_scope_union_redeclare_is_a_duplicate_declaration() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        let exprs = crate::parse::parse(
            program.brand(),
            &crate::machine::model::LabelInterner::new(),
            "UNION Maybe = (Some :Number None :Null)\nUNION Maybe = (Some :Str None :Null)",
        )
        .expect("parse should succeed")
        .into_iter()
        .map(|e| crate::machine::model::WorkingExpression::from_ast(scope.brand(), e))
        .collect();
        let runtime = &mut test_run.runtime;
        let ids = runtime.enter_block(scope.id, exprs, scope);
        let edges: Vec<_> = ids
            .into_iter()
            .map(|id| runtime.install_edge_for_test(id, scope))
            .collect();
        runtime
            .execute()
            .expect("execute does not surface per-slot errors");
        assert!(
            runtime.edge_result_error(edges[0]).is_ok(),
            "the first declaration should succeed, got {:?}",
            runtime.edge_result_error(edges[0]).err(),
        );
        let err = runtime
            .edge_result_error(edges[1])
            .expect_err("redeclaring Maybe in the same block should error");
        assert!(
            matches!(
                &err.kind,
                KErrorKind::DuplicateDeclaration { name, first, second }
                    if name == "Maybe" && *first == 1 && *second == 2,
            ),
            "expected DuplicateDeclaration naming both statements, got {err}",
        );
    }

    /// Two `UNION`s of one name submitted statement-at-a-time (`TestRun::dispatch_in_scope`, not
    /// `enter_block`) at equal arity still redeclare distinctly. Every statement-at-a-time
    /// submission carries its own real lexical position — the numbered-chain world has no
    /// position-free submission — and submitting is the one act that mints a fresh
    /// [`StatementId`], so the second declaration's installer differs from the first's stored
    /// entry regardless of the two schemas' matching arity, and the second install raises
    /// `Rebind`. Redeclaration identity is decided by the installing [`StatementId`] alone. The
    /// statement-at-a-time door builds no claim store and so has no fan-out to rule at, which is
    /// what makes this the `Rebind` path where
    /// [`same_scope_union_redeclare_is_a_duplicate_declaration`] takes the block one.
    #[test]
    fn statement_at_a_time_union_redeclare_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        let first = test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(
                scope.brand(),
                parse_one(&program, "UNION Maybe = (Some :Number None :Null)"),
            ),
            scope,
        );
        let first = test_run.runtime.install_edge_for_test(first, scope);
        test_run
            .runtime
            .execute()
            .expect("execute does not surface per-slot errors");
        assert!(
            test_run.runtime.edge_result_error(first).is_ok(),
            "the first declaration should succeed, got {:?}",
            test_run.runtime.edge_result_error(first).err(),
        );
        let second = test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(
                scope.brand(),
                parse_one(&program, "UNION Maybe = (Some :Str None :Null)"),
            ),
            scope,
        );
        let second = test_run.runtime.install_edge_for_test(second, scope);
        test_run
            .runtime
            .execute()
            .expect("execute does not surface per-slot errors");
        let err = test_run
            .runtime
            .edge_result_error(second)
            .expect_err("redeclaring Maybe statement-at-a-time should error");
        assert!(
            matches!(&err.kind, KErrorKind::Rebind { name } if name == "Maybe"),
            "expected Rebind naming Maybe, got {err}",
        );
    }

    /// Byte-identical `UNION` redeclaration in one block is still two declarations. A
    /// content-equality gate would unify them silently; the fan-out rules on the declared name
    /// before either statement runs, so identical content never gets a say.
    #[test]
    fn identical_content_union_redeclare_is_a_duplicate_declaration() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        let exprs = crate::parse::parse(
            program.brand(),
            &crate::machine::model::LabelInterner::new(),
            "UNION Maybe = (Some :Number None :Null)\nUNION Maybe = (Some :Number None :Null)",
        )
        .expect("parse should succeed")
        .into_iter()
        .map(|e| crate::machine::model::WorkingExpression::from_ast(scope.brand(), e))
        .collect();
        let runtime = &mut test_run.runtime;
        let ids = runtime.enter_block(scope.id, exprs, scope);
        let edges: Vec<_> = ids
            .into_iter()
            .map(|id| runtime.install_edge_for_test(id, scope))
            .collect();
        runtime
            .execute()
            .expect("execute does not surface per-slot errors");
        assert!(
            runtime.edge_result_error(edges[0]).is_ok(),
            "the first declaration should succeed, got {:?}",
            runtime.edge_result_error(edges[0]).err(),
        );
        let err = runtime
            .edge_result_error(edges[1])
            .expect_err("an identical-content redeclaration of Maybe should error");
        assert!(
            matches!(
                &err.kind,
                KErrorKind::DuplicateDeclaration { name, first, second }
                    if name == "Maybe" && *first == 1 && *second == 2,
            ),
            "expected DuplicateDeclaration on identical-content redeclare, got {err}",
        );
    }

    #[test]
    fn union_rejects_odd_part_count() {
        // Typed variants parse as `[Identifier, Type]` pairs; odd-count parts are
        // rejected by the pair-list walker.
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run.run_one_err(parse_one(&program, "UNION Pair = (Some :Number None)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("pair") || msg.contains("multiple of 2")),
            "expected ShapeError on odd part count, got {err}",
        );
    }
}
