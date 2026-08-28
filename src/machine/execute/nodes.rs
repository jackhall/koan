use std::cell::{Cell, RefCell};
use std::rc::Rc;

use smallvec::SmallVec;

use crate::machine::core::ReturnContract;
use crate::machine::core::{ScopeId, ScopeRefFamily, StatementId, assemble_body_chain};
use crate::machine::model::ast::{DispatchShape, KExpression, WorkingExpression};
use crate::machine::{CallFrame, LexicalFrame};
use crate::scheduler::EdgeId;
use crate::source::{FileId, Span};
use crate::witnessed::SealedExtern;

/// The generic per-node work lives in [`crate::scheduler::nodes`]; re-exported here so the Koan
/// execute tree has a single `nodes` surface combining it with the Koan-side [`NodePayload`] /
/// [`NodeScope`] / [`SlotFrame`].
pub(super) use crate::scheduler::nodes::NodeWork;

/// How a slot renders in the drain's deadlock report.
///
/// Nothing is rendered until a deadlock actually fires — [`SlotFrame::sample`] is the only reader —
/// and nothing region-resident is held to render it later, so the label survives a region's death
/// without a pin of its own.
///
/// Minted with the anchor and never carried across one: a tail replace mints a fresh [`SlotFrame`]
/// for the incarnation it installs, which is a different node than the one that retired.
#[derive(Clone, Copy)]
pub(super) enum WorkLabel {
    Source {
        span: Span,
        file: FileId,
    },
    /// A run with no source extent at all. Names the dispatch shape, which is as much as such a
    /// node can say about itself.
    Shape(DispatchShape),
    /// A slot with no expression behind it — a dep-finish, a block fan-out, a test fixture.
    None,
}

impl WorkLabel {
    /// A synthesized run carries the file and extent of the expression it was built out of
    /// ([`WorkingExpression::synthesized`](crate::machine::model::WorkingExpression::synthesized)),
    /// so the shape tag is the floor for a node with no origin at all rather than the normal case.
    /// [`of_ast`](Self::of_ast) is the same read off the raw node, for a label taken before the
    /// working copy exists.
    pub(super) fn of(expr: &WorkingExpression<'_>) -> WorkLabel {
        match (expr.span, expr.file) {
            (Some(span), Some(file)) => WorkLabel::Source { span, file },
            _ => WorkLabel::Shape(expr.shape()),
        }
    }

    /// [`of`](Self::of) against raw AST — a body labelled at the step that declares its tail, when
    /// the freeze into working form happens later (`ActionKind::TailRaw`).
    pub(super) fn of_ast(expr: &KExpression<'_>) -> WorkLabel {
        match (expr.span, expr.file) {
            (Some(span), Some(file)) => WorkLabel::Source { span, file },
            _ => WorkLabel::Shape(expr.shape()),
        }
    }

    fn render(self) -> String {
        match self {
            WorkLabel::Source { span, file } => crate::source::with(file, |f| {
                let (line, col_utf16) = f.resolve(span.start);
                let text = f
                    .text
                    .get(span.start as usize..span.end as usize)
                    .unwrap_or_default()
                    .trim();
                format!("{}:{}:{}: {}", f.path, line, col_utf16, text)
            }),
            WorkLabel::Shape(shape) => format!("<{shape:?}>"),
            WorkLabel::None => "<wait>".to_string(),
        }
    }
}

/// A slot's owned-edge record. The inline width is the most a slot owns without spilling: a binder
/// stamps one name claim plus the `0..=2` bucket claims its plan names, and a bare-name forward
/// adds the one classification edge it installs. So the width is bounded by the binder forms the
/// language has rather than by the program — a slot that re-emits `Forward` across micro-steps can
/// still exceed it and spill to the heap, which is correct, just no longer free.
type OwnedEdges = SmallVec<[EdgeId; 4]>;

/// Koan's `Workload::Frame` — the scheduler-held per-slot memory anchor. Wraps the shared per-call
/// cart with the slot's own [`NodeScope`] handle and lexical chain. The scheduler holds one
/// `Rc<SlotFrame>` per slot and projects the region owner (`FrameStorage`) through
/// [`Anchor::owner`](crate::scheduler::Anchor::owner) where retention and delivery need it.
pub(super) struct SlotFrame {
    pub(super) cart: Rc<CallFrame>,
    pub(super) payload: NodePayload,
    /// The edges **this slot owns** and releases when it terminalizes: the binder's submission
    /// claims ([`Bindings::install_placeholder`](crate::machine::Bindings::install_placeholder) /
    /// `install_pending_overload`) and a bare-name forward's classification edges. Both are named
    /// after allocation, so the list fills in rather than arriving with the anchor, and a tail
    /// replace carries it over: ownership tracks the slot, not the anchor. Empty for most slots.
    owned_edges: RefCell<OwnedEdges>,
    /// Whether this slot's submission **stamped claims** into its scope's claim store. A slot's
    /// lexical chain index is its statement's, which its eagerly-dispatched sub-slots share, so the
    /// index alone does not say whose claims those are; this flag does, and it is what the
    /// retirement hook gates on before indexing the store by that index. Set exactly where the
    /// stamp happens, and never inherited — a claim-owning slot never tail-replaces (see
    /// [`replacing`](Self::replacing)).
    claimed: Cell<bool>,
    /// The identity a binding this slot installs is stamped with
    /// ([`Installer::Statement`](crate::machine::Installer)). Inherited by a tail replace through
    /// [`replacing`](Self::replacing), so one statement keeps one id however many times it steps
    /// and however many anchors it wears.
    statement: StatementId,
    /// Whether this slot **opened the scope of the cart it runs in** — true exactly for the slot an
    /// [`opening`](Self::opening) replace installed a fresh cart for, whose body therefore finalizes
    /// that cart's scope. A `Yoked` sub-expression slot sharing the cart, and a top-level slot
    /// running in the run frame, both carry `false`, so their `Done` closes nothing.
    opened_scope: bool,
    /// Minted here and never inherited — see [`WorkLabel`].
    label: WorkLabel,
}

impl crate::scheduler::Anchor for SlotFrame {
    type Owner = crate::machine::FrameStorage;
    fn owner(&self) -> &Rc<crate::machine::FrameStorage> {
        self.cart.storage()
    }
}

impl SlotFrame {
    /// Mint a slot anchor for a **freshly submitted** statement. The statement id is minted here, so
    /// submitting is the one act that creates a declaration identity. A submission always runs in a
    /// cart some other act established — the ambient one, or the run frame — so the fresh slot
    /// opened no scope.
    pub(super) fn new(
        cart: Rc<CallFrame>,
        scope: NodeScope,
        chain: Rc<LexicalFrame>,
        label: WorkLabel,
    ) -> Rc<SlotFrame> {
        Rc::new(SlotFrame {
            cart,
            payload: NodePayload { scope, chain },
            owned_edges: RefCell::new(OwnedEdges::new()),
            claimed: Cell::new(false),
            statement: StatementId::next(),
            opened_scope: false,
            label,
        })
    }

    /// Mint the anchor a tail replace swaps in for `retiring` **in the cart it already runs in**,
    /// taking over everything that belongs to the **slot** rather than to the anchor wearing it. A
    /// tail hop continues one statement rather than submitting another, so inheriting the id is what
    /// keeps a binding the replaced slot installs from looking like a second declaration of its own
    /// name. Every hand-over lives in this one constructor, so a replace cannot carry one and drop
    /// another.
    pub(super) fn replacing(
        cart: Rc<CallFrame>,
        scope: NodeScope,
        chain: Rc<LexicalFrame>,
        retiring: &SlotFrame,
        label: WorkLabel,
    ) -> Rc<SlotFrame> {
        debug_assert!(
            !retiring.claimed.get(),
            "a claim-owning slot never tail-replaces: block_tail's callers (MATCH / TRY arms, \
             EVAL, USING) and CLOSE OVER are no binder form, so the scope a statement's claims \
             were installed into is the scope it retires against",
        );
        Rc::new(SlotFrame {
            cart,
            payload: NodePayload { scope, chain },
            owned_edges: RefCell::new(retiring.take_owned_edges()),
            claimed: Cell::new(false),
            statement: retiring.statement,
            opened_scope: retiring.opened_scope,
            label,
        })
    }

    /// [`replacing`](Self::replacing)'s twin for a replace that installs a **fresh `cart`**, which
    /// this slot's body is what runs in. Installing the cart and claiming its scope are the same
    /// act, so they are the same constructor — there is no way to swap a fresh cart in and leave
    /// nobody to close it, nor to claim a scope a prior slot opened.
    pub(super) fn opening(
        cart: Rc<CallFrame>,
        scope: NodeScope,
        chain: Rc<LexicalFrame>,
        retiring: &SlotFrame,
        label: WorkLabel,
    ) -> Rc<SlotFrame> {
        debug_assert!(
            !retiring.claimed.get(),
            "a claim-owning slot never tail-replaces: block_tail's callers (MATCH / TRY arms, \
             EVAL, USING) and CLOSE OVER are no binder form, so the scope a statement's claims \
             were installed into is the scope it retires against",
        );
        Rc::new(SlotFrame {
            cart,
            payload: NodePayload { scope, chain },
            owned_edges: RefCell::new(retiring.take_owned_edges()),
            claimed: Cell::new(false),
            statement: retiring.statement,
            opened_scope: true,
            label,
        })
    }

    /// Render this slot for the drain's deadlock report — reached only when the queues drained with
    /// this slot still parked.
    pub(super) fn sample(&self) -> String {
        self.label.render()
    }

    /// Close the scope of the cart this slot runs in, iff this slot opened it: the per-call frame's
    /// body has finished (a `Done` return, or a tail `Continue` retiring this iteration), so the
    /// scope takes no further binds and its reach-set seals.
    pub(super) fn close_opened_scope(&self) {
        if self.opened_scope {
            self.cart.with_scope(|s| s.close());
        }
    }

    pub(super) fn own_edges(&self, edges: impl IntoIterator<Item = EdgeId>) {
        self.owned_edges.borrow_mut().extend(edges);
    }

    /// Record that this slot's statement index addresses claims in the store. Separate from
    /// [`own_edges`](Self::own_edges) because the two are not one act at the stamp: a submission
    /// owns each claim edge as it installs it, so there is no moment holding the whole set, while
    /// the flag is set once for the statement — and a key naming neither a name nor a bucket
    /// still addresses the store.
    pub(super) fn mark_claimed(&self) {
        self.claimed.set(true);
    }

    /// Whether this slot stamped claims — the retirement hook's gate. A slot that stamped none
    /// shares its statement's index with the slot that did, so asking the store on its behalf would
    /// retire another slot's live claims.
    pub(super) fn installed_claims(&self) -> bool {
        self.claimed.get()
    }

    pub(super) fn statement(&self) -> StatementId {
        self.statement
    }

    /// Take the slot's owned edges, leaving it holding none — so the retirement that releases them,
    /// and the tail replace that hands them to a fresh anchor, are both exactly-once by
    /// construction.
    pub(super) fn take_owned_edges(&self) -> OwnedEdges {
        std::mem::take(&mut *self.owned_edges.borrow_mut())
    }
}

/// The lexical-chain reshape the harness's `Continue` apply performs: decided at the
/// [`Outcome::Continue`](super::outcome::Outcome::Continue) construction site while the contract is
/// live, assembled in the apply against the post-step frame, so the anchor's stored chain names no
/// lifetime ([frames.md § Lexical-chain reshape](../../../design/per-call-region/frames.md#lexical-chain-reshape-at-the-replace)).
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
    /// Decide the reshape before the contract is erased onto the replacement payload.
    /// `Function`/`PerCall` (a deferred FN body) both assemble the FN-body chain; any other contract
    /// under a block entry prepends.
    pub(super) fn decide(
        block_entry: Option<ScopeId>,
        contract: Option<&ReturnContract>,
        body_index: usize,
    ) -> Self {
        let Some(scope_id) = block_entry else {
            return ChainOp::Unchanged;
        };
        match contract {
            Some(ReturnContract::Function { .. } | ReturnContract::PerCall { .. }) => {
                ChainOp::AssembleBody { body_index }
            }
            _ => ChainOp::PushBlock {
                scope_id,
                body_index,
            },
        }
    }

    /// `body_frame` is the cart the body runs in — the freshly installed frame for a
    /// `FreshChild`/`FreshTail` tail, or the slot's already-installed current cart for an `Inherit`
    /// FN-body re-entry (the folded `invoke`) — read only by the `AssembleBody` arm.
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

/// Slot-stored scope handle. It names no lifetime, so the node it sits on pins no `'run` through its
/// scope; both arms are cart-witnessed, re-projected from the slot's live frame at read time rather
/// than re-anchored at a free `'run`, which is what keeps the borrow honest across a tail-call cart
/// swap ([scope-handles.md § Slot-table scope handle](../../../design/per-call-region/scope-handles.md#slot-table-scope-handle)).
#[derive(Clone, Copy)]
pub(super) enum NodeScope {
    /// A scope in a region the cart holds a pin claim on, opened at read against the slot's frame
    /// `Rc` at a `for<'b>` brand.
    YokedChild(SealedExtern<ScopeRefFamily>),
    /// The cart's own scope. No pointer at all, so the frame `Rc` already on the slot is the sole
    /// liveness witness.
    Yoked,
}

/// The opaque per-node workload payload — the Koan stand-in for the scheduler's generic
/// `KoanWorkload::Payload`. Lifetime-free (erased [`NodeScope`], `Rc` chain), so the node it sits on
/// pins no `'run` through it.
#[derive(Clone)]
pub(super) struct NodePayload {
    pub(super) scope: NodeScope,
    /// Immutable cactus-chain naming this node's lexical position. Head frame is the innermost
    /// enclosing block; tail (`parent: None`) is top-level. See `core/lexical_frame.rs`.
    pub(super) chain: Rc<LexicalFrame>,
}

#[cfg(test)]
mod tests;
