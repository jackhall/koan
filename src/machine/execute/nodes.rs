use std::rc::Rc;

use crate::machine::core::kfunction::body::{ErasedContract, ReturnContract};
use crate::machine::core::ScopeId;
use crate::machine::model::ast::KExpression;
use crate::machine::model::{Carried, KObject, KType};
use crate::machine::{CallArena, KError, LexicalFrame, NodeId, Scope, TraceFrame};

use super::dispatch::{ResumeFn, SchedulerView};
use super::outcome::Outcome;
use super::{CatchFinish, CombineFinish};

/// Terminal output of a node's run. Once a slot's `results` entry holds either variant,
/// no further write to that slot occurs until it is freed and reused.
pub(super) enum NodeOutput<'run> {
    /// A produced value in the two-arm currency (a runtime [`KObject`] or a raw type).
    /// Use [`NodeOutput::value`] to wrap an object.
    Value(Carried<'run>),
    Err(KError),
}

impl<'run> NodeOutput<'run> {
    /// Wrap a runtime object as the `Object` arm.
    pub(super) fn value(o: &'run KObject<'run>) -> Self {
        NodeOutput::Value(Carried::Object(o))
    }

    /// Wrap a type as the `Type` arm. Pair with `arena.alloc_ktype`.
    pub(super) fn ktype(t: &'run KType<'run>) -> Self {
        NodeOutput::Value(Carried::Type(t))
    }
}

/// Outcome of a node's run. `Replace` is the tail-call path: rewrite the slot's work and
/// re-enqueue the same index so it runs again with no fresh slot allocated, giving constant
/// memory across tail-call sequences. When `frame` is `Some`, its `scope()` becomes the
/// slot's scope and its `arena()` owns per-call allocations; `None` keeps the existing
/// frame and scope. `function`, when set, names the user-fn whose body the replacement is
/// entering — any error landing on this slot gets a `TraceFrame` appended for the trace.
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
pub(super) enum NodeStep<'run> {
    Done(NodeOutput<'run>),
    Replace {
        work: NodeWork<'run>,
        frame: Option<Rc<CallArena>>,
        function: Option<ReturnContract<'run>>,
        block_entry: Option<ScopeId>,
        /// Body-scope chain index for FN-body / MATCH-arm / TRY-arm tail-replace
        /// (mirrors [`Outcome::Continue::body_index`]).
        /// Positions the freshly-pushed block frame at index `N` for multi-statement
        /// tail-into-last; `0` is the single-statement case.
        body_index: usize,
    },
}

/// Host-side closure for a [`NodeWork::DispatchCombine`] slot — the dispatch-side dual of
/// [`CombineFinish`]. Receives the dep terminals in submission order, a read-only
/// [`SchedulerView`], and the slot's own index (for an overload re-park keyed on it), and returns
/// an [`Outcome`] — resolve, tail-redispatch, or **park again** (e.g. an overload park on a freshly
/// resolved producer). The dispatcher genuinely reads evolving graph state, hence the extra `idx`
/// and the dispatch-only park-sites; an action [`CombineFinish`] needs neither. The harness applies
/// the returned outcome, so the finish — like every decide — issues no graph write.
pub(super) type DispatchCombineFinish<'run> = Box<
    dyn for<'a> FnOnce(&SchedulerView<'run, 'a>, &[Carried<'run>], usize) -> Outcome<'run> + 'run,
>;

/// What a scheduler node will run. `Lift` exists because the push/notify model
/// assumes a single producer slot per result — see [design/execution-model.md §
/// Lift: push/notify single-producer model](../../../design/execution-model.md#lift-pushnotify-single-producer-model).
/// `Combine` is the dual of `Bind`: a host-side N→1 combinator that waits on a
/// fixed set of dep slots and runs a host closure over their resolved values.
pub(super) enum NodeWork<'run> {
    /// Resolve and schedule a single expression — the birth state of a dispatch slot, classified
    /// by [`run_dispatch`](super::dispatch::run_dispatch) on its first poll. `pre_subs` carries any
    /// recursively pre-submitted sub-Dispatches keyed by their slot index in `expr.parts`,
    /// populated at submit time for binder-shaped expressions so a nested binder's placeholders
    /// install at the outermost submission point; empty otherwise. `run_dispatch` reuses these
    /// instead of allocating fresh sub-Dispatches for the named slots.
    Dispatch {
        expr: KExpression<'run>,
        pre_subs: Vec<(usize, NodeId)>,
    },
    /// A parked dispatch slot woken to re-run its decide. `resume` is the opaque
    /// `SchedulerView -> Outcome` closure the parking decide captured (its family's internals
    /// stay hidden from the router); [`run_dispatch_resume`](super::dispatch::run_dispatch_resume)
    /// clears the slot's stale dep edges and runs it. `carrier` is the parked expression surfaced
    /// for the drain-end deadlock summary, or `None` for a park that carries no renderable
    /// expression (it falls back to a generic tag).
    DispatchResume {
        carrier: Option<KExpression<'run>>,
        resume: ResumeFn<'run>,
    },
    /// `deps` layout is `[park_producers..., owned_subs...]`. `park_count` is the
    /// size of the park-producer prefix — those slots are sibling producers this
    /// Combine merely reads at finish-time and does NOT own. Only the
    /// `deps[park_count..]` suffix gets installed as `DepEdge::Owned` and
    /// cascade-freed at success; the prefix installs as `Notify` (park) edges.
    Combine {
        deps: Vec<NodeId>,
        park_count: usize,
        finish: CombineFinish<'run>,
    },
    /// Catching dual of a single-dep `Combine`: waits on `from` and hands its terminal
    /// (Value or Err) to `finish`. `Combine` short-circuits on dep-error before its
    /// finish runs; `Catch`'s finish always runs so the closure can decide to recover.
    Catch {
        from: NodeId,
        finish: CatchFinish<'run>,
    },
    /// Dispatch-side dual of `Combine`: identical dep-resolution and edge layout
    /// (`[park_producers..., owned_subs...]`, `park_count` the prefix), but the `finish`
    /// returns a [`NodeStep`] so a dispatch continuation can re-park. Drives the eager-subs /
    /// head-deferred / constructor park-sites: the scheduler resolves the deps and hands the
    /// values to a dispatch finish that splices / classifies / invokes, learning nothing about
    /// the dispatch internals (the splice into a `KExpression` lives entirely in the finish).
    ///
    /// `dep_error_frame` is attached to a dep-error short-circuit (before the finish runs) so a
    /// site that wants to surface the consuming call in the trace (e.g. eager-subs' `<bind>`
    /// frame keyed off the call expression) can; `None` propagates frameless.
    DispatchCombine {
        deps: Vec<NodeId>,
        park_count: usize,
        finish: DispatchCombineFinish<'run>,
        dep_error_frame: Option<TraceFrame>,
    },
    Lift(LiftState<'run>),
}

impl<'run> NodeWork<'run> {
    /// `Dispatch` birth state with empty `pre_subs`. Submission rewrites `pre_subs` in place
    /// (`add_with_chain`) for binder-shaped expressions; every other site dispatches with none.
    pub(super) fn dispatch(expr: KExpression<'run>) -> Self {
        NodeWork::Dispatch {
            expr,
            pre_subs: Vec::new(),
        }
    }
}

/// `Pending(from)` parks on `from`'s terminal; `Ready(output)` holds the stamped
/// producer terminal. The `Pending → Ready` transition is the sole responsibility
/// of `Scheduler::finalize`; a queued Lift in `Pending` indicates a wake misfire.
pub(super) enum LiftState<'run> {
    Pending(NodeId),
    Ready(NodeOutput<'run>),
}

/// Slot-stored scope handle. `Anchored` holds a run-lifetime borrow directly — a genuinely
/// run-lived scope (a fresh `&'run` child a binder body allocated in a real arena); NOT the
/// builtins-only [`ScopeKind::Root`](crate::machine::core::ScopeKind). A per-call frame scope
/// instead stores `Yoked` — no borrow at all — and is re-projected from the slot's own
/// [`Node::frame`] cart at read time (single-cart: the frame `Rc` already on the slot is the
/// sole liveness witness, so there is no second `Rc` clone and no contention with
/// `try_reset_for_tail`'s uniqueness check). Storing the marker rather than a fabricated `&'run`
/// is what keeps the borrow honest across a TCO `try_reset_for_tail`: nothing persisted points
/// into the reset arena; the live frame is re-read each step.
///
/// `Copy` because both arms are trivially copyable (a shared ref / a unit) and submission
/// threads the handle through `pre_subs` recursion without re-deriving it.
#[derive(Clone, Copy)]
pub(super) enum NodeScope<'run> {
    Anchored(&'run Scope<'run>),
    Yoked,
}

/// A node's per-call frame state: the execution cart, its ping-pong reserve, and the erased
/// return contract. Lifetime-free — the cart `Rc` pins everything its members point at, and the
/// contract is erased ([`ErasedContract`]) and re-anchored at the Done read boundary witnessed
/// by `cart`. Every node owns a `CallFrame`: the cart is the arena the slot's step runs against,
/// falling back to the run frame at top level (see `Scheduler::submit_node`), and an invoke
/// reuses the *reserve* rather than the active cart, so the slot's cart is never taken out from
/// under it. `reserve` and `contract` are sparse.
pub(super) struct CallFrame {
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
    /// Return contract enforced on Done — an FN/builtin call (`Function`), a deferred FN's resolved
    /// per-call type (`PerCall`), or a MATCH/TRY arm's `-> :T` (`Arm`) — erased for lifetime-free
    /// storage and re-anchored against `cart` at the Done boundary, where it enforces the declared
    /// return type and supplies the error-frame label. `None` for slots with no declared-return
    /// obligation.
    ///
    /// Tail chains keep the **first** contract: once set, a nested tail call does not overwrite it
    /// (`execute.rs` `next_contract`), so the runtime check fires against the *original* caller's
    /// declared return, not the tail-most callee's. (The kept contract's pointees stay live without
    /// pinning the first frame — a `Function`/`PerCall` points at the `'run` callee or its
    /// captured scope, an `Arm` is only the first contract at top level.)
    pub(super) contract: Option<ErasedContract>,
}

pub(super) struct Node<'run> {
    pub(super) work: NodeWork<'run>,
    pub(super) scope: NodeScope<'run>,
    /// The slot's per-call frame state (cart + reserve + erased contract) — never absent, see
    /// [`CallFrame`].
    pub(super) frame: CallFrame,
    /// Immutable cactus-chain naming this node's lexical position. Head frame is the
    /// innermost enclosing block; tail (`parent: None`) is top-level. See
    /// `core/lexical_frame.rs`.
    pub(super) chain: Rc<LexicalFrame>,
}

/// Owned `NodeId`s a node must read before running, or `None` if it has no
/// owned read-deps. `Dispatch` spawns rather than reads, so returns `None`. For
/// `Combine`, only the `deps[park_count..]` suffix is owned; the park-producer
/// prefix is installed separately as `Notify` edges by `Scheduler::add`.
pub(super) fn work_deps<'run>(work: &NodeWork<'run>) -> Option<Vec<NodeId>> {
    match work {
        // `pre_subs` ride along structurally on the slot; they are not read-deps of the
        // Dispatch itself. A `DispatchResume` parks on notify-only edges (installed
        // separately), so it owns no read-deps either.
        NodeWork::Dispatch { .. } | NodeWork::DispatchResume { .. } => None,
        NodeWork::Combine {
            deps, park_count, ..
        } => Some(deps[*park_count..].to_vec()),
        NodeWork::DispatchCombine {
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
pub(super) fn work_park_producers<'run, 'b>(work: &'b NodeWork<'run>) -> &'b [NodeId] {
    match work {
        NodeWork::Combine {
            deps, park_count, ..
        } => &deps[..*park_count],
        NodeWork::DispatchCombine {
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
