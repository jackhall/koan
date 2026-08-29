//! combine, defer_to, and tail-call slot reuse.

use super::super::super::outcome::Outcome;
use crate::builtins::test_support::TestRun;
use crate::builtins::test_support::probe_symbol;
use crate::machine::core::{program_storage, run_root_storage};
use crate::machine::model::ReturnType;
use crate::machine::model::{Carried, KObject};

use super::{let_expr, working_one};

#[test]
fn dep_finish_waits_on_deps_then_runs_finish() {
    // Pins that dep-finish waits on every dep before invoking finish and that
    // finish-returned Outcome::Done(Value) lands in the slot's result.
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let registry = test_run.registry_handle();
    let labels = &registry.registries().labels;
    let runtime = &mut test_run.runtime;
    let dep_a = runtime.dispatch_in_scope(let_expr(&program, labels, "ca", 7.0), scope, 1);
    let dep_b = runtime.dispatch_in_scope(let_expr(&program, labels, "cb", 11.0), scope, 2);
    let dep_finish_id = runtime.add_dep_finish(
        &[dep_a, dep_b],
        scope,
        |_sched, terminals| {
            let a = match terminals[0].cell.open_at().value() {
                Carried::Object(KObject::Number(n)) => *n,
                _ => {
                    return Outcome::Done(Err(crate::machine::KError::new(
                        crate::machine::KErrorKind::ShapeError("a not number".into()),
                    )));
                }
            };
            let b = match terminals[1].cell.open_at().value() {
                Carried::Object(KObject::Number(n)) => *n,
                _ => {
                    return Outcome::Done(Err(crate::machine::KError::new(
                        crate::machine::KErrorKind::ShapeError("b not number".into()),
                    )));
                }
            };
            let allocated = _sched.current_scope().fold_resident_object(|brand| {
                KObject::KString(brand.allocator().text(&format!("{a}+{b}")))
            });
            Outcome::done_resident(_sched.current_scope(), Carried::Object(allocated))
        },
        3,
    );
    let watch = runtime.install_edge_for_test(dep_finish_id, scope);
    runtime.execute().unwrap();
    assert!(
        runtime
            .read_edge_result_with(
                watch,
                |v| matches!(v.object(), KObject::KString(s) if *s == "7+11")
            )
            .expect("value")
    );
}

#[test]
fn dep_finish_short_circuits_on_dep_error() {
    // Pins that finish does not run when any dep errored, and that the
    // propagated error carries a "<deps>" frame.
    use crate::machine::KErrorKind;
    use std::cell::Cell;
    use std::rc::Rc;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let registry = test_run.registry_handle();
    let labels = &registry.registries().labels;
    let runtime = &mut test_run.runtime;

    // One dep that delivers a value and one that cannot resolve its name — an ordinary erroring
    // dispatch, which is what the consumer's walk fills its edge with.
    let dep_ok = runtime.dispatch_in_scope(let_expr(&program, labels, "ok", 99.0), scope, 1);
    let dep_err = runtime.dispatch_in_scope(
        working_one(&program, labels, "LET bad = (undefined_thing)"),
        scope,
        2,
    );

    let invoked: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let invoked_clone = Rc::clone(&invoked);
    let dep_finish_id = runtime.add_dep_finish(
        &[dep_ok, dep_err],
        scope,
        move |_sched, _terminals| {
            invoked_clone.set(true);
            let scope = _sched.current_scope();
            let allocated = scope.fold_resident_object(|brand| {
                KObject::KString(brand.allocator().text("finish ran"))
            });
            Outcome::done_resident(scope, Carried::Object(allocated))
        },
        3,
    );
    let watch = runtime.install_edge_for_test(dep_finish_id, scope);
    runtime.execute().unwrap();

    assert!(!invoked.get(), "finish must not run when a dep errored");
    let err = match runtime.edge_result_error(watch) {
        Err(e) => e.clone(),
        Ok(()) => panic!("combine should have errored"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(n) if n == "undefined_thing"),
        "the dep's own error is what propagates, got {err}",
    );
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
    use crate::machine::model::{KType, SignatureDraft, SignatureElement};

    fn body<'run>(_ctx: &BodyCtx<'_, 'run, '_>) -> Action<'run> {
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
        SignatureDraft {
            return_type: ReturnType::Resolved(KType::STR),
            elements: vec![SignatureElement::Keyword(probe_symbol("DEFERTEST"))],
        },
        body,
        test_run.registries(),
        &mut crate::machine::WriteGate::for_test(),
    );

    let watch = test_run.dispatch_watched_in(scope, super::keyword_expr(&program, "DEFERTEST"));
    test_run.runtime.execute().unwrap();
    assert!(
        test_run
            .runtime
            .read_edge_result_with(
                watch,
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
    let registry = test_run.registry_handle();
    let labels = &registry.registries().labels;
    let watch = test_run.dispatch_watched_in(
        root,
        working_one(
            &program,
            labels,
            "MATCH true -> :Str WITH (true -> (\"hi\") false -> (\"no\"))",
        ),
    );

    test_run.runtime.execute().unwrap();

    assert!(
        test_run
            .runtime
            .read_edge_result_with(
                watch,
                |v| matches!(v.object(), KObject::KString(s) if *s == "hi")
            )
            .expect("value")
    );
    assert_eq!(
        test_run.runtime.len(),
        1,
        "tail-call slot reuse = the MATCH's original slot should have been rewritten \
         to evaluate the matched branch's body, not allocate a new slot",
    );
}
