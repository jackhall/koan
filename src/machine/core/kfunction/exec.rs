//! The innermost layer of the body executor — pure koan semantics, no scheduler task format, no
//! lifting.
//!
//! `exec` runs a body in its per-call frame and describes — in its *native* terms
//! ([`KExpression`], [`Carried`]) — what should happen next, as an [`ExecOutcome`]: it failed, it
//! tail-calls after some leading statements, or (a first-call deferred-`Expression` return) it
//! resolves a return-type sub-dispatch before tail-replacing. It names *expressions to dispatch* —
//! never a scheduler step, never the scheduler itself.
//!
//! The scheduler-aware shell that maps an [`ExecOutcome`] onto the scheduler is
//! `execute::dispatch::exec::invoke`: it reuses the live dispatcher's resolution, turns the outcome
//! into an `Outcome` (`Tail → Outcome::Continue`, …), and lets the scheduler lift any produced
//! value at the done boundary. Keeping that out of here is what lets `exec` stay scheduler-agnostic
//! and `'run`-free.
//!
//! ## Lifetimes
//!
//! [`run_user_fn`] takes two, because the AST and the delivered arguments genuinely differ: the
//! function and its body expressions are **borrowed** from the long-lived, immutable AST (`'ast`,
//! which outlives the run), while the arguments live in the caller's step (`'step`, which dies with
//! the call) until they are deep-cloned into the frame. [`ExecOutcome`] itself carries only `'ast`:
//! its dispatchable expressions are AST borrows, and a deferred-`Type` return rides as a `ret`
//! reference already re-homed into the captured-scope region (an `'ast`-lifetime ancestor), so no
//! frame-local value escapes on the outcome.

use std::rc::Rc;

use crate::machine::core::{BindingIndex, CallFrame, KError, KErrorKind, KoanRegion};
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{
    elaborate_type_identifier, DeferredReturn, ElabResult, Elaborator, KType, Record, ReturnType,
};
use crate::machine::model::values::{Carried, Held, KObject};
use crate::witnessed::reattach_with;

use super::body::{body_statement_refs, Body};
use super::KFunction;

/// A body's execution context: the per-call `region` it runs in. Owned (an `Rc`), so it carries no
/// lifetime; the body re-projects its scope from the region on demand. The region rides forward via
/// the `Rc` — no borrow is stored.
#[derive(Clone)]
pub struct ExecFrame {
    /// The per-call region the body executes in: it backs allocations, and its child scope is the
    /// body's scope. Owned — supplied (and, for TCO, reset) by the scheduler.
    pub region: Rc<CallFrame>,
}

/// **exec → scheduler.** What running a body describes next, in `exec`'s native currency. One
/// lifetime: its dispatchable expressions are **borrowed** from the long-lived, immutable AST
/// (`'ast`), and a deferred-`Type` return rides as a `ret` reference already re-homed into the
/// captured-scope region (also `'ast`-lived), so no frame-local produced value escapes on the
/// outcome — the scheduler still lifts the body's *value* at the done boundary, but not via this.
pub enum ExecOutcome<'ast> {
    /// The body failed; propagate the error.
    Errored(KError),
    /// Run the body as a flat sequence: dispatch each `leading` expression — the non-tail
    /// statements, whose results flow into the `Scope` as bindings and are otherwise discarded —
    /// then `tail` in the same frame, whose value is the body's result. All borrowed from the AST.
    /// `ret` is the return contract the scheduler stamps on the tail-replace — a proper tail call,
    /// so a recursive body stays TCO-flat.
    Tail {
        leading: Vec<&'ast KExpression<'ast>>,
        tail: &'ast KExpression<'ast>,
        ret: PerCallReturn<'ast>,
    },
    /// A deferred-`Expression` return on its **first** call: resolve `type_expr` (an async
    /// sub-dispatch — `Er.Carrier`, `sig WITH {…}`) as a single dep-finish dependency, run `leading` as
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
/// the signature (`FromSignature` → `ReturnContract::Function`); a deferred-`Type` return whose type
/// resolved synchronously carries it **already re-homed** into the captured-scope region (`Resolved`
/// → `ReturnContract::PerCall`), so the body tail-replaces and the lift boundary checks + stamps
/// against it — no dep-finish, TCO preserved. The re-home (via [`home_return_type`]) is what lets the
/// elaboration read the param-bound child scope at the frame brand yet hand back a `ret` reference
/// that outlives the dying frame.
pub enum PerCallReturn<'ast> {
    FromSignature,
    Resolved(&'ast KType<'ast>),
}

/// Home a deferred FN's resolved return *type* in `region` (the captured-scope region — a strict
/// ancestor the cart keeps live), so the `PerCall` contract's `ret` reference outlives the dying call
/// frame. The elaboration reads the param-bound child scope at the frame brand, so `kt`'s lifetime is
/// the (short) brand; the re-anchor caps the clone at `region`'s lifetime, witnessed by `region`
/// itself. Sound because a **non-module** return type's reach is `Rc`-owned (recursive sets) or
/// definition-stable (a signature / abstract member visible at the FN's definition, hence resident in
/// `region` or a lexical ancestor of it) — pinned independent of the dying frame. A concrete
/// first-class **module is rejected**: a module value's identity is not a return type (return a
/// signature or the `:Module` kind), and it is the one return type that would borrow the dying frame,
/// so erroring here is what lets the value carrier hold no per-value region anchor. (A module returned
/// as a *value* is unaffected — it rides the value channel's witness set like a returned closure.)
pub(crate) fn home_return_type<'r>(
    kt: &KType<'_>,
    region: &'r KoanRegion,
) -> Result<&'r KType<'r>, KError> {
    if matches!(kt, KType::Module { .. }) {
        return Err(KError::new(KErrorKind::ShapeError(
            "a module cannot be a function's return type; return a signature or the `:Module` kind"
                .to_string(),
        )));
    }
    // Re-anchor the (non-module, hence reach-stable) clone from the elaboration brand to `region`'s
    // lifetime, witnessed by `region` — the safe-signature `reattach_with`, so the home carries no
    // `unsafe` of its own beyond the substrate's single retype.
    let relocated: KType<'r> = reattach_with::<KType<'static>, _>(kt.clone(), region);
    Ok(region.alloc_ktype(relocated))
}

/// The new `invoke` for a user-defined function: bind `args` into `ctx`'s scope (a frame/scope
/// operation), then describe the body as an [`ExecOutcome`] — `Tail` of the non-tail statements +
/// the last, or `DeferredExprTail` for a first-call deferred-`Expression` return. `ctx` is
/// **borrowed** so the caller retains it; the
/// carrier lifetime of `func` is free — only read. `args` is the argument record from
/// [`super::bind_by_name`] (a `Record<Carried>`, resolved values keyed by parameter name).
///
/// Pure wrt the scheduler: it mutates only `ctx`'s own scope (param binds) and, for a deferred
/// `Type` return, elaborates the return type inline against that scope; then describes the body
/// as a `Tail` (the lift boundary checks + stamps against the carried `PerCall` contract) — or, for
/// a first-call deferred `Expression` return, a `DeferredExprTail` (the type needs a sub-dispatch).
/// `in_contract_chain` true means this is a subsequent tail call whose own contract keep-first would
/// discard, so it skips resolving its return type. Body statements are **borrowed** (`'ast`).
pub fn run_user_fn<'ast, 'step>(
    func: &'ast KFunction<'ast>,
    args: Record<Carried<'step>>,
    ctx: &ExecFrame,
    in_contract_chain: bool,
) -> ExecOutcome<'ast> {
    // Materialize the bound args as a record value **in the frame**, then bind each parameter to a
    // reference into the record's cell — one deep-clone per field (`Carried` → owned `Held`), and
    // the record carries its per-field type record. The record's cells double as the parameter
    // bindings (scope bindings store `&KObject`). Built at the frame brand: the deep-cloned record is
    // relocated into the brand region — a shortening of the caller `'step`, witnessed by the frame's
    // own region (the value outlives the synchronous brand) — so the seed fabricates no `&'a`. Its
    // foreign reach is pinned by the call scope's reach-set, folded at the bind seam before this runs.
    let bind = ctx.region.with_scope(|child| -> Result<(), KError> {
        let cells: Record<Held> = args.map(|carried| Held::from_carried(*carried));
        let args_value =
            reattach_with::<KObject<'static>, _>(KObject::record_of_held(cells), child.region);
        let args_record = child.region.alloc_object(args_value);
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
                // `Type` form (`-> Er`): elaborate it inline against the per-call (param-bound)
                // child scope at the frame brand, then **re-home** the resolved type into the
                // captured-scope region inside the open — so the elaborated `KType` is freed from the
                // brand (it lands in an `'ast`-lifetime ancestor) and rides the tail-replace as a
                // reference that outlives the dying frame.
                DeferredReturn::Type(type_expr) => {
                    let captured_region = func.captured_scope().region;
                    let homed = ctx.region.with_scope(|child| {
                        let mut elaborator = Elaborator::new(child);
                        let kt = match elaborate_type_identifier(&mut elaborator, type_expr) {
                            ElabResult::Done(kt) => kt,
                            // The param install + fn_def carrier scan jointly guarantee resolution;
                            // fall back to Any so the body's own dispatch surfaces any real error.
                            ElabResult::Park(_) | ElabResult::Unbound(_) => KType::Any,
                        };
                        home_return_type(&kt, captured_region)
                    });
                    let ret_ref = match homed {
                        Ok(ret_ref) => ret_ref,
                        Err(e) => return ExecOutcome::Errored(e),
                    };
                    let (leading, tail) = split_leading_tail(body_expr);
                    ExecOutcome::Tail {
                        leading,
                        tail,
                        ret: PerCallReturn::Resolved(ret_ref),
                    }
                }
                // `Expression` form (`-> Er.Carrier`, `sig WITH {…}`): the type needs a sub-dispatch,
                // so hand it back for the lowering to resolve as a dep-finish dependency before tail-replacing.
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
