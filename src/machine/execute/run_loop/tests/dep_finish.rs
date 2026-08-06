//! combine, defer_to, and tail-call slot reuse.

use super::super::super::outcome::Outcome;
use crate::builtins::test_support::{resident_carrier, TestRun};
use crate::machine::core::{program_storage, run_root_storage, FrameStorageExt};
use crate::machine::model::ReturnType;
use crate::machine::model::WorkingExpression;
use crate::machine::model::{Carried, KObject};

use super::{let_expr, working_one};
use crate::machine::model::Scalar;

#[test]
fn dep_finish_waits_on_deps_then_runs_finish() {
    // Pins that dep-finish waits on every dep before invoking finish and that
    // finish-returned Outcome::Done(Value) lands in the slot's result.
    use crate::machine::execute::TerminalDepFinish;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;
    let dep_a = runtime.dispatch_in_scope(let_expr(&program, "ca", 7.0), scope);
    let dep_b = runtime.dispatch_in_scope(let_expr(&program, "cb", 11.0), scope);
    let finish: TerminalDepFinish = Box::new(|_sched, terminals| {
        let a = match terminals.owned(0).delivered.open_at().value() {
            Carried::Object(KObject::Number(n)) => *n,
            _ => {
                return Outcome::Done(Err(crate::machine::KError::new(
                    crate::machine::KErrorKind::ShapeError("a not number".into()),
                )))
            }
        };
        let b = match terminals.owned(1).delivered.open_at().value() {
            Carried::Object(KObject::Number(n)) => *n,
            _ => {
                return Outcome::Done(Err(crate::machine::KError::new(
                    crate::machine::KErrorKind::ShapeError("b not number".into()),
                )))
            }
        };
        let allocated = _sched
            .current_scope()
            .fold_resident_object(|brand| KObject::KString(brand.alloc_text(&format!("{a}+{b}"))));
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
            |v| matches!(v.object(), KObject::KString(s) if *s == "7+11")
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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;

    // Allocate two placeholder Dispatch slots, drain the queue so execute()
    // doesn't revisit them, then overwrite their results directly.
    let mk_dispatch = || {
        crate::machine::execute::dispatch::decide_tail(
            WorkingExpression::new(program.brand().region(), Vec::new()),
            None,
        )
    };
    let dep_ok = runtime.add(mk_dispatch(), scope);
    let dep_err = runtime.add(mk_dispatch(), scope);
    let store = runtime.scheduler_mut();
    store.clear_node(dep_ok);
    store.clear_node(dep_err);
    let _ = store.pop_next();
    let _ = store.pop_next();
    let value = region.brand().alloc_scalar(Scalar::Number(99.0));
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

    let program = program_storage();
    let region = run_root_storage();
    // A distinct region the producer's terminal reaches; the hold's foreign bundle will be its sole
    // strong owner once we drop our own handle.
    let foreign = run_root_storage();
    let weak = Rc::downgrade(&foreign);

    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;

    let mk_dispatch = || {
        crate::machine::execute::dispatch::decide_tail(
            WorkingExpression::new(program.brand().region(), Vec::new()),
            None,
        )
    };
    let dep_ok = runtime.add(mk_dispatch(), scope);
    let store = runtime.scheduler_mut();
    store.clear_node(dep_ok);
    let _ = store.pop_next();
    let value = region.brand().alloc_scalar(Scalar::Number(42.0));
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
        // Read the pulled number under the dep envelope's own pins and re-anchor it in the
        // consumer's own region — the consumer-pull shape, with nothing from the producer's region
        // escaping the open guard.
        let n = match terminals.owned(0).delivered.open_at().value() {
            Carried::Object(KObject::Number(n)) => *n,
            _ => unreachable!("dep_ok delivered a Number object"),
        };
        let scope = _sched.current_scope();
        let allocated = scope.fold_resident_object(|brand| *brand.alloc_scalar(Scalar::Number(n)));
        Outcome::done_resident(scope, Carried::Object(allocated))
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
    use crate::machine::model::{KType, SignatureDraft, SignatureElement};

    fn body<'run>(_ctx: &BodyCtx<'run, '_>) -> Action<'run> {
        let finish: AwaitContinue<'run> = Box::new(|fctx, _results| {
            let v = fctx.scope.brand().alloc_string("from-combine");
            Action::done_resident(fctx.scope, Carried::Object(v))
        });
        Action::await_deps(crate::scheduler::Deps::new(), finish)
    }

    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    register_builtin(
        scope,
        "DEFERTEST",
        SignatureDraft {
            return_type: ReturnType::Resolved(KType::STR),
            elements: vec![SignatureElement::Keyword("DEFERTEST")],
        },
        body,
        &test_run.types,
        &mut crate::machine::WriteGate::for_test(),
    );

    let runtime = &mut test_run.runtime;
    let id = runtime.dispatch_in_scope(super::keyword_expr(&program, "DEFERTEST"), scope);
    runtime.execute().unwrap();
    assert!(
        runtime
            .read_result_with(
                id,
                |v| matches!(v.object(), KObject::KString(s) if *s == "from-combine")
            )
            .expect("value"),
        "DEFERTEST slot's terminal should match the dep-finish's terminal",
    );
}

#[test]
fn tail_call_reuses_node_slot_in_place() {
    // Pins that an `Outcome::Continue` tail rewrites the caller's slot in place rather
    // than spawning a fresh one (verified via runtime.len() == 1 below).
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let root = test_run.scope;
    let runtime = &mut test_run.runtime;
    let id = runtime.dispatch_in_scope(
        working_one(
            &program,
            "MATCH true -> :Str WITH (true -> (\"hi\") false -> (\"no\"))",
        ),
        root,
    );

    runtime.execute().unwrap();

    assert!(runtime
        .read_result_with(
            id,
            |v| matches!(v.object(), KObject::KString(s) if *s == "hi")
        )
        .expect("value"));
    assert_eq!(
        runtime.len(),
        1,
        "tail-call slot reuse = the MATCH's original slot should have been rewritten \
         to evaluate the matched branch's body, not allocate a new slot",
    );
}
