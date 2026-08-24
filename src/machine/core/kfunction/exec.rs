//! The innermost layer of the body executor: pure koan semantics, no scheduler task format, no
//! lifting. `exec` runs a body in its per-call frame and describes what happens next in its native
//! terms ([`KExpression`], [`Carried`]) as an [`ExecOutcome`] — never a scheduler step. The
//! scheduler-aware shell that maps an outcome onto the scheduler is
//! `execute::decide::exec::invoke`; keeping it out of here is what lets `exec` stay
//! scheduler-agnostic and `'run`-free.
//!
//! ## One lifetime
//!
//! [`ExecOutcome`] carries a single lifetime `'ast`: dispatchable expressions are borrowed from the
//! long-lived AST. A deferred-`Type` return's resolved type is a `Copy` `KType` handle, lifetime-free,
//! so it rides the outcome by value with no second lifetime to thread.

use crate::machine::{DeliveredCarried, KErrorKind};
use std::rc::Rc;

use crate::machine::core::{BindingIndex, CallFrame, DeclarationSite, KError};
use crate::machine::model::Carried;
use crate::machine::model::KExpression;
use crate::machine::model::{DeferredReturn, KType, ReturnType, TypeResolution};

use super::KFunction;
use super::body::{Body, body_statement_refs};
use crate::machine::model::{
    BindKind, BinderSymbol, RunRegistries, TypeSymbol, render_label, wrong_binder_class,
};

/// A body's execution context: the per-call `region` it runs in. Owned (an `Rc`), so it carries no
/// lifetime; the body re-projects its scope from the region on demand.
#[derive(Clone)]
pub struct ExecFrame {
    /// The per-call region the body executes in: it backs allocations and its child scope is the
    /// body's scope. Supplied by the scheduler; a tail hop supplies a freshly minted one.
    pub region: Rc<CallFrame>,
}

/// **exec → scheduler.** What running a body describes next, in `exec`'s native currency. The
/// `'ast` lifetime is the borrowed body statements; the resolved return handle is `Copy` and
/// lifetime-free.
pub enum ExecOutcome<'ast> {
    /// The body failed; propagate the error.
    Errored(KError),
    /// Run the body flat: dispatch each `leading` (non-tail) statement — results flow into the
    /// `Scope` as bindings, else discarded — then `tail` in the same frame, whose value is the
    /// result. `ret` is the return contract the scheduler stamps on the tail-replace, so a recursive
    /// body stays TCO-flat.
    Tail {
        leading: Vec<&'ast KExpression<'ast>>,
        tail: &'ast KExpression<'ast>,
        ret: PerCallReturn,
    },
    /// A deferred-`Expression` return on its **first** call: resolve `type_expr` (`er.Carrier`,
    /// `sig WITH {…}`) as a single dep-finish dependency, run `leading` as sibling statements, then
    /// tail-replace into `tail` carrying the resolved per-call type. Subsequent calls skip resolution
    /// under keep-first, so the recursion stays TCO-flat.
    DeferredExprTail {
        type_expr: KExpression<'ast>,
        leading: Vec<&'ast KExpression<'ast>>,
        tail: &'ast KExpression<'ast>,
    },
}

/// The return contract a [`ExecOutcome::Tail`] carries. A resolved-return FN reads its type off the
/// signature (`FromSignature` → `ReturnContract::Function`); a deferred-`Type` return whose type
/// resolved synchronously carries the resolved handle (`Resolved` → `ReturnContract::PerCall`), so
/// the lift boundary checks + stamps against it — no dep-finish, TCO preserved. The handle is
/// `Copy` and lifetime-free, so it rides the tail-replace with no re-home.
pub enum PerCallReturn {
    FromSignature,
    Resolved(KType),
}

/// `invoke` for a user-defined function: bind `args` into `ctx`'s scope, then describe the body as an
/// [`ExecOutcome`] — `Tail` of the non-tail statements + the last, or `DeferredExprTail` for a
/// first-call deferred-`Expression` return. `ctx` is borrowed so the caller retains it. `args` is the
/// call's arguments as **delivery envelopes** in the signature's declaration order, selected out of
/// the carriers the dispatcher lifted off the call expression by `part_slots` — nothing is keyed.
/// Envelopes only, not a value slice beside an envelope slice: the envelope already carries the
/// [`Carried`] it delivers, so an argument with no envelope is not a state the bind can be handed.
///
/// Pure wrt the scheduler: it mutates only `ctx`'s own scope (param binds) and, for a deferred `Type`
/// return, elaborates the return type inline against that scope. `in_contract_chain` true means this
/// is a subsequent tail call whose contract keep-first would discard, so it skips resolving its return
/// type. Body statements are borrowed (`'ast`).
pub fn run_user_fn<'ast>(
    func: &'ast KFunction<'ast>,
    args: &[&DeliveredCarried],
    ctx: &ExecFrame,
    in_contract_chain: bool,
    registries: &RunRegistries,
) -> ExecOutcome<'ast> {
    // Bind each parameter into the frame's own scope through the value/type doors, off the one
    // envelope the argument arrived in. An object is deep-copied into the frame region under the
    // reach its own delivered carrier mints (`bind_delivered`) — every value argument arrives with
    // an envelope, a region-pure literal included, because the frame scope opens at a `for<'b>`
    // brand a bare caller reference cannot cross. A type is owned data, so it crosses by clone and
    // lands in the frame region through the single storage door (`register_type_direct`), pinning
    // nothing. Built at the frame brand so nothing fabricates a free `&'a`.
    let bind = ctx.region.with_scope(|child| -> Result<(), KError> {
        // The frame's own scope: minted for this call and not yet published, so the parameter binds
        // take the construction door rather than riding a step outcome.
        let gate = &mut crate::machine::core::bindings::WriteGate::for_unpublished_scope();
        // The signature's own parameter schema names each slot; the slice supplies its value.
        // Nothing was re-keyed for this call — the pair is zipped in declaration order.
        // The schema's names are classified where the signature was built, and the binding tables
        // key by that same vocabulary, so a bind reads the symbol straight off the schema: no
        // interner reach, and nothing allocated per parameter.
        for ((binder, _), delivered) in func.signature.params().iter().zip(args) {
            match (binder, arg_channel(delivered)) {
                // The projection is identity — the whole delivered value binds. The copy is a deep
                // clone into the frame region, so the carrier's residence-only host is not part of
                // its reach (a tail call's retiring frame must not ride this binding).
                (BinderSymbol::Value(name), ArgChannel::Value) => {
                    child.bind_delivered_direct(
                        *name,
                        delivered,
                        BindingIndex::value(0),
                        |c| Ok(c.object()),
                        registries,
                        gate,
                    )?;
                }
                // Type-denoting params (a `:Signature`-kind slot, a type alias) register a type, not a
                // value binding. The arg is already a resolved type; the door clones it into the
                // frame region. A *module* argument is a value and takes the Object arm above.
                //
                // A type-denoting FN parameter is a per-call frame-scope binding, not a declaration
                // statement subject to same-declaration checks, so it takes the born-with-the-scope
                // site.
                (BinderSymbol::Type(name), ArgChannel::Type(kt)) => {
                    child.register_type_direct(
                        *name,
                        kt,
                        DeclarationSite::AT_CONSTRUCTION,
                        registries,
                        gate,
                    )?;
                }
                // Dispatch resolves every type-denoting argument before the call, so a name that
                // is still unlowered here names nothing bindable.
                (_, ArgChannel::Unresolved(name)) => {
                    return Err(KError::new(KErrorKind::UnboundName(render_label(
                        name.symbol(),
                        registries,
                    ))));
                }
                // The parameter's token class and the argument's channel disagree: a Type token
                // names a type and a value token names a value, so neither can take the other's
                // delivery.
                (BinderSymbol::Value(name), ArgChannel::Type(_)) => {
                    let name = render_label(name.symbol(), registries);
                    return Err(KError::new(KErrorKind::ShapeError(wrong_binder_class(
                        &name,
                        BindKind::Type,
                    ))));
                }
                (BinderSymbol::Type(name), ArgChannel::Value) => {
                    let name = render_label(name.symbol(), registries);
                    return Err(KError::new(KErrorKind::ShapeError(wrong_binder_class(
                        &name,
                        BindKind::Value,
                    ))));
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
            )));
        }
    };
    match func.signature.return_type() {
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
                // frame brand. The resolved type is a `Copy` handle, so it rides the tail-replace
                // directly with no re-home.
                DeferredReturn::Type(type_expr) => {
                    let resolved = ctx.region.with_scope(|child| {
                        let resolved: Result<KType, KError> =
                            match child.resolve_type_identifier(type_expr, None, registries) {
                                TypeResolution::Done(kt) => Ok(kt),
                                // A park at this point cannot be honored — the body is about to
                                // run — so fall back to Any and let the body's own dispatch surface
                                // any real error.
                                TypeResolution::Park(_) => Ok(KType::ANY),
                                // A miss is a real error: the return names no type. Surfacing it
                                // here rather than widening to Any is what makes `-> some_value` (a
                                // return slot naming a value — a module included) a diagnostic
                                // instead of a silently unconstrained return.
                                TypeResolution::Unbound(message) => {
                                    Err(KError::new(KErrorKind::ShapeError(message)))
                                }
                            };
                        resolved
                    });
                    let ret = match resolved {
                        Ok(ret) => ret,
                        Err(e) => return ExecOutcome::Errored(e),
                    };
                    let (leading, tail) = split_leading_tail(body_expr);
                    ExecOutcome::Tail {
                        leading,
                        tail,
                        ret: PerCallReturn::Resolved(ret),
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

/// Which door an argument's envelope binds through, read once under the envelope's own pin. Every
/// arm's payload is pin-free — a `Copy` [`KType`] handle, a rendered name — so the classification
/// outlives the open and the bind runs outside it.
enum ArgChannel {
    Value,
    Type(KType),
    Unresolved(TypeSymbol),
}

/// Classify a delivered argument's channel. The value arm carries nothing: the bind reads the object
/// back out of the same envelope through the binding door, which is what relocates it into the frame
/// region.
fn arg_channel(delivered: &DeliveredCarried) -> ArgChannel {
    delivered.open(|live| match live {
        Carried::Object(_) => ArgChannel::Value,
        Carried::Type(kt) => ArgChannel::Type(kt),
        Carried::UnresolvedType(ti) => ArgChannel::Unresolved(ti),
    })
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
