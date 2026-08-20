//! The dispatch-side invoke — the single entry that runs a resolved call. A builtin runs through
//! the action harness (its bound args handed to `BodyCtx` as a transient owned record); a
//! user-defined body runs through [`crate::machine::core::kfunction::exec::run_user_fn`] and its
//! [`ExecOutcome`] is lowered to an [`Action::tail`] the shared
//! [`run_action`](super::run_action) interprets.
//! Kept out of `ctx.rs` (the dispatcher facade) so the dispatcher core stays thin; pure body
//! semantics live one layer down in [`crate::machine::core::kfunction::exec`].

use super::super::harness::KoanWorkload;
use super::super::ignore_results;
use super::super::nodes::{ChainOp, NodeWork, WorkLabel};
use super::super::obligation::{ReturnObligation, with_obligation};
use super::super::outcome::Outcome;
use super::ctx::DecideCtx;
use std::rc::Rc;

use crate::machine::core::ReturnContract;
use crate::machine::core::{Action, BlockEntry, FramePlacement, TailContract};
use crate::machine::core::{Body, CallFrame, KFunction, OpenedFunction};
use crate::machine::core::{ExecFrame, ExecOutcome, PerCallReturn, run_user_fn};
use crate::machine::model::{ExpressionPart, KExpression, WorkingExpression, WorkingPart};
use crate::machine::model::{Record, SignatureElement};
use crate::machine::{DeliveredCarried, KError, KErrorKind};
use crate::witnessed::BumpVec;

/// Fold a resolved call into a [`Outcome::Continue`] — the dispatcher's one invoke entry, routing on
/// the picked body:
/// - **builtin** → [`FramePlacement::Inherit`] and a deferred [`invoke_builtin`], which runs the
///   action harness (`BodyCtx` → `Action` → `run_action`) against the slot's current cart.
/// - **user-defined** → [`enter_user_fn`], which mints the per-call cart and binds the arguments
///   into it *here*, in the step that emits the replace.
///
/// The invoke carries no contract of its own — `picked`'s return is resolved by `run_user_fn` (or
/// skipped when this is a nested tail). So an invoke that lands inside an established chain wraps
/// the continuation it installs with the ambient obligation, keeping the first caller's declared
/// return alive across the frame-installing hop; the nested tail's own contract loses.
pub(super) fn invoke_continue<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    picked: OpenedFunction<'step>,
    working_expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    match &picked.value().body {
        Body::Builtin(_) => Outcome::Continue {
            label: WorkLabel::of(&working_expr),
            work: builtin_work(picked, working_expr, view.current_obligation_duplicate()),
            frame: FramePlacement::Inherit,
            chain: ChainOp::Unchanged,
            block_entry: BlockEntry::None,
        },
        _ => enter_user_fn(view, picked, working_expr),
    }
}

/// `obligation` wraps the continuation before the [`NodeWork::new`] erase, so the replacement step
/// re-deposits the established declared-return checker.
fn builtin_work<'step>(
    picked: OpenedFunction<'step>,
    working_expr: WorkingExpression<'step>,
    obligation: Option<ReturnObligation>,
) -> NodeWork<'step, KoanWorkload> {
    let continuation = ignore_results(Box::new(move |view, _idx| {
        invoke_builtin(view, picked, working_expr)
    }));
    let continuation = with_obligation(obligation, continuation);
    NodeWork::new(continuation)
}

/// Frameless (`Inherit`), so the working expression and the slot's cart are the same region here as
/// at the decide that folded the call — nothing crosses a region boundary and the read is an
/// ordinary resident read.
fn invoke_builtin<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    picked: OpenedFunction<'step>,
    working_expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let function = picked.value();
    let Body::Builtin(f) = &function.body else {
        unreachable!("invoke_builtin is installed only for a builtin body");
    };
    let f = *f;
    // A literal arg is region-pure and contributes no cell — on the builtin lane nothing binds at a
    // `for<'b>` brand, so an absent entry reads as "no foreign reach".
    let arg_carriers = carriers_from_expr(view, &working_expr);
    let arg_carriers = map_arg_carriers(function, &arg_carriers);
    let args = match function.bind_args(working_expr.parts, view.current_scope(), view.types()) {
        Ok(args) => args,
        Err(e) => return Outcome::Done(Err(e)),
    };
    run_action_builtin(view, f, args, arg_carriers)
}

/// Enter a resolved **user-defined** call: mint the per-call cart and bind the call's arguments into
/// it, then hand the body's statements on to the reinstalled incarnation as a [`Outcome::Continue`].
///
/// **The tail hop's whole region crossing runs here, before the replace**: every read of
/// `working_expr` and every argument adoption into the fresh cart happens while the retiring region
/// is still this step's own, so no hold spans the hop
/// ([tail-call-optimization.md § Soundness](../../../../design/tail-call-optimization.md#soundness)).
fn enter_user_fn<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    picked: OpenedFunction<'step>,
    working_expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let function = picked.value();
    // A uniquely-picked call is admitted shape-only by dispatch, so validate each argument against
    // its declared parameter type before the type-trusting frame bind — a non-satisfying typed
    // argument (e.g. a module that doesn't satisfy a `:Signature` param) is caught here.
    if let Err(e) = function.validate_call_args(working_expr.parts, view.types()) {
        return Outcome::Done(Err(e));
    }
    let mut arg_carriers = carriers_from_expr(view, &working_expr);
    if let Err(e) = deliver_value_args(view, &working_expr, &mut arg_carriers) {
        return Outcome::Done(Err(e));
    }
    // Each envelope's relocation at the bind door mints that binding's reach in the per-call region,
    // so every foreign region an argument borrows into is pinned for the call's life — no separate
    // deposit here.
    let named_carriers = map_arg_carriers(function, &arg_carriers);
    // Chained off the closure's captured (definition) scope, so a closure's captured per-call frame
    // survives the hop while the caller's cart does not.
    let frame = CallFrame::new(function.captured_scope());
    let exec_frame = ExecFrame {
        region: Rc::clone(&frame),
    };
    // An established contract chain is exactly one with a live obligation, so the duplicate's
    // presence answers both reads: a deferred-return FN dispatched as a tail call inside one skips
    // resolving its own (keep-first-discarded) return type — see `run_user_fn`.
    let obligation = view.current_obligation_duplicate();
    let in_chain = obligation.is_some();
    let label = WorkLabel::of(&working_expr);
    match run_user_fn(
        function,
        &named_carriers,
        &exec_frame,
        in_chain,
        view.types(),
    ) {
        ExecOutcome::Tail { leading, tail, ret } => {
            // A deferred `Type` return's per-call type rides a `PerCall` contract, checked +
            // stamped at the lift boundary like any FN return, so a recursive deferred body stays
            // TCO-flat.
            let contract = match ret {
                PerCallReturn::FromSignature => ReturnContract::Function(picked.reseal()),
                PerCallReturn::Resolved(ret) => ReturnContract::PerCall {
                    func: picked.reseal(),
                    ret,
                },
            };
            body_continue(
                frame,
                leading.into_iter().copied().collect(),
                *tail,
                TailContract::Eager(Some(contract)),
                label,
                obligation,
            )
        }
        ExecOutcome::DeferredExprTail {
            type_expr,
            leading,
            tail,
        } => {
            // First-call deferred `Expression` return: the return-type expression is a body-chain
            // sibling, so it joins the leading run and the lowering's finish reads the last result
            // (the resolved type) into a `PerCall` contract before tail-replacing into the body
            // terminal — subsequent calls skip resolution, so the recursion stays TCO-flat.
            let mut leading: Vec<KExpression<'step>> = leading.into_iter().copied().collect();
            leading.push(type_expr);
            body_continue(
                frame,
                leading,
                *tail,
                TailContract::FromLastResult {
                    func: picked.reseal(),
                },
                label,
                obligation,
            )
        }
        ExecOutcome::Errored(e) => Outcome::Done(Err(e)),
    }
}

/// The reinstalled incarnation's work: lower the bound call's body into the cart the replace
/// installs. The body statements arrive as borrowed AST (`'step` names the callee's own definition
/// storage, which the fresh cart chains as an ancestor), and the working copies are frozen at the
/// *reinstalled* step's brand — the cart's own region — which is why the lowering waits for that
/// step instead of riding [`enter_user_fn`].
///
/// The tail re-enters the installed cart with `Inherit` — a second `FreshTail` here would mint
/// another cart, discarding the one already holding the bound params — and the block entry carries
/// it so the lowering fans any leading statements into it.
fn body_continue<'step>(
    frame: Rc<CallFrame>,
    leading: Vec<KExpression<'step>>,
    tail: KExpression<'step>,
    contract: TailContract<'step>,
    label: WorkLabel,
    obligation: Option<ReturnObligation>,
) -> Outcome<'step> {
    let work_frame = Rc::clone(&frame);
    let continuation = ignore_results(Box::new(move |view: &DecideCtx<'_, 'step, '_>, _idx| {
        let brand = view.current_scope().brand();
        super::run_action(
            view,
            Action::tail(
                leading
                    .into_iter()
                    .map(|e| WorkingExpression::from_ast(brand, e))
                    .collect(),
                WorkingExpression::from_ast(brand, tail),
                contract,
                FramePlacement::Inherit,
                BlockEntry::FrameScope(work_frame),
            ),
        )
    }));
    let continuation = with_obligation(obligation, continuation);
    Outcome::Continue {
        work: NodeWork::new(continuation),
        frame: FramePlacement::FreshTail { frame },
        chain: ChainOp::Unchanged,
        block_entry: BlockEntry::None,
        label,
    }
}

/// Lift each spliced cell off the working expression, **one entry per part** — parallel to
/// `working_expr.parts` rather than a sparse `(slot, …)` list, so every reader addresses a part and
/// its envelope by the same index with nothing to keep in step. A `None` entry reads as "no foreign
/// reach".
///
/// The cells rest in the region the dispatching step put them in; the lift re-owns each one's reach
/// under the step's coverage, so every reader downstream works off an envelope that survives the
/// call independently of that region. Once per call, not once per reader.
fn carriers_from_expr<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    working_expr: &WorkingExpression<'step>,
) -> BumpVec<'step, Option<DeliveredCarried>> {
    let mut carriers = BumpVec::with_capacity_in(working_expr.parts.len(), view.scratch());
    carriers.extend(working_expr.parts.iter().map(|part| match &part.value {
        WorkingPart::Spliced { cell } => Some(view.current_scope().lift_spliced(cell)),
        _ => None,
    }));
    carriers
}

/// Re-key the slot-indexed arg carriers onto their parameter names. A committed call's parts line up
/// 1:1 with `picked`'s signature elements ([`KFunction::validate_call_args`] enforces it), so the
/// element at a carrier's slot names its parameter. A `None` entry contributes no record field — the
/// shape a region-pure arg takes on the builtin lane, where nothing binds at a `for<'b>` brand. A
/// user-defined call fills every value slot ([`deliver_value_args`]), so the record this returns
/// there holds one envelope per parameter and is the whole argument currency the frame bind reads.
fn map_arg_carriers<'e, 'step>(
    picked: &KFunction<'step>,
    arg_carriers: &'e [Option<DeliveredCarried>],
) -> Record<&'e DeliveredCarried> {
    let mut record = Record::new();
    for (slot, carrier) in arg_carriers.iter().enumerate() {
        if let (Some(carrier), Some(SignatureElement::Argument(arg))) =
            (carrier, picked.signature.elements().get(slot))
        {
            record.insert(arg.name.to_string(), carrier);
        }
    }
    record
}

/// `args` reaches the `BodyCtx` by reference as a transient record — never a `KObject`, never
/// region-allocated. `arg_carriers` are the per-parameter reach carriers (a value-embedding body
/// folds / merges the one it embeds; an absent entry is region-pure).
fn run_action_builtin<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    f: crate::machine::core::ActionFn,
    args: Record<crate::machine::model::Held<'step>>,
    arg_carriers: Record<&DeliveredCarried>,
) -> Outcome<'step> {
    use crate::machine::core::BodyCtx;

    let frame = view.current_frame();
    let chain = view.active_chain();
    let action = {
        let body_ctx = BodyCtx {
            scope: view.current_scope(),
            frame: frame.as_ref(),
            chain,
            args: &args,
            arg_carriers: &arg_carriers,
            installer: view.installer(),
            ctx: view.step_ctx(),
            types: view.types(),
            out: view.out(),
            program: view.program(),
        };
        f(&body_ctx)
    };
    super::run_action(view, action)
}

/// After this, every value part of `working_expr` has a delivery envelope in its own slot of
/// `arg_carriers`, which is what the frame bind takes. The two resolving arms place their value in
/// the call scope's region and envelope it there (`deliver_resident_object`) —
/// `view.current_scope()` *is* the call scope (the run loop opens each step's scope from the
/// Continue-installed cart), so the fold never lands in the caller's scope. The envelope's own
/// coverage is empty, so a literal argument still pins nothing.
///
/// The envelope is what the bind's `for<'b>` brand admits
/// ([`CallFrame::with_scope`](crate::machine::CallFrame::with_scope)) — a bare `&'step
/// KObject<'step>` names a lifetime the opened frame scope has no relation to. Keyword parts
/// contribute nothing. Any other value part is unreachable (the bind sites resolve value parts to
/// `Spliced`/literal first) and surfaces as a diagnostic rather than a silent mis-bind.
fn deliver_value_args<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    working_expr: &WorkingExpression<'step>,
    arg_carriers: &mut [Option<DeliveredCarried>],
) -> Result<(), KError> {
    for (part, lifted) in working_expr.parts.iter().zip(arg_carriers.iter_mut()) {
        match &part.value {
            WorkingPart::Ast(ExpressionPart::Keyword(_)) => {}
            // Already delivered: the bind relocates the value into the frame region off this very
            // envelope, so there is nothing to adopt here on the way.
            WorkingPart::Spliced { .. } => {}
            // Resolve a literal into the run region now (mirrors `literal_pass_through`) so it
            // reaches the bind as a delivered `'step` value. A string literal bumps its bytes here,
            // so the value is region-pure but not `'static` and takes the zero-dep fold door.
            WorkingPart::Ast(ExpressionPart::Literal(lit)) => {
                let object = view
                    .current_scope()
                    .fold_resident_object(|brand| lit.to_kobject(*brand));
                *lifted = Some(view.current_scope().deliver_resident_object(object));
            }
            // A `#(...)` quote's `KObject::KExpression` body is data, but the value it rides in is
            // invariant in its region lifetime with no `'static` rebuild and no fold-brand
            // construction, so it takes the expression door — whose signature is what proves the
            // cell reaches nothing outside the region it is bumped into.
            WorkingPart::Ast(ExpressionPart::QuotedExpression(body)) => {
                let object = view
                    .current_scope()
                    .brand()
                    .alloc_expression(body.expression());
                *lifted = Some(view.current_scope().deliver_resident_object(object));
            }
            _ => {
                return Err(KError::new(KErrorKind::User(
                    "exec: a call argument was not a resolved value at the bind site".to_string(),
                )));
            }
        }
    }
    Ok(())
}
