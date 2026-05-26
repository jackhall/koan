use crate::machine::model::KObject;
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};

use crate::builtins::err;

/// `MODULE_TYPE_OF <m:Module> <name>` → `TypeExprRef` carrying the abstract type bound
/// under `name` in `m`'s `type_members` table. Surface analogue of `M.Type`, but reachable
/// as a scheduled call so a functor body can synthesize it from a parameter module value.
/// The `m` slot is strictly `Module`; bare Type-token operands (`MODULE_TYPE_OF Foo Type`)
/// ride the auto-wrap rails — they sub-dispatch through `value_lookup` and arrive here
/// as a `Future(KModule)`. The shared [`crate::machine::model::values::resolve_module`] helper
/// covers both the direct `KModule` path and the `(KModule, frame)` lifted form.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let m = match bundle.require_module("m") {
        Ok(m) => m,
        Err(e) => return err(e),
    };
    // The `name` slot accepts a Type token (e.g. `Type`, `Elt`) — abstract type names
    // classify as Type per the token-classification rules, not Identifier. The lookup uses
    // the bare leaf name from the resolved `KType`.
    let name = match bundle.require_ktype("name") {
        Ok(t) => t.name(),
        Err(e) => return err(e),
    };
    // Pull the abstract type's concrete `KType` (post-3.1: `KType::UserType { kind:
    // Module, .. }` minted by opaque ascription) out of the `type_members` table directly
    // so the consumer downstream sees the identity-bearing variant rather than a
    // re-elaborated leaf.
    let kt = match m.type_members.borrow().get(&name).cloned() {
        Some(kt) => kt,
        None => {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "module `{}` has no abstract type member `{}`",
                m.path, name
            ))));
        }
    };
    BodyResult::Value(scope.arena.alloc(KObject::KTypeValue(kt)))
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
    use crate::machine::execute::Scheduler;
    use crate::machine::model::{KObject, KType};
    use crate::machine::RuntimeArena;

    /// `(MODULE_TYPE_OF M Type)` reads the `Type` slot from a module's `type_members`
    /// table. Sets up an opaquely-ascribed module so `Type` is bound, then verifies the
    /// builtin returns a `KTypeValue` whose `KType::UserType { kind: Module, .. }`
    /// carries the abstract type's identity.
    #[test]
    fn module_type_of_resolves_via_module_member() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = ((LET Type = Number) (LET compare = 0))\n\
             SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
             LET Mod = (IntOrd :| OrderedSig)",
        );
        let result = run_one(scope, parse_one("MODULE_TYPE_OF Mod Type"));
        match result {
            KObject::KTypeValue(kt) => {
                // Post-collapse: opaque-ascription abstract-type members live as
                // `KType::AbstractType { source_module, name }`. Surface name is `Type`.
                assert_eq!(kt.name(), "Type");
                assert!(matches!(kt, KType::AbstractType { .. }));
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// MODULE_TYPE_OF on a module without that abstract member produces a clean
    /// `ShapeError` naming the module and the missing member.
    #[test]
    fn module_type_of_unknown_member_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "MODULE Foo = (LET x = 1)");
        // `Foo` is a Type token; the TypeExprRef-lhs overload looks it up against the
        // surrounding scope. `Bogus` is also a Type token naming a nonexistent abstract
        // member.
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("MODULE_TYPE_OF Foo Bogus"), scope);
        sched.execute().expect("scheduler runs to completion");
        let res = sched.read_result(id);
        assert!(res.is_err(), "expected MODULE_TYPE_OF on missing member to err");
    }

    /// Miri audit-slate: pins type-op dispatch through the per-call arena under tree
    /// borrows. A functor body invokes `(MODULE_TYPE_OF Er Type)` on its per-call
    /// parameter; `body` allocates the resulting `KTypeValue` into the per-call scope's
    /// arena. The returned `KModule` plus the bound type member must survive subsequent
    /// arena churn — the per-call-arena reclamation + lift machinery have to keep storage
    /// live for both the module pointer and the dispatched type-op value. Mirrors the
    /// structure of
    /// [`crate::builtins::fn_def::tests::functor::dual_write::functor_body_module_dispatch_does_not_dangle`]
    /// but pins the type-op-in-per-call-arena path rather than the plain functor lift.
    ///
    /// Module-system functor-params Stage B: parameter migrated from the lowercase
    /// workaround (`elem`) to the documented Type-class form (`Er`). Stage A's
    /// per-call dual-write makes the surface form work end-to-end through the
    /// signature-typed parameter path that previously parked on a missing top-level
    /// binding.
    #[test]
    fn type_op_dispatch_does_not_dangle() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
             MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
             LET ElemMod = (IntOrd :| OrderedSig)",
        );
        // Functor body invokes MODULE_TYPE_OF on the per-call parameter `Er`. The
        // dispatched `KTypeValue` is allocated in the per-call arena and bound into the
        // result module via `LET Tslot` (uppercase — drives the LET TypeExprRef
        // overload that writes into `bindings.types`). A second plain `LET probe = 11`
        // gives a value-side binding to read back.
        run(
            scope,
            "FN (LIFT_TYPE Er :OrderedSig) -> Module = \
             (MODULE Result = ((LET Tslot = (MODULE_TYPE_OF Er Type)) (LET probe = 11)))",
        );
        run(scope, "LET Held = (LIFT_TYPE (ElemMod))");

        // Subsequent allocations and FN calls churn the run-root arena. The lifted
        // `KModule` and its child scope (carrying the dispatched type-op value) must
        // survive that churn.
        run(scope, "FN (NOOP) -> Number = (1)");
        for _ in 0..20 {
            run_one(scope, parse_one("NOOP"));
        }
        // Another functor call to allocate more per-call frames (and drop them).
        run(scope, "LET Other = (LIFT_TYPE (ElemMod))");

        // Hold the original `held` module across all that churn and read both surfaces
        // the audit pins: `child_scope()` (the captured-scope transmute) and
        // `type_members` (the RefCell on the Module).
        let data = scope.bindings().data();
        let m = match data.get("Held").map(|(o, _)| *o) {
            Some(KObject::KTypeValue(KType::Module { module: m, frame: _ })) => *m,
            other => panic!("Held should be a module, got {:?}", other.map(|o| o.ktype())),
        };
        let probe = m.child_scope().bindings().data().get("probe").map(|(o, _)| *o);
        assert!(
            matches!(probe, Some(KObject::Number(n)) if *n == 11.0),
            "Held.probe must still read 11.0 after subsequent churn",
        );
        // `Tslot` landed in `bindings.types` via the LET TypeExprRef overload — the
        // dispatched `KTypeValue` from the per-call MODULE_TYPE_OF call.
        let tslot = m.child_scope().resolve_type("Tslot");
        assert!(
            tslot.is_some(),
            "Held.Tslot must still resolve through bindings.types after churn",
        );
        // The RefCell on `type_members` is the other half of the Module's lifetime
        // surface; the borrow must complete cleanly (we don't assert contents here —
        // the body's module isn't opaquely ascribed, so type_members is empty).
        let _ = m.type_members.borrow();
    }
}
