//! combine, defer_to, and tail-call slot reuse.

use super::super::super::outcome::Outcome;
use crate::builtins::test_support::TestRun;
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
        Outcome::done_resident(Carried::Object(allocated))
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
    store.set_result(dep_ok, Ok(Carried::Object(value)));
    // A synthetic terminal carries no finalize-seeded retention hold; the dep pull requires one.
    store.seed_retention(dep_ok, std::rc::Rc::clone(&region), 1);
    store.set_result(
        dep_err,
        Err(KError::new(KErrorKind::ShapeError(
            "dep_err synthetic".into(),
        ))),
    );

    let invoked: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let invoked_clone = Rc::clone(&invoked);
    let finish: TerminalDepFinish = Box::new(move |_sched, _terminals| {
        invoked_clone.set(true);
        Outcome::done_resident(Carried::Object(value))
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
            Action::done_resident(Carried::Object(v))
        });
        Action::AwaitDeps {
            deps: Vec::new(),
            finish,
        }
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
