//! Scheduler-facing types a builtin body uses to spawn additional work. Defined in
//! `kfunction` so `BuiltinFn` / `BodyResult` can name them without `kfunction` importing
//! from `execute`; `execute/scheduler.rs` impls `SchedulerHandle`.

use std::rc::Rc;

use crate::machine::model::ast::{ExpressionPart, KExpression};

use crate::machine::core::kerror::KError;
use crate::machine::core::{CallArena, LexicalFrame, Scope, ScopeId};
use crate::machine::model::values::KObject;

use super::body::BodyResult;

/// Stable handle to a node in the scheduler's DAG.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub usize);

impl NodeId {
    pub fn index(self) -> usize {
        self.0
    }
}

pub trait SchedulerHandle<'a> {
    fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId;
    /// Schedule a `Combine` slot: wait on `owned_subs` ++ `park_producers` to terminalize,
    /// then run `finish` over their resolved values. `owned_subs` are sub-Dispatches this
    /// Combine allocated itself (cascade-freed at success); `park_producers` are existing
    /// sibling slots it reads but does NOT own (kept alive past the Combine's success).
    /// `finish` sees results in `[park_producers..., owned_subs...]` order.
    /// Misclassifying an existing sibling slot as `owned_subs` cascade-frees it on success
    /// and leaves later top-level reads pointing at a freed slot.
    fn add_combine(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        scope: &'a Scope<'a>,
        finish: CombineFinish<'a>,
    ) -> NodeId;
    /// Schedule a `Catch` slot: wait on `from` to terminalize, then run `finish` with its
    /// `Result`. Unlike `Combine`, an errored `from` does not short-circuit — the closure
    /// receives `Err(KError)` and can choose to recover or re-raise.
    fn add_catch(&mut self, from: NodeId, scope: &'a Scope<'a>, finish: CatchFinish<'a>) -> NodeId;
    /// Active slot's `Rc<CallArena>`. See
    /// [per-call-arena-protocol.md § Active-frame propagation](../../../../design/per-call-arena-protocol.md#active-frame-propagation)
    /// and [§ Outer-frame chain](../../../../design/per-call-arena-protocol.md#outer-frame-chain-for-builtin-built-frames).
    fn current_frame(&self) -> Option<Rc<CallArena>>;

    /// Run a closure with `active_frame` temporarily set to `frame`, then restore the
    /// previous. Sub-slots added via `add_dispatch` / `add_combine` inside the closure
    /// inherit `frame`, keeping the per-call arena alive for their lifetimes.
    fn with_active_frame(
        &mut self,
        frame: Rc<CallArena>,
        body: &mut dyn FnMut(&mut dyn SchedulerHandle<'a>),
    );

    /// Take the active frame for reuse on a TCO Replace iff it is uniquely owned. See
    /// [per-call-arena-protocol.md § TCO frame reuse](../../../../design/per-call-arena-protocol.md#tco-frame-reuse).
    fn try_take_reusable_frame_for_tail(&mut self) -> Option<Rc<CallArena>>;

    /// Active slot's lexical chain. Mirrors [`Self::current_frame`]. See `LexicalFrame`.
    fn current_lexical_chain(&self) -> Option<Rc<LexicalFrame>>;

    /// Enter a new lexical block: mint a frame `(scope_id, i)` per statement (parent =
    /// current `active_chain`) and dispatch each statement against `scope`.
    fn enter_block(
        &mut self,
        scope_id: ScopeId,
        statements: Vec<KExpression<'a>>,
        scope: &'a Scope<'a>,
    ) -> Vec<NodeId>;

    /// Schedule `expr` against `scope` with `chain` attached explicitly. The ambient
    /// `add_dispatch` reads `active_chain` instead; this is the only way to override that.
    fn add_dispatch_with_chain(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        chain: Rc<LexicalFrame>,
    ) -> NodeId;

    /// Schedule each top-level statement in `body_expr` against `scope`. Routes through
    /// [`Self::enter_block`] with `scope.id` so body statements get fresh `(scope.id, i)`
    /// frames stacked over the call-site chain.
    ///
    /// A body counts as multi-statement only when *every* part is `ExpressionPart::Expression(_)`;
    /// otherwise the whole body is dispatched as a single statement. The all-Expression
    /// rule prevents `LET x = (FN ...)` from being mis-split (its inner `Expression` part
    /// would otherwise look like a second statement).
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
/// static elements are captured in the closure.
pub type CombineFinish<'a> = Box<
    dyn FnOnce(&'a Scope<'a>, &mut dyn SchedulerHandle<'a>, &[&'a KObject<'a>]) -> BodyResult<'a>
        + 'a,
>;

/// Host-side closure for `Catch` slots. Receives the watched slot's terminal as a
/// `Result` so the closure can branch on either outcome.
pub type CatchFinish<'a> = Box<
    dyn FnOnce(
            &'a Scope<'a>,
            &mut dyn SchedulerHandle<'a>,
            Result<&'a KObject<'a>, KError>,
        ) -> BodyResult<'a>
        + 'a,
>;
