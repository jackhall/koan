//! `MODULE <name:Identifier> = <body:KExpression>` — declare a structure (a bundle of
//! type definitions, values, and functions). A module is a value, so it binds value-side under a
//! snake_case name; a second overload takes the Type-token name and reports the respelling. See
//! [design/typing/modules.md](../../design/typing/modules.md) for the surface design.
//!
//! [`await_module_body`] is the body-dispatch-and-bind tail, shared with `GROUP`
//! ([`super::group_def`]) — a group *is* a module, so it differs only in the child scope it mints.

use crate::machine::model::KExpression;
use crate::machine::model::KType;
use crate::machine::model::Module;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{KKind, SigSchema};
use crate::machine::BindingIndex;
use crate::machine::StepCarried;
use crate::machine::{Action, BodyCtx};
use crate::machine::{NameLookup, Scope, TraceFrame};

use super::{arg, kw, sig};

/// The MODULE body: mints the child scope and hands it to [`await_module_body`], which dispatches
/// the body block against it and binds the module **value** into the parent scope's `data`.
pub fn body<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    use crate::machine::{require_identifier_name, require_kexpression};

    let name = crate::try_action!(require_identifier_name(
        ctx.args, "name", "MODULE", ctx.types
    ));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "MODULE", "body"));
    let child_scope = ctx
        .scope
        .brand()
        .alloc_scope(Scope::child_under_module(ctx.scope, name.clone()));
    await_module_body(child_scope, name, body_expr, ctx.bind_index(), "MODULE")
}

/// Dispatch a module body block against an already-minted `child_scope` and bind the resulting
/// module value in the parent scope — the tail every module-shaped declaration shares. `GROUP`
/// (`super::group_def`) mints its child through [`Scope::child_under_group`] and pre-registers the
/// group's operator powerset into it before calling this; `MODULE` mints a plain
/// [`Scope::child_under_module`]. `surface` labels the trace frame an erroring finalize carries.
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
    surface: &'static str,
) -> Action<'a> {
    use super::await_body::{await_body_in_scope, ChildScopeSeal};

    let name_for_finish = name;
    await_body_in_scope(
        child_scope,
        body_expr,
        ChildScopeSeal::SealBeforeFinish,
        move |fctx| {
            // Idempotent-finalize guard: a re-bound name short-circuits, re-surfacing the
            // already-bound module value from its **stored** reach.
            if let Some(NameLookup::Bound(hit)) = fctx
                .scope
                .bindings()
                .lookup_value_carrier(&name_for_finish, None)
            {
                return Action::Done(Ok(StepCarried::born(
                    fctx.scope.resident_value_carrier(hit.obj, hit.stored),
                )));
            }
            let module: &'a Module<'a> = fctx
                .scope
                .brand()
                .alloc_module(Module::new(name_for_finish.clone(), child_scope));
            // Mirror the module's type members into `type_members`. The cross-kind exclusion keeps
            // `data` and `types` disjoint by name, so this is an exact mirror of `iter_types` (no
            // value-member name can also be a type name to filter out). A nested `MODULE` is a
            // value member, so it lives in the child's `data` and is typed by its own self-sig.
            {
                let mut tm = module.type_members.borrow_mut();
                for (member, kt) in child_scope.bindings().iter_types() {
                    tm.insert(member, kt);
                }
            }
            // Seal the module's self-sig now that `type_members` reflects the body — a plain
            // module carries no SIG, so the raw derivation is the whole signature.
            module.seal_self_sig(SigSchema::raw_self_sig(module), fctx.types);
            // Fused MODULE-finish bind: the module's stored reach is derived off the child scope held
            // **directly** here (never by walking the built value) — the home-borrow bit included,
            // `true` because the same-region child's own region owner covers this scope's region
            // before home-omission — and the Object-arm module value is allocated and bound
            // value-side (`bindings.data`) under it. The returned terminal witnesses that same value
            // from the same stored reach.
            match fctx.scope.bind_module(
                name_for_finish.clone(),
                module,
                child_scope,
                bind_index,
                fctx.types,
            ) {
                Ok((obj, stored)) => Action::Done(Ok(StepCarried::born(
                    fctx.scope.resident_value_carrier(obj, stored),
                ))),
                Err(e) => Action::Done(Err(e.with_frame(TraceFrame::bare(
                    "<module>",
                    format!("{surface} {name_for_finish} body"),
                )))),
            }
        },
    )
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
    Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
        "module `{name}` is named with a Type token, but a module is a value — the Type-token \
         namespace names what can type a field. Name it snake_case, e.g. `{suggestion}`",
        suggestion = super::let_binding::snake_case_identifier(&name),
    )))))
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry) {
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
        Some((
            super::identifier_part_binder_name,
            crate::machine::BindKind::Value,
        )),
        None,
        types,
    );
    crate::builtins::register_builtin_full(
        scope,
        "MODULE",
        module_sig(KType::of_kind(KKind::ProperType)),
        body_type_named,
        None,
        None,
        types,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{lookup_module, parse_one, TestRun};
    use crate::machine::model::KObject;
    use crate::machine::model::Module;
    use crate::machine::model::SigSchema;
    use crate::machine::{run_root_storage, FrameStorageExt};
    use crate::machine::{BindingIndex, KErrorKind};

    /// The binder name comes off the `Identifier` name part — a module binds value-side, so the
    /// submit-time placeholder is tagged `Value`.
    #[test]
    fn binder_name_extracts_module_name() {
        let expr = parse_one("MODULE foo = (LET x = 1)");
        let name = crate::builtins::identifier_part_binder_name(&expr);
        assert_eq!(name.as_deref(), Some("foo"));
    }

    /// A Type-token module name is refused by the second overload, whose only job is the
    /// respelling diagnostic — a module is a value, and the Type-token namespace names what can
    /// type a field.
    #[test]
    fn type_token_module_name_errors_with_the_snake_case_respelling() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        let err = test_run.run_one_err(parse_one("MODULE IntOrd = (LET x = 1)"));
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
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        let err = test_run.run_one_err(parse_one(
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
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("MODULE foo = (LET x = 1)");
        assert!(
            matches!(scope.bindings().data().get("foo").map(|(o, _, _)| *o),
                Some(KObject::Module(m)) if m.path == "foo"),
            "MODULE binds the module value on the value channel",
        );
        assert!(
            scope.resolve_type("foo").is_none(),
            "a module is a value — nothing lands in `types`",
        );
    }

    #[test]
    fn bare_module_name_surfaces_as_object_value() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("MODULE foo = (LET x = 1)");
        // A module named in expression position reads back on the value channel's Object arm.
        let bare = test_run.run_one(parse_one("foo"));
        match bare {
            KObject::Module(module) => assert_eq!(module.path, "foo"),
            other => panic!(
                "bare module name must read back as an Object-arm module value, got {}",
                other.ktype().name(&test_run.types)
            ),
        }
        // PRINT returns the rendered string — a bare module renders as its path.
        let printed = test_run.run_one(parse_one("PRINT foo"));
        match printed {
            KObject::KString(s) => assert_eq!(s, "foo"),
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
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("MODULE int_ord = (LET compare = 7)");
        let listed = test_run.run_one(parse_one("[int_ord, int_ord]"));
        match listed {
            KObject::List(items, elem) => {
                // Ruling 12: a module's self-sig renders structurally, not by the module name.
                assert_eq!(
                    elem.name(&test_run.types),
                    ":(LIST OF SIG (compare: Number))",
                    "the memoized element type is the module self-sig"
                );
                assert_eq!(items.len(), 2);
                assert!(
                    items.iter().all(
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
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run(
            "SIG Ordered = (VAL compare :Number)\n\
             MODULE int_ord = (LET compare = 7)",
        );
        // A parenthesized module expression evaluates to the Object-arm module value, so the list
        // element is `Held::Object` memoized as the module's self-sig, which (ruling 12) renders
        // structurally as `SIG (compare: Number)` rather than by the module name.
        let listed = test_run.run_one(parse_one("[(int_ord)]"));
        match listed {
            KObject::List(items, elem) => {
                assert_eq!(
                    elem.name(&test_run.types),
                    ":(LIST OF SIG (compare: Number))",
                    "element memoizes to the module self-sig"
                );
                assert_eq!(items.len(), 1);
                assert!(
                    matches!(&items[0], Held::Object(KObject::Module(m)) if m.path == "int_ord"),
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
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("MODULE foo = (LET x = 1)");
        let result = test_run.run_one(parse_one("foo.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 1.0));
    }

    #[test]
    fn module_with_multiple_statements_in_parens() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("MODULE foo = ((LET x = 1) (LET y = 2))");
        assert!(matches!(test_run.run_one(parse_one("foo.x")), KObject::Number(n) if *n == 1.0));
        assert!(matches!(test_run.run_one(parse_one("foo.y")), KObject::Number(n) if *n == 2.0));
    }

    #[test]
    fn module_member_function_via_let_fn() {
        // `LET <name> = (FN ...)` binds under a clean identifier; bare FN lands under
        // its signature key and isn't reachable as `foo.<name>` via ATTR.
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("MODULE foo = (LET double = (FN (DOUBLE x :Number) -> Number = (x)))");
        let foo = lookup_module(scope, "foo", &test_run.types);
        assert!(foo.child_scope().bindings().data().contains_key("double"));
    }

    #[test]
    fn module_unknown_member_errors() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("MODULE foo = (LET x = 1)");
        let err = test_run.run_one_err(parse_one("foo.bogus"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("foo") && msg.contains("`bogus`")),
            "expected ShapeError naming foo and bogus, got {err}",
        );
    }

    #[test]
    fn nested_module_accessible_via_chained_attr() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("MODULE outer =\n  MODULE inner = (LET x = 7)");
        let result = test_run.run_one(parse_one("outer.inner.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// MODULE body parks on an outer-scheduler placeholder for a sibling forward
    /// reference instead of erroring as `UnboundName`.
    #[test]
    fn module_body_parks_on_outer_placeholder() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("LET y = 7\nMODULE foo = (LET x = y)");
        let result = test_run.run_one(parse_one("foo.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// A failing body statement must not bind `foo` in the parent scope.
    #[test]
    fn module_body_error_short_circuits_finalize() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
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
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        let child = region
            .brand()
            .alloc_scope(crate::machine::Scope::child_under_module(
                scope,
                "foo".into(),
            ));
        let module: &Module<'_> = region
            .brand()
            .alloc_module(Module::new("foo".into(), child));
        // Every mint seals its self-sig (2d eager-seal invariant), so a manually pre-seeded
        // module seals its (empty) interface before it is bound and its `ktype()` is read.
        module.seal_self_sig(SigSchema::raw_self_sig(module), &test_run.types);
        scope
            .bind_module(
                "foo".into(),
                module,
                child,
                BindingIndex::value(0),
                &test_run.types,
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
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("LET y = 7\nMODULE foo = ((LET x = y) (LET z = 11))");
        let foo = lookup_module(scope, "foo", &test_run.types);
        let inner = foo.child_scope().bindings().data();
        assert!(
            matches!(inner.get("x").map(|(o, _, _)| *o), Some(KObject::Number(n)) if *n == 7.0)
        );
        assert!(
            matches!(inner.get("z").map(|(o, _, _)| *o), Some(KObject::Number(n)) if *n == 11.0)
        );
    }
}
