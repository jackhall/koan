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

use crate::machine::core::{BindingIndex, CallArena, KError, LexicalFrame, Scope};
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::Record;
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

/// A joined dep's terminal status, handed to a continuation ([`ExecOutcome::Suspend`]) on re-entry —
/// **success or error only, no value**. Non-tail statements deposit their results into the
/// `Scope` (effects-through-scope), so the continuation re-reads `ctx.scope()` for any value it
/// needs and only inspects this to propagate a dep error. Carrying no value also keeps `exec`
/// free of the `'run` lift lifetime.
pub type DepResult = Result<(), KError>;

/// The continuation of a suspended body, in `exec`'s native terms: re-entered with a **borrow** of
/// its frame and the join terminals, yielding another [`ExecOutcome`]. Higher-ranked over the frame
/// borrow `'f`: a stored continuation outlives any single re-entry, so its produced-value lifetime
/// must be the frame it is *handed* on re-entry (`ExecOutcome<'ast, 'f>`), not a `'frame` baked into the
/// stored type (which would have to outlive the run — backwards). Borrow-free otherwise:
/// frame-local state is re-read via `frame.scope()`, not captured.
pub type Resume<'ast> =
    Box<dyn for<'f> FnOnce(&'f Frame, &[DepResult]) -> ExecOutcome<'ast, 'f> + 'ast>;

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
    /// Suspend: dispatch and await `join`, then re-enter `resume` with their terminals. `resume`'s
    /// produced-value lifetime is bound to the frame it is handed on re-entry, not this `'frame`.
    Suspend {
        join: Vec<&'ast KExpression<'ast>>,
        resume: Resume<'ast>,
    },
}

/// The new `invoke` for a user-defined function: bind `args` into `ctx`'s scope (a frame/scope
/// operation), then describe the body as an [`ExecOutcome`] — `Tail` of the non-tail statements +
/// the last, or `Suspend` for a deferred return. `ctx` is owned; the carrier lifetime of `func` is
/// free — only read. `args` is the argument record from [`super::bind_by_name`] (a `Record<Carried>`,
/// resolved values keyed by parameter name).
///
/// Resolved-return path only (deferred return → `Suspend` is a later increment). Pure: it
/// mutates only `ctx`'s own scope (param binds), then describes the body. The body statements are
/// **borrowed** from `func` (`'ast`), never cloned; `'frame` is free here (no value produced).
pub fn run_user_fn<'ast, 'frame>(
    func: &'ast KFunction<'ast>,
    args: Record<Carried<'frame>>,
    ctx: Frame,
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
    let mut leading = body_statement_refs(body_expr);
    let tail = leading
        .pop()
        .expect("body_statement_refs always yields at least one");
    ExecOutcome::Tail { leading, tail }
}
