//! combine, defer_to, and tail-call slot reuse.

use super::super::super::outcome::Outcome;
use crate::builtins::default_scope;
use crate::machine::execute::KoanRuntime;
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::ReturnType;
use crate::machine::model::{Carried, KObject};
use crate::machine::KoanRegion;

use super::let_expr;

#[test]
fn dep_finish_waits_on_deps_then_runs_finish() {
    // Pins that dep-finish waits on every dep before invoking finish and that
    // finish-returned Outcome::Done(Value) lands in the slot's result.
    use crate::machine::execute::DepFinish;
    let arena = KoanRegion::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = KoanRuntime::new();
    let dep_a = sched.dispatch_in_scope(let_expr("ca", 7.0), scope);
    let dep_b = sched.dispatch_in_scope(let_expr("cb", 11.0), scope);
    let finish: DepFinish = Box::new(|_sched, results| {
        let a = match results[0] {
            Carried::Object(KObject::Number(n)) => *n,
            _ => {
                return Outcome::Done(Err(crate::machine::KError::new(
                    crate::machine::KErrorKind::ShapeError("a not number".into()),
                )))
            }
        };
        let b = match results[1] {
            Carried::Object(KObject::Number(n)) => *n,
            _ => {
                return Outcome::Done(Err(crate::machine::KError::new(
                    crate::machine::KErrorKind::ShapeError("b not number".into()),
                )))
            }
        };
        let allocated = _sched
            .current_scope()
            .arena
            .alloc_object(KObject::KString(format!("{a}+{b}")));
        Outcome::Done(Ok(Carried::Object(allocated)))
    });
    let dep_finish_id = sched.add_dep_finish(vec![dep_a, dep_b], vec![], scope, finish);
    sched.execute().unwrap();
    assert!(matches!(sched.read(dep_finish_id).object(), KObject::KString(s) if s == "7+11"));
}

#[test]
fn dep_finish_short_circuits_on_dep_error() {
    // Pins that finish does not run when any dep errored, and that the
    // propagated error carries a "<deps>" frame.
    use crate::machine::execute::DepFinish;
    use crate::machine::{KError, KErrorKind};
    use std::cell::Cell;
    use std::rc::Rc;
    let arena = KoanRegion::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = KoanRuntime::new();

    // Allocate two placeholder Dispatch slots, drain the queue so execute()
    // doesn't revisit them, then overwrite their results directly.
    let mk_dispatch = || crate::machine::execute::dispatch::decide(KExpression::new(Vec::new()));
    let dep_ok = sched.add(mk_dispatch(), scope);
    let dep_err = sched.add(mk_dispatch(), scope);
    let store = sched.scheduler_mut();
    store.clear_node(dep_ok);
    store.clear_node(dep_err);
    let _ = store.pop_next();
    let _ = store.pop_next();
    let value = arena.alloc_object(KObject::Number(99.0));
    store.set_result(dep_ok, Ok(Carried::Object(value)));
    store.set_result(
        dep_err,
        Err(KError::new(KErrorKind::ShapeError(
            "dep_err synthetic".into(),
        ))),
    );

    let invoked: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let invoked_clone = Rc::clone(&invoked);
    let finish: DepFinish = Box::new(move |_sched, _results| {
        invoked_clone.set(true);
        Outcome::Done(Ok(Carried::Object(value)))
    });
    let dep_finish_id = sched.add_dep_finish(vec![dep_ok, dep_err], vec![], scope, finish);
    sched.execute().unwrap();

    assert!(!invoked.get(), "finish must not run when a dep errored");
    let result = sched.read_result(dep_finish_id);
    let err = match result {
        Err(e) => e.clone(),
        Ok(_) => panic!("combine should have errored"),
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
    use crate::builtins::{default_scope, register_builtin};
    use crate::machine::core::kfunction::action::{Action, AwaitContinue, BodyCtx};
    use crate::machine::model::ast::ExpressionPart;
    use crate::machine::model::Carried;
    use crate::machine::model::{ExpressionSignature, KType, SignatureElement};

    fn body<'run>(_ctx: &BodyCtx<'run, '_>) -> Action<'run> {
        let finish: AwaitContinue<'run> = Box::new(|fctx, _results| {
            let v = fctx
                .scope
                .arena
                .alloc_object(KObject::KString("from-combine".into()));
            Action::Done(Ok(Carried::Object(v)))
        });
        Action::AwaitDeps {
            deps: Vec::new(),
            finish,
        }
    }

    let arena = KoanRegion::new();
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

    let mut sched = KoanRuntime::new();
    let id = sched.dispatch_in_scope(
        KExpression::new(vec![crate::source::Spanned::bare(ExpressionPart::Keyword(
            "DEFERTEST".into(),
        ))]),
        scope,
    );
    sched.execute().unwrap();
    assert!(
        matches!(sched.read(id).object(), KObject::KString(s) if s == "from-combine"),
        "DEFERTEST slot's terminal should match the dep-finish's terminal",
    );
}

#[test]
fn tail_call_reuses_node_slot_in_place() {
    // Pins that an `Outcome::Continue` tail rewrites the caller's slot in place rather
    // than spawning a fresh one (verified via sched.len() == 1 below).
    let arena = KoanRegion::new();
    let root = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = KoanRuntime::new();
    let exprs = crate::parse::parse("MATCH true -> :Str WITH (true -> (\"hi\") false -> (\"no\"))")
        .expect("parse should succeed");
    assert_eq!(exprs.len(), 1);
    let id = sched.dispatch_in_scope(exprs.into_iter().next().unwrap(), root);

    sched.execute().unwrap();

    assert!(matches!(sched.read(id).object(), KObject::KString(s) if s == "hi"));
    assert_eq!(
        sched.len(),
        1,
        "tail-call slot reuse = the MATCH's original slot should have been rewritten \
         to evaluate the matched branch's body, not allocate a new slot",
    );
}
