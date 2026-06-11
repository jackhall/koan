//! **Under construction, feature-gated (`exec-v2`).** The `'run`-aware shell between the pure
//! [`exec`](crate::machine::core::kfunction::exec) layer and the scheduler.
//!
//! `exec` describes what to do next in its native currency — a [`Next`] of `KExpression`s,
//! a continuation, and an *unlifted* value. This adapter turns that into the scheduler's
//! **opaque [`Task`]** format:
//!
//! - wraps each *expression to dispatch* (effects / tail / join) as a [`DispatchTask`];
//! - wraps an `exec` continuation into a scheduler [`Continuation`];
//! - **lifts** the produced value out of the dying frame into the surviving arena — `exec`
//!   holds no lift handle, so lifting happens here, on the scheduler side.
//!
//! This is the only place that knows both `KExpression` (handed up by `exec`) and the scheduler.
//! The scheduler below sees only opaque `Task`s and lifted `Carried<'run>` values. The two
//! factory operations — task-from-expression and continuation-from-`exec`-continuation — are
//! deliberately general so other builtins that schedule body statements + a join (e.g. MODULE)
//! can reuse them.

use crate::machine::core::kfunction::exec::{DepResult, Frame, Resume as ExecResume, Next};
use crate::machine::core::CallArena;
use crate::machine::model::ast::KExpression;
use crate::machine::model::values::Carried;
use crate::machine::KError;

/// Opaque, owned unit of dispatchable work — the scheduler's currency. Running it yields an
/// [`Outcome`]; the scheduler never introspects it. Pure wrt the scheduler (no handle threaded).
pub trait Task<'run> {
    fn run(self: Box<Self>) -> Outcome<'run>;
}

/// A boxed task. Lifetime-free in `'frame` (any frame rides inside as an `Rc`).
pub type BoxTask<'run> = Box<dyn Task<'run> + 'run>;

/// A parked computation the scheduler stashes: it owns its frame (`ctx`), names the deps to
/// spawn+await (`join`), and re-enters `resume` with their results. Borrow-free — see
/// [`crate::machine::core::kfunction::exec`].
pub struct Continuation<'run> {
    pub ctx: Frame,
    pub join: Vec<BoxTask<'run>>,
    pub resume: Box<dyn FnOnce(Frame, &[DepResult<'run>]) -> Outcome<'run> + 'run>,
}

/// **adapter → scheduler (terminal).** Opaque and `'frame`-free: the value is already lifted to
/// `'run`, tails/suspensions carry their frames inside their tasks.
pub enum Outcome<'run> {
    Errored(KError),
    /// Lifted out of the frame by the adapter.
    Value(Carried<'run>),
    Tail {
        effects: Vec<BoxTask<'run>>,
        tail: BoxTask<'run>,
    },
    Suspend(Continuation<'run>),
}

/// A task that dispatches one koan expression in its owned frame — the one place `KExpression`
/// re-enters as a unit of scheduler work. `run` dispatches `expr`, gets a [`Next`] back from
/// `exec`/dispatch, and routes it through [`next_to_outcome`].
pub struct DispatchTask<'run> {
    pub ctx: Frame,
    pub expr: KExpression<'run>,
}

impl<'run> Task<'run> for DispatchTask<'run> {
    fn run(self: Box<Self>) -> Outcome<'run> {
        todo!("dispatch self.expr in self.ctx, then next_to_outcome(step, ctx)")
    }
}

/// Wrap an expression to dispatch as an opaque scheduler task (factory (a)).
pub fn task_from_expr<'run>(expr: KExpression<'run>, ctx: Frame) -> BoxTask<'run> {
    Box::new(DispatchTask { ctx, expr })
}

/// Wrap an `exec` continuation as a scheduler [`Continuation`] (factory (b)): its `resume`
/// composes the `exec` continuation with [`next_to_outcome`], so the scheduler stays opaque.
pub fn continuation_from<'run>(
    ctx: Frame,
    join: Vec<KExpression<'run>>,
    _exec_resume: ExecResume<'run>,
) -> Continuation<'run> {
    let join = join
        .into_iter()
        .map(|e| task_from_expr(e, ctx.clone()))
        .collect();
    Continuation {
        ctx,
        join,
        resume: Box::new(|_ctx, _results| todo!("next_to_outcome(exec_resume(ctx, results), ctx)")),
    }
}

/// Translate an `exec` [`Next`] into a scheduler [`Outcome`]: lift `Value`, wrap each expression
/// as a [`DispatchTask`], wrap a continuation. `ctx` is the frame the step ran in.
pub fn next_to_outcome<'run>(step: Next<'run>, ctx: Frame) -> Outcome<'run> {
    match step {
        Next::Errored(e) => Outcome::Errored(e),
        Next::Value(_unlifted) => {
            // TODO(rewrite): lift `_unlifted` out of `ctx.arena` into `ctx.arena.outer().arena`
            // via `super::lift::lift_kobject` / `lift_ktype`, then `Outcome::Value(lifted)`.
            let _dest: Option<&CallArena> = Some(&ctx.arena);
            todo!("lift the unlifted value, then Outcome::Value")
        }
        Next::Tail { effects, tail } => Outcome::Tail {
            effects: effects
                .into_iter()
                .map(|e| task_from_expr(e, ctx.clone()))
                .collect(),
            tail: task_from_expr(tail, ctx),
        },
        Next::Suspend { join, resume } => Outcome::Suspend(continuation_from(ctx, join, resume)),
    }
}
