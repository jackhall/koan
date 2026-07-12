//! `MODULE <name:ProperType> = <body:KExpression>` — declare a structure (a bundle of
//! type definitions, values, and functions). See
//! [design/typing/modules.md](../../design/typing/modules.md) for the surface design.
//!
//! Body statements dispatch on the OUTER scheduler against a fresh child scope via
//! [`await_body_in_scope`](super::await_body::await_body_in_scope), so a body statement
//! referencing an earlier sibling at the same outer block parks on the outer placeholder
//! like any other forward reference, and the parent binding lands at dep-finish, not when
//! MODULE's body returns to the dispatcher.

use crate::machine::execute::StepCarried;
use crate::machine::model::types::{KKind, SigSchema};
use crate::machine::model::values::Module;
use crate::machine::model::KType;
use crate::machine::{NameLookup, Scope, TraceFrame};

use super::{arg, kw, sig};

/// The MODULE body: mints the child scope, dispatches the body block against it via
/// [`await_body_in_scope`](super::await_body::await_body_in_scope), and the finish installs
/// the `KType::Module` identity into the parent scope.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use super::await_body::{await_body_in_scope, ChildScopeSeal};
    use crate::machine::core::kfunction::action::{
        require_bare_type_name, require_kexpression, Action,
    };

    let name = crate::try_action!(require_bare_type_name(ctx.args, "name", "MODULE"));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "MODULE", "body"));
    let child_scope = ctx
        .scope
        .brand()
        .alloc_scope(Scope::child_under_module(ctx.scope, name.clone()));
    let bind_index = ctx.bind_index();
    let name_for_finish = name;
    await_body_in_scope(
        child_scope,
        body_expr,
        ChildScopeSeal::SealBeforeFinish,
        move |fctx| {
            // Idempotent-finalize guard: a re-bound name short-circuits, surfacing the
            // already-installed identity from its **stored** reach as the Object-arm module value.
            if let Some(NameLookup::Bound(hit)) = fctx
                .scope
                .bindings()
                .lookup_type_carrier(&name_for_finish, None)
            {
                return Action::Done(Ok(StepCarried::born(
                    fctx.scope.surface_type_hit(hit.kt, hit.stored),
                )));
            }
            let module: &'a Module<'a> = fctx
                .scope
                .brand()
                .alloc_module(Module::new(name_for_finish.clone(), child_scope));
            // Mirror the module's type-side bindings into `type_members`. The cross-kind exclusion
            // keeps `data` and `types` disjoint by name, so this is an exact mirror of `iter_types`
            // (no value-member name can also be a type name to filter out).
            {
                let mut tm = module.type_members.borrow_mut();
                for (member, kt) in child_scope.bindings().iter_types() {
                    tm.insert(member, kt.clone());
                }
            }
            // Seal the module's self-sig now that `type_members` reflects the body — a plain
            // module carries no SIG, so the raw derivation is the whole signature.
            module.seal_self_sig(SigSchema::raw_self_sig(module));
            let identity = KType::Module { module };
            // Fused MODULE-finish upsert: the module's stored reach is derived off the child scope held
            // **directly** here (never by walking the built `KType::Module`) — the home-borrow bit
            // included, `true` because the same-region child's own region owner covers this scope's
            // region before home-omission — then upsert-installed under it. The install stays
            // type-side (`bindings.types`); the returned terminal surfaces the same identity as the
            // Object-arm module value from that stored reach.
            match fctx.scope.register_module_upsert(
                name_for_finish.clone(),
                identity,
                child_scope,
                bind_index,
            ) {
                Ok((kt_ref, stored)) => Action::Done(Ok(StepCarried::born(
                    fctx.scope.surface_type_hit(kt_ref, stored),
                ))),
                Err(e) => Action::Done(Err(e.with_frame(TraceFrame::bare(
                    "<module>",
                    format!("MODULE {} body", name_for_finish),
                )))),
            }
        },
    )
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(
        KType::empty_signature(),
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
        Some((super::type_part_binder_name, crate::machine::BindKind::Type)),
        None,
        false,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    use crate::machine::model::values::Module;
    use crate::machine::model::{KObject, KType};
    use crate::machine::{BindingIndex, KErrorKind, Scope};

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
        let region = run_root_storage();
        let scope = run_root_silent(&region);
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
    fn bare_module_name_surfaces_as_object_value() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "MODULE Foo = (LET x = 1)");
        // A module named in expression position surfaces on the value channel's Object arm.
        match run_one(scope, parse_one("Foo")) {
            KObject::Module(module) => assert_eq!(module.path, "Foo"),
            other => panic!(
                "bare module name must surface as an Object-arm module value, got {}",
                other.ktype().name()
            ),
        }
        // The binding itself stays type-side (Decision: binding doors install `KType::Module`).
        assert!(matches!(
            scope.resolve_type("Foo"),
            Some(KType::Module { .. })
        ));
        assert!(
            scope.bindings().data().get("Foo").is_none(),
            "the module's value channel is a read-time surfacing, not a data binding",
        );
        // PRINT returns the rendered string — a bare module renders as its path.
        match run_one(scope, parse_one("PRINT Foo")) {
            KObject::KString(s) => assert_eq!(s, "Foo"),
            other => panic!(
                "PRINT Foo returns the path string, got {}",
                other.ktype().name()
            ),
        }
    }

    #[test]
    fn module_in_list_surfaces_as_object_element_memoized_to_self_sig() {
        use crate::machine::model::values::Held;
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(
            scope,
            "SIG OrderedSig = (VAL compare :Number)\n\
             MODULE IntOrd = (LET compare = 7)",
        );
        // A parenthesized module expression evaluates to the Object-arm module value, so the list
        // element is `Held::Object` memoized as the module's self-sig (`Signature{SelfOf}`, whose
        // name renders as the module path).
        match run_one(scope, parse_one("[(IntOrd)]")) {
            KObject::List(items, elem) => {
                assert_eq!(
                    elem.name(),
                    "IntOrd",
                    "element memoizes to the module self-sig"
                );
                assert_eq!(items.len(), 1);
                assert!(
                    matches!(&items[0], Held::Object(KObject::Module(m)) if m.path == "IntOrd"),
                    "the list element is the Object-arm module value",
                );
            }
            other => panic!("expected a list, got {}", other.ktype().name()),
        }
    }

    #[test]
    fn module_member_access_via_attr() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "MODULE Foo = (LET x = 1)");
        let result = run_one(scope, parse_one("Foo.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 1.0));
    }

    #[test]
    fn module_with_multiple_statements_in_parens() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "MODULE Foo = ((LET x = 1) (LET y = 2))");
        assert!(matches!(run_one(scope, parse_one("Foo.x")), KObject::Number(n) if *n == 1.0));
        assert!(matches!(run_one(scope, parse_one("Foo.y")), KObject::Number(n) if *n == 2.0));
    }

    #[test]
    fn module_member_function_via_let_fn() {
        // `LET <name> = (FN ...)` binds under a clean identifier; bare FN lands under
        // its signature key and isn't reachable as `Foo.<name>` via ATTR.
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(
            scope,
            "MODULE Foo = (LET double = (FN (DOUBLE x :Number) -> Number = (x)))",
        );
        let foo = resolve_module(scope, "Foo");
        assert!(foo.child_scope().bindings().data().contains_key("double"));
    }

    #[test]
    fn module_unknown_member_errors() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
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
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "MODULE Outer =\n  MODULE Inner = (LET x = 7)");
        let result = run_one(scope, parse_one("Outer.Inner.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// MODULE body parks on an outer-scheduler placeholder for a sibling forward
    /// reference instead of erroring as `UnboundName`.
    #[test]
    fn module_body_parks_on_outer_placeholder() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "LET y = 7\nMODULE Foo = (LET x = y)");
        let result = run_one(scope, parse_one("Foo.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// A failing body statement must not bind `Foo` in the parent scope.
    #[test]
    fn module_body_error_short_circuits_finalize() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
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
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let child = region
            .brand()
            .alloc_scope(crate::machine::Scope::child_under_module(
                scope,
                "Foo".into(),
            ));
        let module: &Module<'_> = region
            .brand()
            .alloc_module(Module::new("Foo".into(), child));
        let identity = KType::Module { module };
        // Pre-seed the type-only identity, then re-run `MODULE Foo = ...`. The finalize
        // guard reads `types`, finds the pre-seeded identity, and short-circuits without
        // re-binding — the original `&Module` pointer survives.
        scope.register_builtin_type("Foo".into(), identity, BindingIndex::value(0));
        run(scope, "MODULE Foo = (LET y = 2)");
        let foo = resolve_module(scope, "Foo");
        assert!(std::ptr::eq(foo, module));
    }

    /// Miri audit-slate: exercises the MODULE dep-finish continuation's captured
    /// `child_scope: &'a Scope<'a>` and finalize writes under tree borrows.
    #[test]
    fn module_body_dispatch_does_not_dangle() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "LET y = 7\nMODULE Foo = ((LET x = y) (LET z = 11))");
        let foo = resolve_module(scope, "Foo");
        let inner = foo.child_scope().bindings().data();
        assert!(
            matches!(inner.get("x").map(|(o, _, _)| *o), Some(KObject::Number(n)) if *n == 7.0)
        );
        assert!(
            matches!(inner.get("z").map(|(o, _, _)| *o), Some(KObject::Number(n)) if *n == 11.0)
        );
    }
}
