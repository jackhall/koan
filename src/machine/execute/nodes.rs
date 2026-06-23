use std::rc::Rc;

use super::runtime::KoanWorkload;
use crate::machine::core::kfunction::body::{ErasedContract, ReturnContract};
use crate::machine::core::{assemble_body_chain, ErasedScopePtr, ScopeId};
use crate::machine::model::Carried;
use crate::machine::{CallFrame, KError, LexicalFrame, NodeId};

/// The generic per-node state lives in [`crate::scheduler::nodes`]; re-exported here so the Koan
/// execute tree has a single `nodes` surface combining them with the Koan-side [`NodeStep`] /
/// [`NodePayload`] / [`NodeScope`].
pub(super) use crate::scheduler::nodes::{Node, NodeFrame, NodeWork};

/// Outcome of a node's run. `Replace` is the tail-call path: rewrite the slot's work and
/// re-enqueue the same index so it runs again with no fresh slot allocated, giving constant
/// memory across tail-call sequences. When `frame` is `Some`, its `scope()` becomes the
/// slot's scope and its `region()` owns per-call allocations; `None` keeps the existing
/// frame and scope. `contract`, when set, is the erased return contract the replacement is
/// entering — kept-first against the slot's prior contract by the reinstall site; any error
/// landing on this slot is checked against it. `chain` is the pre-decided lexical-chain reshape
/// (see [`ChainOp`]), already lowered from the contract variant so this whole variant is
/// lifetime-free — the only `'`-bearing arm is `Done`.
// `Replace` is intrinsically the large variant (it carries `NodeWork` plus the
// frame/contract/chain tail-call payload); `Done` only grows with the cached
// `KExpression` it indirectly holds. Boxing a short-lived return value's hot tail-call
// path to balance the variants is the wrong trade — the imbalance is inherent.
#[allow(clippy::large_enum_variant)]
pub(super) enum NodeStep<'step> {
    /// The terminal value is born at the step lifetime `'step` (the consumer frame the step ran
    /// against): it is finalized *within* the step that produced it (the run loop's `run_step` erases
    /// it into the slot store before the step's frame witness drops), so it never crosses the
    /// step-guard exit as a fabricated `'run`. The only lifetime-bearing arm — `Replace`'s contract
    /// is erased and its chain reshape lowered to a [`ChainOp`] in `apply_outcome`, so it carries no
    /// `'run`.
    Done(Result<Carried<'step>, KError>),
    Replace {
        work: NodeWork<KoanWorkload>,
        frame: Option<Rc<CallFrame>>,
        contract: Option<ErasedContract>,
        chain: ChainOp,
    },
    /// The slot is spliced out as an alias of `producer` (a bare-name forward whose producer was not
    /// yet ready). The slot's consumers have already been moved onto `producer`'s notify list; this
    /// just marks the slot so `read_result` follows through to `producer`. See [`Outcome::Forward`].
    Alias(NodeId),
}

/// The lexical-chain reshape a [`NodeStep::Replace`] applies, decided in `apply_outcome` from the
/// `Continue`'s `block_entry` annotation and the contract *variant* (while still live), then
/// assembled in the run loop against the post-step frame. Splitting the decision (contract-reading,
/// at apply) from the assembly (frame-reading, in the run loop) is what lets `Replace` shed its
/// `'run`: the variant is read before erasure and frozen into this lifetime-free tag.
pub(super) enum ChainOp {
    /// TCO in the same lexical block — chain unchanged.
    Unchanged,
    /// FN-body invoke (a `Function`/`PerCall` contract): rebuild from the body scope's lexical
    /// `outer` walk so depth tracks lexical nesting, not call depth, with the body at `body_index`.
    AssembleBody { body_index: usize },
    /// Block entry (MATCH / TRY arm, non-`Function` contract): prepend `(scope_id, body_index)` to
    /// the chain. `body_index` positions the pushed frame for multi-statement tail-into-last (`0` is
    /// the single-statement case).
    PushBlock {
        scope_id: ScopeId,
        body_index: usize,
    },
}

impl ChainOp {
    /// Decide the reshape from a `Continue`'s `block_entry` and the still-live contract variant,
    /// before the contract is erased onto the [`NodeStep::Replace`]. `Function`/`PerCall` (a deferred
    /// FN body) both assemble the FN-body chain; any other contract under a block entry prepends.
    pub(super) fn decide(
        block_entry: Option<ScopeId>,
        contract: Option<&ReturnContract<'_>>,
        body_index: usize,
    ) -> Self {
        let Some(scope_id) = block_entry else {
            return ChainOp::Unchanged;
        };
        match contract {
            Some(ReturnContract::Function(_) | ReturnContract::PerCall { .. }) => {
                ChainOp::AssembleBody { body_index }
            }
            _ => ChainOp::PushBlock {
                scope_id,
                body_index,
            },
        }
    }

    /// Assemble the new chain in the run loop. `body_frame` is the cart the body runs in — the
    /// freshly installed frame for a `FreshChild`/`ReuseReserve` tail, or the slot's already-installed
    /// current cart for an `Inherit` FN-body re-entry (the folded `invoke`) — read only by the
    /// `AssembleBody` arm.
    pub(super) fn apply(
        self,
        prev_chain: Rc<LexicalFrame>,
        body_frame: &CallFrame,
    ) -> Rc<LexicalFrame> {
        match self {
            ChainOp::Unchanged => prev_chain,
            ChainOp::AssembleBody { body_index } => {
                assemble_body_chain(body_frame.scope(), prev_chain, body_index)
            }
            ChainOp::PushBlock {
                scope_id,
                body_index,
            } => LexicalFrame::push(Some(prev_chain), scope_id, body_index),
        }
    }
}

/// Slot-stored scope handle, carrying no lifetime so the node it sits on does not pin `'run`
/// through its scope. Both arms are **cart-witnessed** — re-projected from the slot's live frame at
/// read time, never re-anchored at a free `'run`:
///
/// - `Yoked` — no pointer at all: the slot's scope *is* its own per-call cart's scope, re-projected
///   from the [`Node::frame`](crate::scheduler::nodes::Node) cart (`scope_bounded`). Single-cart: the
///   frame `Rc` already on the slot is the sole liveness witness, so there is no second `Rc` clone
///   and no contention with `try_reset_for_tail`'s uniqueness check.
/// - `YokedChild` — an erased [`ErasedScopePtr`] to a block scope a builtin allocated in a cart
///   *ancestor* region (an `InScope` body — USING / MODULE / SIG / TRY). Re-attached at read with a
///   borrow bounded by the slot's frame `Rc` ([`ErasedScopePtr::reattach`]), sound because the cart's
///   `FrameStorage.outer` chain pins that ancestor region for as long as the slot holds the cart.
///   Distinct from `Yoked` only in that the child differs from the cart's own scope, so it needs a
///   stored pointer.
///
/// Storing an erased, frame-witnessed handle keeps the borrow honest across a TCO `try_reset_for_tail`
/// (nothing persisted points into the reset region; the live frame is re-read each step) and keeps the
/// slot from naming `'run` in its node-stored scope state.
///
/// `Copy` because both arms are trivially copyable ([`ErasedScopePtr`] is `Copy` / a unit) and
/// submission threads the handle through `pre_subs` recursion without re-deriving it.
#[derive(Clone, Copy)]
pub(super) enum NodeScope {
    YokedChild(ErasedScopePtr),
    Yoked,
}

/// The opaque per-node workload payload: the Koan name-resolution state the scheduler stores on a
/// slot, threads through a step, and hands back, but does not own as scheduler machinery — the
/// slot's [`NodeScope`] handle and its lexical [`chain`](Self::chain). Lifetime-free (the scope is
/// an erased `NodeScope`, the chain an `Rc`), so the node it sits on pins no `'run` through it. This
/// is the concrete Koan stand-in for the generic workload payload the scheduler is parametric
/// over (`KoanWorkload::Payload`). Cheap-`Clone`: `NodeScope` is `Copy`, the chain
/// is an `Rc`.
#[derive(Clone)]
pub(super) struct NodePayload {
    pub(super) scope: NodeScope,
    /// Immutable cactus-chain naming this node's lexical position. Head frame is the
    /// innermost enclosing block; tail (`parent: None`) is top-level. See
    /// `core/lexical_frame.rs`.
    pub(super) chain: Rc<LexicalFrame>,
}

#[cfg(test)]
mod tests {
    //! Miri coverage for the `NodeScope::YokedChild` lifetime fabrication: each test pins the
    //! erase→reattach shape under tree borrows; logical assertions are minimal — these fail when
    //! Miri reports UB, not on values.

    use super::*;
    use crate::builtins::default_scope;
    use crate::machine::core::KoanRegion;
    use crate::machine::model::KObject;
    use crate::machine::{BindingIndex, Scope};

    /// A `NodeScope::YokedChild` erases a cart-ancestor block scope to a lifetime-free
    /// `ErasedScopePtr` (`erase`) and reattaches it ([`ErasedScopePtr::reattach`]) at read — the
    /// fabrication the scheduler performs each step for a `YokedChild` slot, the borrow bounded by
    /// the slot's frame. Mirrors the erase→reattach pair plus a subsequent region mutation through a
    /// sibling pointer; fails on UB, not values.
    #[test]
    fn node_scope_yoked_child_erase_reattach_roundtrip() {
        let region = KoanRegion::new();
        let scope = default_scope(&region, Box::new(std::io::sink()));
        let v = region.alloc_object(KObject::Number(7.0));
        scope
            .bind_value("k".to_string(), v, BindingIndex::BUILTIN)
            .unwrap();

        let ns = NodeScope::YokedChild(ErasedScopePtr::erase(scope));
        let NodeScope::YokedChild(ptr) = &ns else {
            unreachable!("constructed YokedChild")
        };
        // Reattach with a borrow bounded by the region witness; read a binding back, then mutate the
        // region through a sibling pointer while the reattached scope is still live.
        let reattached: &Scope<'_> = ptr.reattach_witnessed(&region);
        assert!(matches!(reattached.lookup("k"), Some(KObject::Number(n)) if *n == 7.0));
        let _other = region.alloc_object(KObject::Number(8.0));
        assert!(reattached.lookup("k").is_some());
    }
}
