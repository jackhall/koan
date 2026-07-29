//! combine, defer_to, and tail-call slot reuse.

use super::super::super::outcome::Outcome;
use crate::builtins::test_support::{resident_carrier, TestRun};
use crate::machine::core::{run_root_storage, FrameStorageExt};
use crate::machine::model::KExpression;
use crate::machine::model::ReturnType;
use crate::machine::model::{Carried, KObject};

use super::let_expr;

#[test]
fn dep_finish_waits_on_deps_then_runs_finish() {
    // Pins that dep-finish waits on every dep before invoking finish and that
    // finish-returned Outcome::Done(Value) lands in the slot's result.
    use crate::machine::execute::TerminalDepFinish;
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;
    let dep_a = runtime.dispatch_in_scope(let_expr("ca", 7.0), scope);
    let dep_b = runtime.dispatch_in_scope(let_expr("cb", 11.0), scope);
    let finish: TerminalDepFinish = Box::new(|_sched, terminals| {
        let a = match terminals.owned(0).value {
            Carried::Object(KObject::Number(n)) => *n,
            _ => {
                return Outcome::Done(Err(crate::machine::KError::new(
                    crate::machine::KErrorKind::ShapeError("a not number".into()),
                )))
            }
        };
        let b = match terminals.owned(1).value {
            Carried::Object(KObject::Number(n)) => *n,
            _ => {
                return Outcome::Done(Err(crate::machine::KError::new(
                    crate::machine::KErrorKind::ShapeError("b not number".into()),
                )))
            }
        };
        let allocated = _sched
            .current_scope()
            .brand()
            .alloc_object(KObject::KString(format!("{a}+{b}")));
        Outcome::done_resident(_sched.current_scope(), Carried::Object(allocated))
    });
    let mut deps = crate::scheduler::ResolvedDeps::new();
    deps.own(dep_a);
    deps.own(dep_b);
    let dep_finish_id = runtime.add_dep_finish(deps, scope, finish);
    runtime.execute().unwrap();
    assert!(runtime
        .read_result_with(
            dep_finish_id,
            |v| matches!(v.object(), KObject::KString(s) if s == "7+11")
        )
        .expect("value"));
}

#[test]
fn dep_finish_short_circuits_on_dep_error() {
    // Pins that finish does not run when any dep errored, and that the
    // propagated error carries a "<deps>" frame.
    use crate::machine::execute::TerminalDepFinish;
    use crate::machine::{KError, KErrorKind};
    use std::cell::Cell;
    use std::rc::Rc;
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;

    // Allocate two placeholder Dispatch slots, drain the queue so execute()
    // doesn't revisit them, then overwrite their results directly.
    let mk_dispatch =
        || crate::machine::execute::dispatch::decide_tail(KExpression::new(Vec::new()), None);
    let dep_ok = runtime.add(mk_dispatch(), scope);
    let dep_err = runtime.add(mk_dispatch(), scope);
    let store = runtime.scheduler_mut();
    store.clear_node(dep_ok);
    store.clear_node(dep_err);
    let _ = store.pop_next();
    let _ = store.pop_next();
    let value = region.brand().alloc_object(KObject::Number(99.0));
    store.set_result(dep_ok, Ok(Carried::Object(value)), resident_carrier(scope));
    // A synthetic terminal carries no finalize-seeded retention hold; the dep pull requires one.
    // This slot reaches nothing foreign, so its hold's owned bundle is empty.
    store.seed_retention(
        dep_ok,
        std::rc::Rc::clone(&region),
        crate::machine::core::FrameCoverage::empty(),
        1,
    );
    store.set_result(
        dep_err,
        Err(KError::new(KErrorKind::ShapeError(
            "dep_err synthetic".into(),
        ))),
        resident_carrier(scope),
    );

    let invoked: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let invoked_clone = Rc::clone(&invoked);
    let finish: TerminalDepFinish = Box::new(move |_sched, _terminals| {
        invoked_clone.set(true);
        Outcome::done_resident(_sched.current_scope(), Carried::Object(value))
    });
    let mut deps = crate::scheduler::ResolvedDeps::new();
    deps.own(dep_ok);
    deps.own(dep_err);
    let dep_finish_id = runtime.add_dep_finish(deps, scope, finish);
    runtime.execute().unwrap();

    assert!(!invoked.get(), "finish must not run when a dep errored");
    let result = runtime.result_error(dep_finish_id);
    let err = match result {
        Err(e) => e.clone(),
        Ok(()) => panic!("combine should have errored"),
    };
    assert!(
        err.frames.iter().any(|f| f.function == "<deps>"),
        "propagated error should carry a <deps> frame, got {err}",
    );
}

/// Retention-timeline acceptance (claim: *foreign pins housed in the retention hold release at
/// pull-count zero*). A finalized producer's hold now carries the terminal's owned **foreign**
/// bundle alongside the region owner; both halves are released together when the last destination
/// pull is discharged — the hold's own event, not the slot's free. Seed a synthetic producer's hold
/// with a non-empty foreign bundle (the sole strong owner of a distinct region), wire one consumer
/// that pulls it, and confirm the reached region stays live while the pull is outstanding and
/// releases once the pull discharges the hold.
#[test]
fn retention_hold_foreign_bundle_releases_at_pull_zero() {
    use crate::machine::execute::TerminalDepFinish;
    use std::rc::Rc;

    let region = run_root_storage();
    // A distinct region the producer's terminal reaches; the hold's foreign bundle will be its sole
    // strong owner once we drop our own handle.
    let foreign = run_root_storage();
    let weak = Rc::downgrade(&foreign);

    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;

    let mk_dispatch =
        || crate::machine::execute::dispatch::decide_tail(KExpression::new(Vec::new()), None);
    let dep_ok = runtime.add(mk_dispatch(), scope);
    let store = runtime.scheduler_mut();
    store.clear_node(dep_ok);
    let _ = store.pop_next();
    let value = region.brand().alloc_object(KObject::Number(42.0));
    store.set_result(dep_ok, Ok(Carried::Object(value)), resident_carrier(scope));
    // Seed the hold with a foreign bundle pinning `foreign`, and one outstanding pull.
    store.seed_retention(
        dep_ok,
        Rc::clone(&region),
        crate::machine::core::FrameCoverage::of(Rc::clone(&foreign)),
        1,
    );
    // Drop our own strong handle: the hold's foreign bundle is now the sole owner of `foreign`.
    drop(foreign);
    assert!(
        weak.upgrade().is_some(),
        "the hold's foreign bundle keeps the reached region alive while the pull is outstanding",
    );

    // A finish that reads (pulls) the dep once — the delivered terminal clones the hold's
    // (owner, foreign) out, and discharging that pull brings the count to zero and drops the hold.
    let finish: TerminalDepFinish = Box::new(move |_sched, terminals| {
        let v = match terminals.owned(0).value {
            Carried::Object(object) => object,
            _ => unreachable!("dep_ok delivered a Number object"),
        };
        Outcome::done_resident(_sched.current_scope(), Carried::Object(v))
    });
    let mut deps = crate::scheduler::ResolvedDeps::new();
    deps.own(dep_ok);
    let dep_finish_id = runtime.add_dep_finish(deps, scope, finish);
    runtime.execute().unwrap();

    // The single pull discharged to zero, dropping the hold — owner and foreign together — so the
    // reached region is released. Bit-for-bit today's owner timeline, now carrying the foreign half.
    assert!(
        weak.upgrade().is_none(),
        "the hold's foreign bundle releases at pull-count zero, alongside the owner",
    );
    assert!(runtime
        .read_result_with(dep_finish_id, |v| matches!(
            v.object(),
            KObject::Number(n) if *n == 42.0
        ))
        .expect("the finish delivered the pulled value"),);
}

#[test]
fn defer_to_lifts_slot_terminal_off_dep_finish_id() {
    // Pins the binder-body wrap-up shape MODULE / SIG use: an `Action::AwaitDeps` body parks the
    // slot as a dep-finish and leaves it with the dep-finish's terminal.
    use crate::builtins::register_builtin;
    use crate::machine::core::{Action, AwaitContinue, BodyCtx};
    use crate::machine::model::Carried;
    use crate::machine::model::ExpressionPart;
    use crate::machine::model::{ExpressionSignature, KType, SignatureElement};

    fn body<'run>(_ctx: &BodyCtx<'run, '_>) -> Action<'run> {
        let finish: AwaitContinue<'run> = Box::new(|fctx, _results| {
            let v = fctx
                .scope
                .brand()
                .alloc_object(KObject::KString("from-combine".into()));
            Action::done_resident(fctx.scope, Carried::Object(v))
        });
        Action::await_deps(crate::scheduler::Deps::new(), finish)
    }

    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    register_builtin(
        scope,
        "DEFERTEST",
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::STR),
            elements: vec![SignatureElement::Keyword("DEFERTEST".into())],
        },
        body,
        &test_run.types,
        &mut crate::machine::WriteGate::for_test(),
    );

    let runtime = &mut test_run.runtime;
    let id = runtime.dispatch_in_scope(
        KExpression::new(vec![crate::source::Spanned::bare(ExpressionPart::Keyword(
            "DEFERTEST".into(),
        ))]),
        scope,
    );
    runtime.execute().unwrap();
    assert!(
        runtime
            .read_result_with(
                id,
                |v| matches!(v.object(), KObject::KString(s) if s == "from-combine")
            )
            .expect("value"),
        "DEFERTEST slot's terminal should match the dep-finish's terminal",
    );
}

#[test]
fn tail_call_reuses_node_slot_in_place() {
    // Pins that an `Outcome::Continue` tail rewrites the caller's slot in place rather
    // than spawning a fresh one (verified via runtime.len() == 1 below).
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let root = test_run.scope;
    let runtime = &mut test_run.runtime;
    let exprs = crate::parse::parse("MATCH true -> :Str WITH (true -> (\"hi\") false -> (\"no\"))")
        .expect("parse should succeed");
    assert_eq!(exprs.len(), 1);
    let id = runtime.dispatch_in_scope(exprs.into_iter().next().unwrap(), root);

    runtime.execute().unwrap();

    assert!(runtime
        .read_result_with(
            id,
            |v| matches!(v.object(), KObject::KString(s) if s == "hi")
        )
        .expect("value"));
    assert_eq!(
        runtime.len(),
        1,
        "tail-call slot reuse = the MATCH's original slot should have been rewritten \
         to evaluate the matched branch's body, not allocate a new slot",
    );
}
