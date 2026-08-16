//! The dispatch-side `invoke` — the single entry that runs a resolved call. A builtin runs through
//! the action harness (its bound args handed to `BodyCtx` as a transient owned record); a
//! user-defined body runs through [`crate::machine::core::kfunction::exec::run_user_fn`] and its
//! [`ExecOutcome`] is lowered to an [`Action::Tail`] the shared
//! [`run_action`](super::super::runtime::run_action) interprets.
//! `invoke` is a **pure decide**: it reads a [`DecideCtx`] and the per-call `frame` the harness
//! already acquired (frame acquisition is the harness's write), and hands the deferred body dispatch
//! to `run_action` declaratively. Kept out of `ctx.rs` (the dispatcher facade) so the dispatcher core
//! stays thin; pure body semantics live one layer down in [`crate::machine::core::kfunction::exec`].

use super::super::harness::KoanWorkload;
use super::super::ignore_results;
use super::super::nodes::{ChainOp, NodeWork};
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

/// Fold a resolved call into a [`Outcome::Continue`] — the dispatcher's one invoke entry, routing on
/// the picked body:
/// - **builtin** → [`FramePlacement::Inherit`] and a deferred [`invoke_builtin`], which runs the
///   action harness (`BodyCtx` → `Action` → `run_action`) against the slot's current cart.
/// - **user-defined** → [`enter_user_fn`], which mints the per-call cart and binds the arguments
///   into it *here*, in the step that emits the replace.
///
/// Every call reaches here with its value parts already `Spliced`/literal-resolved (the eager-subs
/// and synchronous bind paths splice them first), so there is no fall-through.
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
            work: builtin_work(picked, working_expr, view.current_obligation_duplicate()),
            frame: FramePlacement::Inherit,
            chain: ChainOp::Unchanged,
            block_entry: BlockEntry::None,
        },
        _ => enter_user_fn(view, picked, working_expr),
    }
}

/// A dep-free decide [`NodeWork`] whose closure runs [`invoke_builtin`] against the cart the slot
/// already holds. `carrier` is the call's deadlock-summary sample. `obligation` wraps the
/// continuation (before the [`NodeWork::new`] erase) so the replacement step re-deposits the
/// established declared-return checker.
fn builtin_work<'step>(
    picked: OpenedFunction<'step>,
    working_expr: WorkingExpression<'step>,
    obligation: Option<ReturnObligation>,
) -> NodeWork<'step, KoanWorkload> {
    let carrier = working_expr.summarize();
    let continuation = ignore_results(Box::new(move |view, _idx| {
        invoke_builtin(view, picked, working_expr)
    }));
    let continuation = with_obligation(obligation, continuation);
    NodeWork::new(continuation, Some(carrier))
}

/// Run a resolved **builtin** call through the action harness. Frameless (`Inherit`), so the working
/// expression and the slot's cart are the same region here as at the decide that folded the call —
/// nothing crosses a region boundary and the read is an ordinary resident read.
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
    // Per-argument reach carriers, read back off the spliced cells (value and reach as one unit). A
    // literal arg is region-pure and contributes no cell — on the builtin lane nothing binds at a
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
/// **This is the whole of the tail hop's region crossing, and it runs before the replace.** Every
/// read of `working_expr` — the parts run, its cache, and the cells resting in it — happens here, in
/// the step that still owns that region; `run_user_fn`'s bind deep-clones each argument into the
/// fresh cart's region under the argument's own delivered reach, deliberately leaving the retiring
/// frame out of the binding. So the reinstalled incarnation's first step reads nothing in the region
/// the replace retires, and the scheduler needs no hold spanning the hop
/// ([workgraph/design/reach.md § Retention model](../../../../workgraph/design/reach.md#retention-model)).
/// The cart rides the [`FramePlacement::FreshTail`] rather than being minted at apply, which is what
/// puts the bind on this side of the boundary.
fn enter_user_fn<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    picked: OpenedFunction<'step>,
    working_expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let function = picked.value();
    // A uniquely-picked call is admitted shape-only by dispatch, so validate each argument against
    // its declared parameter type before the type-trusting `bind_by_name` — a non-satisfying typed
    // argument (e.g. a module that doesn't satisfy a `:Signature` param) is caught here.
    if let Err(e) = function.validate_call_args(working_expr.parts, view.types()) {
        return Outcome::Done(Err(e));
    }
    let mut arg_carriers = carriers_from_expr(view, &working_expr);
    if let Err(e) = deliver_value_args(view, &working_expr, &mut arg_carriers) {
        return Outcome::Done(Err(e));
    }
    // The single re-key onto parameter names: one walk of the signature over the slot-indexed
    // carriers, producing the argument record `run_user_fn` binds from. Each envelope's relocation
    // at the bind door mints that binding's reach in the per-call region, so every foreign region an
    // argument borrows into is pinned for the call's life — no separate deposit here.
    let named_carriers = map_arg_carriers(function, &arg_carriers);
    // The callee's cart: chained off the closure's captured (definition) scope, so a closure's
    // captured per-call frame survives the hop while the caller's cart does not.
    let frame = CallFrame::new(function.captured_scope());
    let exec_frame = ExecFrame {
        region: Rc::clone(&frame),
    };
    // A deferred-return FN dispatched as a tail call inside an established contract chain skips
    // resolving its own (keep-first-discarded) return type — see `run_user_fn`. An established
    // chain is exactly one with a live obligation, so the duplicate's presence answers both reads.
    let obligation = view.current_obligation_duplicate();
    let in_chain = obligation.is_some();
    let carrier = working_expr.summarize();
    match run_user_fn(
        function,
        &named_carriers,
        &exec_frame,
        in_chain,
        view.types(),
    ) {
        ExecOutcome::Tail { leading, tail, ret } => {
            // A resolved return reads its type off the signature; a deferred `Type` return carries
            // the per-call type (a `Copy` handle from `run_user_fn`) as a `PerCall` contract,
            // checked + stamped at the lift boundary like any FN return, so a recursive deferred
            // body stays TCO-flat.
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
                carrier,
                obligation,
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
            // resolution — stays TCO-flat. The type expression is a body sibling, so it joins the
            // leading run.
            let mut leading: Vec<KExpression<'step>> = leading.into_iter().copied().collect();
            leading.push(type_expr);
            body_continue(
                frame,
                leading,
                *tail,
                TailContract::FromLastResult {
                    func: picked.reseal(),
                },
                carrier,
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
    carrier: String,
    obligation: Option<ReturnObligation>,
) -> Outcome<'step> {
    let work_frame = Rc::clone(&frame);
    let continuation = ignore_results(Box::new(move |view: &DecideCtx<'_, 'step, '_>, _idx| {
        // The body crosses into the scheduler here, one working node per statement: each is a
        // slice copy of the parsed run into the installed cart's own region.
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
        work: NodeWork::new(continuation, Some(carrier)),
        frame: FramePlacement::FreshTail { frame },
        chain: ChainOp::Unchanged,
        block_entry: BlockEntry::None,
    }
}

/// Lift each spliced cell off the working expression, **one entry per part**: `Some(envelope)` for a
/// `Spliced` part, `None` for every other (a literal arg is region-pure — "no entry = no foreign
/// reach"). Parallel to `working_expr.parts` rather than a sparse `(slot, …)` list, so every reader
/// below addresses a part and its envelope by the same index with nothing to keep in step.
///
/// The cells rest in the region the dispatching step put them in; the lift re-owns each one's reach
/// under the step's coverage, so every reader downstream (the per-call reach store, a
/// value-embedding builtin's fold) works off an envelope that survives the call independently of
/// that region. Once per call, not once per reader.
fn carriers_from_expr<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    working_expr: &WorkingExpression<'step>,
) -> Vec<Option<DeliveredCarried>> {
    working_expr
        .parts
        .iter()
        .map(|part| match &part.value {
            WorkingPart::Spliced { cell } => Some(view.current_scope().lift_spliced(cell)),
            _ => None,
        })
        .collect()
}

/// Re-key the slot-indexed arg carriers onto their parameter names — the **one** walk of the
/// signature over the carriers on either lane. A committed call's parts line up 1:1 with `picked`'s
/// signature elements (`validate_call_args` enforces it), so the element at a carrier's slot names
/// its parameter. A `None` entry is read as "no foreign reach" and contributes no record field — the
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

/// Lower an action-harness builtin: hand its owned `args` to the `BodyCtx` by reference — a
/// transient record, never a `KObject`, never region-allocated — call the `ActionFn`, then
/// interpret the returned `Action` through the shared `run_action`. `arg_carriers` are the
/// per-parameter reach carriers (a value-embedding body folds / merges the one it embeds; an
/// absent entry is region-pure).
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
    // `run_action` lowers the `Action` to an `Outcome`; the harness applies the result. The step
    // view carries the ambient obligation a tail action keep-firsts against.
    super::run_action(view, action)
}

/// Deliver the call's value arguments: after this, every value part of `working_expr` has a delivery
/// envelope in its own slot of `arg_carriers`, which is what the frame bind takes. A `Spliced` part
/// already holds the one [`carriers_from_expr`] lifted for it — the cells rest in the *dispatching*
/// step's region, whose shell a framed tail hop has retired, so that lift runs under the step's own
/// coverage and is paid once per call, not once per reader. The two resolving arms **fill their own
/// slot**: the value is placed in the call scope's region and enveloped there
/// ([`Scope::deliver_resident_object`]) — `view.current_scope()` *is* the call scope (the run loop
/// opens each step's scope from the Continue-installed cart), so the fold never lands in the caller's
/// scope. The envelope's own coverage is empty, so a literal argument still pins nothing.
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
