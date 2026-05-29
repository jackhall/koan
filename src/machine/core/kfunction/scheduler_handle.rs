//! Scheduler-facing types a builtin body uses to spawn additional work: stable `NodeId`
//! handles, the `SchedulerHandle` trait (with the default `enter_body_block` planner
//! shared by binder builtins), and the `CombineFinish` closure type for `Combine` slots.
//! Defined in `kfunction` so `BuiltinFn` / `BodyResult` can name them without `kfunction`
//! importing from `execute`; `execute/scheduler.rs` impls `SchedulerHandle`.

use std::rc::Rc;

use crate::machine::model::ast::{ExpressionPart, KExpression};

use crate::machine::core::{CallArena, LexicalFrame, Scope, ScopeId};
use crate::machine::core::kerror::KError;
use crate::machine::model::values::KObject;

use super::body::BodyResult;

/// Stable handle to a node in the scheduler's DAG.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub usize);

impl NodeId {
    pub fn index(self) -> usize { self.0 }
}

/// Side-channel a builtin body uses to spawn additional `Dispatch` nodes.
pub trait SchedulerHandle<'a> {
    fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId;
    /// Schedule a `Combine` slot: wait on `owned_subs` ++ `park_producers` to
    /// terminalize, then run `finish` over their resolved values. `owned_subs`
    /// are sub-Dispatches this Combine allocated itself (cascade-freed at
    /// success); `park_producers` are existing sibling slots whose values it
    /// reads but does NOT own (kept alive past the Combine's success). The
    /// `finish` closure sees results in `[park_producers..., owned_subs...]`
    /// order — the `Notify` (park) prefix first, the `Owned` (sub) suffix
    /// after. Misclassifying an existing sibling slot as `owned_subs`
    /// cascade-frees it on success and leaves later top-level reads pointing
    /// at a freed slot.
    fn add_combine(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        scope: &'a Scope<'a>,
        finish: CombineFinish<'a>,
    ) -> NodeId;
    /// Schedule a `Catch` slot: wait on `from` to terminalize, then run `finish` with its
    /// `Result`. Unlike `Combine`, an errored `from` does not short-circuit — the closure
    /// receives `Err(KError)` and can choose to recover (build a `Tagged` carrier via
    /// `KError::to_tagged` for TRY's branch dispatcher) or re-raise. The primitive backs
    /// the `TRY-WITH` builtin; no other caller today.
    fn add_catch(
        &mut self,
        from: NodeId,
        scope: &'a Scope<'a>,
        finish: CatchFinish<'a>,
    ) -> NodeId;
    /// Active slot's `Rc<CallArena>`, so a builtin building a new per-call frame whose
    /// child scope's `outer` points into the call site can chain that Rc onto the new
    /// frame. Without this, builtins whose new frame's outer is a per-call scope (rather
    /// than a captured lexical scope) would hand out a frame whose `outer` dangles the
    /// moment the slot's old frame is dropped on TCO replace.
    fn current_frame(&self) -> Option<Rc<CallArena>>;

    /// Run a closure with `active_frame` temporarily set to `frame`, then restore the
    /// previous `active_frame`. Sub-slots added via `add_dispatch` / `add_combine`
    /// inside the closure inherit `frame`, keeping the per-call arena alive for the
    /// lifetimes of those sub-slots.
    fn with_active_frame(
        &mut self,
        frame: Rc<CallArena>,
        body: &mut dyn FnMut(&mut dyn SchedulerHandle<'a>),
    );

    /// Take the active frame for reuse on a TCO Replace iff it is uniquely owned —
    /// i.e. no closure or sub-slot has cloned the `Rc` out. On `Some`, the caller
    /// becomes the sole owner; calling [`CallArena::try_reset_for_tail`] on it is
    /// guaranteed to succeed. On `None`, the active frame is left in place and the
    /// caller must allocate a fresh frame.
    ///
    /// The "uniquely owned" gate is what keeps reuse semantically equivalent to
    /// drop-and-alloc: any escaped `Rc` (returned closure, list element carrying a
    /// `KFunction(_, Some(rc))`, ...) keeps strong_count > 1 and refuses reuse.
    fn try_take_reusable_frame_for_tail(&mut self) -> Option<Rc<CallArena>>;

    /// Active slot's lexical chain. Mirrors [`Self::current_frame`]. Builtins that
    /// assemble a new chain (the FN-body invoke path) read this to find the call-
    /// site chain; nothing else reads it today. See `LexicalFrame`.
    fn current_lexical_chain(&self) -> Option<Rc<LexicalFrame>>;

    /// Enter a new lexical block. Mints a frame `(scope_id, i)` per statement (parent
    /// = current `active_chain`) and dispatches each statement against `scope`,
    /// returning their `NodeId`s. The single primitive every block-entry site funnels
    /// through: top-level (`interpret.rs`), MODULE / SIG body
    /// (`enter_body_block`), and TRY body's success-as-block.
    fn enter_block(
        &mut self,
        scope_id: ScopeId,
        statements: Vec<KExpression<'a>>,
        scope: &'a Scope<'a>,
    ) -> Vec<NodeId>;

    /// Schedule `expr` against `scope` with `chain` attached explicitly — escape
    /// hatch for callers that have already computed the right chain (FN-body invoke,
    /// `enter_block`'s internals). The ambient `add_dispatch` reads `active_chain`
    /// instead; this method is the only way to override that default.
    fn add_dispatch_with_chain(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        chain: Rc<LexicalFrame>,
    ) -> NodeId;

    /// Schedule each top-level statement in `body_expr` against `scope` and return their
    /// `NodeId`s. Routes through [`Self::enter_block`] with `scope.id`, so the body
    /// statements get fresh `(scope.id, i)` frames stacked over the call-site chain.
    ///
    /// A body counts as multi-statement only when *every* part is `ExpressionPart::Expression(_)`;
    /// otherwise the whole body is dispatched as a single statement. The stricter all-
    /// Expression rule prevents `LET x = (FN ...)` from being mis-split (its inner
    /// `Expression` part would otherwise look like a second statement).
    fn enter_body_block(
        &mut self,
        scope: &'a Scope<'a>,
        body_expr: KExpression<'a>,
    ) -> Vec<NodeId> {
        let is_multi_statement = !body_expr.parts.is_empty()
            && body_expr
                .parts
                .iter()
                .all(|p| matches!(p.value, ExpressionPart::Expression(_)));

        let statements: Vec<KExpression<'a>> = if is_multi_statement {
            body_expr
                .parts
                .into_iter()
                .filter_map(|p| match p.value {
                    ExpressionPart::Expression(e) => Some(*e),
                    _ => None,
                })
                .collect()
        } else {
            vec![body_expr]
        };
        self.enter_block(scope.id, statements, scope)
    }
}

/// Host-side closure for `Combine` slots. Receives the dep values in submission order;
/// static elements are captured in the closure. Returning a `BodyResult` lets the closure
/// surface a structured error without a special-case channel.
pub type CombineFinish<'a> = Box<
    dyn FnOnce(&'a Scope<'a>, &mut dyn SchedulerHandle<'a>, &[&'a KObject<'a>]) -> BodyResult<'a>
        + 'a,
>;

/// Host-side closure for `Catch` slots. Receives the watched slot's terminal as a
/// `Result` — `Ok(&KObject)` on success, `Err(KError)` on failure — so the closure can
/// branch on either outcome (TRY's per-arm dispatch).
pub type CatchFinish<'a> = Box<
    dyn FnOnce(
        &'a Scope<'a>,
        &mut dyn SchedulerHandle<'a>,
        Result<&'a KObject<'a>, KError>,
    ) -> BodyResult<'a>
    + 'a,
>;
