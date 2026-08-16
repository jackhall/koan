//! Dispatch-layer submission: the one entry point that turns a [`WorkingExpression`] into a
//! submitted dispatch slot. Binder discovery is parse-static and **per-statement**: a node caches
//! what it itself installs ([`KExpression::binder_plan`]), so submission reads one field and does
//! no AST recursion. It allocates the slot via [`Scheduler::alloc_node`] and, for a statement
//! submission, stamps that binder's placeholder / pending-overload entries on the scope before the
//! slot is ever popped, so a later sibling parks rather than surfacing `UnboundName` /
//! `DispatchFailed`.
//!
//! A submission carries a [`SubmitContext`]: a `Statement` position installs; a `SubDispatch`
//! rejects a binder with
//! [`KErrorKind::NestedBinder`](crate::machine::KErrorKind::NestedBinder). Binding is a
//! statement-level act — the legal positions are exactly statement position and a lazily-captured
//! body (see
//! [design/execution/name-placeholders.md](../../../../design/execution/name-placeholders.md)).

use crate::machine::ProducerId;
use crate::machine::model::BinderKey;
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::machine::{BindingIndex, KError, KErrorKind, LexicalFrame, NodeId, Scope, WriteGate};
use crate::scheduler::EdgeId;

use super::super::harness::{Host, KoanWorkload};
use super::super::nodes::{NodeScope, SlotFrame};
use crate::scheduler::Scheduler;

/// Where a [`KoanRuntime::submit_expression`] lands, deciding how its cached binder plan is
/// treated.
#[derive(Clone, Copy)]
pub(in crate::machine::execute) enum SubmitContext {
    /// A statement position (top level, a block/body statement, or a fresh single-statement block):
    /// the expression's cached binder plan installs on the scope at the freshly allocated node.
    Statement,
    /// An eagerly-evaluated sub-dispatch (dep realization). A binder here is a slot-terminal
    /// [`KErrorKind::NestedBinder`], with no exceptions: the eager positions are all value
    /// positions, and a value position is not where a name is introduced.
    SubDispatch,
}

impl<'run> Host<'run> {
    /// Submit `expr` as a dispatch slot against `scope` (with handle `node_scope` and
    /// `explicit_chain`, resolved by the calling submission wrapper). For a
    /// [`SubmitContext::Statement`] submission, installs the statement's own parse-time binder plan
    /// ([`KExpression::binder_plan`]) on the scope with this slot's freshly allocated node id —
    /// before the slot is ever popped, so a later sibling parks rather than failing. A
    /// [`SubmitContext::SubDispatch`] carrying a binder pre-errors the node.
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

        // Eager-position binder: pre-error the slot. Slot-terminal (TRY-catchable), propagates
        // through the dep like any failed dep. Every binder form is rejected here — name-installing
        // declarations (LET, TYPE, MODULE, SIG, UNION, NEWTYPE, GROUP) and named
        // `FN` / `OP` definitions alike: an eager sub-dispatch cannot install into the enclosing scope
        // soundly, and a definition whose registration silently vanished would be worse than an error.
        // A value position takes the anonymous form (`FN :{…} -> T = (…)`, which installs nothing);
        // a definition that must also bind a name is one statement, in the combined `LET <name> = FN …`
        // spelling.
        let installs = statement_binder_plan(&expr);
        if let (SubmitContext::SubDispatch, Some(key)) = (ctx, &installs) {
            let carrier = expr.summarize();
            // A rejected declaration that registers overloads (an `FN` / `OP` in a `LET`'s value
            // slot) has a one-statement spelling to suggest; a nested plain `LET` does not.
            let error = KError::new(KErrorKind::NestedBinder {
                expr: carrier.clone(),
                suggest_flat: !key.buckets.is_empty(),
            });
            let (cart, framed) = self.ambient.submission_cart();
            let anchor = SlotFrame::new(cart, node_scope, chain);
            return sched.alloc_node(super::decide_error(error, carrier), &[], anchor, framed);
        }

        // Only a statement installs; the plan read above also gates the sub-dispatch rejection.
        let installs = match ctx {
            SubmitContext::Statement => installs,
            SubmitContext::SubDispatch => None,
        };

        let (cart, framed) = self.ambient.submission_cart();
        let anchor = SlotFrame::new(cart, node_scope, chain.clone());
        let id = sched.alloc_node(
            super::decide_tail(expr, None),
            &[],
            std::rc::Rc::clone(&anchor),
            framed,
        );

        // Stamp each cached binder's placeholder at the enclosing statement's lexical position — the
        // SAME `BindingIndex` the eventual `register_*` call at finalize installs. Installs are
        // best-effort: lenient when `data[name]` is already a KFunction or the same slot re-installs.
        if let Some(key) = installs {
            let bind_index = BindingIndex::value(chain.index);
            // The claim's edge is destined at **this** scope's region — the scope the name is being
            // introduced into — so a consumer parking on the claim inherits that destination and its
            // delivery lands where the binding lives. Holding the owner across the install is the
            // wiring-time proof the region is pinned; the slot was allocated on the line above, so
            // the install cannot see a terminal producer.
            let destination = scope
                .region_owner()
                .upgrade()
                .expect("a live scope reference implies a live region owner");
            let mut edges: Vec<EdgeId> = Vec::new();
            let claim = |sched: &mut Scheduler<KoanWorkload>| sched.install_edge(id, &destination);
            // The submission-channel stamp is run-loop-owned: dispatch submits the binder with no
            // koan frame on the stack, the same footing the apply loop writes on.
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
            // The slot owns every name it stamped: it releases them when it terminalizes, and hands
            // them on to a fresh anchor if a tail replace mints one.
            anchor.own_edges(edges);
        }
        id
    }
}

/// What a statement installs, read back off the working node the parsed statement crossed over as.
/// Per-statement and nothing more: the node's *own* plan key, never anything its slots contain — the
/// namespace a block introduces is legible from its statement spines alone. A node the scheduler
/// synthesized carries no plan and installs nothing, which is the whole rule for it: a binder is
/// always a parsed statement.
fn statement_binder_plan(expr: &WorkingExpression<'_>) -> Option<BinderKey> {
    // A redundant single-`Expression` paren wrapper (`((…))`) is the same statement, so it reads
    // its child's plan straight through. A binder is always keyword-led, so this never co-occurs
    // with the plan branch below.
    if let [only] = expr.parts
        && let WorkingPart::Ast(ExpressionPart::Expression(child)) = only.value
    {
        return child.binder_plan().map(|key| key.to_owned_key());
    }
    expr.binder_plan().map(|key| key.to_owned_key())
}
