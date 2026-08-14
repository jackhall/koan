use std::cell::RefCell;
use std::rc::Rc;

use super::runtime::KoanWorkload;
use crate::machine::core::ReturnContract;
use crate::machine::core::{ScopeId, ScopeRefFamily, StatementId, assemble_body_chain};
use crate::machine::{CallFrame, KError, LexicalFrame};
use crate::scheduler::EdgeId;
use crate::witnessed::SealedExtern;

use super::StepCarried;

/// The generic per-node work lives in [`crate::scheduler::nodes`]; re-exported here so the Koan
/// execute tree has a single `nodes` surface combining it with the Koan-side [`NodeStep`] /
/// [`NodePayload`] / [`NodeScope`] / [`SlotFrame`]. `NodeWork` is the **live** construction-site
/// currency every install path takes; `StoredWork` is what the scheduler hands back at run, with the
/// continuation sealed on the owned tier against the slot's anchor.
pub(super) use crate::scheduler::nodes::{NodeWork, StoredWork};

/// Koan's `Workload::Frame` — the scheduler-held per-slot memory anchor. Wraps the shared
/// per-call cart with the slot's own [`NodeScope`] handle and lexical [`chain`]. The scheduler
/// holds one `Rc<SlotFrame>` per slot and projects the region owner (`FrameStorage`) through
/// [`Anchor::owner`] where retention and delivery need it.
pub(super) struct SlotFrame {
    pub(super) cart: Rc<CallFrame>,
    pub(super) payload: NodePayload,
    /// The edges **this slot owns** and releases when it terminalizes: the binder claims its
    /// submission stamped onto its scope
    /// ([`Scope::install_placeholder`](crate::machine::Scope::install_placeholder) /
    /// `install_pending_overload`), and the classification edges a bare-name forward wires to read
    /// its producer through. Both are named after allocation — minting an edge needs the id the
    /// allocation hands back — so the list fills in rather than arriving with the anchor, and a tail
    /// replace that mints a fresh anchor carries it over: ownership tracks the slot, not the anchor.
    /// Empty for most slots.
    owned_edges: RefCell<Vec<EdgeId>>,
    /// The statement this slot is running — the identity a binding it installs is stamped with
    /// ([`Installer::Statement`](crate::machine::Installer)). Fixed at construction and inherited
    /// by a tail replace through [`replacing`](Self::replacing), so one statement keeps one id
    /// however many times it steps and however many anchors it wears.
    statement: StatementId,
    /// Whether this slot **opened the scope of the cart it runs in** — true exactly for the slot a
    /// [`opening`](Self::opening) replace installed a fresh cart for, whose body therefore finalizes
    /// that cart's scope. A `Yoked` sub-expression slot sharing the cart, and a top-level slot
    /// running in the run frame, both carry `false`, so their `Done` closes nothing. A bit rather
    /// than a slot name: the question is only ever asked of the slot that holds the anchor, so
    /// naming the owner would be answering "is this me?" with an identity comparison the anchor
    /// already answers by construction.
    opened_scope: bool,
}

impl crate::scheduler::Anchor for SlotFrame {
    type Owner = crate::machine::FrameStorage;
    fn owner(&self) -> &Rc<crate::machine::FrameStorage> {
        self.cart.storage()
    }
}

impl SlotFrame {
    /// Mint a slot anchor for a **freshly submitted** statement, from the cart plus the slot's
    /// scope handle and chain. The statement id is minted here, so submitting is the one act that
    /// creates a declaration identity. A submission always runs in a cart some other act
    /// established — the ambient one, or the run frame — so the fresh slot opened no scope.
    pub(super) fn new(
        cart: Rc<CallFrame>,
        scope: NodeScope,
        chain: Rc<LexicalFrame>,
    ) -> Rc<SlotFrame> {
        Rc::new(SlotFrame {
            cart,
            payload: NodePayload { scope, chain },
            owned_edges: RefCell::new(Vec::new()),
            statement: StatementId::next(),
            opened_scope: false,
        })
    }

    /// Mint the anchor a tail replace swaps in for `retiring` **in the cart it already runs in**,
    /// taking over everything that belongs to the **slot** rather than to the anchor wearing it: the
    /// owned edges, the statement identity, and whether the slot opened its cart's scope. A tail hop
    /// continues one statement rather than submitting another, so inheriting the id is what keeps a
    /// binding the replaced slot installs from looking like a second declaration of its own name.
    /// Every hand-over lives in this one constructor, so a replace cannot carry one and drop another.
    pub(super) fn replacing(
        cart: Rc<CallFrame>,
        scope: NodeScope,
        chain: Rc<LexicalFrame>,
        retiring: &SlotFrame,
    ) -> Rc<SlotFrame> {
        Rc::new(SlotFrame {
            cart,
            payload: NodePayload { scope, chain },
            owned_edges: RefCell::new(retiring.take_owned_edges()),
            statement: retiring.statement,
            opened_scope: retiring.opened_scope,
        })
    }

    /// [`replacing`](Self::replacing)'s twin for a replace that installs a **fresh `cart`**, which
    /// this slot's body is what runs in: the slot opens that cart's scope here and closes it at its
    /// own finish ([`close_opened_scope`](Self::close_opened_scope)). Installing the cart and
    /// claiming its scope are the same act, so they are the same constructor — there is no way to
    /// swap a fresh cart in and leave nobody to close it, nor to claim a scope a prior slot opened.
    pub(super) fn opening(
        cart: Rc<CallFrame>,
        scope: NodeScope,
        chain: Rc<LexicalFrame>,
        retiring: &SlotFrame,
    ) -> Rc<SlotFrame> {
        Rc::new(SlotFrame {
            cart,
            payload: NodePayload { scope, chain },
            owned_edges: RefCell::new(retiring.take_owned_edges()),
            statement: retiring.statement,
            opened_scope: true,
        })
    }

    /// Close the scope of the cart this slot runs in, iff this slot opened it: the per-call frame's
    /// body has finished (a `Done` return, or a tail `Continue` retiring this iteration), so the
    /// scope takes no further binds and its reach-set seals. A slot that opened no scope — a `Yoked`
    /// sub-expression sharing its parent's cart, a top-level slot in the run frame — finishes
    /// without closing anything.
    pub(super) fn close_opened_scope(&self) {
        if self.opened_scope {
            self.cart.with_scope(|s| s.close());
        }
    }

    /// Take ownership of `edges` on this slot's behalf — the submission's binder claims, or a
    /// forward's classification edge as the harness wires it.
    pub(super) fn own_edges(&self, edges: impl IntoIterator<Item = EdgeId>) {
        self.owned_edges.borrow_mut().extend(edges);
    }

    /// The statement this slot is running.
    pub(super) fn statement(&self) -> StatementId {
        self.statement
    }

    /// Take the slot's owned edges, leaving it holding none — so the retirement that releases them,
    /// and the tail replace that hands them to a fresh anchor, are both exactly-once by
    /// construction.
    pub(super) fn take_owned_edges(&self) -> Vec<EdgeId> {
        std::mem::take(&mut *self.owned_edges.borrow_mut())
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
/// [`StepCarried::seal_at_step`] into finalize; the other arms carry no value (an error, an
/// [`EdgeId`], or a tail-replace payload), so `'step` names only the `DoneWitnessed` carrier's brand.
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
    /// A ready bare-name forward: this slot's terminal *is* the terminal behind `edge`. `run_step`
    /// relocates it into this slot's region (carrying its own witness) and finalizes — no re-check,
    /// the producer already enforced its own contract — then releases `edge`, the probe the harness
    /// classified through. (`Alias` is the not-yet-ready twin.)
    ForwardReady(EdgeId),
    Replace {
        work: NodeWork<'step, KoanWorkload>,
        frame: Option<Rc<CallFrame>>,
        chain: ChainOp,
        /// A block overlay the tail slot runs in, erased to a cart-witnessed carrier (lifetime-free,
        /// so `Replace` stays `'run`-free). `Some` only for a frameless tail entering a
        /// caller-allocated overlay without a per-call frame (USING): the run loop installs it as the
        /// slot's [`NodeScope::YokedChild`]. `None` keeps the slot's existing scope (every framed
        /// tail re-projects `Yoked` from its own cart).
        overlay_scope: Option<SealedExtern<ScopeRefFamily>>,
    },
    /// The slot is spliced out as an alias of the producer behind `edge` (a bare-name forward whose
    /// producer was not yet ready). The splice re-points the slot's parked edges at that producer,
    /// moves them onto its notify list, and reclaims the slot — nothing survives as a residual. See
    /// [`Outcome::Forward`](super::outcome::Outcome::Forward).
    Alias(EdgeId),
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
        contract: Option<&ReturnContract>,
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
/// thin `&Scope` — or a unit), so the handle threads through a dispatch step without re-deriving it.
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
