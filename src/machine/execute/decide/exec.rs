//! The dispatch-side invoke — the single entry that runs a resolved call. A builtin runs through
//! the action harness (its bound args handed to `BodyCtx` as a transient owned record); a
//! user-defined body runs through [`crate::machine::core::kfunction::exec::run_user_fn`] and its
//! [`ExecOutcome`] is lowered to an [`Action::tail`] the shared
//! [`run_action`](super::run_action) interprets.
//! Kept out of `ctx.rs` (the dispatcher facade) so the dispatcher core stays thin; pure body
//! semantics live one layer down in [`crate::machine::core::kfunction::exec`].

use super::super::nodes::{ChainOp, WorkLabel};
use super::super::obligation::ReturnObligation;
use super::super::outcome::DepTerminal;
use super::super::outcome::Outcome;
use super::super::outcome::Replacement;
use super::super::{NodeContinuation, decide_only, erase_bumped};
use super::ctx::DecideCtx;
use std::rc::Rc;

use crate::machine::core::BoundArgs;
use crate::machine::core::ReturnContract;
use crate::machine::core::{Action, BlockEntry, FramePlacement, TailContract};
use crate::machine::core::{Body, CallFrame, KFunction, OpenedFunction};
use crate::machine::core::{ExecFrame, ExecOutcome, PerCallReturn, run_user_fn};
use crate::machine::model::{ExpressionPart, KExpression, WorkingExpression, WorkingPart};
use crate::machine::{DeliveredCarried, KError, KErrorKind, NodeId};
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
            replacement: Replacement::inherit(builtin_work(
                view,
                picked,
                working_expr,
                view.current_obligation(),
            )),
            chain: ChainOp::Unchanged,
            block_entry: BlockEntry::None,
        },
        _ => enter_user_fn(view, picked, working_expr),
    }
}

/// `obligation` rides the continuation as data, so the replacement step re-deposits the established
/// declared-return checker. The invoke is `Inherit`, so the closure — `Copy` captures only — is
/// hosted in the cart `view` already stands in.
fn builtin_work<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    picked: OpenedFunction<'step>,
    working_expr: WorkingExpression<'step>,
    obligation: Option<ReturnObligation>,
) -> NodeContinuation<'step> {
    NodeContinuation::new(
        obligation,
        erase_bumped(
            view.current_scope().brand(),
            decide_only(move |view: &DecideCtx<'_, 'step, '_>, _idx| {
                invoke_builtin(view, picked, working_expr)
            }),
        ),
    )
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
    // The values half of the argument view, on the step scratch: one slot per declared parameter,
    // aligned with the signature's own schema. No name-keyed container is built for the call.
    let schema = function.signature.params();
    let mut slots = BumpVec::with_capacity_in(schema.len(), view.scratch());
    if let Err(e) = function.bind_args_into(
        working_expr.parts,
        view.current_scope(),
        view.registries(),
        &arg_carriers,
        &mut slots,
    ) {
        return Outcome::Done(Err(e));
    }
    run_action_builtin(view, f, BoundArgs::new(schema, &slots))
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
    if let Err(e) = function.validate_call_args(working_expr.parts, view.registries()) {
        return Outcome::Done(Err(e));
    }
    let mut arg_carriers = carriers_from_expr(view, &working_expr);
    if let Err(e) = deliver_value_args(view, &working_expr, &mut arg_carriers) {
        return Outcome::Done(Err(e));
    }
    // Each envelope's relocation at the bind door mints that binding's reach in the per-call region,
    // so every foreign region an argument borrows into is pinned for the call's life — no separate
    // deposit here.
    let named_carriers = parameter_carriers(function, &arg_carriers, view.scratch());
    // Chained off the closure's captured (definition) scope, so a closure's captured per-call frame
    // survives the hop while the caller's cart does not.
    let frame = CallFrame::new(function.captured_scope());
    let exec_frame = ExecFrame {
        region: Rc::clone(&frame),
    };
    // An established contract chain is exactly one with a live obligation, so the copy's
    // presence answers both reads: a deferred-return FN dispatched as a tail call inside one skips
    // resolving its own (keep-first-discarded) return type — see `run_user_fn`.
    let obligation = view.current_obligation();
    let in_chain = obligation.is_some();
    let label = WorkLabel::of(&working_expr);
    // The call's own extent, retained `Copy` so a contract's error arm can render the invoked
    // expression's text without the success path paying for it.
    let site = working_expr.source_ref();
    match run_user_fn(
        function,
        &named_carriers,
        &exec_frame,
        in_chain,
        view.registries(),
    ) {
        ExecOutcome::Tail { leading, tail, ret } => {
            // A deferred `Type` return's per-call type rides a `PerCall` contract, checked +
            // stamped at the lift boundary like any FN return, so a recursive deferred body stays
            // TCO-flat.
            let contract = match ret {
                PerCallReturn::FromSignature => ReturnContract::Function {
                    func: picked.reseal(),
                    site,
                },
                PerCallReturn::Resolved(ret) => ReturnContract::PerCall {
                    func: picked.reseal(),
                    ret,
                    site,
                },
            };
            body_continue(
                frame,
                &leading,
                None,
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
            body_continue(
                frame,
                &leading,
                Some(type_expr),
                *tail,
                TailContract::FromLastResult {
                    func: picked.reseal(),
                    site,
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
/// The continuation bumps into the fresh cart's own region, so its captures are all `Copy`: the
/// leading statements ride as a region slice co-located with the closure, and the cart itself is
/// re-derived from the slot's anchor at wake rather than captured. `extra` is a deferred-return
/// type expression, which joins the leading run as its last statement.
///
/// The tail re-enters the installed cart with `Inherit` — a second `FreshTail` here would mint
/// another cart, discarding the one already holding the bound params — and the block entry carries
/// it so the lowering fans any leading statements into it.
fn body_continue<'step>(
    frame: Rc<CallFrame>,
    leading: &[&KExpression<'step>],
    extra: Option<KExpression<'step>>,
    tail: KExpression<'step>,
    contract: TailContract<'step>,
    label: WorkLabel,
    obligation: Option<ReturnObligation>,
) -> Outcome<'step> {
    let replacement = Replacement::fresh_tail(&frame, |host| {
        let mut run =
            BumpVec::with_capacity_in(leading.len() + extra.is_some() as usize, host.allocator());
        run.extend(leading.iter().map(|statement| **statement));
        run.extend(extra);
        let leading: &[KExpression<'_>] = run.leak();
        let call = erase_bumped(
            host,
            move |view: &DecideCtx<'_, '_, '_>,
                  _results: &[Result<DepTerminal<'_>, KError>],
                  _idx: NodeId| {
                let brand = view.current_scope().brand();
                let cart = view
                    .current_frame()
                    .expect("a body-enter wake runs against the cart its replace installed");
                super::run_action(
                    view,
                    Action::tail(
                        leading
                            .iter()
                            .map(|statement| WorkingExpression::from_ast(brand, *statement))
                            .collect(),
                        WorkingExpression::from_ast(brand, tail),
                        contract,
                        FramePlacement::Inherit,
                        BlockEntry::FrameScope(cart),
                    ),
                )
            },
        );
        NodeContinuation::new(obligation, call)
    });
    Outcome::Continue {
        replacement,
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

/// Select the delivery envelopes belonging to a call's *parameters*, in declaration order — the
/// values half of the user-defined lane's argument view. A committed call's parts line up 1:1 with
/// the signature's elements ([`KFunction::validate_call_args`] enforces it), so `part_slots`
/// addresses each parameter's envelope positionally. Every value slot is filled on this lane
/// ([`deliver_value_args`] guarantees it), so the slice this returns holds one envelope per
/// parameter and is the whole argument currency the frame bind reads. Nothing is keyed.
fn parameter_carriers<'e, 'step>(
    picked: &KFunction<'step>,
    arg_carriers: &'e [Option<DeliveredCarried>],
    scratch: crate::witnessed::BumpAllocator<'step>,
) -> BumpVec<'step, &'e DeliveredCarried> {
    let slots = picked.signature.part_slots();
    let mut carriers = BumpVec::with_capacity_in(slots.len(), scratch);
    carriers.extend(
        slots
            .iter()
            .filter_map(|slot| arg_carriers.get(*slot as usize).and_then(Option::as_ref)),
    );
    carriers
}

/// `args` reaches the `BodyCtx` as the schema-keyed view — never a `KObject`, never
/// region-allocated, and never a per-call map. Its slots carry the bound cells and the
/// per-parameter reach carriers (a value-embedding body folds / merges the one it embeds; an
/// absent carrier is region-pure).
fn run_action_builtin<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    f: crate::machine::core::ActionFn,
    args: BoundArgs<'step, '_>,
) -> Outcome<'step> {
    use crate::machine::core::BodyCtx;

    let frame = view.current_frame();
    let chain = view.active_chain();
    let action = {
        let body_ctx = BodyCtx {
            scope: view.current_scope(),
            frame: frame.as_ref(),
            chain,
            args,
            installer: view.installer(),
            ctx: view.step_ctx(),
            registries: view.registries(),
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
