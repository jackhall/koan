//! `MODULE <name:TypeExprRef> = <body:KExpression>` — declare a structure (a bundle of
//! type definitions, values, and functions). See
//! [design/module-system.md](../../../design/module-system.md) for the surface design.
//!
//! Construction shape: the body is a parens-wrapped KExpression. Each top-level
//! `Expression` part inside the body is dispatched as an independent statement against a
//! fresh child scope on the OUTER scheduler — so a body statement referencing a name from
//! an earlier sibling top-level binding parks on the outer placeholder the same way any
//! other forward reference does. The body schedules a `Combine` whose finish closure
//! captures the child scope into a [`Module`] value (`name`, `child_scope`, `type_members`
//! initially empty), allocates it in the parent's arena, and binds it under the module's
//! name in the parent. Members reachable as `Foo.<member>` go through ATTR's `KModule`
//! overload (see `attr.rs`), which looks `<member>` up in the captured
//! `child_scope.bindings().data()`.
//!
//! The MODULE slot itself returns `BodyResult::DeferTo(combine_id)` so its terminal lifts
//! off the Combine's terminal — the parent's `Foo` binding lands at Combine-finish time,
//! not when MODULE's body returns to the dispatcher.

use crate::runtime::machine::model::{KObject, KType};
use crate::runtime::machine::{ArgumentBundle, BodyResult, CombineFinish, Frame, KError, KErrorKind, Scope, SchedulerHandle};
use crate::runtime::machine::model::types::UserTypeKind;
use crate::runtime::machine::model::values::Module;

use crate::runtime::machine::model::ast::KExpression;

use crate::runtime::machine::core::kfunction::argument_bundle::{extract_bare_type_name, extract_kexpression};
use super::{arg, err, kw, register_builtin_with_pre_run, sig};

pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // The name slot is `KType::TypeExprRef` — module names use the type-token shape
    // (`MODULE Foo`, `MODULE OrderedSig` would be a SIG, not a MODULE; the ascription
    // result is what's `OrderedSig`). Reject parameterized forms — module names are bare
    // leaves until functors land in stage 2.
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

    // Plan each top-level body statement onto the outer scheduler. A statement referencing
    // a sibling name dispatched in the same batch parks on its placeholder via the standard
    // notify-walk; the inner-scheduler version of this code couldn't see those placeholders.
    let deps = sched.plan_body_statements(child_scope, body_expr);

    // The closure runs on the outer scheduler's main loop after every body statement has
    // terminalized. `name` is moved in by clone so it lives across the closure's life.
    //
    // Capture the active per-call frame at MODULE-dispatch time so a functor body's
    // `MODULE Result = (...)` can attach the frame's `Rc` to the produced `KModule`. The
    // captured frame keeps `child_scope`'s arena alive even after the FN's call frame
    // would otherwise drop. For top-level MODULEs there's no active frame; the produced
    // `KModule(_, None)` matches the existing behavior.
    let active_frame = sched.current_frame();
    let name_for_finish = name.clone();
    let finish: CombineFinish<'a> = Box::new(move |parent_scope, _sched, _results| {
        // Idempotent-finalize guard (stage 3.2 defense-in-depth). MODULE does not
        // participate in `pending_types`, so the guard never fires through cycle-close
        // today — but if a future refactor lets MODULE join the SCC sweep, this is the
        // entry point that needs to short-circuit. Pinned by
        // `module_finalize_is_idempotent_when_both_maps_populated`.
        let bindings = parent_scope.bindings();
        if bindings.types().get(&name_for_finish).is_some() {
            if let Some(existing) = bindings.data().get(&name_for_finish).copied() {
                return BodyResult::Value(existing);
            }
        }
        let arena = parent_scope.arena;
        let module: &'a Module<'a> =
            arena.alloc_module(Module::new(name_for_finish.clone(), child_scope));
        // Mirror the child scope's type-class bindings into the module's `type_members`
        // table so abstract-type slots declared in the body surface to dispatch-time
        // sharing-constraint checks. `LET Elt = Number` inside a MODULE body writes
        // `bindings.types["Elt"] = KType::Number` *only* (the `register_type` path skips
        // `data`); nominal sub-declarations like `MODULE Inner = ...` dual-write both
        // `types` and `data` via `register_nominal`. The filter below picks only entries
        // that LIVE on the type side without a value-side counterpart — those are the
        // pure type-class bindings the module's surface treats as abstract-type members.
        // Nominal sub-declarations stay value-only-from-ATTR's-perspective (ATTR's
        // `type_members` lookup runs ahead of the `data` lookup, so a type_members
        // entry would shadow the value-side `KModule` carrier on chained `Outer.Inner.x`
        // access — that ordering breaks unless we exclude the dual-bound names here).
        // Without this mirror the module's `type_members` stays empty and a FN-return-
        // type `(SIG_WITH SetSig ((Elt: Number)))` pin can't admit the returned module.
        // Opaque ascription overwrites the affected entries with freshly-minted
        // `UserType { kind: Module, .. }` identities (see `ascribe.rs::body_opaque`);
        // the body-side concrete values flow through unascribed and `:!` (transparent)
        // paths.
        {
            let types_guard = child_scope.bindings().types();
            let data_guard = child_scope.bindings().data();
            let mut tm = module.type_members.borrow_mut();
            for (k, v) in types_guard.iter() {
                if data_guard.contains_key(k) {
                    continue;
                }
                tm.insert(k.clone(), (**v).clone());
            }
        }
        let module_obj: &'a KObject<'a> =
            arena.alloc_object(KObject::KModule(module, active_frame.clone()));
        // Dual-write the module's per-declaration identity into `bindings.types`
        // alongside the value-side carrier so a type-class slot typed by `name_for_finish`
        // resolves to the same `KType::UserType { kind: Module, scope_id, name }` the
        // carrier's `ktype()` synthesizes.
        let identity = KType::UserType {
            kind: UserTypeKind::Module,
            scope_id: module.scope_id(),
            name: name_for_finish.clone(),
        };
        match parent_scope.register_nominal(name_for_finish.clone(), identity, module_obj) {
            Ok(obj) => BodyResult::Value(obj),
            Err(e) => BodyResult::Err(e.with_frame(Frame {
                function: "<module>".to_string(),
                expression: format!("MODULE {} body", name_for_finish),
            })),
        }
    });
    let combine_id = sched.add_combine(deps, scope, finish);
    BodyResult::DeferTo(combine_id)
}

/// Dispatch-time placeholder extractor for MODULE. `parts[1]` is the `Type(t)` token of the
/// module's name slot. Same shape as STRUCT / SIG / named UNION.
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    expr.binder_name_from_type_part()
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin_with_pre_run(
        scope,
        "MODULE",
        sig(KType::AnyUserType { kind: UserTypeKind::Module }, vec![
            kw("MODULE"),
            arg("name", KType::TypeExprRef),
            kw("="),
            arg("body", KType::KExpression),
        ]),
        body,
        Some(pre_run),
    );
}

#[cfg(test)]
mod tests {
    use crate::runtime::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::runtime::machine::model::KObject;
    use crate::runtime::machine::{KErrorKind, RuntimeArena};

    /// Smoke test for MODULE's pre_run extractor: structural extraction of the `Type(_)`
    /// token at `parts[1]`.
    #[test]
    fn pre_run_extracts_module_name() {
        let expr = parse_one("MODULE Foo = (LET x = 1)");
        let name = super::pre_run(&expr);
        assert_eq!(name.as_deref(), Some("Foo"));
    }

    #[test]
    fn module_binds_under_name_in_scope() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "MODULE Foo = (LET x = 1)");
        let data = scope.bindings().data();
        assert!(matches!(data.get("Foo"), Some(KObject::KModule(_, _))));
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
        // Multi-statement bodies use parens with statements separated by commas (which the
        // whitespace pass collapses to whitespace inside expression frames). The parser
        // wraps each statement in an Expression sub-part of the body slot, and MODULE's
        // body-runner dispatches each Expression in the child scope.
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
        // Module member functions must use `LET <name> = (FN ...)` to bind
        // under a clean identifier. Bare FN inside a MODULE body lands under the FN's
        // signature key, not under an identifier — accessible only via dispatch from inside
        // the module body, not via `Foo.<name>`.
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE Foo = (LET double = (FN (DOUBLE x :Number) -> Number = (x)))",
        );
        let data = scope.bindings().data();
        let foo = match data.get("Foo") {
            Some(KObject::KModule(m, _)) => *m,
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

    /// Body-statement forward-reference test: a MODULE body's `LET x = y` references a
    /// name `y` bound by a sibling top-level statement in the same batch. Pre-refactor the
    /// MODULE body ran on a fresh inner scheduler that couldn't see the outer placeholder
    /// for `y`, so this would surface `UnboundName`. Post-refactor body statements
    /// dispatch on the outer scheduler and park on the placeholder like any other forward
    /// reference.
    #[test]
    fn module_body_parks_on_outer_placeholder() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET y = 7\nMODULE Foo = (LET x = y)");
        let result = run_one(scope, parse_one("Foo.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// Failing body statement (unbound name) must surface as the MODULE node's error and
    /// must NOT bind `Foo` in the parent scope. Pins the Combine's short-circuit for the
    /// binder-finalize path.
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

    /// MODULE finalize is idempotent when both `bindings.types[name]` and
    /// `bindings.data[name]` are already populated. MODULE does not participate in
    /// `pending_types` today (per roadmap stage 3.2 decision), so the guard is
    /// dormant — but pinned here so a future SCC extension that adds MODULE cannot
    /// silently regress the short-circuit. Exercises the guard by running MODULE
    /// twice with the same name and observing the second dispatch errors as
    /// `Rebind` (the guard only fires when both maps are populated; the second
    /// dispatch errors before reaching it). The DIRECT-call shape of the guard is
    /// covered by `crate::runtime::machine::core::bindings`'s
    /// `try_register_nominal_*` tests plus the idempotent register_nominal arm.
    #[test]
    fn module_finalize_short_circuits_on_idempotent_state() {
        use crate::runtime::machine::model::types::{KType, UserTypeKind};
        use crate::runtime::machine::model::values::Module;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let child = arena.alloc_scope(crate::runtime::machine::Scope::child_under_module(
            scope,
            "Foo".into(),
        ));
        let module: &Module<'_> = arena.alloc_module(Module::new("Foo".into(), child));
        let module_obj = arena.alloc_object(KObject::KModule(module, None));
        let identity = KType::UserType {
            kind: UserTypeKind::Module,
            scope_id: module.scope_id(),
            name: "Foo".into(),
        };
        scope
            .register_nominal("Foo".into(), identity, module_obj)
            .unwrap();
        // Re-dispatching `MODULE Foo = (...)` against this scope errors at the
        // placeholder install before reaching the Combine-finish guard — `Foo` is
        // already bound. This pins the surface behavior: name-already-bound errors
        // surface as `Rebind`, the guard never has to handle a same-name re-dispatch.
        run(scope, "MODULE Foo = (LET y = 2)");
        // Foo's data binding still points at the pre-seeded module pointer (re-dispatch
        // did not overwrite it).
        let data = scope.bindings().data();
        let foo = data.get("Foo").copied().expect("Foo still bound");
        assert!(std::ptr::eq(foo, module_obj));
    }

    /// Miri audit-slate: pins the MODULE body's Combine continuation closure under tree
    /// borrows. The closure captures `child_scope: &'a Scope<'a>` and a `String` name, runs
    /// on the outer scheduler's main loop after every body statement terminalizes, and
    /// builds a `Module` over the captured scope. The captured-reference and finalize-write
    /// shapes here are the post-refactor analogue of the `module_child_scope_transmute_does_not_dangle`
    /// site — exercise them through the actual Combine path, not just the `Module` constructor.
    #[test]
    fn module_body_dispatch_does_not_dangle() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET y = 7\nMODULE Foo = ((LET x = y) (LET z = 11))");
        let data = scope.bindings().data();
        let foo = match data.get("Foo") {
            Some(KObject::KModule(m, _)) => *m,
            _ => panic!("Foo should be a module"),
        };
        let inner = foo.child_scope().bindings().data();
        assert!(matches!(inner.get("x"), Some(KObject::Number(n)) if *n == 7.0));
        assert!(matches!(inner.get("z"), Some(KObject::Number(n)) if *n == 11.0));
    }
}
