//! Scheduler-facing types a builtin body uses to spawn additional work. Defined in
//! `kfunction` so `BodyResult` can name them without `kfunction` importing
//! from `execute`; `execute/scheduler.rs` impls `SchedulerHandle`.

use std::rc::Rc;

use crate::machine::model::ast::{ExpressionPart, KExpression};

use crate::machine::core::kerror::KError;
use crate::machine::core::{assemble_body_chain, CallArena, LexicalFrame, Scope, ScopeId};
use crate::machine::model::values::{Carried, KObject};

use super::body::BodyResult;

/// Stable handle to a node in the scheduler's DAG.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub usize);

impl NodeId {
    pub fn index(self) -> usize {
        self.0
    }
}

pub trait SchedulerHandle<'a, 's> {
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
        body: &mut dyn FnMut(&mut dyn SchedulerHandle<'a, 's>),
    );

    /// Acquire the per-call frame for the body this invoke is entering: reuse the slot's
    /// reserve cart (reset in place) when it is uniquely owned, else allocate a fresh frame
    /// under `outer`. Reuse always draws from the *reserve*, never the live active cart, so an
    /// invoke never empties `active_frame` — the slot's own cart rides through to the post-step.
    /// See [per-call-arena-protocol.md § TCO frame reuse](../../../../design/per-call-arena-protocol.md#tco-frame-reuse).
    fn acquire_tail_frame(&mut self, outer: &Scope<'_>) -> Rc<CallArena>;

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

    /// Dispatch `expr` as a sub-slot of the currently-active per-call frame, storing the
    /// slot's scope as a `Yoked` handle re-projected from the frame cart rather than a
    /// fabricated `&'a`. The caller must be inside [`Self::with_active_frame`]; the scope
    /// is taken from that frame, so none is passed. Used by the MATCH/TRY arm and FN body
    /// seeds to avoid a free `'a`-fabrication at the seed.
    fn add_dispatch_with_chain_in_frame(
        &mut self,
        expr: KExpression<'a>,
        chain: Rc<LexicalFrame>,
    ) -> NodeId;

    /// Ambient-chain sibling of [`Self::add_dispatch_with_chain_in_frame`]: dispatch `expr`
    /// in the active frame inheriting the ambient `active_chain`. Used by the FN deferred
    /// return-type expression sub-Dispatch.
    fn add_dispatch_in_frame(&mut self, expr: KExpression<'a>) -> NodeId;

    /// Framed sibling of [`Self::add_combine`]: schedule a `Combine` whose scope is the
    /// active frame's child, stored `Yoked` (re-projected from the frame cart) rather than a
    /// fabricated `&'a`. Used by the FN deferred return-type Combine.
    fn add_combine_in_frame(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        finish: CombineFinish<'a>,
    ) -> NodeId;

    /// Framed sibling of [`Self::add_catch`]: schedule a `Catch` whose scope is the active
    /// frame's child, stored `Yoked` (re-projected from the frame cart) rather than a fabricated
    /// `&'a`. Used by CATCH / TRY, which watch a sub-slot dispatched against their own (framed)
    /// scope.
    fn add_catch_in_frame(&mut self, from: NodeId, finish: CatchFinish<'a>) -> NodeId;

    /// The executing slot's scope, materialized on demand as a **short** borrow bounded by this
    /// `&self` call — never held across a `&mut self` scheduler call. This is the read-boundary:
    /// an `Anchored` slot hands back its genuinely run-lived `&Scope`, a `Yoked` slot re-projects
    /// from the live frame cart via the bounded brand. A body fetches the scope per use rather than
    /// receiving it as a step-long argument, so no live borrow blocks the in-step TCO frame reset —
    /// which is what lets the read boundary be a bounded brand instead of a fabricated `&'run`.
    fn current_scope(&self) -> &Scope<'a>;

    /// Schedule against the **executing slot's own scope handle** — the honest
    /// re-dispatch-against-my-own-scope path. The sub-slot inherits the running slot's
    /// [`NodeScope`]: a binder's genuinely run-lived decl-scope stays `Anchored(&'a)`, a per-call
    /// frame child stays `Yoked`. The body passes no scope (it holds only a `&'frame` borrow it
    /// cannot widen back to `&'a`). Distinct from `*_in_frame`, which forces the *active frame's*
    /// scope — wrong for a binder whose decl-scope is not that frame's child.
    fn add_dispatch_here(&mut self, expr: KExpression<'a>) -> NodeId;
    /// `Combine` sibling of [`Self::add_dispatch_here`].
    fn add_combine_here(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        finish: CombineFinish<'a>,
    ) -> NodeId;
    /// `Catch` sibling of [`Self::add_dispatch_here`].
    fn add_catch_here(&mut self, from: NodeId, finish: CatchFinish<'a>) -> NodeId;

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

    /// Dispatch a body's non-tail `statements` as sibling sub-slots in `frame`, each positioned at
    /// body-chain index `i + 1` (the params / `it` sit at idx 0) over the frame's body scope, with
    /// the parent chain reconstructed from the call site via [`assemble_body_chain`]. The shared
    /// "execute a block of expressions" primitive: a multi-statement FN body (`KFunction::invoke`),
    /// a deferred return-type dep, and a MATCH/TRY arm body (the action harness) all use it. The
    /// caller tail-replaces into the body's last statement separately. Returns the sub-slots' ids.
    fn dispatch_body_statements(
        &mut self,
        frame: &Rc<CallArena>,
        statements: Vec<KExpression<'a>>,
    ) -> Vec<NodeId> {
        let body_scope = frame.scope_for_bind();
        let body_scope_id = body_scope.id;
        let parent = assemble_body_chain(
            body_scope,
            self.current_lexical_chain()
                .expect("a body block runs inside an active lexical chain"),
            0,
        )
        .parent
        .clone();
        let mut ids = Vec::with_capacity(statements.len());
        for (i, statement) in statements.into_iter().enumerate() {
            let statement_chain = LexicalFrame::push(parent.clone(), body_scope_id, i + 1);
            let mut bid = None;
            self.with_active_frame(frame.clone(), &mut |s| {
                bid = Some(s.add_dispatch_with_chain_in_frame(statement.clone(), statement_chain.clone()));
            });
            ids.push(bid.expect("body dispatch spawns"));
        }
        ids
    }
}

/// Host-side closure for `Combine` slots. Receives the dep terminals in submission order
/// as [`Carried`] (an object or a type flowing in the type channel); static elements are
/// captured in the closure. A value-consuming finish calls `.object()` on each; a
/// type-resolving dep (a VAL type, an FN return type, a field type) arrives as
/// [`Carried::Type`].
pub type CombineFinish<'a> = Box<
    dyn for<'s> FnOnce(&mut dyn SchedulerHandle<'a, 's>, &[Carried<'a>]) -> BodyResult<'a> + 'a,
>;

/// Host-side closure for `Catch` slots. Receives the watched slot's terminal as a
/// `Result` so the closure can branch on either outcome.
pub type CatchFinish<'a> = Box<
    dyn for<'s> FnOnce(
            &mut dyn SchedulerHandle<'a, 's>,
            Result<&'a KObject<'a>, KError>,
        ) -> BodyResult<'a>
        + 'a,
>;
