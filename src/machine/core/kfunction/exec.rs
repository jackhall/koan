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
//! contract lifetime the lift boundary consumes it within. The cap keeps a `ret` reference from
//! out-claiming the caller region a resolved parameter type borrows into. `KExpression`'s invariance
//! blocks collapsing the two.

use crate::machine::CarrierWitness;
use std::rc::Rc;

use crate::machine::core::{BindingIndex, CallFrame, KError, KErrorKind, RegionBrand};
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{
    elaborate_type_identifier, DeferredReturn, Elaborator, KType, Record, ReturnType,
    TypeResolution,
};
use crate::machine::model::values::{Carried, CarriedFamily, Held, KObject};
use crate::witnessed::Sealed;

use super::body::{body_statement_refs, Body};
use super::KFunction;

/// A body's execution context: the per-call `region` it runs in. Owned (an `Rc`), so it carries no
/// lifetime; the body re-projects its scope from the region on demand.
#[derive(Clone)]
pub struct ExecFrame {
    /// The per-call region the body executes in: it backs allocations and its child scope is the
    /// body's scope. Supplied (and, for TCO, reset) by the scheduler.
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
    /// A deferred-`Expression` return on its **first** call: resolve `type_expr` (`Er.Carrier`,
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
    Resolved(&'step KType<'step>),
}

/// Home a deferred FN's resolved return *type* into `region` (the captured-scope region, a live
/// ancestor), capped at the caller-supplied `'a`. The elaboration reads the param-bound child scope
/// at the frame brand, so `kt`'s lifetime is the short brand; the re-anchor lifts the clone to `'a`.
///
/// The cap is load-bearing: callers pass the **contract** lifetime (`'step`), not the captured
/// region's full lifetime. A non-module return type can embed a caller-region scope borrow (a
/// [`KType::Signature`]'s `decl_scope_ref`, a [`KType::AbstractType`]'s `Module` source), so the
/// clone's reach borrows into the caller region — valid for `'step` (the caller awaits the callee and
/// the reach-set fold pins the argument), but it must not lengthen to the captured region's lifetime,
/// which can outlive the caller.
///
/// A concrete first-class **module is rejected**: a module value's identity is not a return type, and
/// its borrow points into the dying per-call frame rather than the caller — invalid even at `'step`.
/// (A module returned as a *value* rides the value channel's witness set, unaffected.)
pub(crate) fn home_return_type<'a>(
    kt: &KType<'_>,
    region: RegionBrand<'a>,
) -> Result<&'a KType<'a>, KError> {
    if matches!(kt, KType::Module { .. }) {
        return Err(KError::new(KErrorKind::ShapeError(
            "a module cannot be a function's return type; return a signature or the `:Module` kind"
                .to_string(),
        )));
    }
    // `alloc_ktype` erases the clone's elaboration-brand lifetime and re-anchors it to `region` at
    // the caller's contract lifetime `'a`.
    Ok(region.alloc_ktype(kt.clone()))
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
    arg_carriers: &Record<&Sealed<CarriedFamily, CarrierWitness>>,
    ctx: &ExecFrame,
    in_contract_chain: bool,
) -> ExecOutcome<'ast, 'step>
where
    'ast: 'step,
{
    // Materialize the bound args as a record value in the frame, then bind each parameter to a
    // reference into the record's cell (one deep-clone per field, `Carried` → owned `Held`). The
    // record's cells double as the parameter bindings. Built at the frame brand so the seed fabricates
    // no `&'a`; its foreign reach is pinned by the call scope's reach-set.
    let bind = ctx.region.with_scope(|child| -> Result<(), KError> {
        let cells: Record<Held> = args.map(|carried| Held::from_carried(*carried));
        let args_record = child.brand().alloc_object(KObject::record_of_held(cells));
        if let KObject::Record(cells, _types) = args_record {
            for (name, cell) in cells.iter() {
                match cell {
                    Held::Object(object) => {
                        // Mint the parameter's reach from its delivered arg carrier (home-omitted) so
                        // a later read rebuilds its carrier. A region-pure arg has no entry → empty reach.
                        // The home-borrow bit is captured alongside, since the home-omitted reach drops it.
                        let stored = arg_carriers
                            .get(name)
                            .map(|carrier| child.host_reach_of(carrier.witness()))
                            .unwrap_or_default();
                        let _ =
                            child.bind_value(name.clone(), object, BindingIndex::value(0), stored);
                    }
                    // Type-denoting params (`Er`-style) register a type, not a value binding. The arg
                    // is already a resolved type, so register it directly. A module-typed argument
                    // reaches its child scope's region, so store the carrier's home-omitted reach.
                    Held::Type(kt) => {
                        let stored = arg_carriers
                            .get(name)
                            .map(|carrier| child.host_reach_of(carrier.witness()))
                            .unwrap_or_default();
                        child.register_type(
                            name.clone(),
                            kt.clone(),
                            BindingIndex::value(0),
                            stored,
                        );
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
                // `Type` form (`-> Er`): elaborate inline against the per-call child scope at the
                // frame brand, then re-home the resolved type into the captured-scope region so it is
                // freed from the brand and rides the tail-replace as a `'step` reference.
                DeferredReturn::Type(type_expr) => {
                    // Cap the home at `'step` (the `'ast: 'step` bound coerces `&'ast` down) so the
                    // contract `ret` can't out-claim a caller-region borrow — see `home_return_type`.
                    let captured_region: RegionBrand<'step> = func.captured_scope().brand();
                    let homed = ctx.region.with_scope(|child| {
                        let mut elaborator = Elaborator::new(child);
                        let kt = match elaborate_type_identifier(&mut elaborator, type_expr) {
                            TypeResolution::Done(kt) => kt,
                            // The param install + fn_def carrier scan jointly guarantee resolution;
                            // fall back to Any so the body's own dispatch surfaces any real error.
                            TypeResolution::Park(_) | TypeResolution::Unbound(_) => KType::Any,
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
