use std::rc::Rc;

use crate::machine::core::kfunction::body::{ErasedContract, ReturnContract};
use crate::machine::core::ScopeId;
use crate::machine::model::Carried;
use crate::machine::{CallArena, KError, LexicalFrame, NodeId, Scope};

use super::outcome::dep_error_frame;
use super::{short_circuit, DepFinish, NodeCont};

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
    Done(Result<Carried<'run>, KError>),
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
    /// The slot is spliced out as an alias of `producer` (a bare-name forward whose producer was not
    /// yet ready). The slot's consumers have already been moved onto `producer`'s notify list; this
    /// just marks the slot so `read_result` follows through to `producer`. See [`Outcome::Forward`].
    Alias(NodeId),
}

/// What a scheduler node will run: wait on `deps`, then run `cont` over their resolved terminals
/// (passed as `Result`s — the continuation, not the handler, decides short-circuit vs recover).
/// `deps` layout is `[park_producers..., owned_subs...]`; `park_count` is the park-producer prefix
/// (`Notify` edges, kept alive), the suffix installs `Owned` (cascade-freed at success). A dispatch
/// decide (birth or resume) waits on no owned deps and ignores the results; a combine reads its dep
/// values; a catch reads its single dep's `Result`. `carrier` is the deadlock-report sample (a
/// decide's expression summary, else `None`). The per-family behavior lives in `cont`, built by the
/// [`short_circuit`](super::outcome::short_circuit) / [`catch_cont`](super::outcome::catch_cont) /
/// [`ignore_results`](super::outcome::ignore_results) combinators, so the node itself never
/// branches and names no AST.
pub(super) struct NodeWork<'run> {
    pub(in crate::machine::execute) deps: Vec<NodeId>,
    pub(in crate::machine::execute) park_count: usize,
    pub(in crate::machine::execute) cont: NodeCont<'run>,
    pub(in crate::machine::execute) carrier: Option<String>,
}

impl<'run> NodeWork<'run> {
    /// A dep-finish node built for direct submission (not via `apply_outcome`): the path shared by
    /// `submit_dep_finish_in_own_scope` and the test fixture. Waits on `deps` (a `park_count`-long
    /// park prefix, owned suffix), short-circuits on the first errored dep under the
    /// [`dep_error_frame`] label, else hands the resolved values to `finish`.
    pub(in crate::machine::execute) fn awaiting(
        deps: Vec<NodeId>,
        park_count: usize,
        finish: DepFinish<'run>,
    ) -> Self {
        NodeWork {
            deps,
            park_count,
            cont: short_circuit(Some(dep_error_frame()), finish),
            carrier: None,
        }
    }
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

/// Owned `NodeId`s a node must read before running: the `deps[park_count..]` suffix. The
/// park-producer prefix is installed separately as `Notify` edges.
pub(super) fn work_deps<'run>(work: &NodeWork<'run>) -> Vec<NodeId> {
    work.deps[work.park_count..].to_vec()
}

/// Park-producer prefix (sibling slots whose values the node reads but does not own). The caller
/// installs each as a `Notify` edge separately from the Owned path.
pub(super) fn work_park_producers<'run, 'b>(work: &'b NodeWork<'run>) -> &'b [NodeId] {
    &work.deps[..work.park_count]
}
