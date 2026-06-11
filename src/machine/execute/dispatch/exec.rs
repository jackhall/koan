//! **Feature-gated (`exec-v2`).** The dispatch-side lowering of an [`ExecOutcome`] onto the
//! scheduler — the reuse-path counterpart of the pruned parallel adapter. The live dispatcher's
//! resolution is reused (the caller hands in the resolved `working_expr`); an eligible body runs
//! through `run_user_fn`, and the resulting [`ExecOutcome`] is mapped to a `BodyResult` (then a
//! `NodeStep`) using the scheduler's own primitives — `acquire_tail_frame`, the body-chain
//! dispatch, `add_combine_in_frame`. Kept out of `ctx.rs` (the dispatcher facade) so the
//! dispatcher core stays thin; pure body semantics live one layer down in
//! [`crate::machine::core::kfunction::exec`].

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
use crate::machine::model::types::{DeferredReturn, ReturnType};
use crate::machine::model::Carried;
use crate::machine::NodeId;

/// Reuse the dispatcher's resolution, but run an eligible body through the exec-v2 executor instead
/// of `KFunction::invoke`. Returns `None` to fall through to the legacy `bind` + `invoke` path.
/// Eligible = a user-defined, resolved- or deferred-`TypeExpr`-return body whose value parts are all
/// `Future`-resolved or literal (a literal resolves into the run arena here). Any other part shape
/// (or a deferred-`Expression` return, which needs a sub-dispatch) falls through.
pub(super) fn try_exec_v2_call<'run>(
    ctx: &mut DispatchCtx<'run, '_>,
    picked: &'run KFunction<'run>,
    working_expr: &KExpression<'run>,
    idx: usize,
) -> Option<NodeStep<'run>> {
    let Body::UserDefined(_) = &picked.body else {
        return None;
    };
    match &picked.signature.return_type {
        ReturnType::Resolved(_) | ReturnType::Deferred(DeferredReturn::TypeExpr(_)) => {}
        ReturnType::Deferred(DeferredReturn::Expression(_)) => return None,
    }

    let mut args: Vec<Carried<'run>> = Vec::new();
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

    let bound = match picked.bind_by_name(CallArgs::Positional(args)) {
        Ok(record) => record,
        Err(e) => return Some(NodeStep::Done(NodeOutput::Err(e))),
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
    Some(ctx.body_result_to_step(result, idx))
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
