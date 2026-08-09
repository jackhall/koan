//! `MODULE <name:Identifier> = <body:KExpression>` — declare a structure (a bundle of
//! type definitions, values, and functions). A module is a value, so it binds value-side under a
//! snake_case name; a second overload takes the Type-token name and reports the respelling. See
//! [design/typing/modules.md](../../design/typing/modules.md) for the surface design.
//!
//! [`await_module_body`] is the body-dispatch-and-bind tail, shared with `GROUP`
//! ([`super::group_def`]) — a group *is* a module, so it differs only in the child scope it mints.

use crate::machine::body_statement_refs;
use crate::machine::core::bindings::WriteOp;
use crate::machine::model::KExpression;
use crate::machine::model::KType;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{announced_type_declaration, TypeDeclarationSurface};
use crate::machine::model::{pair_list_names, AnnouncedData, FieldNameKind};
use crate::machine::model::{KKind, SigSchema};
use crate::machine::model::{Module, ModuleDraft};
use crate::machine::BindingIndex;
use crate::machine::StepCarried;
use crate::machine::WriteGate;
use crate::machine::{Action, BodyCtx};
use crate::machine::{KError, KErrorKind};
use crate::machine::{NameLookup, Scope};

use super::{arg, kw, sig};

/// The MODULE body: pre-announces the body's top-level type declarations, mints the child scope
/// carrying that window, and hands it to [`await_module_body`], which dispatches the body block
/// against it and binds the module **value** into the parent scope's `data`.
pub fn body<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    use crate::machine::{require_identifier_name, require_kexpression};

    let name = crate::try_action!(require_identifier_name(
        ctx.args, "name", "MODULE", ctx.types
    ));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "MODULE", "body"));
    let announced = crate::try_action!(announce_type_members(&body_expr, &name));
    let child_scope = ctx.scope.alloc_child_under_module(name.clone(), announced);
    await_module_body(child_scope, name, body_expr, ctx.bind_index())
}

/// Pre-scan `body`'s **top-level** statements for the type declarations the body announces, so
/// every one of their names is visible to every statement regardless of order — which is what lets
/// a plain module host a mutually-recursive group.
///
/// A `NEWTYPE` announces one standalone member. A `UNION` announces one member per statically
/// scannable variant tag, each **owned** by the union's binder: a variant is never
/// bare-name-resolvable and never lands in `bindings.types`, so it is reached only through the
/// binder or the qualified sigil. A `UNION` whose schema does not scan announces nothing at all —
/// its own dispatch surfaces the real diagnostic.
///
/// Nested and computed declarations are untouched by construction: the scan sees only the statement
/// split [`body_statement_refs`] draws, the same boundary `GROUP` reads its members off.
pub(super) fn announce_type_members(
    body: &KExpression<'_>,
    module: &str,
) -> Result<Option<AnnouncedData>, KError> {
    let mut announced = AnnouncedData::default();
    for statement in body_statement_refs(body) {
        let Some(surface) = announced_type_declaration(statement) else {
            continue;
        };
        let Some(name) = statement.binder_name_from_type_part() else {
            continue;
        };
        if announced.declares(name) || announced.binds(name) {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "module `{module}` declares type `{name}` twice",
            ))));
        }
        match surface {
            TypeDeclarationSurface::NewType => {
                announced.announce(name.to_string());
            }
            TypeDeclarationSurface::Union => {
                // The variant tags are the union's announced members. A schema this scan cannot
                // read is left entirely unannounced rather than half-announced.
                let Some(schema) = union_schema(statement) else {
                    continue;
                };
                match pair_list_names(&schema, "UNION schema", FieldNameKind::Type) {
                    Ok(tags) => announced.announce_binder(name.to_string(), tags),
                    Err(_) => continue,
                }
            }
        }
    }
    Ok((!announced.is_empty()).then_some(announced))
}

/// The schema expression of a `UNION <name> = (<schema>)` statement — its final slot.
fn union_schema<'a>(statement: &KExpression<'a>) -> Option<KExpression<'a>> {
    match statement.parts.last()?.value {
        crate::machine::model::ExpressionPart::Expression(schema) => Some(*schema),
        _ => None,
    }
}

/// Dispatch a module body block against an already-minted `child_scope` and bind the resulting
/// module value in the parent scope — the tail every module-shaped declaration shares. `GROUP`
/// (`super::group_def`) mints its child through [`Scope::alloc_child_under_group`] and pre-registers
/// the group's operator powerset into it before calling this; `MODULE` mints a plain
/// [`Scope::alloc_child_under_module`].
///
/// Body statements dispatch on the OUTER scheduler (see
/// [`await_body_in_scope`](super::await_body::await_body_in_scope)), so a body statement
/// referencing an earlier sibling at the same outer block parks on the outer placeholder like any
/// other forward reference, and the parent binding lands at dep-finish, not when the declaration's
/// body returns to the dispatcher.
pub(super) fn await_module_body<'a>(
    child_scope: &'a Scope<'a>,
    name: String,
    body_expr: KExpression<'a>,
    bind_index: BindingIndex,
) -> Action<'a> {
    use super::await_body::await_body_in_scope;

    let name_for_finish = name;
    await_body_in_scope(child_scope, body_expr, move |fctx| {
        // Idempotent-finalize guard: a re-bound name short-circuits, re-surfacing the
        // already-bound module value from its **stored** reach.
        if let Some(NameLookup::Bound(sealed)) =
            fctx.scope.bindings().lookup_value(&name_for_finish, None)
        {
            return Action::done(Ok(StepCarried::born_delivered(
                fctx.scope.lift_resident(sealed),
            )));
        }
        // Belt check on the announcement contract: every announced member's declaration ran and
        // filled its slot, so the group sealed. A body statement that errored already errored
        // the module before this, so reaching here means the scan and dispatch disagreed about
        // what a statement declares.
        if let Some(window) = child_scope.own_declaration_window() {
            if !window.is_sealed() {
                let unfilled: Vec<String> = window
                    .unfilled_members()
                    .into_iter()
                    .map(|(name, owner)| owner.unwrap_or(name).to_string())
                    .collect();
                return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "module `{name_for_finish}` announced type `{}` but its declaration never \
                         sealed",
                    unfilled.join("`, `"),
                )))
                .with_frame(crate::machine::TraceFrame::bare(
                    "<module>",
                    format!("MODULE {name_for_finish}"),
                ))));
            }
        }
        // Mirror the module's type members into the draft. The cross-kind exclusion keeps
        // `data` and `types` disjoint by name, so this is an exact mirror of `iter_types` (no
        // value-member name can also be a type name to filter out). A nested `MODULE` is a
        // value member, so it lives in the child's `data` and is typed by its own self-sig.
        let mut draft = ModuleDraft::empty();
        for (member, kt) in child_scope.bindings().iter_types() {
            draft.type_members.insert(member, kt);
        }
        // The self-sig is derived from the draft and interned before the module exists — a
        // plain module carries no SIG, so the raw derivation is the whole signature.
        let self_sig = fctx
            .types
            .signature(SigSchema::raw_self_sig(child_scope, &draft));
        let module: &'a Module<'a> =
            Module::alloc_at_child_scope(&name_for_finish, child_scope, draft, self_sig);
        // Fused MODULE-finish seal: the module reference held **directly** here (never
        // recovered by walking the built value) is merged into this scope's region, which mints
        // and retains the child's region as the Object-arm module value's reach — this scope's
        // own region, for a same-region child. The returned terminal witnesses that same value
        // from the same composed reach; the value-side (`bindings.data`) write rides the outcome.
        let sealed = fctx.scope.seal_module(module);
        let write = WriteOp::Value {
            name: name_for_finish,
            index: bind_index,
            sealed: sealed.duplicate(),
        };
        Action::done(Ok(StepCarried::born_delivered(
            fctx.scope.lift_resident(sealed),
        )))
        .with_effect(write)
    })
}

/// The Type-token-named overload (`MODULE IntOrd = …`, `GROUP VecOps FOLD LEFT = …`): a module is a
/// value, so its name belongs in the value namespace. Registered with no binder hook — it always
/// errors, so it installs nothing.
pub(super) fn body_type_named<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    use crate::machine::require_bare_type_name;
    use crate::machine::{KError, KErrorKind};

    let name = crate::try_action!(require_bare_type_name(
        ctx.args, "name", "MODULE", ctx.types
    ));
    Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
        "module `{name}` is named with a Type token, but a module is a value — the Type-token \
         namespace names what can type a field. Name it snake_case, e.g. `{suggestion}`",
        suggestion = super::let_binding::snake_case_identifier(&name),
    )))))
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry, gate: &mut WriteGate) {
    let module_sig = |name_kt: KType| {
        sig(
            KType::EMPTY_SIGNATURE,
            vec![
                kw("MODULE"),
                arg("name", name_kt),
                kw("="),
                arg("body", KType::KEXPRESSION),
            ],
        )
    };
    crate::builtins::register_builtin_full(
        scope,
        "MODULE",
        module_sig(KType::IDENTIFIER),
        body,
        true,
        types,
        gate,
    );
    crate::builtins::register_builtin_full(
        scope,
        "MODULE",
        module_sig(KType::of_kind(KKind::ProperType)),
        body_type_named,
        false,
        types,
        gate,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{lookup_module, parse_one, TestRun};
    use crate::machine::model::KObject;
    use crate::machine::model::SigSchema;
    use crate::machine::model::{Module, ModuleDraft};
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    use crate::machine::{BindingIndex, KErrorKind};

    /// The binder name comes off the `Identifier` name part — a module binds value-side, so the
    /// submit-time placeholder is tagged `Value`.
    #[test]
    fn binder_name_extracts_module_name() {
        let program = program_storage();
        let expr = parse_one(&program, "MODULE foo = (LET x = 1)");
        let name = crate::machine::model::binder::identifier_part_binder_name(&expr);
        assert_eq!(name, Some("foo"));
    }

    /// A Type-token module name is refused by the second overload, whose only job is the
    /// respelling diagnostic — a module is a value, and the Type-token namespace names what can
    /// type a field.
    #[test]
    fn type_token_module_name_errors_with_the_snake_case_respelling() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        let err = test_run.run_one_err(parse_one(&program, "MODULE IntOrd = (LET x = 1)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("a module is a value") && msg.contains("`int_ord`")),
            "expected the snake_case respelling diagnostic, got {err}",
        );
        assert!(
            scope.bindings().data().get("IntOrd").is_none(),
            "the erroring overload binds nothing",
        );
    }

    /// A MODULE-body manifest member named `Type` collides with the builtin `Type`
    /// meta-type. Builtins are immutable and unshadowable in either channel
    /// ([`crate::machine::core::scope`] `shadows_builtin_type`), so `LET Type = Number`
    /// raises `Rebind` naming `Type` rather than declaring the member. Modules and
    /// signatures name their principal abstract type member `Carrier`
    /// (see [design/typing/modules.md](../../design/typing/modules.md)); this pins the
    /// collision so the docs and the implementation cannot silently disagree.
    #[test]
    fn module_member_named_type_collides_with_builtin_type() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        let err = test_run.run_one_err(parse_one(
            &program,
            "MODULE int_ord = ((LET Type = Number) (LET zero = 0))",
        ));
        assert!(
            matches!(&err.kind, KErrorKind::Rebind { name } if name == "Type"),
            "a MODULE member named `Type` must be a Rebind naming `Type`, got {err}",
        );
        assert!(
            scope.bindings().data().get("int_ord").is_none(),
            "the colliding module binds nothing",
        );
    }

    #[test]
    fn module_binds_under_name_in_scope() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        test_run.run("MODULE foo = (LET x = 1)");
        assert!(
            matches!(scope.lookup("foo"), Some(KObject::Module(m)) if m.path == "foo"),
            "MODULE binds the module value on the value channel",
        );
        assert!(
            scope.resolve_type("foo").is_none(),
            "a module is a value — nothing lands in `types`",
        );
    }

    #[test]
    fn bare_module_name_surfaces_as_object_value() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("MODULE foo = (LET x = 1)");
        // A module named in expression position reads back on the value channel's Object arm.
        let bare = test_run.run_one(parse_one(&program, "foo"));
        match bare {
            KObject::Module(module) => assert_eq!(module.path, "foo"),
            other => panic!(
                "bare module name must read back as an Object-arm module value, got {}",
                other.ktype().name(&test_run.types)
            ),
        }
        // PRINT returns the rendered string — a bare module renders as its path.
        let printed = test_run.run_one(parse_one(&program, "PRINT foo"));
        match printed {
            KObject::KString(s) => assert_eq!(*s, "foo"),
            other => panic!(
                "PRINT foo returns the path string, got {}",
                other.ktype().name(&test_run.types)
            ),
        }
    }

    /// A bare module name in list-element position name-resolves like any other bound
    /// identifier, so the list holds the module values and memoizes their self-sig element type
    /// — the same result the parenthesized `[(m)]` form produces.
    #[test]
    fn bare_module_names_in_list_resolve_and_memoize_self_sig() {
        use crate::machine::model::Held;
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("MODULE int_ord = (LET compare = 7)");
        let listed = test_run.run_one(parse_one(&program, "[int_ord, int_ord]"));
        match listed {
            KObject::List(items, elem) => {
                // Ruling 12: a module's self-sig renders structurally, not by the module name.
                assert_eq!(
                    elem.name(&test_run.types),
                    ":(LIST OF SIG (compare: Number))",
                    "the memoized element type is the module self-sig"
                );
                assert_eq!(items.elements().len(), 2);
                assert!(
                    items.elements().iter().all(
                        |i| matches!(i, Held::Object(KObject::Module(m)) if m.path == "int_ord")
                    ),
                    "each element is the Object-arm module value",
                );
            }
            other => panic!(
                "expected a list, got {}",
                other.ktype().name(&test_run.types)
            ),
        }
    }

    #[test]
    fn module_in_list_surfaces_as_object_element_memoized_to_self_sig() {
        use crate::machine::model::Held;
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "SIG Ordered = (VAL compare :Number)\n\
             MODULE int_ord = (LET compare = 7)",
        );
        // A parenthesized module expression evaluates to the Object-arm module value, so the list
        // element is `Held::Object` memoized as the module's self-sig, which (ruling 12) renders
        // structurally as `SIG (compare: Number)` rather than by the module name.
        let listed = test_run.run_one(parse_one(&program, "[(int_ord)]"));
        match listed {
            KObject::List(items, elem) => {
                assert_eq!(
                    elem.name(&test_run.types),
                    ":(LIST OF SIG (compare: Number))",
                    "element memoizes to the module self-sig"
                );
                assert_eq!(items.elements().len(), 1);
                assert!(
                    matches!(&items.elements()[0], Held::Object(KObject::Module(m)) if m.path == "int_ord"),
                    "the list element is the Object-arm module value",
                );
            }
            other => panic!(
                "expected a list, got {}",
                other.ktype().name(&test_run.types)
            ),
        }
    }

    #[test]
    fn module_member_access_via_attr() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("MODULE foo = (LET x = 1)");
        let result = test_run.run_one(parse_one(&program, "foo.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 1.0));
    }

    #[test]
    fn module_with_multiple_statements_in_parens() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("MODULE foo = ((LET x = 1) (LET y = 2))");
        assert!(
            matches!(test_run.run_one(parse_one(&program, "foo.x")), KObject::Number(n) if *n == 1.0)
        );
        assert!(
            matches!(test_run.run_one(parse_one(&program, "foo.y")), KObject::Number(n) if *n == 2.0)
        );
    }

    #[test]
    fn module_member_function_via_let_fn() {
        // `LET <name> = (FN ...)` binds under a clean identifier; bare FN lands under
        // its signature key and isn't reachable as `foo.<name>` via ATTR.
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        test_run.run("MODULE foo = (LET double = FN (DOUBLE x :Number) -> Number = (x))");
        let foo = lookup_module(scope, "foo", &test_run.types);
        assert!(foo.child_scope().bindings().data().contains_key("double"));
    }

    #[test]
    fn module_unknown_member_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("MODULE foo = (LET x = 1)");
        let err = test_run.run_one_err(parse_one(&program, "foo.bogus"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("foo") && msg.contains("`bogus`")),
            "expected ShapeError naming foo and bogus, got {err}",
        );
    }

    #[test]
    fn nested_module_accessible_via_chained_attr() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("MODULE outer =\n  MODULE inner = (LET x = 7)");
        let result = test_run.run_one(parse_one(&program, "outer.inner.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// MODULE body parks on an outer-scheduler placeholder for a sibling forward
    /// reference instead of erroring as `UnboundName`.
    #[test]
    fn module_body_parks_on_outer_placeholder() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("LET y = 7\nMODULE foo = (LET x = y)");
        let result = test_run.run_one(parse_one(&program, "foo.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// A failing body statement must not bind `foo` in the parent scope.
    #[test]
    fn module_body_error_short_circuits_finalize() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        test_run.run("MODULE foo = (LET x = nonexistent_name)");
        assert!(
            scope.bindings().data().get("foo").is_none(),
            "foo must not bind when its body errors",
        );
    }

    /// Pre-seed the `foo` module value through the value-side door, then re-dispatch
    /// `MODULE foo = ...`. The finalize guard reads `data`, short-circuits on the existing
    /// binding, and leaves the pre-seeded `&Module` pointer intact.
    #[test]
    fn module_finalize_short_circuits_on_idempotent_state() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        let child = scope.alloc_child_under_module("foo".into(), None);
        // Every mint carries its self-sig (2d eager-seal invariant), so a manually pre-seeded
        // module derives its interface from the same (empty) draft production would, before the
        // value exists.
        let draft = ModuleDraft::empty();
        let self_sig = test_run
            .types
            .signature(SigSchema::raw_self_sig(child, &draft));
        let module: &Module<'_> = Module::alloc_at_child_scope("foo", child, draft, self_sig);
        let sealed = scope.seal_module(module);
        scope
            .bind_value_direct(
                "foo".into(),
                sealed,
                BindingIndex::value(0),
                &mut crate::machine::WriteGate::for_test(),
            )
            .expect("pre-seed the module value binding");
        test_run.run("MODULE foo = (LET y = 2)");
        let foo = lookup_module(scope, "foo", &test_run.types);
        assert!(std::ptr::eq(foo, module));
    }

    /// Miri audit-slate: exercises the MODULE dep-finish continuation's captured
    /// `child_scope: &'a Scope<'a>` and finalize writes under tree borrows.
    #[test]
    fn module_body_dispatch_does_not_dangle() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        test_run.run("LET y = 7\nMODULE foo = ((LET x = y) (LET z = 11))");
        let foo = lookup_module(scope, "foo", &test_run.types);
        let inner = foo.child_scope();
        assert!(matches!(inner.lookup("x"), Some(KObject::Number(n)) if *n == 7.0));
        assert!(matches!(inner.lookup("z"), Some(KObject::Number(n)) if *n == 11.0));
    }
}
