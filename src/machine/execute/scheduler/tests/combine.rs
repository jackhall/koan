//! combine, defer_to, and tail-call slot reuse.

use super::super::super::nodes::{NodeOutput, NodeWork};
use super::super::Scheduler;
use crate::builtins::default_scope;
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::ReturnType;
use crate::machine::model::{Carried, KObject};
use crate::machine::RuntimeArena;

use super::let_expr;

#[test]
fn combine_waits_on_deps_then_runs_finish() {
    // Pins that Combine waits on every dep before invoking finish and that
    // finish-returned BodyResult::Value lands in the slot's result.
    use crate::machine::{BodyResult, CombineFinish};
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let dep_a = sched.add_dispatch(let_expr("ca", 7.0), scope);
    let dep_b = sched.add_dispatch(let_expr("cb", 11.0), scope);
    let finish: CombineFinish = Box::new(|_sched, results| {
        let a = match results[0] {
            Carried::Object(KObject::Number(n)) => *n,
            _ => {
                return BodyResult::Err(crate::machine::KError::new(
                    crate::machine::KErrorKind::ShapeError("a not number".into()),
                ))
            }
        };
        let b = match results[1] {
            Carried::Object(KObject::Number(n)) => *n,
            _ => {
                return BodyResult::Err(crate::machine::KError::new(
                    crate::machine::KErrorKind::ShapeError("b not number".into()),
                ))
            }
        };
        let allocated = _sched
            .current_scope()
            .arena
            .alloc_object(KObject::KString(format!("{a}+{b}")));
        BodyResult::value(allocated)
    });
    let combine_id = sched.add_combine(vec![dep_a, dep_b], vec![], scope, finish);
    sched.execute().unwrap();
    assert!(matches!(sched.read(combine_id).object(), KObject::KString(s) if s == "7+11"));
}

#[test]
fn combine_short_circuits_on_dep_error() {
    // Pins that finish does not run when any dep errored, and that the
    // propagated error carries a "<combine>" frame.
    use crate::machine::{BodyResult, CombineFinish, KError, KErrorKind};
    use std::cell::Cell;
    use std::rc::Rc;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();

    // Allocate two placeholder Dispatch slots, drain the queue so execute()
    // doesn't revisit them, then overwrite their results directly.
    let mk_dispatch = || NodeWork::dispatch(KExpression::new(Vec::new()));
    let dep_ok = sched.add(mk_dispatch(), scope);
    let dep_err = sched.add(mk_dispatch(), scope);
    sched.store.clear_node(dep_ok);
    sched.store.clear_node(dep_err);
    let _ = sched.queues.pop_next();
    let _ = sched.queues.pop_next();
    let value = arena.alloc_object(KObject::Number(99.0));
    sched.store.set_result(dep_ok, NodeOutput::value(value));
    sched.store.set_result(
        dep_err,
        NodeOutput::Err(KError::new(KErrorKind::ShapeError(
            "dep_err synthetic".into(),
        ))),
    );

    let invoked: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let invoked_clone = Rc::clone(&invoked);
    let finish: CombineFinish = Box::new(move |_sched, _results| {
        invoked_clone.set(true);
        BodyResult::value(value)
    });
    let combine_id = sched.add_combine(vec![dep_ok, dep_err], vec![], scope, finish);
    sched.execute().unwrap();

    assert!(!invoked.get(), "finish must not run when a dep errored");
    let result = sched.read_result(combine_id);
    let err = match result {
        Err(e) => e.clone(),
        Ok(_) => panic!("combine should have errored"),
    };
    assert!(
        err.frames.iter().any(|f| f.function == "<combine>"),
        "propagated error should carry a <combine> frame, got {err}",
    );
}

#[test]
fn defer_to_lifts_slot_terminal_off_combine_id() {
    // Pins the binder-body wrap-up shape MODULE / SIG use: a body returning
    // `BodyResult::DeferTo(combine_id)` leaves its slot with the Combine's terminal.
    use crate::builtins::{default_scope, register_builtin};
    use crate::machine::model::ast::ExpressionPart;
    use crate::machine::model::{ExpressionSignature, KType, SignatureElement};
    use crate::machine::{ArgumentBundle, BodyResult, CombineFinish};

    fn body<'a, 's>(
        sched: &mut dyn crate::machine::SchedulerHandle<'a, 's>,
        _bundle: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        let finish: CombineFinish<'a> = Box::new(
            |_sched: &mut dyn crate::machine::SchedulerHandle<'a, '_>, _results| {
                let v = _sched
                    .current_scope()
                    .arena
                    .alloc_object(KObject::KString("from-combine".into()));
                BodyResult::value(v)
            },
        );
        let combine_id = sched.add_combine_in_frame(Vec::new(), Vec::new(), finish);
        BodyResult::DeferTo(combine_id)
    }

    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    register_builtin(
        scope,
        "DEFERTEST",
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Str),
            elements: vec![SignatureElement::Keyword("DEFERTEST".into())],
        },
        body,
    );

    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(
        KExpression::new(vec![crate::machine::core::source::Spanned::bare(
            ExpressionPart::Keyword("DEFERTEST".into()),
        )]),
        scope,
    );
    sched.execute().unwrap();
    assert!(
        matches!(sched.read(id).object(), KObject::KString(s) if s == "from-combine"),
        "DEFERTEST slot's terminal should match the Combine's terminal",
    );
}

#[test]
fn tail_call_reuses_node_slot_in_place() {
    // Pins that `BodyResult::Tail` rewrites the caller's slot in place rather
    // than spawning a fresh one (verified via sched.len() == 1 below).
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let exprs = crate::parse::parse("MATCH true -> :Str WITH (true -> (\"hi\") false -> (\"no\"))")
        .expect("parse should succeed");
    assert_eq!(exprs.len(), 1);
    let id = sched.add_dispatch(exprs.into_iter().next().unwrap(), root);

    sched.execute().unwrap();

    assert!(matches!(sched.read(id).object(), KObject::KString(s) if s == "hi"));
    assert_eq!(
        sched.len(),
        1,
        "tail-call slot reuse = the MATCH's original slot should have been rewritten \
         to evaluate the matched branch's body, not allocate a new slot",
    );
}
