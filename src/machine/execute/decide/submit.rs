//! Dispatch-layer submission: turns a [`WorkingExpression`] into a freshly allocated dispatch slot
//! via [`Scheduler::alloc_node`]. Binder discovery is parse-static and **per-statement** — a node
//! caches what it itself installs ([`WorkingExpression::binder_plan`]), so submission reads one
//! field and does no AST recursion. A statement submission stamps that binder's placeholder /
//! pending-overload entries on the scope before the slot is ever popped, so a later sibling parks
//! rather than surfacing `UnboundName` / `DispatchFailed`.
//!
//! Binding is a statement-level act — the legal positions are exactly statement position and a
//! lazily-captured body (see
//! [design/execution/name-placeholders.md](../../../../design/execution/name-placeholders.md)); a
//! [`SubmitContext::SubDispatch`] binder is rejected with [`KErrorKind::NestedBinder`].

use crate::machine::ProducerId;
use crate::machine::model::StoredBinderKey;
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::machine::{BindingIndex, KError, KErrorKind, LexicalFrame, NodeId, Scope, WriteGate};
use crate::scheduler::EdgeId;

use super::super::harness::{Host, KoanWorkload};
use super::super::nodes::{NodeScope, SlotFrame, WorkLabel};
use crate::scheduler::Scheduler;

/// Where a [`Host::submit_expression`] lands, deciding how its cached binder plan is treated.
#[derive(Clone, Copy)]
pub(in crate::machine::execute) enum SubmitContext {
    /// Top level, a block/body statement, or a fresh single-statement block.
    Statement,
    /// An eagerly-evaluated sub-dispatch (dep realization). A binder here is rejected with no
    /// exceptions: the eager positions are all value positions, and a value position is not where a
    /// name is introduced.
    SubDispatch,
}

impl<'run> Host<'run> {
    /// `node_scope` and `explicit_chain` are resolved by the calling submission wrapper. A
    /// [`SubmitContext::Statement`] binder plan installs against this slot's freshly allocated node
    /// id before the slot is ever popped, so a later sibling parks rather than failing.
    pub(in crate::machine::execute) fn submit_expression<'a, 'step>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        expr: WorkingExpression<'a>,
        scope: &'step Scope<'step>,
        node_scope: NodeScope,
        explicit_chain: Option<std::rc::Rc<LexicalFrame>>,
        ctx: SubmitContext,
    ) -> NodeId {
        let chain = explicit_chain
        .or_else(|| self.ambient.active_payload().map(|p| p.chain.clone()))
        .expect("every dispatched node has a chain — submission outside enter_block / ambient payload is a bug");

        // Eager-position binder: pre-error the slot, slot-terminal and TRY-catchable. No binder
        // form is exempt — an eager sub-dispatch cannot install into the enclosing scope soundly,
        // and a definition whose registration silently vanished would be worse than an error. A
        // value position takes the anonymous `FN :{…} -> <Return> = (…)`, which installs nothing.
        let installs = statement_binder_plan(&expr).map(StoredBinderKey::to_owned_key);
        if let (SubmitContext::SubDispatch, Some(key)) = (ctx, &installs) {
            let carrier = expr.summarize();
            // A rejected declaration that registers overloads (an `FN` / `OP` in a `LET`'s value
            // slot) has a one-statement spelling to suggest; a nested plain `LET` does not.
            let error = KError::new(KErrorKind::NestedBinder {
                expr: carrier.clone(),
                suggest_flat: !key.buckets.is_empty(),
            });
            return self.submit_pre_errored(sched, &expr, node_scope, chain, error);
        }

        // Only a statement installs; the plan read above also gates the sub-dispatch rejection.
        let installs = match ctx {
            SubmitContext::Statement => installs,
            SubmitContext::SubDispatch => None,
        };

        let (cart, framed) = self.ambient.submission_cart();
        let anchor = SlotFrame::new(cart, node_scope, chain.clone(), WorkLabel::of(&expr));
        let id = sched.alloc_node(
            super::decide_tail(expr, None),
            &[],
            std::rc::Rc::clone(&anchor),
            framed,
        );

        // The stamp carries the SAME `BindingIndex` the finalize write does, so a consumer's
        // visibility test stays consistent across the pending → finalized transition.
        if let Some(key) = installs {
            let bind_index = BindingIndex::value(chain.index);
            // The claim's edge is destined at **this** scope's region — the scope the name is
            // being introduced into — so a consumer parking on the claim inherits that destination
            // and its delivery lands where the binding lives. Holding the owner across the install
            // is the wiring-time proof the region is pinned; the slot is allocated before the
            // stamp, so no install can name a terminal producer.
            let destination = scope
                .region_owner()
                .upgrade()
                .expect("a live scope reference implies a live region owner");
            let mut edges: Vec<EdgeId> = Vec::new();
            let claim = |sched: &mut Scheduler<KoanWorkload>| sched.install_edge(id, &destination);
            let mut gate = WriteGate::for_run_loop();
            if let Some((name, kind)) = key.name {
                let edge = claim(sched);
                edges.push(edge);
                let _ = scope.install_placeholder(
                    name,
                    ProducerId::from_scheduler_edge(edge),
                    bind_index,
                    kind,
                    &mut gate,
                );
            }
            for bucket in key.buckets {
                let edge = claim(sched);
                edges.push(edge);
                let _ = scope.install_pending_overload(
                    bucket,
                    ProducerId::from_scheduler_edge(edge),
                    bind_index,
                    &mut gate,
                );
            }
            // The slot owns every claim it stamped: it releases the edges when it terminalizes, and
            // the same act records that this slot's statement index addresses claims in the store.
            anchor.own_claim_edges(edges);
        }
        id
    }

    /// Allocate a slot that is **terminal-errored before it runs** — the shape a statement rejected
    /// at submission takes. Slot-terminal and TRY-catchable, it propagates through the dep like any
    /// other failed dep, and it claims nothing: a rejected declaration introduces no name.
    pub(in crate::machine::execute) fn submit_pre_errored(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        expr: &WorkingExpression<'_>,
        node_scope: NodeScope,
        chain: std::rc::Rc<LexicalFrame>,
        error: KError,
    ) -> NodeId {
        let (cart, framed) = self.ambient.submission_cart();
        let anchor = SlotFrame::new(cart, node_scope, chain, WorkLabel::of(expr));
        sched.alloc_node(super::decide_error(error), &[], anchor, framed)
    }
}

/// What a statement installs, read back off the working node the parsed statement crossed over as.
/// The node's *own* plan key, never anything its slots contain — the namespace a block introduces is
/// legible from its statement spines alone, which is what lets the block fan-out rule on duplicate
/// declarations before any statement runs. A scheduler-synthesized node carries no plan and
/// installs nothing: a binder is always a parsed statement.
///
/// The stored (borrowed) form: every key it names is a borrow into the declaring node's own region,
/// so reading a block's whole namespace allocates nothing. The submission path materializes the
/// owned twin only for the statement it is actually stamping.
pub(in crate::machine::execute) fn statement_binder_plan<'a>(
    expr: &WorkingExpression<'a>,
) -> Option<StoredBinderKey<'a>> {
    // A redundant single-`Expression` paren wrapper (`((…))`) is the same statement, so it reads
    // its child's plan straight through. A binder is always keyword-led, so this never co-occurs
    // with the plan branch below.
    if let [only] = expr.parts
        && let WorkingPart::Ast(ExpressionPart::Expression(child)) = only.value
    {
        return child.binder_plan();
    }
    expr.binder_plan()
}
