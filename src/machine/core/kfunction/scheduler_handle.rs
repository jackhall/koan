//! Scheduler-facing types a builtin body uses to spawn additional work: stable `NodeId`
//! handles, the `SchedulerHandle` trait (with the default `plan_body_statements` planner
//! shared by binder builtins), and the `CombineFinish` closure type for `Combine` slots.
//! Defined in `kfunction` so `BuiltinFn` / `BodyResult` can name them without `kfunction`
//! importing from `execute`; `execute/scheduler.rs` impls `SchedulerHandle`.

use std::rc::Rc;

use crate::machine::model::ast::{ExpressionPart, KExpression};

use crate::machine::core::{CallArena, Scope};
use crate::machine::model::values::KObject;

use super::body::BodyResult;

/// Stable handle to a node in the scheduler's DAG. Lives in `kfunction` so `BodyResult` and
/// `SchedulerHandle` can name a node without `kfunction` importing from `execute`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub usize);

impl NodeId {
    pub fn index(self) -> usize { self.0 }
}

/// Side-channel a builtin body uses to spawn additional `Dispatch` nodes. Defined in
/// `kfunction` so `BuiltinFn` can reference it without importing the scheduler module;
/// `Scheduler` impls it in `execute/scheduler.rs`.
///
/// `current_frame` returns the active slot's `Rc<CallArena>` so a builtin building a new
/// per-call frame whose child scope's `outer` points into the call site can chain that Rc
/// onto the new frame. Without this, MATCH-style builtins (whose new frame's outer is a
/// per-call scope, not a captured lexical scope) would hand out a frame whose `outer`
/// dangles the moment the slot's old frame is dropped on TCO replace.
pub trait SchedulerHandle<'a> {
    fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId;
    /// Schedule a `Combine` slot: wait on `deps` to terminalize, then run `finish` over
    /// their resolved values. The dual of `Bind` for host-side N→1 combinators (list /
    /// dict literals today; MODULE / SIG body wrap-up in flight). `deps` order is the
    /// order `finish` sees its `&[&'a KObject<'a>]` slice.
    fn add_combine(
        &mut self,
        deps: Vec<NodeId>,
        scope: &'a Scope<'a>,
        finish: CombineFinish<'a>,
    ) -> NodeId;
    fn current_frame(&self) -> Option<Rc<CallArena>>;

    /// Run a closure with `active_frame` temporarily set to `frame`. Sub-slots
    /// added via `add_dispatch` / `add_combine` inside the closure inherit `frame`
    /// (see the `frame = self.active_frame.clone()` line in `Scheduler::add`),
    /// keeping the per-call arena alive for the lifetimes of those sub-slots.
    ///
    /// Module-system functor-params Stage B: `KFunction::invoke`'s deferred-return
    /// path uses this to spawn the body Dispatch and the return-type elaboration
    /// Dispatch under the new per-call frame, so the frame's `Rc` stays alive
    /// until both sub-slots finalize. The previous `active_frame` (the caller's)
    /// is restored on return.
    fn with_active_frame(
        &mut self,
        frame: Rc<CallArena>,
        body: &mut dyn FnMut(&mut dyn SchedulerHandle<'a>),
    );

    /// Schedule each top-level statement in `body_expr` against `scope` and return their
    /// `NodeId`s. Caller (MODULE / SIG body) wraps these in a `Combine` whose finish
    /// closure builds the binder value once all statements terminalize.
    ///
    /// A body counts as multi-statement only when *every* part is `ExpressionPart::Expression(_)`;
    /// otherwise the whole body is dispatched as a single statement. The stricter all-
    /// Expression rule prevents `LET x = (FN ...)` from being mis-split (its inner
    /// `Expression` part would otherwise look like a second statement).
    fn plan_body_statements(
        &mut self,
        scope: &'a Scope<'a>,
        body_expr: KExpression<'a>,
    ) -> Vec<NodeId> {
        let is_multi_statement = !body_expr.parts.is_empty()
            && body_expr
                .parts
                .iter()
                .all(|p| matches!(p, ExpressionPart::Expression(_)));

        if is_multi_statement {
            body_expr
                .parts
                .into_iter()
                .filter_map(|p| match p {
                    ExpressionPart::Expression(e) => Some(self.add_dispatch(*e, scope)),
                    _ => None,
                })
                .collect()
        } else {
            vec![self.add_dispatch(body_expr, scope)]
        }
    }
}

/// Host-side closure for `Combine` slots. Receives the dep values in submission order;
/// static elements (e.g. literal scalars in a list literal) are captured in the closure.
/// Returning a `BodyResult` lets the closure surface a structured error (e.g. dict-literal
/// key conversion) without a special-case channel.
pub type CombineFinish<'a> = Box<
    dyn FnOnce(&'a Scope<'a>, &mut dyn SchedulerHandle<'a>, &[&'a KObject<'a>]) -> BodyResult<'a>
        + 'a,
>;
