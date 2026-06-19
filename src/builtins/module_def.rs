//! `MODULE <name:ProperType> = <body:KExpression>` — declare a structure (a bundle of
//! type definitions, values, and functions). See
//! [design/typing/modules.md](../../design/typing/modules.md) for the surface design.
//!
//! Body statements dispatch on the OUTER scheduler against a fresh child scope, so a
//! body statement referencing an earlier sibling at the same outer block parks on the
//! outer placeholder like any other forward reference. The MODULE slot returns an
//! `Action::AwaitDeps` over those body slots, so the parent binding lands at dep-finish,
//! not when MODULE's body returns to the dispatcher.

use crate::machine::model::types::KKind;
use crate::machine::model::values::Module;
use crate::machine::model::KType;
use crate::machine::{Scope, TraceFrame};

use super::{arg, kw, sig};

/// The MODULE body: mints the child scope, dispatches the body block
/// against it (an `InScope` dep-finish dependency), and the finish installs the `KType::Module` identity into
/// the parent scope.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{
        require_bare_type_name, require_kexpression, Action, AwaitContinue, Dep, DepPlacement,
    };
    use crate::machine::model::Carried;

    let name = crate::try_action!(require_bare_type_name(ctx.args, "name", "MODULE"));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "MODULE", "body"));
    let child_scope = ctx
        .scope
        .arena
        .alloc_scope(Scope::child_under_module(ctx.scope, name.clone()));
    let active_frame = ctx.frame.map(|f| f.storage_rc());
    let bind_index = ctx.bind_index();
    let name_for_finish = name;
    let finish: AwaitContinue<'a> = Box::new(move |fctx, _results| {
        // Idempotent-finalize guard: a re-bound name short-circuits.
        if let Some(kt) = fctx.scope.bindings().lookup_type(&name_for_finish, None) {
            return Action::Done(Ok(Carried::Type(fctx.scope.arena.alloc_ktype(kt.clone()))));
        }
        let module: &'a Module<'a> = fctx
            .scope
            .arena
            .alloc_module(Module::new(name_for_finish.clone(), child_scope));
        // Mirror pure type-side bindings into the module's `type_members`.
        {
            let bindings = child_scope.bindings();
            let data_names: std::collections::HashSet<String> =
                bindings.iter_data().into_iter().map(|(n, _)| n).collect();
            let mut tm = module.type_members.borrow_mut();
            for (member, kt) in bindings.iter_types() {
                if data_names.contains(&member) {
                    continue;
                }
                tm.insert(member, kt.clone());
            }
        }
        let identity = KType::Module {
            module,
            frame: active_frame.clone(),
        };
        match fctx
            .scope
            .register_type_upsert(name_for_finish.clone(), identity, bind_index)
        {
            Ok(kt_ref) => Action::Done(Ok(Carried::Type(
                fctx.scope.arena.alloc_ktype(kt_ref.clone()),
            ))),
            Err(e) => Action::Done(Err(e.with_frame(TraceFrame::bare(
                "<module>",
                format!("MODULE {} body", name_for_finish),
            )))),
        }
    });
    Action::AwaitDeps {
        deps: vec![Dep::Dispatch {
            expr: body_expr,
            placement: DepPlacement::InScope(child_scope),
        }],
        finish,
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(
        KType::OfKind(KKind::Module),
        vec![
            kw("MODULE"),
            arg("name", KType::OfKind(KKind::ProperType)),
            kw("="),
            arg("body", KType::KExpression),
        ],
    );
    crate::builtins::register_builtin_full(
        scope,
        "MODULE",
        signature,
        body,
        Some(super::type_part_binder_name),
        None,
        false,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::machine::model::values::Module;
    use crate::machine::model::{KObject, KType};
    use crate::machine::{BindingIndex, KErrorKind, RuntimeArena, Scope};

    /// MODULE is type-only: the `&Module` rides the `KType::Module` identity in
    /// `bindings.types`. Recover it for inspection.
    fn resolve_module<'a>(scope: &'a Scope<'a>, name: &str) -> &'a Module<'a> {
        match scope.resolve_type(name) {
            Some(KType::Module { module, .. }) => module,
            other => panic!("expected {name} to be a Module identity in types, got {other:?}"),
        }
    }

    #[test]
    fn binder_name_extracts_module_name() {
        let expr = parse_one("MODULE Foo = (LET x = 1)");
        let name = expr.binder_name_from_type_part();
        assert_eq!(name.as_deref(), Some("Foo"));
    }

    #[test]
    fn module_binds_under_name_in_scope() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "MODULE Foo = (LET x = 1)");
        assert!(matches!(
            scope.resolve_type("Foo"),
            Some(KType::Module { .. })
        ));
        assert!(
            scope.bindings().data().get("Foo").is_none(),
            "MODULE is type-only — no value-side carrier in data",
        );
    }

    #[test]
    fn module_member_access_via_attr() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "MODULE Foo = (LET x = 1)");
        let result = run_one(scope, parse_one("Foo.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 1.0));
    }

    #[test]
    fn module_with_multiple_statements_in_parens() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "MODULE Foo = ((LET x = 1) (LET y = 2))");
        assert!(matches!(run_one(scope, parse_one("Foo.x")), KObject::Number(n) if *n == 1.0));
        assert!(matches!(run_one(scope, parse_one("Foo.y")), KObject::Number(n) if *n == 2.0));
    }

    #[test]
    fn module_member_function_via_let_fn() {
        // `LET <name> = (FN ...)` binds under a clean identifier; bare FN lands under
        // its signature key and isn't reachable as `Foo.<name>` via ATTR.
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE Foo = (LET double = (FN (DOUBLE x :Number) -> Number = (x)))",
        );
        let foo = resolve_module(scope, "Foo");
        assert!(foo.child_scope().bindings().data().contains_key("double"));
    }

    #[test]
    fn module_unknown_member_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "MODULE Foo = (LET x = 1)");
        let err = run_one_err(scope, parse_one("Foo.bogus"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Foo") && msg.contains("`bogus`")),
            "expected ShapeError naming Foo and bogus, got {err}",
        );
    }

    #[test]
    fn nested_module_accessible_via_chained_attr() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "MODULE Outer =\n  MODULE Inner = (LET x = 7)");
        let result = run_one(scope, parse_one("Outer.Inner.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// MODULE body parks on an outer-scheduler placeholder for a sibling forward
    /// reference instead of erroring as `UnboundName`.
    #[test]
    fn module_body_parks_on_outer_placeholder() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET y = 7\nMODULE Foo = (LET x = y)");
        let result = run_one(scope, parse_one("Foo.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// A failing body statement must not bind `Foo` in the parent scope.
    #[test]
    fn module_body_error_short_circuits_finalize() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "MODULE Foo = (LET x = nonexistent_name)");
        assert!(
            scope.bindings().data().get("Foo").is_none(),
            "Foo must not bind when its body errors",
        );
    }

    /// Pre-seed the type-only `Foo` identity, then re-dispatch `MODULE Foo = ...`. The
    /// finalize guard reads `types`, short-circuits on the existing identity, and leaves
    /// the pre-seeded `&Module` pointer intact.
    #[test]
    fn module_finalize_short_circuits_on_idempotent_state() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let child = arena.alloc_scope(crate::machine::Scope::child_under_module(
            scope,
            "Foo".into(),
        ));
        let module: &Module<'_> = arena.alloc_module(Module::new("Foo".into(), child));
        let identity = KType::Module {
            module,
            frame: None,
        };
        // Pre-seed the type-only identity, then re-run `MODULE Foo = ...`. The finalize
        // guard reads `types`, finds the pre-seeded identity, and short-circuits without
        // re-binding — the original `&Module` pointer survives.
        scope.register_type("Foo".into(), identity, BindingIndex::value(0));
        run(scope, "MODULE Foo = (LET y = 2)");
        let foo = resolve_module(scope, "Foo");
        assert!(std::ptr::eq(foo, module));
    }

    /// Miri audit-slate: exercises the MODULE dep-finish continuation's captured
    /// `child_scope: &'a Scope<'a>` and finalize writes under tree borrows.
    #[test]
    fn module_body_dispatch_does_not_dangle() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET y = 7\nMODULE Foo = ((LET x = y) (LET z = 11))");
        let foo = resolve_module(scope, "Foo");
        let inner = foo.child_scope().bindings().data();
        assert!(matches!(inner.get("x").map(|(o, _)| *o), Some(KObject::Number(n)) if *n == 7.0));
        assert!(matches!(inner.get("z").map(|(o, _)| *o), Some(KObject::Number(n)) if *n == 11.0));
    }
}
