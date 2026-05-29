//! `MODULE <name:TypeExprRef> = <body:KExpression>` — declare a structure (a bundle of
//! type definitions, values, and functions). See
//! [design/typing/modules.md](../../design/typing/modules.md) for the surface design.
//!
//! Body statements dispatch on the OUTER scheduler against a fresh child scope, so a
//! body statement referencing an earlier sibling at the same outer block parks on the
//! outer placeholder like any other forward reference. The MODULE slot returns
//! `BodyResult::DeferTo(combine_id)` so the parent binding lands at Combine-finish,
//! not when MODULE's body returns to the dispatcher.

use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CombineFinish, Frame, KError, KErrorKind, Resolution,
    Scope, SchedulerHandle,
};
use crate::machine::model::values::Module;

use crate::machine::model::ast::KExpression;

use crate::machine::core::kfunction::argument_bundle::{extract_bare_type_name, extract_kexpression};
use super::{arg, err, kw, register_nominal_binder, sig};

pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // Parameterized name forms are rejected — module names are bare leaves.
    let name = match extract_bare_type_name(&bundle, "name", "MODULE") {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    let body_expr = match extract_kexpression(&mut bundle, "body") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "MODULE body slot must be a parenthesized expression".to_string(),
            )));
        }
    };

    let arena = scope.arena;
    let child_scope = arena.alloc_scope(Scope::child_under_module(scope, name.clone()));

    let deps = sched.enter_body_block(child_scope, body_expr);

    // Capture the active per-call frame so a functor body's `MODULE Result = (...)` can
    // attach the frame's `Rc` to the produced `KModule`, keeping `child_scope`'s arena
    // alive past the FN call frame. Top-level MODULEs have no active frame.
    let active_frame = sched.current_frame();
    // D7 nominal-binder carve-out: siblings see one another regardless of source order.
    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::nominal(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);
    let name_for_finish = name.clone();
    let finish: CombineFinish<'a> = Box::new(move |parent_scope, _sched, _results| {
        // Idempotent-finalize guard: short-circuits if a future SCC-sweep extension
        // re-enters MODULE finalize against an already-bound name.
        let bindings = parent_scope.bindings();
        if bindings.lookup_type(&name_for_finish, None).is_some() {
            if let Some(Resolution::Value(existing)) =
                bindings.lookup_value(&name_for_finish, None)
            {
                return BodyResult::Value(existing);
            }
        }
        let arena = parent_scope.arena;
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
        let module_obj: &'a KObject<'a> =
            arena.alloc(KObject::KTypeValue(identity.clone()));
        match parent_scope.register_nominal(
            name_for_finish.clone(),
            identity,
            module_obj,
            bind_index,
        ) {
            Ok(obj) => BodyResult::Value(obj),
            Err(e) => BodyResult::Err(
                e.with_frame(Frame::bare("<module>", format!("MODULE {} body", name_for_finish))),
            ),
        }
    });
    let combine_id = sched.add_combine(deps, vec![], scope, finish);
    BodyResult::DeferTo(combine_id)
}

/// Dispatch-time placeholder extractor for MODULE: `parts[1]` is the name token.
pub(crate) fn binder_name(expr: &KExpression<'_>) -> Option<String> {
    expr.binder_name_from_type_part()
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_nominal_binder(
        scope,
        "MODULE",
        sig(KType::AnyModule, vec![
            kw("MODULE"),
            arg("name", KType::TypeExprRef),
            kw("="),
            arg("body", KType::KExpression),
        ]),
        body,
        Some(binder_name),
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::machine::model::{KObject, KType};
    use crate::machine::{BindingIndex, KErrorKind, RuntimeArena};

    #[test]
    fn binder_name_extracts_module_name() {
        let expr = parse_one("MODULE Foo = (LET x = 1)");
        let name = super::binder_name(&expr);
        assert_eq!(name.as_deref(), Some("Foo"));
    }

    #[test]
    fn module_binds_under_name_in_scope() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "MODULE Foo = (LET x = 1)");
        let data = scope.bindings().data();
        assert!(matches!(
            data.get("Foo").map(|(o, _)| *o),
            Some(KObject::KTypeValue(KType::Module { .. }))
        ));
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
        run(
            scope,
            "MODULE Foo = ((LET x = 1) (LET y = 2))",
        );
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
        let data = scope.bindings().data();
        let foo = match data.get("Foo").map(|(o, _)| *o) {
            Some(KObject::KTypeValue(KType::Module { module: m, .. })) => *m,
            _ => panic!("Foo should be a module"),
        };
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
        run(
            scope,
            "MODULE Outer =\n  MODULE Inner = (LET x = 7)",
        );
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

    /// Pre-seed a name, then re-dispatching `MODULE Foo = ...` must surface as
    /// `Rebind` at placeholder install (the finalize guard never has to fire) and
    /// leave the pre-seeded pointer intact.
    #[test]
    fn module_finalize_short_circuits_on_idempotent_state() {
        use crate::machine::model::types::KType;
        use crate::machine::model::values::Module;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let child = arena.alloc_scope(crate::machine::Scope::child_under_module(
            scope,
            "Foo".into(),
        ));
        let module: &Module<'_> = arena.alloc_module(Module::new("Foo".into(), child));
        let identity = KType::Module { module, frame: None };
        let module_obj = arena.alloc(KObject::KTypeValue(identity.clone()));
        scope
            .register_nominal("Foo".into(), identity, module_obj, BindingIndex::BUILTIN)
            .unwrap();
        run(scope, "MODULE Foo = (LET y = 2)");
        let data = scope.bindings().data();
        let (foo, _) = data.get("Foo").copied().expect("Foo still bound");
        assert!(std::ptr::eq(foo, module_obj));
    }

    /// Miri audit-slate: exercises the MODULE Combine continuation's captured
    /// `child_scope: &'a Scope<'a>` and finalize writes under tree borrows.
    #[test]
    fn module_body_dispatch_does_not_dangle() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET y = 7\nMODULE Foo = ((LET x = y) (LET z = 11))");
        let data = scope.bindings().data();
        let foo = match data.get("Foo").map(|(o, _)| *o) {
            Some(KObject::KTypeValue(KType::Module { module: m, .. })) => *m,
            _ => panic!("Foo should be a module"),
        };
        let inner = foo.child_scope().bindings().data();
        assert!(matches!(inner.get("x").map(|(o, _)| *o), Some(KObject::Number(n)) if *n == 7.0));
        assert!(matches!(inner.get("z").map(|(o, _)| *o), Some(KObject::Number(n)) if *n == 11.0));
    }
}
