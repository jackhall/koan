use std::rc::Rc;

use crate::machine::core::kfunction::body::{ErasedContract, ReturnContract};
use crate::machine::core::ScopeId;
use crate::machine::model::ast::KExpression;
use crate::machine::model::{Carried, KObject, KType};
use crate::machine::{CallArena, CatchFinish, CombineFinish, KError, LexicalFrame, NodeId, Scope};

use super::dispatch::DispatchState;

/// Terminal output of a node's run. Once a slot's `results` entry holds either variant,
/// no further write to that slot occurs until it is freed and reused.
pub(super) enum NodeOutput<'a> {
    /// A produced value in the two-arm currency (a runtime [`KObject`] or a raw type).
    /// Use [`NodeOutput::value`] to wrap an object.
    Value(Carried<'a>),
    Err(KError),
}

impl<'a> NodeOutput<'a> {
    /// Wrap a runtime object as the `Object` arm.
    pub(super) fn value(o: &'a KObject<'a>) -> Self {
        NodeOutput::Value(Carried::Object(o))
    }

    /// Wrap a type as the `Type` arm. Pair with `arena.alloc_ktype`.
    pub(super) fn ktype(t: &'a KType<'a>) -> Self {
        NodeOutput::Value(Carried::Type(t))
    }
}

/// Outcome of a node's run. `Replace` is the tail-call path: rewrite the slot's work and
/// re-enqueue the same index so it runs again with no fresh slot allocated, giving constant
/// memory across tail-call sequences. When `frame` is `Some`, its `scope()` becomes the
/// slot's scope and its `arena()` owns per-call allocations; `None` keeps the existing
/// frame and scope. `function`, when set, names the user-fn whose body the replacement is
/// entering — any error landing on this slot gets a `Frame` appended for the trace.
///
/// `block_entry` annotates lexical-block entry. `None` keeps the slot's current
/// `LexicalFrame` chain unchanged. `Some(scope_id)` enters a new lexical block: when
/// `function` is `None` the reinstall site prepends `(scope_id, 0)` to the chain; when
/// `function` is `Some(_)` the chain is rebuilt via `assemble_body_chain` (the FN-body
/// rule that keeps chain depth = lexical nesting depth, NOT call depth).
// `Replace` is intrinsically the large variant (it carries `NodeWork` plus the
// frame/function/chain tail-call payload); `Done` only grows with the cached
// `KExpression` it indirectly holds. Boxing a short-lived return value's hot tail-call
// path to balance the variants is the wrong trade — the imbalance is inherent.
#[allow(clippy::large_enum_variant)]
pub(super) enum NodeStep<'a> {
    Done(NodeOutput<'a>),
    Replace {
        work: NodeWork<'a>,
        frame: Option<Rc<CallArena>>,
        function: Option<ReturnContract<'a>>,
        block_entry: Option<ScopeId>,
        /// Body-scope chain index for FN-body / MATCH-arm / TRY-arm tail-replace
        /// (mirrors [`crate::machine::core::kfunction::body::BodyResult::Tail::body_index`]).
        /// Positions the freshly-pushed block frame at index `N` for multi-statement
        /// tail-into-last; `0` is the single-statement case.
        body_index: usize,
    },
}

/// What a scheduler node will run. `Lift` exists because the push/notify model
/// assumes a single producer slot per result — see [design/execution-model.md §
/// Lift: push/notify single-producer model](../../../design/execution-model.md#lift-pushnotify-single-producer-model).
/// `Combine` is the dual of `Bind`: a host-side N→1 combinator that waits on a
/// fixed set of dep slots and runs a host closure over their resolved values.
pub(super) enum NodeWork<'a> {
    /// Resolve and schedule a single expression. `state` carries the
    /// dispatch slot's per-variant cached state, with `Initialized` as the
    /// universal birth state and one variant per `DispatchShape` for the
    /// stateful driver to transition into on first classification.
    /// `state.init.pre_subs` carries any recursively pre-submitted sub-
    /// Dispatches keyed by their slot index in `expr.parts`, populated at
    /// submit time for binder-shaped expressions so a nested binder's
    /// placeholders install at the outermost submission point; empty
    /// otherwise. Phase 4 of `run_dispatch` reuses these instead of
    /// allocating fresh sub-Dispatches for the named slots.
    Dispatch {
        expr: KExpression<'a>,
        state: DispatchState<'a>,
    },
    /// `deps` layout is `[park_producers..., owned_subs...]`. `park_count` is the
    /// size of the park-producer prefix — those slots are sibling producers this
    /// Combine merely reads at finish-time and does NOT own. Only the
    /// `deps[park_count..]` suffix gets installed as `DepEdge::Owned` and
    /// cascade-freed at success; the prefix installs as `Notify` (park) edges.
    Combine {
        deps: Vec<NodeId>,
        park_count: usize,
        finish: CombineFinish<'a>,
    },
    /// Catching dual of a single-dep `Combine`: waits on `from` and hands its terminal
    /// (Value or Err) to `finish`. `Combine` short-circuits on dep-error before its
    /// finish runs; `Catch`'s finish always runs so the closure can decide to recover.
    Catch {
        from: NodeId,
        finish: CatchFinish<'a>,
    },
    Lift(LiftState<'a>),
}

impl<'a> NodeWork<'a> {
    /// `Dispatch` in the `Initialized` birth state with empty `pre_subs`.
    /// Sites that need to carry pre-submitted sub-Dispatches across a
    /// re-Dispatch go through [`DispatchState::initialized`] directly.
    pub(super) fn dispatch(expr: KExpression<'a>) -> Self {
        NodeWork::Dispatch {
            expr,
            state: DispatchState::initialized(Vec::new()),
        }
    }
}

/// `Pending(from)` parks on `from`'s terminal; `Ready(output)` holds the stamped
/// producer terminal. The `Pending → Ready` transition is the sole responsibility
/// of `Scheduler::finalize`; a queued Lift in `Pending` indicates a wake misfire.
pub(super) enum LiftState<'a> {
    Pending(NodeId),
    Ready(NodeOutput<'a>),
}

/// Slot-stored scope handle. `Anchored` holds a run-lifetime borrow directly — a genuinely
/// run-lived scope (a fresh `&'a` child a binder body allocated in a real arena); NOT the
/// builtins-only [`ScopeKind::Root`](crate::machine::core::ScopeKind). A per-call frame scope
/// instead stores `Yoked` — no borrow at all — and is re-projected from the slot's own
/// [`Node::frame`] cart at read time (single-cart: the frame `Rc` already on the slot is the
/// sole liveness witness, so there is no second `Rc` clone and no contention with
/// `try_reset_for_tail`'s uniqueness check). Storing the marker rather than a fabricated `&'a`
/// is what keeps the borrow honest across a TCO `try_reset_for_tail`: nothing persisted points
/// into the reset arena; the live frame is re-read each step.
///
/// `Copy` because both arms are trivially copyable (a shared ref / a unit) and submission
/// threads the handle through `pre_subs` recursion without re-deriving it.
#[derive(Clone, Copy)]
pub(super) enum NodeScope<'a> {
    Anchored(&'a Scope<'a>),
    Yoked,
}

/// A node's per-call frame state: the execution cart, its ping-pong reserve, and the erased
/// return contract. Lifetime-free — the cart `Rc` pins everything its members point at, and the
/// contract is erased ([`ErasedContract`]) and re-anchored at the Done read boundary witnessed
/// by `cart`. Every node owns a `Frame`: the cart is the arena the slot's step runs against,
/// falling back to the run frame at top level (see `Scheduler::submit_node`), and an invoke
/// reuses the *reserve* rather than the active cart, so the slot's cart is never taken out from
/// under it. `reserve` and `contract` are sparse.
pub(super) struct Frame {
    /// The cart this slot's step runs against. Cloned onto every sub-slot dispatched in the same
    /// body, so it is uniquely owned only at a TCO collapse point (the gate
    /// `CallArena::try_reset_for_tail` checks). The Rc drops on Done or Replace; its arena drops
    /// then only if no escaped closure still holds the captured scope. Lexical scoping
    /// (`KFunction::captured`) makes each per-call child's `outer` the FN's captured scope, so no
    /// frame holds references a successor frame at the same slot needs — TCO drop is immediate
    /// with no `prev` chain.
    pub(super) cart: Rc<CallArena>,
    /// Per-slot reserve cart for the ping-pong rotation that lets stateful eager-subs resumes
    /// reuse a `CallArena` across iterations. See
    /// [per-call-arena-protocol.md § Ping-pong reserve frame](../../../design/per-call-arena-protocol.md#ping-pong-reserve-frame).
    pub(super) reserve: Option<Rc<CallArena>>,
    /// Return contract enforced on Done — an FN/builtin call (`Function`) or a MATCH/TRY arm's
    /// `-> :T` (`Arm`) — erased for lifetime-free storage and re-anchored against `cart` at the
    /// Done boundary, where it enforces the declared return type and supplies the error-frame
    /// label. `None` for slots with no declared-return obligation. Set in lockstep with `cart`.
    ///
    /// TCO limitation: when A tail-calls B, this is rewritten to B's contract, so the runtime
    /// return-type check only fires against the tail-most contract. Sound only when intermediate
    /// frames' types agree — to be enforced statically by the future type-check pass.
    pub(super) contract: Option<ErasedContract>,
}

pub(super) struct Node<'a> {
    pub(super) work: NodeWork<'a>,
    pub(super) scope: NodeScope<'a>,
    /// The slot's per-call frame state (cart + reserve + erased contract) — never absent, see
    /// [`Frame`].
    pub(super) frame: Frame,
    /// Immutable cactus-chain naming this node's lexical position. Head frame is the
    /// innermost enclosing block; tail (`parent: None`) is top-level. See
    /// `core/lexical_frame.rs`.
    pub(super) chain: Rc<LexicalFrame>,
}

/// Owned `NodeId`s a node must read before running, or `None` if it has no
/// owned read-deps. `Dispatch` spawns rather than reads, so returns `None`. For
/// `Combine`, only the `deps[park_count..]` suffix is owned; the park-producer
/// prefix is installed separately as `Notify` edges by `Scheduler::add`.
pub(super) fn work_deps<'a>(work: &NodeWork<'a>) -> Option<Vec<NodeId>> {
    match work {
        // `pre_subs` ride along structurally on the slot's `DispatchState`; they are
        // not read-deps of the Dispatch itself.
        NodeWork::Dispatch { .. } => None,
        NodeWork::Combine {
            deps, park_count, ..
        } => Some(deps[*park_count..].to_vec()),
        NodeWork::Catch { from, .. } => Some(vec![*from]),
        NodeWork::Lift(LiftState::Pending(from)) => Some(vec![*from]),
        NodeWork::Lift(LiftState::Ready(_)) => None,
    }
}

/// Park-producer prefix for a `Combine` (sibling slots whose values it splices
/// but does not own). Empty for every other work shape. The caller installs
/// each entry as a `Notify` edge separately from the Owned-edge install path.
pub(super) fn work_park_producers<'a, 'b>(work: &'b NodeWork<'a>) -> &'b [NodeId] {
    match work {
        NodeWork::Combine {
            deps, park_count, ..
        } => &deps[..*park_count],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::core::{KError, KErrorKind};

    #[test]
    fn work_deps_lift_pending_returns_from_node() {
        let work = NodeWork::Lift(LiftState::Pending(NodeId(7)));
        assert_eq!(work_deps(&work), Some(vec![NodeId(7)]));
    }

    #[test]
    fn work_deps_lift_ready_returns_none() {
        let work = NodeWork::Lift(LiftState::Ready(NodeOutput::Err(KError::new(
            KErrorKind::User("stamped".to_string()),
        ))));
        assert!(work_deps(&work).is_none());
    }
}
