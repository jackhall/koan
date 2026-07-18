//! The dispatch-side `invoke` — the single entry that runs a resolved call. A builtin runs through
//! the action harness (its bound args as a `KObject::Record` `BodyCtx`); a user-defined body runs
//! through [`crate::machine::core::kfunction::exec::run_user_fn`] and its [`ExecOutcome`] is lowered
//! to an [`Action::Tail`] the shared [`run_action`](super::super::runtime::run_action) interprets.
//! `invoke` is a **pure decide**: it reads a `SchedulerView` and the per-call `frame` the harness
//! already acquired (frame acquisition is the harness's write), and hands the deferred body dispatch
//! to `run_action` declaratively. Kept out of `ctx.rs` (the dispatcher facade) so the dispatcher core
//! stays thin; pure body semantics live one layer down in [`crate::machine::core::kfunction::exec`].

use super::super::ignore_results;
use super::super::nodes::{ChainOp, NodeWork};
use super::super::obligation::{with_obligation, ReturnObligation};
use super::super::outcome::Outcome;
use super::super::runtime::KoanWorkload;
use super::SchedulerView;
use crate::machine::core::ReturnContract;
use crate::machine::core::{run_user_fn, ExecFrame, ExecOutcome, PerCallReturn};
use crate::machine::core::{Action, BlockEntry, FramePlacement, TailContract};
use crate::machine::core::{Body, KFunction};
use crate::machine::model::{Carried, Parseable};
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::model::{Record, SignatureElement};
use crate::machine::{DeliveredCarried, KError, KErrorKind};
use crate::scheduler::ResolvedDeps;

/// Fold a resolved call into a [`Outcome::Continue`]: the producer installs the per-call cart and
/// `invoke` runs against it on the next pop. A user fn's `Continue` carries
/// [`FramePlacement::FreshTail`] (the harness mints the TCO cart fresh at apply); a builtin's
/// carries [`FramePlacement::Inherit`] (it runs in the current frame). The decide handler owns
/// `picked`, so the builtin-vs-user-fn frame decision is made here, not in the harness.
pub(super) fn invoke_continue<'step>(
    view: &SchedulerView<'step, '_>,
    picked: &'step KFunction<'step>,
    working_expr: KExpression<'step>,
) -> Outcome<'step> {
    let frame = match &picked.body {
        Body::Builtin(_) => FramePlacement::Inherit,
        _ => FramePlacement::FreshTail {
            outer: picked.captured_scope(),
        },
    };
    // The invoke step carries no contract of its own — `picked`'s return is resolved inside `invoke`
    // (or skipped when this is a nested tail). So a fresh-tail invoke that lands inside an established
    // chain wraps the invoke continuation with the ambient obligation, keeping the first caller's
    // declared return alive across the frame-installing hop; the nested tail's own contract loses.
    Outcome::Continue {
        work: invoke_work(picked, working_expr, view.current_obligation_duplicate()),
        frame,
        chain: ChainOp::Unchanged,
        block_entry: BlockEntry::None,
    }
}

/// A dep-free decide [`NodeWork`] whose closure runs the folded [`invoke`] against the cart the
/// producer's `Continue` installed. `carrier` is the call's deadlock-summary sample. `obligation`
/// wraps the invoke continuation (before the [`NodeWork::new`] erase) so a nested tail's invoke step
/// re-deposits the established declared-return checker.
fn invoke_work<'step>(
    picked: &'step KFunction<'step>,
    working_expr: KExpression<'step>,
    obligation: Option<ReturnObligation>,
) -> NodeWork<KoanWorkload> {
    let carrier = working_expr.summarize();
    let continuation = ignore_results(Box::new(move |view, _idx| {
        invoke(view, picked, working_expr)
    }));
    let continuation = match obligation {
        Some(obligation) => with_obligation(obligation, continuation),
        None => continuation,
    };
    NodeWork::new(ResolvedDeps::new(), continuation, Some(carrier))
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
    // Per-argument reach carriers, read back off the spliced cells (value and reach as one unit). A
    // literal arg is region-pure and contributes no cell.
    let arg_carriers = carriers_from_expr(&working_expr);
    if let Body::Builtin(f) = &picked.body {
        let f = *f;
        let arg_carriers = map_arg_carriers(picked, arg_carriers);
        let args = match picked.bind_args(&working_expr, view.current_scope(), view.types()) {
            Ok(args) => args,
            Err(e) => return Outcome::Done(Err(e)),
        };
        return run_action_builtin(view, f, args, arg_carriers);
    }

    // A uniquely-picked call is admitted shape-only by dispatch, so validate each argument against
    // its declared parameter type before the type-trusting `bind_by_name` — a non-satisfying typed
    // argument (e.g. a module that doesn't satisfy a `:Signature` param) is caught here.
    if let Err(e) = picked.validate_call_args(&working_expr, view.types()) {
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

    // The per-call frame the producer's `Continue` (`FreshTail`) already minted and installed
    // as the slot's cart — `invoke` runs against it, so read it from the view rather than a param.
    let frame = view
        .current_frame()
        .expect("a user-fn invoke runs against the Continue-installed per-call cart");
    // Re-key onto parameter names so `run_user_fn` stores each binding's reach from its own carrier,
    // keyed to match `bound`. `extract_carried_args` already folded every delivered arg's reach into
    // this same per-call scope (through `adopt_sealed`), so every foreign region an argument borrows
    // into is pinned for the call's life — no separate deposit here.
    let named_carriers = map_arg_carriers(picked, arg_carriers);
    let exec_frame = ExecFrame {
        region: frame.clone(),
    };
    // A deferred-return FN dispatched as a tail call inside an established contract chain skips
    // resolving its own (keep-first-discarded) return type — see `run_user_fn`.
    let in_chain = view.in_contract_chain();
    match run_user_fn(picked, bound, &named_carriers, &exec_frame, in_chain) {
        ExecOutcome::Tail { leading, tail, ret } => {
            // A resolved return reads its type off the signature; a deferred `Type` return carries
            // the per-call type (already re-homed into the captured-scope region by `run_user_fn`)
            // as a `PerCall` contract, checked + stamped at the lift boundary like any FN return, so
            // a recursive deferred body stays TCO-flat.
            let contract = match ret {
                PerCallReturn::FromSignature => ReturnContract::Function(picked),
                PerCallReturn::Resolved(ret_ref) => ReturnContract::PerCall {
                    func: picked,
                    ret: ret_ref,
                },
            };
            // The frame is already the slot's installed cart, so the tail re-enters it with
            // `Inherit` — a `FreshTail` here would mint a second cart, discarding the one already
            // holding the bound params — and the block entry carries it so the lowering fans any
            // leading statements into it.
            super::super::runtime::run_action(
                view,
                Action::Tail {
                    leading: leading.into_iter().map(|e| (*e).clone()).collect(),
                    tail: tail.clone(),
                    contract: TailContract::Eager(Some(contract)),
                    frame_placement: FramePlacement::Inherit,
                    block_entry: BlockEntry::FrameScope(frame),
                },
            )
        }
        ExecOutcome::DeferredExprTail {
            type_expr,
            leading,
            tail,
        } => {
            // First-call deferred `Expression` return: the leading body statements and the
            // return-type expression run as body-chain siblings in the installed cart; the
            // lowering's finish reads the last result (the resolved type) into a `PerCall` contract
            // before tail-replacing into the body terminal, so the recursion — subsequent calls skip
            // resolution — stays TCO-flat.
            let mut statements: Vec<KExpression<'step>> =
                leading.into_iter().map(|e| (*e).clone()).collect();
            statements.push(type_expr.clone());
            super::super::runtime::run_action(
                view,
                Action::Tail {
                    leading: statements,
                    tail: tail.clone(),
                    contract: TailContract::FromLastResult { func: picked },
                    frame_placement: FramePlacement::Inherit,
                    block_entry: BlockEntry::FrameScope(frame),
                },
            )
        }
        ExecOutcome::Errored(e) => Outcome::Done(Err(e)),
    }
}

/// Borrow each spliced cell back off the working expression as its `(slot, carrier)` pair. A literal
/// part carries no cell (region-pure, "no entry = no foreign reach"). The cells are borrowed, not
/// duplicated: the working expression outlives the call, and every reader downstream (the per-call
/// reach store, a value-embedding builtin's fold) takes a `&Sealed`, so nothing is copied here.
fn carriers_from_expr<'e, 'step>(
    working_expr: &'e KExpression<'step>,
) -> Vec<(usize, &'e DeliveredCarried)> {
    working_expr
        .parts
        .iter()
        .enumerate()
        .filter_map(|(i, part)| match &part.value {
            ExpressionPart::Spliced { cell } => Some((i, cell)),
            _ => None,
        })
        .collect()
}

/// Re-key the slot-indexed arg carriers onto their parameter names. A committed call's parts line up
/// 1:1 with `picked`'s signature elements (`validate_call_args` enforces it), so the element at a
/// carrier's slot names its parameter. A region-pure arg has no entry, read as "no foreign reach".
fn map_arg_carriers<'e, 'step>(
    picked: &KFunction<'step>,
    arg_carriers: Vec<(usize, &'e DeliveredCarried)>,
) -> Record<&'e DeliveredCarried> {
    let mut record = Record::new();
    for (slot, carrier) in arg_carriers {
        if let Some(SignatureElement::Argument(arg)) = picked.signature.elements.get(slot) {
            record.insert(arg.name.clone(), carrier);
        }
    }
    record
}

/// Lower an action-harness builtin: expose its owned `args` as the `BodyCtx`'s `KObject::Record`,
/// call the `ActionFn`, then interpret the returned `Action` through the shared `run_action`.
/// `arg_carriers` are the per-parameter reach carriers (a value-embedding body folds / merges the
/// one it embeds; an absent entry is region-pure).
fn run_action_builtin<'step>(
    view: &SchedulerView<'step, '_>,
    f: crate::machine::core::ActionFn,
    args: Record<crate::machine::model::Held<'step>>,
    arg_carriers: Record<&DeliveredCarried>,
) -> Outcome<'step> {
    use crate::machine::core::BodyCtx;
    use crate::machine::model::KObject;

    let scope = view.current_scope();
    // Evidence for the args record's own placement: every arg carrier's reach, minted into the
    // call scope's region up front — the record's leaves may still embed a foreign borrow from
    // whichever carrier they came from. Mode matches each cell's own kind: an `Object` cell is a
    // deep copy (`Copied`/`adopted_reach_of`), a `Type` cell is a shallow clone that still points
    // at its carrier's home (`Kept`/`host_reach_of`), so a Copied-only evidence set would
    // under-cover a module/signature-typed argument.
    let evidence: Vec<crate::machine::core::StoredReach> = args
        .iter()
        .filter_map(|(name, cell)| {
            let carrier = arg_carriers.get(name)?;
            Some(match cell {
                crate::machine::model::Held::Object(_) => scope.adopted_reach_of(carrier),
                crate::machine::model::Held::Type(_) => scope.host_reach_of(carrier),
            })
        })
        .collect();
    let args_obj: &'step KObject<'step> =
        match scope.alloc_object_delivered(KObject::record_of_held(args), &evidence) {
            Ok(args_obj) => args_obj,
            Err(e) => return Outcome::Done(Err(e)),
        };
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
            types: view.types(),
        };
        f(&body_ctx)
    };
    // `run_action` lowers the `Action` to an `Outcome`; the harness applies the result. The step
    // view carries the ambient obligation a tail action keep-firsts against.
    super::super::runtime::run_action(view, action)
}

/// Extract the call's resolved value arguments from `working_expr`'s parts, in order: a `Spliced`
/// part contributes its carried value, a literal resolves into the run region, keyword parts
/// contribute nothing. Returns `None` if a value part is neither spliced nor a literal — unreachable
/// by construction (the bind sites resolve value parts first), which the caller surfaces as a
/// diagnostic.
fn extract_carried_args<'step>(
    view: &SchedulerView<'step, '_>,
    working_expr: &KExpression<'step>,
) -> Option<Vec<Carried<'step>>> {
    let mut args = Vec::new();
    for part in &working_expr.parts {
        match &part.value {
            ExpressionPart::Keyword(_) => {}
            // Adopt the spliced cell into the call scope — an object by structural copy (the copy's
            // reach folds in; a residence-only producer host is released with the working
            // expression, so a tail call's retiring region does not chain into the fresh frame's
            // arena), a type copy-free with its host pinned. `view.current_scope()` *is* the call
            // scope (the run loop opens each step's scope from the Continue-installed cart), so the
            // fold never lands in the caller's scope.
            ExpressionPart::Spliced { cell } => {
                args.push(view.current_scope().adopt_sealed_copied(cell))
            }
            // Resolve a literal into the run region now (mirrors `literal_pass_through`) so it joins
            // the args as a `'step` `Carried`. A `#(...)` quote is a literal for this purpose: its
            // `KObject::KExpression` body is data, and the checked door's family audit passes it —
            // a quote body comes from the parser, which plants no `Spliced` cell, and the scheduler
            // splices only into the parts of an expression it dispatches, never into quoted data.
            ExpressionPart::Literal(_) | ExpressionPart::QuotedExpression(_) => {
                let object = view
                    .current_scope()
                    .brand()
                    .alloc_object_checked(part.value.resolve())
                    .expect("a resolved literal or quoted expression is owned and splice-free");
                args.push(Carried::Object(object));
            }
            _ => return None,
        }
    }
    Some(args)
}
