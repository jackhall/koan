use std::rc::Rc;

use super::runtime::KoanWorkload;
use crate::machine::core::ReturnContract;
use crate::machine::core::{assemble_body_chain, ScopeId, ScopeRefFamily};
use crate::machine::{CallFrame, KError, LexicalFrame, NodeId};
use crate::witnessed::SealedExtern;

use super::StepCarried;

/// The generic per-node work lives in [`crate::scheduler::nodes`]; re-exported here so the Koan
/// execute tree has a single `nodes` surface combining it with the Koan-side [`NodeStep`] /
/// [`NodePayload`] / [`NodeScope`] / [`SlotFrame`].
pub(super) use crate::scheduler::nodes::NodeWork;

/// Koan's `Workload::Frame` — the scheduler-held per-slot memory anchor. Wraps the shared
/// per-call cart with the slot's own [`NodeScope`] handle and lexical [`chain`]. The scheduler
/// holds one `Rc<SlotFrame>` per slot and projects the region owner (`FrameStorage`) through
/// [`Anchor::owner`] where retention and delivery need it.
pub(super) struct SlotFrame {
    pub(super) cart: Rc<CallFrame>,
    pub(super) payload: NodePayload,
}

impl crate::scheduler::Anchor for SlotFrame {
    type Owner = crate::machine::FrameStorage;
    fn owner(&self) -> &Rc<crate::machine::FrameStorage> {
        self.cart.storage()
    }
}

impl SlotFrame {
    /// Mint a slot anchor from the cart plus the slot's scope handle and chain — the one
    /// constructor, so submission/replace mint sites stay one-liners.
    pub(super) fn new(
        cart: Rc<CallFrame>,
        scope: NodeScope,
        chain: Rc<LexicalFrame>,
    ) -> Rc<SlotFrame> {
        Rc::new(SlotFrame {
            cart,
            payload: NodePayload { scope, chain },
        })
    }
}

/// Outcome of a node's run. `Replace` is the tail-call path: rewrite the slot's work and
/// re-enqueue the same index so it runs again with no fresh slot allocated, giving constant
/// memory across tail-call sequences. When `frame` is `Some`, its `scope()` becomes the
/// slot's scope and its `region()` owns per-call allocations; `None` keeps the existing
/// frame and scope. `chain` is the pre-decided lexical-chain reshape (see [`ChainOp`]). The
/// slot's declared-return obligation rides the replacement's continuation as a capture (wrapped
/// at the construction site), not a slot field.
///
/// The value terminal rides the step brand `'step` as a [`StepCarried`], confined to the step tail's
/// rank-2 open (`run_loop.rs`) until it exits through
/// [`StepCarried::seal_at_step`] into finalize; the other arms carry no value (an error, a producer
/// [`NodeId`], or a tail-replace payload), so `'step` names only the `DoneWitnessed` carrier's brand.
// `Replace` is intrinsically the large variant (`NodeWork` plus the frame/chain tail-call
// payload); boxing the hot tail-call path to balance the variants is the wrong trade.
#[allow(clippy::large_enum_variant)]
pub(super) enum NodeStep<'step> {
    /// The finalized value terminal — a step-branded [`StepCarried`] wrapping the carrier that names
    /// every region it reaches, sealed through
    /// [`finalize_terminal`](super::finalize::NodeFinalize::finalize_terminal), which folds the
    /// producing frame into the witness at close. The **sole** value terminal — object and type
    /// both — so no terminal recomputes a witness beside its value.
    DoneWitnessed(StepCarried<'step>),
    /// The finalized **error** terminal. An error carries no value, so it needs no witness and
    /// finalizes bare, labelled with the frame-gated contract's trace frame.
    Error(KError),
    /// A ready bare-name forward: this slot's terminal *is* `producer`'s. `run_step` relocates
    /// `producer`'s terminal into this slot's region (carrying its own witness) and finalizes — no
    /// re-check, the producer already enforced its own contract. (`Alias` is the not-yet-ready twin.)
    ForwardReady(NodeId),
    Replace {
        work: NodeWork<KoanWorkload>,
        frame: Option<Rc<CallFrame>>,
        chain: ChainOp,
        /// A block overlay the tail slot runs in, erased to a cart-witnessed carrier (lifetime-free,
        /// so `Replace` stays `'run`-free). `Some` only for a frameless tail entering a
        /// caller-allocated overlay without a per-call frame (USING): the run loop installs it as the
        /// slot's [`NodeScope::YokedChild`]. `None` keeps the slot's existing scope (every framed
        /// tail re-projects `Yoked` from its own cart).
        overlay_scope: Option<SealedExtern<ScopeRefFamily>>,
    },
    /// The slot is spliced out as an alias of `producer` (a bare-name forward whose producer was not
    /// yet ready). The slot's consumers have already been moved onto `producer`'s notify list; this
    /// just marks the slot so `read_result_with` follows through to `producer`. See
    /// [`Outcome::Forward`](super::outcome::Outcome::Forward).
    Alias(NodeId),
}

/// The lexical-chain reshape a [`NodeStep::Replace`] applies, decided at the
/// [`Outcome::Continue`](super::outcome::Outcome::Continue) construction site from the tail's
/// `block_entry` and the contract *variant* (while still live),
/// then assembled in the run loop against the post-step frame. Splitting the decision
/// (contract-reading, at the construction site) from the assembly (frame-reading, in the run loop) is
/// what lets `Replace` shed its `'run`: the variant is read before erasure and frozen into this
/// lifetime-free tag, which then rides [`Outcome::Continue`] to the harness.
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
    /// freshly installed frame for a `FreshChild`/`FreshTail` tail, or the slot's already-installed
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
                body_frame.with_scope(|s| assemble_body_chain(s, prev_chain, body_index))
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
///   from the [`Node::frame`](crate::scheduler::nodes::Node) cart through
///   [`CallFrame::with_scope`](crate::machine::CallFrame). Single-cart: the frame `Rc` already on the
///   slot is the sole liveness witness, so there is no second `Rc` clone aliasing the shell.
/// - `YokedChild` — a [`SealedExtern<ScopeRefFamily>`] carrier (a `&'static Scope`) to a block scope a
///   builtin allocated in a cart *ancestor* region (an `InScope` body — USING / MODULE / SIG / TRY).
///   Opened at read against the slot's frame `Rc` ([`SealedExtern::open`] at a `for<'b>` brand), sound
///   because the cart's `FrameStorage.outer` chain pins that ancestor region for as long as the slot
///   holds the cart. Distinct from `Yoked` only in that the child differs from the cart's own scope,
///   so it needs a stored carrier.
///
/// Storing an erased, frame-witnessed carrier keeps the borrow honest across a tail-call cart swap
/// (nothing persisted points into a stale region; the live frame is re-read each step) and keeps the
/// slot from naming `'run` in its node-stored scope state.
///
/// `Copy` because both arms are trivially copyable ([`SealedExtern<ScopeRefFamily>`] is `Copy` — a
/// thin `&Scope` — or a unit) and submission threads the handle through `pre_subs` recursion without
/// re-deriving it.
#[derive(Clone, Copy)]
pub(super) enum NodeScope {
    YokedChild(SealedExtern<ScopeRefFamily>),
    Yoked,
}

/// The opaque per-node workload payload: the Koan name-resolution state the scheduler stores on a
/// slot and threads through a step without owning — the slot's [`NodeScope`] handle and its
/// lexical [`chain`](Self::chain). The concrete Koan stand-in for the scheduler's generic
/// `KoanWorkload::Payload`. Lifetime-free (erased `NodeScope`, `Rc` chain), so the node it sits
/// on pins no `'run` through it.
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
    //! erase→open shape under tree borrows; logical assertions are minimal — these fail when
    //! Miri reports UB, not on values.

    use super::*;
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    use crate::machine::model::KObject;
    use crate::machine::BindingIndex;

    /// A `NodeScope::YokedChild` erases a cart-ancestor block scope to a
    /// [`SealedExtern<ScopeRefFamily>`] carrier (`erase`) and opens it ([`SealedExtern::open`] at a
    /// `for<'b>` brand) at read — the fabrication the scheduler performs each step for a `YokedChild`
    /// slot, witnessed by the slot's frame. Mirrors the erase→open pair plus a region mutation through
    /// a sibling pointer while the opened scope is live; fails on UB, not values.
    #[test]
    fn node_scope_yoked_child_erase_open_roundtrip() {
        let region = run_root_storage();
        let test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        let types = test_run.types.clone();
        scope
            .bind_checked(
                "k".to_string(),
                KObject::Number(7.0),
                BindingIndex::BUILTIN,
                &types,
            )
            .unwrap();

        let ns = NodeScope::YokedChild(SealedExtern::<ScopeRefFamily>::erase(scope));
        let NodeScope::YokedChild(carrier) = &ns else {
            unreachable!("constructed YokedChild")
        };
        // Open at a `for<'b>` brand witnessed by the region; read a binding back, then mutate the
        // region through a sibling pointer while the opened scope is still live.
        carrier.open(region.region(), |reattached| {
            assert!(matches!(reattached.lookup("k"), Some(KObject::Number(n)) if *n == 7.0));
            let _other = region.brand().alloc_object(KObject::Number(8.0));
            assert!(reattached.lookup("k").is_some());
        });
    }
}
