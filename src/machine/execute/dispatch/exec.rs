//! The dispatch-side `invoke` — the single entry that runs a resolved call. A builtin runs through
//! the action harness (its bound args as a `KObject::Record` `BodyCtx`); a user-defined body runs
//! through [`crate::machine::core::kfunction::exec::run_user_fn`] and its [`ExecOutcome`] is lowered
//! to an [`Action::Tail`] the shared [`run_action`](super::super::runtime::run_action) interprets.
//! `invoke` is a **pure decide**: it reads a `SchedulerView` and the per-call `frame` the harness
//! already acquired (frame acquisition is the harness's write), and hands the deferred body dispatch
//! to `run_action` declaratively. Kept out of `ctx.rs` (the dispatcher facade) so the dispatcher core
//! stays thin; pure body semantics live one layer down in [`crate::machine::core::kfunction::exec`].

use super::super::ignore_results;
use super::super::nodes::NodeWork;
use super::super::outcome::Outcome;
use super::super::runtime::KoanWorkload;
use super::SchedulerView;
use crate::machine::core::kfunction::action::{Action, BlockEntry, FramePlacement, TailContract};
use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::core::kfunction::exec::{run_user_fn, ExecFrame, ExecOutcome, PerCallReturn};
use crate::machine::core::kfunction::{Body, KFunction};
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{Record, SignatureElement};
use crate::machine::model::values::CarriedFamily;
use crate::machine::model::{Carried, Parseable};
use crate::machine::{FrameSet, KError, KErrorKind};
use crate::scheduler::ResolvedDeps;
use crate::witnessed::Sealed;

/// Fold a resolved call into a [`Outcome::Continue`]: the producer installs the per-call cart and
/// `invoke` runs against it on the next pop. A user fn's `Continue` carries
/// [`FramePlacement::ReuseReserve`] (the harness mints the TCO cart); a builtin's carries
/// [`FramePlacement::Inherit`] (it runs in the current frame). The decide handler owns `picked`, so
/// the builtin-vs-user-fn frame decision is made here, not in the harness.
pub(super) fn invoke_continue<'step>(
    picked: &'step KFunction<'step>,
    working_expr: KExpression<'step>,
    arg_carriers: Vec<(usize, Sealed<CarriedFamily, FrameSet>)>,
) -> Outcome<'step> {
    let frame = match &picked.body {
        Body::Builtin(_) => FramePlacement::Inherit,
        _ => FramePlacement::ReuseReserve {
            outer: picked.captured_scope(),
        },
    };
    Outcome::Continue {
        work: invoke_work(picked, working_expr, arg_carriers),
        frame,
        contract: None,
        block_entry: BlockEntry::None,
        body_index: 0,
    }
}

/// A dep-free decide [`NodeWork`] whose closure runs the folded [`invoke`] against the cart the
/// producer's `Continue` installed. `carrier` is the call's deadlock-summary sample.
fn invoke_work<'step>(
    picked: &'step KFunction<'step>,
    working_expr: KExpression<'step>,
    arg_carriers: Vec<(usize, Sealed<CarriedFamily, FrameSet>)>,
) -> NodeWork<KoanWorkload> {
    let carrier = working_expr.summarize();
    NodeWork::new(
        ResolvedDeps::new(),
        ignore_results(Box::new(move |view, _idx| {
            invoke(view, picked, working_expr, arg_carriers)
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
    arg_carriers: Vec<(usize, Sealed<CarriedFamily, FrameSet>)>,
) -> Outcome<'step> {
    // An action-harness builtin: build a read-only `BodyCtx`, get the `Action`, and lower it
    // through the shared `run_action` interpreter. Builtins run in the current frame, so the
    // builtin call's `Continue` carries `FramePlacement::Inherit` and this reads nothing.
    if let Body::Builtin(f) = &picked.body {
        let f = *f;
        // Re-key the slot-indexed arg carriers onto their parameter names (the body reads them by
        // name).
        let arg_carriers = map_arg_carriers(picked, arg_carriers);
        let args = match picked.bind_args(&working_expr) {
            Ok(args) => args,
            Err(e) => return Outcome::Done(Err(e)),
        };
        return run_action_builtin(view, f, args, arg_carriers);
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

    let bound = match picked.bind_by_name(args) {
        Ok(record) => record,
        Err(e) => return Outcome::Done(Err(e)),
    };

    // The per-call frame the producer's `Continue` (`ReuseReserve`) already acquired and installed
    // as the slot's cart — `invoke` runs against it, so read it from the view rather than a param.
    let frame = view
        .current_frame()
        .expect("a user-fn invoke runs against the Continue-installed per-call cart");
    // Deposit each delivered argument's reach into the per-call scope's reach-set — the same scope
    // `run_user_fn` deep-clones the arguments into and binds the parameters on — so every foreign
    // region an argument borrows into outlives the call. This is the bind-precise fold replacing the
    // relocate-seam reconstruction for user-fn object args (the seam wrongly folds into the caller
    // scope). Each carrier names the consumer frame ∪ foreign reach, and `fold_reach` omits the home
    // frame, so a region-pure argument deposits nothing while a multi-region one contributes every
    // region it reaches. Slot identity is irrelevant here, so all carriers fold uniformly.
    frame.with_scope(|call_scope| {
        for (_slot, carrier) in &arg_carriers {
            call_scope.fold_reach(carrier.witness());
        }
    });
    // Re-key the arg carriers onto their parameter names so `run_user_fn` can store each parameter
    // binding's reach from its own delivered carrier — the same carriers folded into the call-scope
    // reach above, keyed to match `bound`.
    let named_carriers = map_arg_carriers(picked, arg_carriers);
    let exec_frame = ExecFrame {
        region: frame.clone(),
    };
    // A deferred-return FN dispatched as a tail call inside an established contract chain skips
    // resolving its own (keep-first-discarded) return type — see `run_user_fn`.
    let in_chain = view.in_contract_chain();
    match run_user_fn(picked, bound, &named_carriers, &exec_frame, in_chain) {
        ExecOutcome::Tail { leading, tail, ret } => {
            // The return contract carried on the tail-replace. A resolved return reads its type off
            // the signature; a deferred `Type` return carries the resolved per-call type — already
            // re-homed into the captured-scope region by `run_user_fn` — as a `PerCall` contract,
            // checked + stamped at the lift boundary like any FN return, so the body is a proper tail
            // call and a recursive deferred body stays TCO-flat.
            let contract = match ret {
                PerCallReturn::FromSignature => ReturnContract::Function(picked),
                PerCallReturn::Resolved(ret_ref) => ReturnContract::PerCall {
                    func: picked,
                    ret: ret_ref,
                },
            };
            // The frame is already the slot's installed cart (the producer's `ReuseReserve`), so the
            // tail re-enters it with `Inherit` — re-installing would clobber the ping-pong reserve —
            // and the block entry carries it so the lowering fans any leading statements into it.
            super::super::runtime::run_action(Action::Tail {
                leading: leading.into_iter().map(|e| (*e).clone()).collect(),
                tail: tail.clone(),
                contract: TailContract::Eager(Some(contract)),
                frame_placement: FramePlacement::Inherit,
                block_entry: BlockEntry::FrameScope(frame),
            })
        }
        ExecOutcome::DeferredExprTail {
            type_expr,
            leading,
            tail,
        } => {
            // First-call deferred `Expression` return: the leading body statements and the
            // return-type expression run as body-chain siblings in the installed cart, and the
            // lowering's finish reads the last result — the resolved type — into a `PerCall`
            // contract before tail-replacing into the body terminal, a proper tail call, so the
            // recursion (subsequent calls skip resolution) stays TCO-flat.
            let mut statements: Vec<KExpression<'step>> =
                leading.into_iter().map(|e| (*e).clone()).collect();
            statements.push(type_expr.clone());
            super::super::runtime::run_action(Action::Tail {
                leading: statements,
                tail: tail.clone(),
                contract: TailContract::FromLastResult { func: picked },
                frame_placement: FramePlacement::Inherit,
                block_entry: BlockEntry::FrameScope(frame),
            })
        }
        ExecOutcome::Errored(e) => Outcome::Done(Err(e)),
    }
}

/// Re-key the delivered arg carriers — indexed by their working-expr part slot — onto the parameter
/// name the builtin body reads. A committed call's parts line up 1:1 with `picked`'s signature
/// elements (`validate_call_args` enforces it), so the element at a carrier's slot names its
/// parameter. Only spliced / bound-name args carry a carrier; a scalar-literal arg is region-pure and
/// simply has no entry, which the body reads as "no foreign reach".
fn map_arg_carriers<'step>(
    picked: &KFunction<'step>,
    arg_carriers: Vec<(usize, Sealed<CarriedFamily, FrameSet>)>,
) -> Record<Sealed<CarriedFamily, FrameSet>> {
    let mut record = Record::new();
    for (slot, carrier) in arg_carriers {
        if let Some(SignatureElement::Argument(arg)) = picked.signature.elements.get(slot) {
            record.insert(arg.name.clone(), carrier);
        }
    }
    record
}

/// Lower an action-harness builtin: wrap its owned `args` record as the `KObject::Record` the
/// `BodyCtx` exposes, build the read-only `BodyCtx`, call the `ActionFn`, then interpret the
/// returned `Action` through the shared `run_action`. `arg_carriers` are the per-parameter reach
/// carriers (a value-embedding body folds / merges the one it embeds; an absent entry is region-pure).
fn run_action_builtin<'step>(
    view: &SchedulerView<'step, '_>,
    f: crate::machine::core::kfunction::ActionFn,
    args: Record<crate::machine::model::values::Held<'step>>,
    arg_carriers: Record<Sealed<CarriedFamily, FrameSet>>,
) -> Outcome<'step> {
    use crate::machine::core::kfunction::action::BodyCtx;
    use crate::machine::model::KObject;

    // `bind_args` already produced a fresh, owned `Held` record — move it straight into the
    // region-allocated `KObject::Record` the read-only `BodyCtx` exposes.
    let args_obj: &'step KObject<'step> = view
        .current_scope()
        .brand()
        .alloc_object(KObject::record_of_held(args));
    let frame = view.current_frame();
    let chain = view.current_lexical_chain();
    let action = {
        let body_ctx = BodyCtx {
            scope: view.current_scope(),
            frame: frame.as_ref(),
            chain,
            args: args_obj,
            arg_carriers: &arg_carriers,
            ctx: view.step_ctx(),
        };
        f(&body_ctx)
    };
    // `run_action` is a pure `Action -> Outcome` lowering; the harness applies the result.
    super::super::runtime::run_action(action)
}

/// Extract the call's resolved value arguments from `working_expr`'s parts, in order: a `Spliced`
/// part contributes its carried value, a literal is resolved into the run region, and keyword parts
/// (the signature's own literals) contribute nothing. Returns `None` if a value part is neither
/// spliced nor a literal — unreachable by construction (the bind sites resolve value parts first),
/// which the caller surfaces as a diagnostic.
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
                    .brand()
                    .alloc_object(part.value.resolve());
                args.push(Carried::Object(object));
            }
            _ => return None,
        }
    }
    Some(args)
}
