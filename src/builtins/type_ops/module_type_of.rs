use crate::machine::model::KObject;
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};

use crate::builtins::err;

/// `MODULE_TYPE_OF <m:Module> <name>` → `TypeExprRef` carrying the abstract type bound
/// under `name` in `m`'s `type_members` table. Surface analogue of `M.Type`, reachable as
/// a scheduled call so a functor body can synthesize it from a parameter module value.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let m = match bundle.require_module("m") {
        Ok(m) => m,
        Err(e) => return err(e),
    };
    let name = match bundle.require_ktype("name") {
        Ok(t) => t.name(),
        Err(e) => return err(e),
    };
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
                assert_eq!(kt.name(), "Type");
                assert!(matches!(kt, KType::AbstractType { .. }));
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn module_type_of_unknown_member_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "MODULE Foo = (LET x = 1)");
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("MODULE_TYPE_OF Foo Bogus"), scope);
        sched.execute().expect("scheduler runs to completion");
        let res = sched.read_result(id);
        assert!(res.is_err(), "expected MODULE_TYPE_OF on missing member to err");
    }

    /// Miri audit-slate: pins type-op dispatch through the per-call arena under tree
    /// borrows. A functor body invokes `(MODULE_TYPE_OF Er Type)` on its per-call
    /// parameter; the returned `KModule` plus the bound type member must survive
    /// subsequent arena churn.
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
        run(
            scope,
            "FN (LIFT_TYPE Er :OrderedSig) -> Module = \
             (MODULE Result = ((LET Tslot = (MODULE_TYPE_OF Er Type)) (LET probe = 11)))",
        );
        run(scope, "LET Held = (LIFT_TYPE (ElemMod))");

        // Churn the run-root arena around the held module.
        run(scope, "FN (NOOP) -> Number = (1)");
        for _ in 0..20 {
            run_one(scope, parse_one("NOOP"));
        }
        run(scope, "LET Other = (LIFT_TYPE (ElemMod))");

        let m = match scope.resolve_type("Held") {
            Some(KType::Module { module: m, frame: _ }) => *m,
            other => panic!("Held should be a module identity in types, got {other:?}"),
        };
        let probe = m.child_scope().bindings().data().get("probe").map(|(o, _)| *o);
        assert!(
            matches!(probe, Some(KObject::Number(n)) if *n == 11.0),
            "Held.probe must still read 11.0 after subsequent churn",
        );
        let tslot = m.child_scope().resolve_type("Tslot");
        assert!(
            tslot.is_some(),
            "Held.Tslot must still resolve through bindings.types after churn",
        );
        // Pin the RefCell on `type_members` — the borrow must complete cleanly.
        let _ = m.type_members.borrow();
    }
}
