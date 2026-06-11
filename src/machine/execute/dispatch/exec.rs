//! The dispatch-side `invoke` — the single entry that runs a resolved call. A builtin is invoked
//! directly with its [`ArgumentBundle`]; a user-defined body runs through
//! [`crate::machine::core::kfunction::exec::run_user_fn`] and its [`ExecOutcome`] is lowered onto
//! the scheduler — mapped to a `BodyResult` (then a `NodeStep`) using the scheduler's own
//! primitives (`acquire_tail_frame`, the body-chain dispatch, `add_combine_in_frame`). Kept out of
//! `ctx.rs` (the dispatcher facade) so the dispatcher core stays thin; pure body semantics live one
//! layer down in [`crate::machine::core::kfunction::exec`].

use std::rc::Rc;

use super::super::nodes::{NodeOutput, NodeStep};
use super::DispatchCtx;
use crate::machine::core::kfunction::bind_by_name::CallArgs;
use crate::machine::core::kfunction::exec::{run_user_fn, ExecOutcome, Frame as ExecFrame};
use crate::machine::core::kfunction::{
    Body, BodyResult, CombineFinish, KFunction, SchedulerHandle,
};
use crate::machine::core::{assemble_body_chain, CallArena, LexicalFrame};
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::Carried;
use crate::machine::{KError, KErrorKind, NodeId};

/// The single invoke entry for the dispatcher's bind sites — run a resolved call:
/// - **builtin** → invoked directly with its `ArgumentBundle` (kept distinct from the
///   `Record<Carried>` executor machinery; builtins keep their own I/O);
/// - **user-defined** → the `exec` executor (`run_user_fn` + the `ExecOutcome` lowering).
///
/// Every call reaches here with its value parts already `Future`/literal-resolved (the eager-subs
/// and synchronous bind paths splice them first), so there is no fall-through.
pub(super) fn invoke<'run>(
    ctx: &mut DispatchCtx<'run, '_>,
    picked: &'run KFunction<'run>,
    working_expr: KExpression<'run>,
    idx: usize,
) -> NodeStep<'run> {
    // Builtins keep their `ArgumentBundle` I/O and are called directly — the `exec` executor
    // (`run_user_fn`, `Record<Carried>`) never sees them.
    if let Body::Builtin(f) = &picked.body {
        let f = *f;
        let bundle = match picked.bind(working_expr) {
            Ok(future) => future.bundle,
            Err(e) => return NodeStep::Done(NodeOutput::Err(e)),
        };
        let result = f(ctx, bundle);
        return ctx.body_result_to_step(result, idx);
    }

    // Validate each argument against its declared parameter type before the (type-trusting)
    // `bind_by_name`: a uniquely-picked call is admitted shape-only by dispatch, so a non-satisfying
    // typed argument (e.g. a module that doesn't satisfy a `:Signature` param) is caught here.
    if let Err(e) = picked.validate_call_args(&working_expr) {
        return NodeStep::Done(NodeOutput::Err(e));
    }

    let args = match extract_carried_args(ctx, &working_expr) {
        Some(args) => args,
        // Unreachable by construction (the bind sites resolve value parts to `Future`/literal
        // first); surface a diagnostic rather than silently mis-bind if that ever breaks.
        None => {
            return NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::User(
                "exec: a call argument was not a resolved value at the bind site".to_string(),
            ))))
        }
    };

    let bound = match picked.bind_by_name(CallArgs::Positional(args)) {
        Ok(record) => record,
        Err(e) => return NodeStep::Done(NodeOutput::Err(e)),
    };

    let outer = picked.captured_scope();
    let frame = ctx.acquire_tail_frame(outer);
    let chain = ctx
        .current_lexical_chain()
        .expect("dispatch runs inside an active lexical chain");
    let exec_frame = ExecFrame {
        arena: frame.clone(),
        chain,
    };
    let result = match run_user_fn(picked, bound, &exec_frame) {
        ExecOutcome::Tail { leading, tail } if leading.is_empty() => {
            BodyResult::tail_with_frame(tail.clone(), frame, picked)
        }
        ExecOutcome::Tail { leading, tail } => {
            // Multi-statement body: dispatch the non-tail statements as sibling sub-slots, then
            // tail-replace into the last statement at body index N.
            let body_index = leading.len() + 1;
            dispatch_body_statements(ctx, &frame, &exec_frame.chain, &leading);
            BodyResult::tail_with_frame_at_index(tail.clone(), frame, picked, body_index)
        }
        ExecOutcome::Suspend { join, resume } => {
            // Deferred return: dispatch every body statement as a Combine dep, then a Combine whose
            // finish runs `resume` (the return-type check) over their resolved values. The slot
            // defers to the Combine.
            let body_ids = dispatch_body_statements(ctx, &frame, &exec_frame.chain, &join);
            let finish: CombineFinish<'run> = Box::new(move |_s, results| match resume(results) {
                ExecOutcome::Value(c) => BodyResult::Value(c),
                ExecOutcome::Errored(e) => BodyResult::Err(e),
                _ => unreachable!("a deferred-return resume yields Value or Errored"),
            });
            let mut pending = Some((body_ids, finish));
            let mut combine_id = None;
            ctx.with_active_frame(frame, &mut |s| {
                let (body_ids, finish) = pending.take().expect("body runs once");
                combine_id = Some(s.add_combine_in_frame(body_ids, vec![], finish));
            });
            BodyResult::DeferTo(combine_id.expect("combine spawns"))
        }
        ExecOutcome::Errored(e) => BodyResult::Err(e),
        ExecOutcome::Value(_) => {
            unreachable!("run_user_fn yields Tail or Suspend, not a bare Value")
        }
    };
    ctx.body_result_to_step(result, idx)
}

/// Extract the call's resolved value arguments from `working_expr`'s parts, in order. Returns
/// `None` if any value part isn't a resolved `Carried` (a `Future`-splice or a literal) — the
/// signal to fall through to the legacy binder. Keyword parts are the signature's own literals.
fn extract_carried_args<'run>(
    ctx: &mut DispatchCtx<'run, '_>,
    working_expr: &KExpression<'run>,
) -> Option<Vec<Carried<'run>>> {
    let mut args = Vec::new();
    for part in &working_expr.parts {
        match &part.value {
            ExpressionPart::Keyword(_) => {}
            ExpressionPart::Future(carried) => args.push(*carried),
            // A literal value part isn't `Future`-spliced; resolve it into the run arena now
            // (mirrors `literal_pass_through`) so it joins the args as a `'run` `Carried`.
            ExpressionPart::Literal(_) => {
                let object = ctx.current_scope().arena.alloc_object(part.value.resolve());
                args.push(Carried::Object(object));
            }
            _ => return None,
        }
    }
    Some(args)
}

/// Dispatch a body's statements as sibling sub-slots in `frame`, each positioned by the body chain
/// (assembled from `chain` + the frame's body scope, at the statement's index). Returns their node
/// ids — the multi-statement `Tail` path ignores them; the `Suspend`/Combine path joins on them.
fn dispatch_body_statements<'run>(
    ctx: &mut DispatchCtx<'run, '_>,
    frame: &Rc<CallArena>,
    chain: &Rc<LexicalFrame>,
    statements: &[&KExpression<'run>],
) -> Vec<NodeId> {
    let body_scope_id = frame.scope_for_bind().id;
    let body_chain_parent = assemble_body_chain(frame.scope_for_bind(), chain.clone(), 0)
        .parent
        .clone();
    let mut ids = Vec::with_capacity(statements.len());
    for (i, statement) in statements.iter().enumerate() {
        let statement_chain = LexicalFrame::push(body_chain_parent.clone(), body_scope_id, i + 1);
        let statement = (*statement).clone();
        let mut bid = None;
        ctx.with_active_frame(frame.clone(), &mut |s| {
            bid = Some(
                s.add_dispatch_with_chain_in_frame(statement.clone(), statement_chain.clone()),
            );
        });
        ids.push(bid.expect("body dispatch spawns"));
    }
    ids
}
