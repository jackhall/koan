//! **Under construction, feature-gated (`exec-v2`).** The `'run`-aware shell between the pure
//! [`exec`](crate::machine::core::kfunction::exec) layer and the scheduler.
//!
//! `exec` describes what to do next in its native currency — an [`ExecOutcome`] of `KExpression`s,
//! a continuation, and an *unlifted* value. This adapter turns that into the scheduler's
//! **opaque [`Task`]** format:
//!
//! - wraps each *expression to dispatch* (leading / tail / join) as a [`DispatchTask`];
//! - wraps an `exec` continuation into a scheduler [`Continuation`];
//! - **lifts** the produced value out of the dying frame into the surviving arena — `exec`
//!   holds no lift handle, so lifting happens here, on the scheduler side.
//!
//! This is the only place that knows both `KExpression` (handed up by `exec`) and the scheduler.
//! The scheduler below sees only opaque `Task`s and lifted `Carried<'run>` values. The two
//! factory operations — task-from-expression and continuation-from-`exec`-continuation — are
//! deliberately general so other builtins that schedule body statements + a join (e.g. MODULE)
//! can reuse them.

use std::rc::Rc;

use super::lift::{lift_kobject, lift_ktype};
use crate::machine::core::kfunction::exec::{DepResult, ExecOutcome, Frame, Resume as ExecResume};
use crate::machine::core::{CallArena, RuntimeArena};
use crate::machine::model::ast::KExpression;
use crate::machine::model::values::{Carried, KObject};
use crate::machine::model::KType;
use crate::machine::KError;

/// Opaque, owned unit of dispatchable work — the scheduler's currency. Running it yields a
/// [`SchedOutcome`]; the scheduler never introspects it. Pure wrt the scheduler (no handle
/// threaded).
/// `dest` is the consumer's surviving arena — where a produced value is lifted to; the driver
/// knows it from the dependency graph (who awaits this task's result).
pub trait Task<'run> {
    fn run(self: Box<Self>, dest: &'run RuntimeArena) -> SchedOutcome<'run>;
}

/// A boxed task. Lifetime-free in `'frame` (any frame rides inside as an `Rc`).
pub type BoxTask<'run> = Box<dyn Task<'run> + 'run>;

/// The scheduler-side resume: re-entered with the (owned) frame, the consumer's surviving arena
/// for the lift, and the join terminals; yields a terminal [`SchedOutcome`].
pub type ResumeFn<'run> =
    Box<dyn FnOnce(Frame, &'run RuntimeArena, &[DepResult]) -> SchedOutcome<'run> + 'run>;

/// A parked computation the scheduler stashes: it owns its frame (`ctx`), names the deps to
/// spawn+await (`join`), and re-enters `resume` with their results. Borrow-free — see
/// [`crate::machine::core::kfunction::exec`].
pub struct Continuation<'run> {
    pub ctx: Frame,
    pub join: Vec<BoxTask<'run>>,
    pub resume: ResumeFn<'run>,
}

/// **adapter → scheduler (terminal).** Opaque and `'frame`-free: the value is already lifted to
/// `'run`, tails/suspensions carry their frames inside their tasks.
pub enum SchedOutcome<'run> {
    Errored(KError),
    /// Lifted out of the frame by the adapter.
    Value(Carried<'run>),
    Tail {
        leading: Vec<BoxTask<'run>>,
        tail: BoxTask<'run>,
    },
    Suspend(Continuation<'run>),
}

/// A task that dispatches one koan expression in its owned frame — the one place `KExpression`
/// re-enters as a unit of scheduler work. `run` dispatches `expr`, gets an [`ExecOutcome`] back from
/// `exec`/dispatch, and routes it through [`to_sched_outcome`].
pub struct DispatchTask<'ast> {
    pub ctx: Frame,
    pub expr: &'ast KExpression<'ast>,
}

impl<'ast, 'run> Task<'run> for DispatchTask<'ast>
where
    'ast: 'run,
{
    fn run(self: Box<Self>, _dest: &'run RuntimeArena) -> SchedOutcome<'run> {
        todo!("dispatch self.expr in self.ctx, then to_sched_outcome(exec_outcome, ctx, dest)")
    }
}

/// Wrap a borrowed AST expression to dispatch as an opaque scheduler task (factory (a)). The
/// borrow (`'ast`) outlives the run (`'ast: 'run`), so the task is storable at `'run`.
pub fn task_from_expr<'ast, 'run>(expr: &'ast KExpression<'ast>, ctx: Frame) -> BoxTask<'run>
where
    'ast: 'run,
{
    Box::new(DispatchTask { ctx, expr })
}

/// Wrap an `exec` continuation as a scheduler [`Continuation`] (factory (b)): its `resume`
/// composes the `exec` continuation with [`to_sched_outcome`], so the scheduler stays opaque. The
/// frame is cloned (cheap — two `Rc`s) because both the `exec` resume and `to_sched_outcome` need
/// it.
pub fn continuation_from<'ast, 'run>(
    ctx: Frame,
    join: Vec<&'ast KExpression<'ast>>,
    exec_resume: ExecResume<'ast>,
) -> Continuation<'run>
where
    'ast: 'run,
{
    let join = join
        .into_iter()
        .map(|e| task_from_expr(e, ctx.clone()))
        .collect();
    Continuation {
        ctx,
        join,
        resume: Box::new(move |ctx, dest, results| {
            // The exec outcome may borrow `ctx` (its `Value` rides the frame), so clone the
            // lifetime-free `Frame` for the lift/task-wrapping; both are shared reads of the `Rc`s.
            let exec_outcome = exec_resume(&ctx, results);
            to_sched_outcome(exec_outcome, ctx.clone(), dest)
        }),
    }
}

/// Lift a produced value out of the dying frame into the consumer's surviving arena (`dest`),
/// re-typing `'frame` → `'run`. This is a **relocation**, not a subtype coercion: `lift_kobject` /
/// `lift_ktype` deep-clone the value into self-contained data (frame-borrowing parts re-anchored
/// onto lifetime-free `Rc<CallArena>` handles), so the result borrows nothing from the `'frame`
/// arena, and it is allocated into the `'run`-lived `dest` for a stable `&'run`. The physical copy
/// is what makes the longer `'run` truthful. Mirrors `compute_done_output`'s lift+alloc.
fn lift_value<'frame, 'run>(
    value: Carried<'frame>,
    dying: &Rc<CallArena>,
    dest: &'run RuntimeArena,
) -> Carried<'run> {
    match value {
        Carried::Object(o) => {
            let lifted = lift_kobject(o, dying);
            // SAFETY: `lifted` is self-contained after `lift_kobject` (deep clone + frame-`Rc`
            // re-anchor), so it borrows nothing from the `'frame` arena and re-labelling
            // `'frame` → `'run` is sound; it is moved straight into the `'run` `dest`.
            let lifted: KObject<'run> =
                unsafe { core::mem::transmute::<KObject<'_>, KObject<'run>>(lifted) };
            Carried::Object(dest.alloc_object(lifted))
        }
        Carried::Type(t) => {
            let lifted = lift_ktype(t, dying);
            // SAFETY: as above — `lift_ktype` yields self-contained data (`RecursiveSet`/`Module`
            // frames ride `Rc`), so the `'frame` → `'run` re-label is sound before `dest` alloc.
            let lifted: KType<'run> =
                unsafe { core::mem::transmute::<KType<'_>, KType<'run>>(lifted) };
            Carried::Type(dest.alloc_ktype(lifted))
        }
    }
}

/// Translate an `exec` [`ExecOutcome`] into a scheduler [`SchedOutcome`]: lift `Value` into `dest`
/// (`'frame` → `'run`), wrap each borrowed expression as a [`DispatchTask`], wrap a continuation.
/// `ctx` is the frame the body ran in; `dest` is the consumer's surviving arena.
pub fn to_sched_outcome<'ast, 'frame, 'run>(
    exec_outcome: ExecOutcome<'ast, 'frame>,
    ctx: Frame,
    dest: &'run RuntimeArena,
) -> SchedOutcome<'run>
where
    'ast: 'run,
{
    match exec_outcome {
        ExecOutcome::Errored(e) => SchedOutcome::Errored(e),
        ExecOutcome::Value(unlifted) => SchedOutcome::Value(lift_value(unlifted, &ctx.arena, dest)),
        ExecOutcome::Tail { leading, tail } => SchedOutcome::Tail {
            leading: leading
                .into_iter()
                .map(|e| task_from_expr(e, ctx.clone()))
                .collect(),
            tail: task_from_expr(tail, ctx),
        },
        ExecOutcome::Suspend { join, resume } => {
            SchedOutcome::Suspend(continuation_from(ctx, join, resume))
        }
    }
}

/// **Test harness only — not the real driver.** Runs `task` to its terminal value over an explicit
/// work-stack (a trampoline: never Rust recursion, since Rust has no guaranteed TCO and the
/// work-queue scheduler exists precisely to keep koan's depth off the Rust stack). A `Tail` expands
/// in place: push `tail`, then `leading` in reverse, so a body's leading expressions run first (in
/// order) for the `Scope` bindings they produce. A `Value` is the body's result iff the stack is
/// now empty; otherwise it is a leading expression's discarded value.
///
/// **Only valid for placeholder-free bodies.** A name resolving to a scope *placeholder* is a
/// cross-node dataflow dependency: the producer is another scheduler node, woken via `pending_deps`
/// when it terminalizes. This loop owns only `task`'s own tree — no sibling work-queue, no wake — so
/// it cannot run the producer; such a dispatch parks (`Suspend`, `todo!()` here) or, for an ambient
/// placeholder, deadlocks. The real driver must *be* the scheduler's work-queue. This exists to
/// exercise the leaf/tail seam end-to-end where no dispatch parks.
pub fn run_to_value<'run>(
    task: BoxTask<'run>,
    dest: &'run RuntimeArena,
) -> Result<Carried<'run>, KError> {
    let mut stack: Vec<BoxTask<'run>> = vec![task];
    while let Some(task) = stack.pop() {
        match task.run(dest) {
            SchedOutcome::Errored(e) => return Err(e),
            SchedOutcome::Value(value) => {
                if stack.is_empty() {
                    return Ok(value);
                }
                // A leading expression's value — discarded; its `Scope` effects already landed.
            }
            SchedOutcome::Tail { mut leading, tail } => {
                stack.push(tail);
                while let Some(expr) = leading.pop() {
                    stack.push(expr);
                }
            }
            SchedOutcome::Suspend(_) => {
                todo!("driver: park on Suspend (joins) — needs the scheduler integration")
            }
        }
    }
    unreachable!("stack starts non-empty and only empties by returning the final Value")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::model::values::KObject;

    /// Stub task: terminalizes straight to a fixed value.
    struct ValueTask<'run> {
        value: Carried<'run>,
    }
    impl<'run> Task<'run> for ValueTask<'run> {
        fn run(self: Box<Self>, _dest: &'run RuntimeArena) -> SchedOutcome<'run> {
            SchedOutcome::Value(self.value)
        }
    }

    /// Stub task: runs `leading`, then tails into `tail`.
    struct TailTask<'run> {
        leading: Vec<BoxTask<'run>>,
        tail: BoxTask<'run>,
    }
    impl<'run> Task<'run> for TailTask<'run> {
        fn run(self: Box<Self>, _dest: &'run RuntimeArena) -> SchedOutcome<'run> {
            SchedOutcome::Tail {
                leading: self.leading,
                tail: self.tail,
            }
        }
    }

    #[test]
    fn driver_runs_leading_then_tail_to_value() {
        let arena = RuntimeArena::new();
        let discarded = Carried::Object(arena.alloc_object(KObject::Number(1.0)));
        let result = Carried::Object(arena.alloc_object(KObject::Number(7.0)));
        let tail_task: BoxTask<'_> = Box::new(TailTask {
            leading: vec![Box::new(ValueTask { value: discarded })],
            tail: Box::new(ValueTask { value: result }),
        });
        match run_to_value(tail_task, &arena).expect("drives to a value") {
            Carried::Object(KObject::Number(n)) => assert_eq!(*n, 7.0),
            _ => panic!("expected Number(7)"),
        }
    }
}
