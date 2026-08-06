//! Dispatch-layer submission: the one entry point that turns a [`WorkingExpression`] into a
//! submitted dispatch slot. Binder discovery is parse-static — every parsed node caches the set of
//! binders its subtree installs into the enclosing scope ([`KExpression::binder_installs`], per the
//! position rule) — so submission does no AST recursion. It allocates the slot via
//! [`Scheduler::alloc_node`] and, for a statement submission, stamps each cached binder's
//! placeholder / pending-overload entry on the scope before the slot is ever popped, so a later
//! sibling parks rather than surfacing `UnboundName` / `DispatchFailed`.
//!
//! A submission carries a [`SubmitContext`]: a `Statement` position installs the aggregate; a
//! `SubDispatch` that is not binder-covered rejects a nested binder with
//! [`KErrorKind::NestedBinder`](crate::machine::KErrorKind::NestedBinder) (a binder must be a
//! statement, a body, or nested in another binder's own declaration slot — see
//! [design/execution/name-placeholders.md](../../../../design/execution/name-placeholders.md)).

use crate::machine::model::BinderKey;
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::machine::{KError, KErrorKind, LexicalFrame, NodeId, Scope, WriteGate};

use super::super::nodes::{NodeScope, SlotFrame};
use super::super::runtime::KoanRuntime;

/// Where a [`KoanRuntime::submit_expression`] lands, deciding how its cached binder aggregate is
/// treated.
#[derive(Clone, Copy)]
pub(in crate::machine::execute) enum SubmitContext {
    /// A statement position (top level, a block/body statement, or a fresh single-statement block):
    /// the expression's cached binder aggregate installs on the scope at the freshly allocated node.
    Statement,
    /// An eagerly-evaluated sub-dispatch (dep realization). A nested binder here is a slot-terminal
    /// [`KErrorKind::NestedBinder`] unless `binder_covered` — the dep staged a binder pick's own
    /// declaration slot, whose aggregate the enclosing statement already installed.
    SubDispatch { binder_covered: bool },
}

impl<'run> KoanRuntime<'run> {
    /// Submit `expr` as a dispatch slot against `scope` (with handle `node_scope` and
    /// `explicit_chain`, resolved by the calling submission wrapper). For a [`SubmitContext::Statement`]
    /// submission, installs the parse-time binder aggregate ([`KExpression::binder_installs`]) on the
    /// scope with this slot's freshly allocated node id — before the slot is ever popped, so a later
    /// sibling parks rather than failing. A [`SubmitContext::SubDispatch`] that is not binder-covered
    /// pre-errors the node when the aggregate is non-empty (an eager-position nested binder).
    pub(in crate::machine::execute) fn submit_expression<'a, 'step>(
        &mut self,
        expr: WorkingExpression<'a>,
        scope: &'step Scope<'step>,
        node_scope: NodeScope,
        explicit_chain: Option<std::rc::Rc<LexicalFrame>>,
        ctx: SubmitContext,
    ) -> NodeId {
        let chain = explicit_chain
        .or_else(|| self.active_payload().map(|p| p.chain.clone()))
        .expect("every dispatched node has a chain — submission outside enter_block / ambient payload is a bug");

        // Eager-position nested binder: pre-error the slot. Slot-terminal (TRY-catchable), propagates
        // through the dep like any failed dep. Every binder form is rejected here — name-installing
        // declarations (LET, TYPE, MODULE, SIG, UNION, NEWTYPE, GROUP, RECURSIVE TYPES) and named
        // `FN` / `OP` definitions alike: an eager sub-dispatch cannot install into the enclosing scope
        // soundly, and a definition whose registration silently vanished would be worse than an error.
        // Value positions take the anonymous form (`FN :{…} -> T = (…)`, which installs nothing) or a
        // name bound through a legal binder chain (`LET f = (FN …)`).
        let installs = statement_binder_installs(&expr);
        if matches!(
            ctx,
            SubmitContext::SubDispatch {
                binder_covered: false
            }
        ) && !installs.is_empty()
        {
            let carrier = expr.summarize();
            let error = KError::new(KErrorKind::NestedBinder {
                expr: carrier.clone(),
            });
            let (cart, framed) = self.submission_cart();
            let anchor = SlotFrame::new(cart, node_scope, chain);
            return self
                .sched
                .alloc_node(super::decide_error(error, carrier), anchor, framed);
        }

        // Only a statement installs; the aggregate read above also gates the sub-dispatch rejection.
        let installs = match ctx {
            SubmitContext::Statement => installs,
            SubmitContext::SubDispatch { .. } => Vec::new(),
        };

        let (cart, framed) = self.submission_cart();
        let anchor = SlotFrame::new(cart, node_scope, chain.clone());
        let id = self
            .sched
            .alloc_node(super::decide_tail(expr, None), anchor, framed);

        // Stamp each cached binder's placeholder at the enclosing statement's lexical position — the
        // SAME `BindingIndex` the eventual `register_*` call at finalize installs. Installs are
        // best-effort: lenient when `data[name]` is already a KFunction or the same slot re-installs.
        if !installs.is_empty() {
            let bind_index = scope.binding_position(Some(&chain));
            // The submission-channel stamp is run-loop-owned: dispatch submits the binder with no
            // koan frame on the stack, the same footing the apply loop writes on.
            let mut gate = WriteGate::for_run_loop();
            for key in installs {
                match key {
                    BinderKey::Name(name, kind) => {
                        let _ = scope.install_placeholder(name, id, bind_index, kind, &mut gate);
                    }
                    BinderKey::Bucket(buckets) => {
                        for bucket in buckets {
                            let _ =
                                scope.install_pending_overload(bucket, id, bind_index, &mut gate);
                        }
                    }
                }
            }
        }
        id
    }
}

/// The binder aggregate of a statement, read back off the working node the parsed statement crossed
/// over as. [`KExpression::binder_installs`] states the position rule and computes the aggregate at
/// parse time; this reads the same answer through the slots the crossing preserved — the node's own
/// plan key, then, transitively, what each of its chain-slot children installs. A node the scheduler
/// synthesized carries no plan and installs nothing, which is the whole rule for it: a binder is
/// always a parsed statement.
fn statement_binder_installs(expr: &WorkingExpression<'_>) -> Vec<BinderKey> {
    // A redundant single-`Expression` paren wrapper (`((…))`) passes its child's aggregate straight
    // through. A binder is always keyword-led, so this never co-occurs with the plan branch below.
    if let [only] = expr.parts {
        if let WorkingPart::Ast(ExpressionPart::Expression(child)) = only.value {
            return child
                .binder_installs()
                .iter()
                .map(|key| key.to_owned_key())
                .collect();
        }
    }
    let Some(plan) = expr.binder_plan() else {
        return Vec::new();
    };
    let mut installs = vec![plan.key.to_owned_key()];
    for (index, part) in expr.parts.iter().enumerate() {
        if !plan.chain_slot_mask.get(index).copied().unwrap_or(false) {
            continue;
        }
        if let WorkingPart::Ast(ExpressionPart::Expression(child)) = part.value {
            if !child.is_statement_block() {
                installs.extend(child.binder_installs().iter().map(|key| key.to_owned_key()));
            }
        }
    }
    installs
}
