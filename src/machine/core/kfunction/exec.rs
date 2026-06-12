//! The innermost layer of the body executor — pure koan semantics, no scheduler task format, no
//! lifting.
//!
//! `exec` runs a body in its per-call frame and describes — in its *native* terms
//! ([`KExpression`], [`Carried`]) — what should happen next, as an [`ExecOutcome`]: it failed, it
//! produced a (still-unlifted) value, it tail-calls after some leading statements, or (a first-call
//! deferred-`Expression` return) it resolves a return-type sub-dispatch before tail-replacing. It
//! names *expressions to dispatch* — never a scheduler step, never the scheduler itself.
//!
//! The scheduler-aware shell that maps an [`ExecOutcome`] onto the scheduler is
//! `execute::dispatch::exec::invoke`: it reuses the live dispatcher's resolution, turns the outcome
//! into a `BodyResult` (`Tail → tail_with_frame_contract`, …), and lets the scheduler lift any produced
//! value at the done boundary. Keeping that out of here is what lets `exec` stay scheduler-agnostic
//! and `'run`-free.
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
    /// `ret` is the return contract the scheduler stamps on the tail-replace — a proper tail call,
    /// so a recursive body stays TCO-flat.
    Tail {
        leading: Vec<&'ast KExpression<'ast>>,
        tail: &'ast KExpression<'ast>,
        ret: PerCallReturn<'frame>,
    },
    /// A deferred-`Expression` return on its **first** call: resolve `type_expr` (an async
    /// sub-dispatch — `Er.Carrier`, `sig WITH {…}`) as a single Combine dep, run `leading` as
    /// sibling statements, then tail-replace into `tail` carrying the resolved per-call type as a
    /// `PerCall` contract. A proper tail call once the type is known, so the recursion (whose
    /// subsequent calls skip resolution under keep-first) stays TCO-flat.
    DeferredExprTail {
        type_expr: &'ast KExpression<'ast>,
        leading: Vec<&'ast KExpression<'ast>>,
        tail: &'ast KExpression<'ast>,
    },
}

/// The return contract a [`ExecOutcome::Tail`] carries. A resolved-return FN reads its type off
/// the signature (`FromSignature` → `ReturnContract::Function`); a deferred-return FN whose type
/// resolved synchronously carries the resolved `KType` (`Resolved` → `ReturnContract::PerCall`),
/// so the body tail-replaces and the lift boundary checks + stamps against it — no Combine, TCO
/// preserved.
pub enum PerCallReturn<'frame> {
    FromSignature,
    Resolved(KType<'frame>),
}

/// The new `invoke` for a user-defined function: bind `args` into `ctx`'s scope (a frame/scope
/// operation), then describe the body as an [`ExecOutcome`] — `Tail` of the non-tail statements +
/// the last, or `DeferredExprTail` for a first-call deferred-`Expression` return. `ctx` is
/// **borrowed** so the caller retains it
/// (its `chain` positions the body's `leading` statements when the scheduler dispatches them); the
/// carrier lifetime of `func` is free — only read. `args` is the argument record from
/// [`super::bind_by_name`] (a `Record<Carried>`, resolved values keyed by parameter name).
///
/// Pure wrt the scheduler: it mutates only `ctx`'s own scope (param binds) and, for a deferred
/// `TypeExpr` return, elaborates the return type inline against that scope; then describes the body
/// as a `Tail` (the lift boundary checks + stamps against the carried `PerCall` contract) — or, for
/// a first-call deferred `Expression` return, a `DeferredExprTail` (the type needs a sub-dispatch).
/// `in_contract_chain` true means this is a subsequent tail call whose own contract keep-first would
/// discard, so it skips resolving its return type. Body statements are **borrowed** (`'ast`).
pub fn run_user_fn<'ast, 'frame>(
    func: &'ast KFunction<'ast>,
    args: Record<Carried<'frame>>,
    ctx: &Frame,
    in_contract_chain: bool,
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
        // Builtin bodies run through the action harness; this entry is user-defined only.
        Body::Builtin(_) => {
            return ExecOutcome::Errored(KError::new(crate::machine::KErrorKind::User(
                "run_user_fn called on an action builtin body".to_string(),
            )))
        }
    };
    match &func.signature.return_type {
        ReturnType::Resolved(_) => {
            let (leading, tail) = split_leading_tail(body_expr);
            ExecOutcome::Tail {
                leading,
                tail,
                ret: PerCallReturn::FromSignature,
            }
        }
        ReturnType::Deferred(deferred) => {
            // A subsequent tail call already inside a contract chain: keep-first discards this
            // call's contract, so skip resolving its return type and tail-replace the body like any
            // resolved return (the kept first contract is what the chain's value is checked against).
            if in_contract_chain {
                let (leading, tail) = split_leading_tail(body_expr);
                return ExecOutcome::Tail {
                    leading,
                    tail,
                    ret: PerCallReturn::FromSignature,
                };
            }
            match deferred {
                // `TypeExpr` form (`-> Er`): elaborate it inline against the per-call (param-bound)
                // child scope and carry the resolved type on the tail-replace.
                DeferredReturn::TypeExpr(type_expr) => {
                    let return_type = ctx.arena.with_anchored_child(|_inner_arena, child| {
                        let mut elaborator = Elaborator::new(child);
                        match elaborate_type_expr(&mut elaborator, type_expr) {
                            ElabResult::Done(kt) => kt,
                            // The param install + fn_def carrier scan jointly guarantee resolution;
                            // fall back to Any so the body's own dispatch surfaces any real error.
                            ElabResult::Park(_) | ElabResult::Unbound(_) => KType::Any,
                        }
                    });
                    let (leading, tail) = split_leading_tail(body_expr);
                    ExecOutcome::Tail {
                        leading,
                        tail,
                        ret: PerCallReturn::Resolved(return_type),
                    }
                }
                // `Expression` form (`-> Er.Carrier`, `sig WITH {…}`): the type needs a sub-dispatch,
                // so hand it back for the lowering to resolve as a Combine dep before tail-replacing.
                DeferredReturn::Expression(return_expr) => {
                    let (leading, tail) = split_leading_tail(body_expr);
                    ExecOutcome::DeferredExprTail {
                        type_expr: return_expr,
                        leading,
                        tail,
                    }
                }
            }
        }
    }
}

/// Split a body into its leading (non-tail) statements and the terminal `tail` whose value is the
/// body's result. Always yields at least the tail.
fn split_leading_tail<'ast>(
    body_expr: &'ast KExpression<'ast>,
) -> (Vec<&'ast KExpression<'ast>>, &'ast KExpression<'ast>) {
    let mut leading = body_statement_refs(body_expr);
    let tail = leading
        .pop()
        .expect("body_statement_refs always yields at least one");
    (leading, tail)
}

