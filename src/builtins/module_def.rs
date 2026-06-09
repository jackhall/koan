//! `MODULE <name:TypeExprRef> = <body:KExpression>` — declare a structure (a bundle of
//! type definitions, values, and functions). See
//! [design/typing/modules.md](../../design/typing/modules.md) for the surface design.
//!
//! Body statements dispatch on the OUTER scheduler against a fresh child scope, so a
//! body statement referencing an earlier sibling at the same outer block parks on the
//! outer placeholder like any other forward reference. The MODULE slot returns
//! `BodyResult::DeferTo(combine_id)` so the parent binding lands at Combine-finish,
//! not when MODULE's body returns to the dispatcher.

use crate::machine::model::types::KKind;
use crate::machine::model::values::Module;
use crate::machine::model::KType;
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CombineFinish, Frame, SchedulerHandle, Scope,
};

use super::{arg, err, kw, register_builtin_with_binder, sig};
use crate::machine::core::kfunction::argument_bundle::extract_bare_type_name;

pub fn body<'a, 's>(
    sched: &mut dyn SchedulerHandle<'a, 's>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // Parameterized name forms are rejected — module names are bare leaves.
    let name = match extract_bare_type_name(&bundle, "name", "MODULE") {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    let body_expr = match bundle.extract_kexpression_or_shape_error("MODULE", "body") {
        Ok(e) => e,
        Err(e) => return err(e),
    };

    let arena = sched.current_scope().arena;
    let child_scope = arena.alloc_scope(Scope::child_under_module(
        sched.current_scope(),
        name.clone(),
    ));

    let deps = sched.enter_body_block(child_scope, body_expr);

    // Capture the active per-call frame for the produced KModule's anchor; see
    // per-call-arena-protocol.md § Carriers and § Outer-frame chain.
    let active_frame = sched.current_frame();
    // Non-nominal: the MODULE name obeys source order like any other type name.
    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::value(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);
    let name_for_finish = name.clone();
    let finish: CombineFinish<'a> = Box::new(move |_sched, _results| {
        // Idempotent-finalize guard: short-circuits if a future SCC-sweep extension
        // re-enters MODULE finalize against an already-bound name. MODULE is type-only,
        // so the guard reads `types` (the carrier in `data` is gone).
        let bindings = _sched.current_scope().bindings();
        if let Some(kt) = bindings.lookup_type(&name_for_finish, None) {
            return BodyResult::ktype(_sched.current_scope().arena.alloc_ktype(kt.clone()));
        }
        let arena = _sched.current_scope().arena;
        let module: &'a Module<'a> =
            arena.alloc_module(Module::new(name_for_finish.clone(), child_scope));
        // Mirror pure type-side bindings (entries without a value-side counterpart) into
        // the module's `type_members` so abstract-type slots surface to dispatch-time
        // sharing-constraint checks. Names with both `types` and `data` entries (nominal
        // sub-declarations) are excluded — ATTR's `type_members` lookup runs ahead of
        // its `data` lookup, so mirroring them would shadow chained `Outer.Inner.x`
        // access via the `KModule` value-side carrier.
        {
            let bindings = child_scope.bindings();
            let data_names: std::collections::HashSet<String> =
                bindings.iter_data().into_iter().map(|(n, _)| n).collect();
            let mut tm = module.type_members.borrow_mut();
            for (name, kt) in bindings.iter_types() {
                if data_names.contains(&name) {
                    continue;
                }
                tm.insert(name, kt.clone());
            }
        }
        let identity = KType::Module {
            module,
            frame: active_frame.clone(),
        };
        // Type-only install: the module's identity (carrying its `&Module` and per-call
        // frame anchor) lives in `bindings.types`; ATTR access recovers the value-side
        // `KTypeValue(Module)` via `resolve_type_leaf_carrier`. MODULE doesn't join an SCC
        // type cycle (bodies park on the outer scheduler), so the upsert's overwrite arm
        // never fires for a module — its insert-if-absent / non-equal-Rebind behaviour is
        // what carries here, sharing the one nominal-finalize primitive.
        let _ = arena;
        match _sched.current_scope().register_type_upsert(
            name_for_finish.clone(),
            identity.clone(),
            bind_index,
        ) {
            Ok(kt_ref) => {
                BodyResult::ktype(_sched.current_scope().arena.alloc_ktype(kt_ref.clone()))
            }
            Err(e) => BodyResult::Err(e.with_frame(Frame::bare(
                "<module>",
                format!("MODULE {} body", name_for_finish),
            ))),
        }
    });
    let combine_id = sched.add_combine_here(deps, vec![], finish);
    BodyResult::DeferTo(combine_id)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin_with_binder(
        scope,
        "MODULE",
        sig(
            KType::OfKind(KKind::Module),
            vec![
                kw("MODULE"),
                arg("name", KType::OfKind(KKind::Proper)),
                kw("="),
                arg("body", KType::KExpression),
            ],
        ),
        body,
        Some(super::type_part_binder_name),
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

    /// Miri audit-slate: exercises the MODULE Combine continuation's captured
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
