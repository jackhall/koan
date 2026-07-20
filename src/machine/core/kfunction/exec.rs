//! The innermost layer of the body executor: pure koan semantics, no scheduler task format, no
//! lifting. `exec` runs a body in its per-call frame and describes what happens next in its native
//! terms ([`KExpression`], [`Carried`]) as an [`ExecOutcome`] — never a scheduler step. The
//! scheduler-aware shell that maps an outcome onto the scheduler is
//! `execute::dispatch::exec::invoke`; keeping it out of here is what lets `exec` stay
//! scheduler-agnostic and `'run`-free.
//!
//! ## Two lifetimes
//!
//! [`ExecOutcome`] carries two because the AST and the produced value genuinely differ: dispatchable
//! expressions are borrowed from the long-lived AST (`'ast`), while a deferred-`Type` return's
//! resolved type is re-homed into the captured-scope region but capped at the call's `'step` — the
//! contract lifetime the lift boundary consumes it within. `KExpression`'s invariance blocks
//! collapsing the two.

use crate::machine::{DeliveredCarried, KErrorKind};
use std::rc::Rc;

use crate::machine::core::{BindingIndex, CallFrame, KError, RegionBrand, Scope};
use crate::machine::model::Carried;
use crate::machine::model::KExpression;
use crate::machine::model::{DeferredReturn, KType, Record, ReturnType, TypeResolution};

use super::body::{body_statement_refs, Body};
use super::KFunction;

/// A body's execution context: the per-call `region` it runs in. Owned (an `Rc`), so it carries no
/// lifetime; the body re-projects its scope from the region on demand.
#[derive(Clone)]
pub struct ExecFrame {
    /// The per-call region the body executes in: it backs allocations and its child scope is the
    /// body's scope. Supplied by the scheduler; a tail hop supplies a freshly minted one.
    pub region: Rc<CallFrame>,
}

/// **exec → scheduler.** What running a body describes next, in `exec`'s native currency. See the
/// module docs for the two lifetimes.
pub enum ExecOutcome<'ast, 'step> {
    /// The body failed; propagate the error.
    Errored(KError),
    /// Run the body flat: dispatch each `leading` (non-tail) statement — results flow into the
    /// `Scope` as bindings, else discarded — then `tail` in the same frame, whose value is the
    /// result. `ret` is the return contract the scheduler stamps on the tail-replace, so a recursive
    /// body stays TCO-flat.
    Tail {
        leading: Vec<&'ast KExpression<'ast>>,
        tail: &'ast KExpression<'ast>,
        ret: PerCallReturn<'step>,
    },
    /// A deferred-`Expression` return on its **first** call: resolve `type_expr` (`er.Carrier`,
    /// `sig WITH {…}`) as a single dep-finish dependency, run `leading` as sibling statements, then
    /// tail-replace into `tail` carrying the resolved per-call type. Subsequent calls skip resolution
    /// under keep-first, so the recursion stays TCO-flat.
    DeferredExprTail {
        type_expr: &'ast KExpression<'ast>,
        leading: Vec<&'ast KExpression<'ast>>,
        tail: &'ast KExpression<'ast>,
    },
}

/// The return contract a [`ExecOutcome::Tail`] carries. A resolved-return FN reads its type off the
/// signature (`FromSignature` → `ReturnContract::Function`); a deferred-`Type` return whose type
/// resolved synchronously carries it already re-homed into the captured-scope region (`Resolved` →
/// `ReturnContract::PerCall`), so the lift boundary checks + stamps against it — no dep-finish, TCO
/// preserved. The re-home (via [`home_return_type`]) lets the elaboration read the param-bound child
/// scope at the frame brand yet hand back a `ret` reference at the call's `'step`.
pub enum PerCallReturn<'step> {
    FromSignature,
    Resolved(&'step KType),
}

/// Home a deferred FN's resolved return *type* into `captured`'s region (a live ancestor of the
/// call), capped at the caller-supplied `'a`. The elaboration reads the param-bound child scope at
/// the frame brand, so `kt`'s borrow is the short brand; the clone lands owned in the captured
/// region and comes back at `'a`.
///
/// The cap is load-bearing: callers pass the **contract** lifetime (`'step`), not the captured
/// region's full lifetime. That is return-contract discipline — a `ret` reference must not outlive
/// the window the lift boundary consumes it in — and it holds independently of residence, which a
/// `KType` has none of: the clone is owned data stored through the single door, so it borrows only
/// the destination region.
pub(crate) fn home_return_type<'captured: 'a, 'a>(
    kt: &KType,
    captured: &Scope<'captured>,
) -> &'a KType {
    // Shorten the brand (covariant) before the store, so the resident reference comes back at the
    // contract lifetime rather than the captured region's own.
    let brand: RegionBrand<'a> = captured.brand();
    brand.alloc_ktype(kt.clone())
}

/// `invoke` for a user-defined function: bind `args` into `ctx`'s scope, then describe the body as an
/// [`ExecOutcome`] — `Tail` of the non-tail statements + the last, or `DeferredExprTail` for a
/// first-call deferred-`Expression` return. `ctx` is borrowed so the caller retains it. `args` is the
/// argument record from [`super::bind_by_name`] (resolved values keyed by parameter name).
///
/// Pure wrt the scheduler: it mutates only `ctx`'s own scope (param binds) and, for a deferred `Type`
/// return, elaborates the return type inline against that scope. `in_contract_chain` true means this
/// is a subsequent tail call whose contract keep-first would discard, so it skips resolving its return
/// type. Body statements are borrowed (`'ast`).
pub fn run_user_fn<'ast, 'step>(
    func: &'ast KFunction<'ast>,
    args: Record<Carried<'step>>,
    arg_carriers: &Record<&DeliveredCarried>,
    ctx: &ExecFrame,
    in_contract_chain: bool,
) -> ExecOutcome<'ast, 'step>
where
    'ast: 'step,
{
    // Bind each parameter into the frame's own scope through the value/type doors. An object is
    // deep-copied into the frame region under the reach its own delivered arg carrier mints
    // (`bind_delivered`); a region-pure object argument has no carrier and takes the checked tier. A
    // type is owned data, so it crosses by clone and lands in the frame region through the single
    // storage door (`register_type_delivered`), pinning nothing. Built at the frame brand so nothing
    // fabricates a free `&'a`.
    let bind = ctx.region.with_scope(|child| -> Result<(), KError> {
        for (name, carried) in args.iter() {
            let carrier = arg_carriers.get(name).copied();
            match *carried {
                Carried::Object(object) => match carrier {
                    // The projection is identity — the whole delivered value binds. The copy is a
                    // deep clone into the frame region, so the carrier's residence-only host is not
                    // part of its reach (a tail call's retiring frame must not ride this binding).
                    Some(cell) => {
                        child.bind_delivered(name.clone(), cell, BindingIndex::value(0), |c| {
                            Ok(c.object())
                        })?;
                    }
                    None => {
                        child.bind_checked(
                            name.clone(),
                            object.deep_clone(),
                            BindingIndex::value(0),
                        )?;
                    }
                },
                // Type-denoting params (a `:Signature`-kind slot, a type alias) register a type, not a
                // value binding. The arg is already a resolved type; the door clones it into the
                // frame region. A *module* argument is a value and takes the Object arm above.
                Carried::Type(kt) => {
                    child.register_type_delivered(
                        name.clone(),
                        kt.clone(),
                        BindingIndex::value(0),
                    )?;
                }
                // Dispatch resolves every type-denoting argument before the call, so a name that
                // is still unlowered here names nothing bindable.
                Carried::UnresolvedType(ti) => {
                    return Err(KError::new(KErrorKind::UnboundName(ti.render())));
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
            // Subsequent tail call inside a contract chain: keep-first discards this call's contract,
            // so skip resolving its return type and tail-replace like any resolved return.
            if in_contract_chain {
                let (leading, tail) = split_leading_tail(body_expr);
                return ExecOutcome::Tail {
                    leading,
                    tail,
                    ret: PerCallReturn::FromSignature,
                };
            }
            match deferred {
                // `Type` form (`-> er`): elaborate inline against the per-call child scope at the
                // frame brand, then re-home the resolved type into the captured-scope region so it is
                // freed from the brand and rides the tail-replace as a `'step` reference.
                DeferredReturn::Type(type_expr) => {
                    // Resolve against the param-bound child scope, then clone the resolved type
                    // into the captured region. The home is capped at `'step` so the contract `ret`
                    // can't out-claim the window the lift boundary consumes it in — see
                    // `home_return_type`.
                    let homed = ctx.region.with_scope(|child| {
                        let captured = func.captured_scope();
                        let homed: Result<&'step KType, KError> = match child
                            .resolve_type_identifier(type_expr, None)
                        {
                            TypeResolution::Done(kt) => Ok(home_return_type(kt, captured)),
                            // A park at this point cannot be honored — the body is about to
                            // run — so fall back to Any and let the body's own dispatch surface
                            // any real error.
                            TypeResolution::Park(_) => Ok(home_return_type(&KType::Any, captured)),
                            // A miss is a real error: the return names no type. Surfacing it
                            // here rather than widening to Any is what makes `-> some_value` (a
                            // return slot naming a value — a module included) a diagnostic
                            // instead of a silently unconstrained return.
                            TypeResolution::Unbound(message) => {
                                Err(KError::new(KErrorKind::ShapeError(message)))
                            }
                        };
                        homed
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
                // `Expression` form (`-> er.Carrier`, `sig WITH {…}`): the type needs a sub-dispatch,
                // so hand it back to resolve as a dep-finish dependency before tail-replacing.
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
