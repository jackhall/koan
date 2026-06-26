//! The dispatch-side `invoke` — the single entry that runs a resolved call. A builtin runs through
//! the action harness (its bound args as a `KObject::Record` `BodyCtx`); a user-defined body runs
//! through [`crate::machine::core::kfunction::exec::run_user_fn`] and its [`ExecOutcome`] is lowered
//! to an [`Outcome`] the harness applies. `invoke` is a **pure decide**: it reads a `SchedulerView`
//! and the per-call `frame` the harness already acquired (frame acquisition is the harness's write),
//! and returns the deferred body dispatch declaratively (a `Continue` for the tail, a
//! `ParkThenContinue` over a [`DepRequest::BodyBlock`] for a first-call deferred return). Kept out
//! of `ctx.rs` (the dispatcher facade) so the dispatcher core stays thin; pure body semantics live
//! one layer down in [`crate::machine::core::kfunction::exec`].

use super::super::nodes::NodeWork;
use super::super::outcome::{dep_error_frame, Continuation, Outcome};
use super::super::runtime::KoanWorkload;
use super::super::{ignore_results, DepFinish};
use super::DepRequest;
use super::SchedulerView;
use crate::machine::core::kfunction::action::FramePlacement;
use crate::machine::core::kfunction::bind_by_name::CallArgs;
use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::core::kfunction::exec::{run_user_fn, ExecFrame, ExecOutcome, PerCallReturn};
use crate::machine::core::kfunction::{Body, KFunction};
use crate::machine::execute::lift::lift_ktype;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::{Carried, Parseable};
use crate::machine::{KError, KErrorKind};

/// Fold a resolved call into a [`Outcome::Continue`]: the producer installs the per-call cart and
/// `invoke` runs against it on the next pop. A user fn's `Continue` carries
/// [`FramePlacement::ReuseReserve`] (the harness mints the TCO cart); a builtin's carries
/// [`FramePlacement::Inherit`] (it runs in the current frame). The decide handler owns `picked`, so
/// the builtin-vs-user-fn frame decision is made here, not in the harness.
pub(super) fn invoke_continue<'step>(
    picked: &'step KFunction<'step>,
    working_expr: KExpression<'step>,
) -> Outcome<'step> {
    let frame = match &picked.body {
        Body::Builtin(_) => FramePlacement::Inherit,
        _ => FramePlacement::ReuseReserve {
            outer: picked.captured_scope(),
        },
    };
    Outcome::Continue {
        work: invoke_work(picked, working_expr),
        frame,
        contract: None,
        block_entry: None,
        body_index: 0,
    }
}

/// A dep-free decide [`NodeWork`] whose closure runs the folded [`invoke`] against the cart the
/// producer's `Continue` installed. `carrier` is the call's deadlock-summary sample.
fn invoke_work<'step>(
    picked: &'step KFunction<'step>,
    working_expr: KExpression<'step>,
) -> NodeWork<KoanWorkload> {
    let carrier = working_expr.summarize();
    NodeWork::new(
        Vec::new(),
        0,
        ignore_results(Box::new(move |view, _idx| {
            invoke(view, picked, working_expr)
        })),
        Some(carrier),
    )
}

/// The single invoke entry for the dispatcher's bind sites — run a resolved call:
/// - **builtin** → the action harness (`BodyCtx` → `Action` → `run_action`);
/// - **user-defined** → the `exec` executor (`run_user_fn` + the `ExecOutcome` lowering).
///
/// Every call reaches here with its value parts already `Spliced`/literal-resolved (the eager-subs
/// and synchronous bind paths splice them first), so there is no fall-through.
pub(super) fn invoke<'step>(
    view: &SchedulerView<'step, '_>,
    picked: &'step KFunction<'step>,
    working_expr: KExpression<'step>,
) -> Outcome<'step> {
    // An action-harness builtin: build a read-only `BodyCtx`, get the `Action`, and lower it
    // through the shared `run_action` interpreter. Builtins run in the current frame, so the
    // builtin call's `Continue` carries `FramePlacement::Inherit` and this reads nothing.
    if let Body::Builtin(f) = &picked.body {
        let f = *f;
        let args = match picked.bind(working_expr) {
            Ok(future) => future.args,
            Err(e) => return Outcome::Done(Err(e)),
        };
        return run_action_builtin(view, f, args);
    }

    // Validate each argument against its declared parameter type before the (type-trusting)
    // `bind_by_name`: a uniquely-picked call is admitted shape-only by dispatch, so a non-satisfying
    // typed argument (e.g. a module that doesn't satisfy a `:Signature` param) is caught here.
    if let Err(e) = picked.validate_call_args(&working_expr) {
        return Outcome::Done(Err(e));
    }

    let args = match extract_carried_args(view, &working_expr) {
        Some(args) => args,
        // Unreachable by construction (the bind sites resolve value parts to `Spliced`/literal
        // first); surface a diagnostic rather than silently mis-bind if that ever breaks.
        None => {
            return Outcome::Done(Err(KError::new(KErrorKind::User(
                "exec: a call argument was not a resolved value at the bind site".to_string(),
            ))))
        }
    };

    let bound = match picked.bind_by_name(CallArgs::Positional(args)) {
        Ok(record) => record,
        Err(e) => return Outcome::Done(Err(e)),
    };

    let outer = picked.captured_scope();
    // The per-call frame the producer's `Continue` (`ReuseReserve`) already acquired and installed
    // as the slot's cart — `invoke` runs against it, so read it from the view rather than a param.
    let frame = view
        .current_frame()
        .expect("a user-fn invoke runs against the Continue-installed per-call cart");
    let exec_frame = ExecFrame {
        region: frame.clone(),
    };
    // A deferred-return FN dispatched as a tail call inside an established contract chain skips
    // resolving its own (keep-first-discarded) return type — see `run_user_fn`.
    let in_chain = view.in_contract_chain();
    match run_user_fn(picked, bound, &exec_frame, in_chain) {
        ExecOutcome::Tail { leading, tail, ret } => {
            // The return contract carried on the tail-replace. A resolved return reads its type off
            // the signature; a deferred `Type` return carries the resolved per-call type as a
            // `PerCall` contract — checked + stamped at the lift boundary like any FN return, so the
            // body is a proper tail call and a recursive deferred body stays TCO-flat.
            let contract = match ret {
                PerCallReturn::FromSignature => ReturnContract::Function(picked),
                PerCallReturn::Resolved(kt) => {
                    // Re-home the per-call type in the captured-scope (frame-outer) region — a strict
                    // ancestor the cart keeps live — so the erased contract's `ret` borrow stays
                    // valid past the dying frame, mirroring an `Arm`'s `ret`.
                    let ret_ref = outer.region.alloc_ktype(lift_ktype(&kt, &frame.storage_rc()));
                    ReturnContract::PerCall {
                        func: picked,
                        ret: ret_ref,
                    }
                }
            };
            // Empty `leading` → body_index 1 (the lone statement sits above the params); otherwise
            // the leading statements sit at indices `1..=N` and the tail replaces in at `N + 1`.
            let body_index = leading.len() + 1;
            // Capture the body scope id before `frame` moves; the reinstall site reads it to
            // assemble the chain.
            let block_entry = frame.scope().id;
            let tail_expr = tail.clone();
            if leading.is_empty() {
                // No leading statements: tail-replace directly into the body terminal. The frame is
                // already the slot's installed cart (the producer's `ReuseReserve`), so re-enter it
                // with `Inherit` — re-installing it would clobber the ping-pong reserve.
                return Outcome::Continue {
                    work: super::decide(tail_expr),
                    frame: FramePlacement::Inherit,
                    contract: Some(contract),
                    block_entry: Some(block_entry),
                    body_index,
                };
            }
            // Leading statements become owned siblings in `frame` (one `BodyBlock` dep); the slot
            // parks on them so they cascade-free before the tail continues, restoring the frame's
            // uniqueness so the next call's `try_reset_for_tail` reuses (TCO stays flat). The
            // resolving finish — having waited out every leading statement — emits the tail
            // `Continue`, re-entering the already-installed cart with `Inherit`.
            let statements: Vec<KExpression<'step>> =
                leading.into_iter().map(|e| (*e).clone()).collect();
            let finish: DepFinish<'step> = Box::new(move |_view, _results| Outcome::Continue {
                work: super::decide(tail_expr),
                frame: FramePlacement::Inherit,
                contract: Some(contract),
                block_entry: Some(block_entry),
                body_index,
            });
            Outcome::ParkThenContinue {
                deps: vec![DepRequest::BodyBlock { frame, statements }],
                park_count: 0,
                continuation: Continuation::Finish(finish),
                dep_error_frame: Some(dep_error_frame()),
            }
        }
        ExecOutcome::DeferredExprTail {
            type_expr,
            leading,
            tail,
        } => {
            // First-call deferred `Expression` return: the harness dispatches the leading body
            // statements and the return-type expression as body-chain siblings in `frame` (a single
            // `BodyBlock` dep). The combine reads the last result (the resolved type), builds the
            // `PerCall` contract, and tail-replaces into the body terminal — a proper tail call, so
            // the recursion (subsequent calls skip resolution) stays TCO-flat. The body terminal
            // sits above the params, the leading siblings, and the type slot.
            let mut body_and_type = leading;
            body_and_type.push(type_expr);
            let body_index = body_and_type.len() + 1;
            let statements: Vec<KExpression<'step>> =
                body_and_type.into_iter().map(|e| (*e).clone()).collect();
            let tail_expr = tail.clone();
            // Capture the body scope id before `frame` moves into the `BodyBlock` dep; the finish
            // re-enters that already-installed cart with `Inherit`.
            let block_entry = frame.scope().id;
            let finish: DepFinish<'step> = Box::new(move |view, results| {
                // The return-type expression is the last body statement, so its resolved value is
                // the last result.
                let kt = match results[results.len() - 1] {
                    Carried::Type(t) => t,
                    Carried::Object(other) => {
                        return Outcome::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                            "FN deferred return-type expression produced a non-type {} value",
                            other.ktype().name(),
                        )))))
                    }
                };
                // The per-call type rides the captured-scope (frame-outer) region, a strict ancestor
                // the cart keeps live — same home as the `Type` form's `PerCall.ret`. `kt` was
                // pull-lifted into this node's call frame, which the captured scope outlives, so
                // relocate it with `lift_ktype` (re-anchoring any per-call `Module` frame onto the
                // call frame) rather than a bare clone that would dangle once the frame frees.
                let call_frame = view
                    .current_frame()
                    .expect("a deferred-return finish runs against a per-call frame");
                let ret_ref = picked
                    .captured_scope()
                    .region
                    .alloc_ktype(lift_ktype(kt, &call_frame.storage_rc()));
                let contract = ReturnContract::PerCall {
                    func: picked,
                    ret: ret_ref,
                };
                Outcome::Continue {
                    work: super::decide(tail_expr),
                    frame: FramePlacement::Inherit,
                    contract: Some(contract),
                    block_entry: Some(block_entry),
                    body_index,
                }
            });
            Outcome::ParkThenContinue {
                deps: vec![DepRequest::BodyBlock { frame, statements }],
                park_count: 0,
                continuation: Continuation::Finish(finish),
                dep_error_frame: Some(dep_error_frame()),
            }
        }
        ExecOutcome::Errored(e) => Outcome::Done(Err(e)),
    }
}

/// Lower an action-harness builtin: convert its resolved `args` record into the `KObject::Record`
/// the `BodyCtx` exposes, build the read-only `BodyCtx`, call the `ActionFn`, then interpret the
/// returned `Action` through the shared `run_action`.
fn run_action_builtin<'step>(
    view: &SchedulerView<'step, '_>,
    f: crate::machine::core::kfunction::ActionFn,
    args: crate::machine::model::types::Record<crate::machine::model::values::ArgValue<'step>>,
) -> Outcome<'step> {
    use crate::machine::core::kfunction::action::BodyCtx;
    use crate::machine::model::values::{ArgValue, Held};
    use crate::machine::model::KObject;

    let cells = args.map(|av| match av {
        ArgValue::Object(rc) => Held::Object(rc.deep_clone()),
        ArgValue::Type(t) => Held::Type(t.clone()),
    });
    let args_obj: &'step KObject<'step> = view
        .current_scope()
        .region
        .alloc_object(KObject::record_of_held(cells));
    let frame = view.current_frame();
    let chain = view.current_lexical_chain();
    let action = {
        let body_ctx = BodyCtx {
            scope: view.current_scope(),
            frame: frame.as_ref(),
            chain,
            args: args_obj,
        };
        f(&body_ctx)
    };
    // `run_action` is a pure `Action -> Outcome` lowering; the harness applies the result.
    super::super::runtime::run_action(action)
}

/// Extract the call's resolved value arguments from `working_expr`'s parts, in order. Returns
/// `None` if any value part isn't a resolved `Carried` (a `Spliced`-splice or a literal) — the
/// signal to fall through to the legacy binder. Keyword parts are the signature's own literals.
fn extract_carried_args<'step>(
    view: &SchedulerView<'step, '_>,
    working_expr: &KExpression<'step>,
) -> Option<Vec<Carried<'step>>> {
    let mut args = Vec::new();
    for part in &working_expr.parts {
        match &part.value {
            ExpressionPart::Keyword(_) => {}
            ExpressionPart::Spliced(carried) => args.push(*carried),
            // A literal value part isn't `Spliced`-spliced; resolve it into the run region now
            // (mirrors `literal_pass_through`) so it joins the args as a `'step` `Carried`.
            ExpressionPart::Literal(_) => {
                let object = view
                    .current_scope()
                    .region
                    .alloc_object(part.value.resolve());
                args.push(Carried::Object(object));
            }
            _ => return None,
        }
    }
    Some(args)
}
