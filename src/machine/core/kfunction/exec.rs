//! **Under construction, feature-gated (`exec-v2`).** The innermost layer of the parallel
//! body executor — pure koan semantics, no scheduler task format, no lifting.
//!
//! `exec` runs a body in its per-call frame and describes — in its *native* terms
//! ([`KExpression`], [`Carried`]) — what should happen next, as an [`ExecOutcome`]: it failed, it
//! produced a (still-unlifted) value, it tail-calls after some leading statements, or it suspends
//! awaiting some sub-expressions. It names *expressions to dispatch* and a *continuation* — never a
//! scheduler step, never the scheduler itself.
//!
//! The scheduler-aware shell that maps an [`ExecOutcome`] onto the scheduler is the gated branch in
//! `execute::dispatch` (`DispatchCtx::try_exec_v2_call`): it reuses the live dispatcher's
//! resolution, turns the outcome into a `BodyResult` (`Tail → tail_with_frame`, …), and lets the
//! scheduler lift any produced value at the done boundary. Keeping that out of here is what lets
//! `exec` stay scheduler-agnostic and `'run`-free.
//!
//! ## Two lifetimes
//!
//! [`ExecOutcome`] carries two, because the AST and the produced value genuinely differ: the
//! dispatchable expressions are **borrowed** from the long-lived, immutable AST (`'ast`, which
//! outlives the run), while a produced value lives in the call frame (`'frame`, which dies with
//! the call). `KExpression`'s invariance blocks collapsing them. `exec` holds no lift handle, so
//! it cannot move the value out of the frame; the scheduler lifts it at the done boundary.

use std::rc::Rc;

use crate::machine::core::{BindingIndex, CallArena, KError, KErrorKind, LexicalFrame, Scope};
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{
    elaborate_type_expr, DeferredReturn, ElabResult, Elaborator, KType, Record, ReturnType,
};
use crate::machine::model::values::{Carried, Held, KObject};

use super::body::{body_statement_refs, Body};
use super::KFunction;

/// A body's execution context: the per-call `arena` it runs in, plus its lexical `chain`. Owned
/// (both fields are `Rc`), so it carries no lifetime; the body re-projects its scope from the
/// arena on demand. The arena rides forward via the `Rc` — no borrow is stored.
#[derive(Clone)]
pub struct Frame {
    /// The per-call arena the body executes in: it backs allocations, and its child scope is the
    /// body's scope. Owned — supplied (and, for TCO, reset) by the scheduler.
    pub arena: Rc<CallArena>,
    /// The body's lexical position — the parent chain for sub-expressions it hands back.
    pub chain: Rc<LexicalFrame>,
}

impl Frame {
    /// The scope where bindings land and effects accumulate. Re-projected from the owned arena,
    /// bounded by `&self`: a transient borrow that never escapes.
    pub fn scope(&self) -> &Scope<'_> {
        self.arena.scope_bounded()
    }
}

/// A joined dep's resolved value, handed to a continuation ([`ExecOutcome::Suspend`]) on re-entry.
/// An errored dep short-circuits the scheduler's Combine before the resume runs, so this is always
/// the success value.
pub type DepResult<'frame> = Carried<'frame>;

/// The continuation of a suspended body: re-entered with the resolved join values, yielding the
/// body's terminal [`ExecOutcome`] (a `Value` — e.g. after a deferred-return type check — or an
/// `Errored`). In the reuse path the scheduler-side lowering wraps this into a `CombineFinish`, so
/// it mirrors that shape: dep values in, terminal outcome out (no frame re-read).
pub type Resume<'ast, 'frame> =
    Box<dyn FnOnce(&[DepResult<'frame>]) -> ExecOutcome<'ast, 'frame> + 'frame>;

/// **exec → scheduler.** What running a body describes next, in `exec`'s native currency. Two
/// lifetimes, because the AST and the produced value genuinely differ: the dispatchable
/// expressions are **borrowed** from the long-lived, immutable AST (`'ast`), while a produced
/// value lives in the call frame (`'frame`) until the scheduler lifts it. `KExpression`'s
/// invariance blocks collapsing the two.
pub enum ExecOutcome<'ast, 'frame> {
    /// The body failed; propagate the error.
    Errored(KError),
    /// The body produced its result — **still in the frame, unlifted.** The scheduler lifts it out
    /// to `'run` at the done boundary; `exec` holds no lift handle and cannot.
    Value(Carried<'frame>),
    /// Run the body as a flat sequence: dispatch each `leading` expression — the non-tail
    /// statements, whose results flow into the `Scope` as bindings and are otherwise discarded —
    /// then `tail` in the same frame, whose value is the body's result. All borrowed from the AST.
    Tail {
        leading: Vec<&'ast KExpression<'ast>>,
        tail: &'ast KExpression<'ast>,
    },
    /// Suspend: dispatch and await `join`, then re-enter `resume` with their resolved values to
    /// produce the body's terminal outcome (the deferred-return path: `join` = body statements,
    /// `resume` checks the body value against the per-call return type).
    Suspend {
        join: Vec<&'ast KExpression<'ast>>,
        resume: Resume<'ast, 'frame>,
    },
}

/// The new `invoke` for a user-defined function: bind `args` into `ctx`'s scope (a frame/scope
/// operation), then describe the body as an [`ExecOutcome`] — `Tail` of the non-tail statements +
/// the last, or `Suspend` for a deferred return. `ctx` is **borrowed** so the caller retains it
/// (its `chain` positions the body's `leading` statements when the scheduler dispatches them); the
/// carrier lifetime of `func` is free — only read. `args` is the argument record from
/// [`super::bind_by_name`] (a `Record<Carried>`, resolved values keyed by parameter name).
///
/// Pure wrt the scheduler: it mutates only `ctx`'s own scope (param binds) and, for a deferred
/// `TypeExpr` return, elaborates the return type inline against that scope; then describes the body
/// — `Tail` for a resolved return, `Suspend` (join = all statements, resume = the return-type
/// check) for a deferred one. Body statements are **borrowed** from `func` (`'ast`), never cloned.
/// (The deferred `Expression` return form needs a sub-dispatch and is excluded by the caller.)
pub fn run_user_fn<'ast, 'frame>(
    func: &'ast KFunction<'ast>,
    args: Record<Carried<'frame>>,
    ctx: &Frame,
) -> ExecOutcome<'ast, 'frame> {
    // Materialize the bound args as a record value **in the frame**, then bind each parameter to a
    // reference into the record's cell — one deep-clone per field (`Carried` → owned `Held`), and
    // the record carries its per-field type record. The record's cells double as the parameter
    // bindings (scope bindings store `&KObject`). Concentrated in `with_anchored_child` so the seed
    // fabricates no `&'a`.
    let bind = ctx
        .arena
        .with_anchored_child(|inner_arena, child| -> Result<(), KError> {
            let cells: Record<Held> = args.map(|carried| Held::from_carried(*carried));
            let args_record = inner_arena.alloc_object(KObject::record_of_held(cells));
            if let KObject::Record(cells, _types) = args_record {
                for (name, cell) in cells.iter() {
                    match cell {
                        Held::Object(object) => {
                            let _ = child.bind_value(name.clone(), object, BindingIndex::value(0));
                        }
                        // Type-denoting params (`Er`-style) register a type, not a value binding.
                        // The arg is an already-resolved type, so `type_identity_for` would just
                        // pass it through — register it directly (avoids the def-scope lifetime).
                        Held::Type(kt) => {
                            child.register_type(name.clone(), kt.clone(), BindingIndex::value(0));
                        }
                    }
                }
            }
            Ok(())
        });
    if let Err(e) = bind {
        return ExecOutcome::Errored(e);
    }

    let body_expr = match &func.body {
        Body::UserDefined(expr) => expr,
        // Builtin bodies are their own `BodyFn`s; this entry is user-defined only.
        Body::Builtin(_) => {
            return ExecOutcome::Errored(KError::new(crate::machine::KErrorKind::User(
                "run_user_fn called on a builtin body".to_string(),
            )))
        }
    };
    match &func.signature.return_type {
        ReturnType::Resolved(_) => {
            let mut leading = body_statement_refs(body_expr);
            let tail = leading
                .pop()
                .expect("body_statement_refs always yields at least one");
            ExecOutcome::Tail { leading, tail }
        }
        // Deferred return type referencing a parameter, in its surface `TypeExpr` form: elaborate it
        // against the per-call (param-bound) child scope, then suspend on all body statements. On
        // re-entry the resume checks the body's terminal value against the resolved return type.
        // The `Expression` form needs a sub-dispatch and is excluded by the caller's eligibility.
        ReturnType::Deferred(DeferredReturn::TypeExpr(type_expr)) => {
            let return_type = ctx.arena.with_anchored_child(|_inner_arena, child| {
                let mut elaborator = Elaborator::new(child);
                match elaborate_type_expr(&mut elaborator, type_expr) {
                    ElabResult::Done(kt) => kt,
                    // The param install + fn_def carrier scan jointly guarantee resolution; fall
                    // back to Any so the body's own dispatch surfaces any real error.
                    ElabResult::Park(_) | ElabResult::Unbound(_) => KType::Any,
                }
            });
            let join = body_statement_refs(body_expr);
            let body_terminal_idx = join.len() - 1;
            let summary = func.summarize();
            let resume: Resume<'ast, 'frame> = Box::new(move |results: &[DepResult<'frame>]| {
                let body_value = results[body_terminal_idx];
                let accepted = match body_value {
                    Carried::Object(object) => return_type.matches_value(object),
                    Carried::Type(kind) => return_type.matches_type(kind),
                };
                if accepted {
                    // No return-type stamp yet: the value already satisfies the declared type (the
                    // check passed), so it is at worst a subtype; the coarsening re-tag is a later
                    // increment.
                    ExecOutcome::Value(body_value)
                } else {
                    let got = match body_value {
                        Carried::Object(object) => object.ktype().name(),
                        Carried::Type(kind) => kind.name(),
                    };
                    ExecOutcome::Errored(
                        KError::new(KErrorKind::TypeMismatch {
                            arg: "<return>".to_string(),
                            expected: format!("{} (per-call return type)", return_type.name()),
                            got,
                        })
                        .with_frame(crate::machine::Frame::bare(
                            summary.clone(),
                            summary.clone(),
                        )),
                    )
                }
            });
            ExecOutcome::Suspend { join, resume }
        }
        // The `Expression` form is excluded by the caller's eligibility (it needs a sub-dispatch).
        ReturnType::Deferred(DeferredReturn::Expression(_)) => {
            ExecOutcome::Errored(KError::new(KErrorKind::User(
                "run_user_fn: deferred return-type expression is not yet supported".to_string(),
            )))
        }
    }
}
