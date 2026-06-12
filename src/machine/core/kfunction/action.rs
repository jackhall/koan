//! The scheduler-aware `Action` currency (WIP, gated behind the `action-harness` feature). The peer of
//! [`super::exec::ExecOutcome`]: where `ExecOutcome` is what `run_user_fn` returns (scheduler-
//! *unaware*), `Action` is what a builtin returns and what the harness interprets (scheduler-*aware*).
//! These are the **types only** — they reference core/model types, never `SchedulerHandle`. The
//! interpreter that drives the scheduler from a `Action` lives one layer up in
//! `machine::execute::harness::interpret` (the peer of `dispatch/exec.rs::invoke`).
//!
//! See `scratch/action-spec.md` and `scratch/action-survey{,-r2,-r3}/` for the survey + audit this shape
//! was distilled from.

use std::rc::Rc;

use super::body::ReturnContract;
use crate::machine::core::{CallArena, LexicalFrame, Scope};
use crate::machine::model::ast::KExpression;
use crate::machine::model::{Carried, KObject};
use crate::machine::{KError, NodeId};

/// A builtin body: `fn(&BodyCtx) -> Action`. Replaces
/// [`BuiltinFn`](super::body::BuiltinFn)'s `fn(&mut SchedulerHandle, ArgumentBundle) -> BodyResult`.
/// The builtin mutates `BodyCtx.scope` directly (binding install is a scope write, not a Action
/// effect) and returns a `Action` describing the scheduler continuation.
pub type ActionFn = for<'a> fn(&BodyCtx<'a, '_>) -> Action<'a>;

/// Read-only-ish context a builtin body receives. `scope` is **interior-mutable**: the builtin
/// binds / registers / allocs on it directly before returning a `Action`. `frame` is a *reference to
/// the cart `Rc`* (so MODULE can `Rc::clone` it into `KType::Module`), `None` for def-time builtins.
/// `chain` is `None` for a top-level binder (`bind_index` → `BindingIndex::BUILTIN`). `args` is the
/// builtin's arguments as a `KObject::Record`; unevaluated args ride as `KObject::KExpression`
/// cells.
pub struct BodyCtx<'a, 'c> {
    pub scope: &'c Scope<'a>,
    pub frame: Option<&'c Rc<CallArena>>,
    pub chain: Option<&'c LexicalFrame>,
    pub args: &'c KObject<'a>,
}

/// Wake-time context a finish receives: the slot's **own** scope (interior-mutable, with `.arena`)
/// re-projected at wake — a deferred binder `register_*`s on it here.
pub struct FinishCtx<'a, 'c> {
    pub scope: &'c Scope<'a>,
}

/// A `Combine` finish: re-entered at wake with the resolved dep values, yielding another `Action` the
/// harness recurses into. No `&mut SchedulerHandle` — exec's continuation pattern.
pub type Cont<'a> = Box<dyn FnOnce(&FinishCtx<'a, '_>, &[Carried<'a>]) -> Action<'a> + 'a>;

/// A `Catch` finish: re-entered with the watched slot's `Result`, yielding a `Action`.
pub type CatchCont<'a> =
    Box<dyn FnOnce(&FinishCtx<'a, '_>, Result<&'a KObject<'a>, KError>) -> Action<'a> + 'a>;

/// What happens next for a slot — the four shapes the builtin survey reduced everything to.
pub enum Action<'a> {
    /// Produce a value / error for this slot (after any direct scope mutation the builtin did).
    Done(Result<Carried<'a>, KError>),
    /// Tail-replace into `tail` (after `leading`), carrying `contract`, in a cart per `frame_placement`.
    Tail {
        leading: Vec<Dep<'a>>,
        tail: KExpression<'a>,
        contract: Option<ReturnContract<'a>>,
        frame_placement: FramePlacement<'a>,
    },
    /// Dispatch `deps`, then `finish` over their resolved values yields the next `Action`.
    Combine { deps: Vec<Dep<'a>>, finish: Cont<'a> },
    /// Watch `watched`, recover via `finish`.
    Catch { watched: Dep<'a>, finish: CatchCont<'a> },
}

/// A Combine/Tail dependency. `Dispatch` → an owned sub-slot the harness dispatches; `Existing` → a
/// producer NodeId the builtin already found in scope (a forward-ref / pending type) kept alive as
/// a park-producer.
pub enum Dep<'a> {
    Dispatch { expr: KExpression<'a>, placement: DepPlacement<'a> },
    Existing(NodeId),
}

/// Where a `Dep::Dispatch` attaches — collapses the `_here` / `_in_frame` / `_with_chain` zoo.
pub enum DepPlacement<'a> {
    /// The slot's own `NodeScope` (`add_dispatch_here`) — binders' type sub-dispatches.
    OwnScope,
    /// The active frame's child (`add_dispatch_in_frame`) — FN-body leading statements.
    ActiveFrame,
    /// A builtin-minted child scope (module/sig/recursive/using/try body), carried by reference. A
    /// multi-statement body here fans out one sub-dispatch per top-level statement (`enter_body_block`).
    InScope(&'a Scope<'a>),
    /// An explicit lexical chain (TRY's per-statement arm chains).
    WithChain(Rc<LexicalFrame>),
}

/// The cart a `Tail` runs in.
pub enum FramePlacement<'a> {
    /// Reuse the slot's ping-pong reserve cart (`acquire_tail_frame(outer)`). The TCO tail-call
    /// frame — FN-body invoke, deferred `PerCall` tails. The only harness-constructed cart.
    ReuseReserve { outer: &'a Scope<'a> },
    /// A **pre-built** fresh cart the builtin minted (`CallArena::new`, never the reserve), handed
    /// to the harness to install. The builtin owns construction because it may seed the cart before
    /// the tail dispatches — MATCH/TRY bind `it` into it via `with_anchored_child`; EVAL builds it
    /// for the UAF guard.
    FreshChild { frame: Rc<CallArena> },
    /// No new frame; continue in the slot's current cart. Frameless tails / `Done`.
    Inherit,
}
