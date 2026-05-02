use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use crate::dispatch::arena::{CallArena, RuntimeArena};
use crate::dispatch::kfunction::{
    ArgumentBundle, Body, BodyResult, KFunction, NodeId, SchedulerHandle,
};
use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::{KFuture, Scope};
use crate::parse::kexpression::{ExpressionPart, KExpression};

/// What a scheduler node will produce when its work runs. `Value` is computed inline; `Forward`
/// says "my result is whatever node `id` produces" — set when a `Dispatch` spawns a `Bind` for
/// its sub-expression deps. `read` follows `Forward` chains until it lands on a `Value`.
/// Cycles are statically prevented because every `NodeId` produced by `add_*` is strictly
/// greater than every `NodeId` it could forward to.
enum NodeOutput<'a> {
    Value(&'a KObject<'a>),
    Forward(NodeId),
}

/// What `run_dispatch`/`run_bind` tells the execute loop to do next. `Done(output)` stores the
/// output at the current node's slot — the normal path. `Replace { work, frame }` is the
/// tail-call path: rewrite the current node's `work` and re-enqueue the same `idx` so it runs
/// again with the new work. When `frame` is `Some`, install it on the slot (its `scope()`
/// becomes the slot's scope, its `arena()` owns the per-call allocations) — used by user-fn
/// invocation. `None` keeps the existing frame and scope. Constant memory across tail-call
/// sequences because no fresh slot is allocated.
enum NodeStep<'a> {
    Done(NodeOutput<'a>),
    Replace { work: NodeWork<'a>, frame: Option<Rc<CallArena>> },
}

/// What a scheduler node will run.
///
/// - `Dispatch(expr)` is the entry point: walk the expression's parts, spawn `Dispatch` nodes
///   for nested `Expression` (and `ListLiteral`) parts, and emit a `Bind` node depending on
///   them. If there's no nesting, dispatch + invoke happen inline and the result is stored
///   directly. Replaces the old "eager dispatch in `schedule_expr`" path.
/// - `Bind { expr, subs }` is the old `Pending`: splice each dep's resolved value into `parts`
///   as `Future(...)`, dispatch the resulting expression, invoke the bound future.
/// - `Aggregate { elements }` materializes a list literal once each `Dep` element resolves.
enum NodeWork<'a> {
    Dispatch(KExpression<'a>),
    Bind {
        expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
    },
    Aggregate {
        elements: Vec<AggregateElement<'a>>,
    },
}

/// One slot in an `Aggregate` node. `Static` is an already-resolved value; `Dep` defers to a
/// previously-scheduled node. The mix lets a list literal like `[1 (LET x = 5) z]` schedule
/// only the sub-expression and inline the other two.
enum AggregateElement<'a> {
    Static(KObject<'a>),
    Dep(NodeId),
}

struct Node<'a> {
    work: NodeWork<'a>,
    /// The scope this node executes against. Top-level nodes carry the run-root scope; nodes
    /// spawned during a body's evaluation inherit their spawning node's scope; a user-fn's
    /// tail-replace installs a per-call child scope here so the body's lookups resolve
    /// parameters by name.
    scope: &'a Scope<'a>,
    /// Per-call frame this slot holds. `Some` for user-fn body slots, `None` for top-level
    /// dispatch and sub-Dispatch/Bind/Aggregate slots. The Rc drops when the slot reaches
    /// Done or is replaced; the underlying arena drops at that point only if no other Rc
    /// (e.g., from a closure that captured this frame's scope and escaped) is held.
    /// Lexical scoping (`KFunction::captured`) means each per-call child's `outer` is the
    /// FN's captured scope (run-root for top-level FNs), so a frame holds no references
    /// that a successor frame at the same slot needs — drop on TCO replace is immediate,
    /// no `prev` chain.
    frame: Option<Rc<CallArena>>,
}

/// A dynamic DAG of dispatch and execution work. The parser submits `Dispatch` nodes for each
/// top-level expression; running a `Dispatch` may add child `Dispatch`/`Bind`/`Aggregate`
/// nodes, and a builtin body that holds `&mut dyn SchedulerHandle` can also add `Dispatch`
/// nodes (used by `if_then` for its lazy `value` and by `KFunction::invoke` for user-defined
/// bodies). The execute loop pops from a FIFO queue; a `Bind` whose subs forward through to a
/// not-yet-run node gets re-queued at the back. Cycles are statically prevented because every
/// new node's `NodeId` is strictly greater than every node it can reach, so the queue is
/// guaranteed to drain.
///
/// Each node carries the scope it should run against (`Node::scope`). Sub-nodes spawned by a
/// running node default to the spawning node's scope; user-fn invocation installs a per-call
/// child scope via `NodeStep::Replace { scope: Some(child) }`.
pub struct Scheduler<'a> {
    nodes: Vec<Option<Node<'a>>>,
    results: Vec<Option<NodeOutput<'a>>>,
    queue: VecDeque<usize>,
    /// Slots that returned `Done(Forward(_))` while owning a per-call frame and are now
    /// waiting for their forward chain to resolve. `finalize_ready_frames` only scans this
    /// vec rather than all `nodes`, keeping the per-iteration cost proportional to the
    /// number of in-flight user-fn calls (typically tiny) instead of total scheduler size.
    frame_holding_slots: Vec<usize>,
}

impl<'a> Scheduler<'a> {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            results: Vec::new(),
            queue: VecDeque::new(),
            frame_holding_slots: Vec::new(),
        }
    }

    pub fn len(&self) -> usize { self.nodes.len() }
    pub fn is_empty(&self) -> bool { self.nodes.is_empty() }

    /// Submit an unresolved expression for the scheduler to dispatch + execute against
    /// `scope`. Returns the `NodeId` whose result the eventual dispatch will produce. The
    /// only public way to add work; everything else (`Bind`, `Aggregate`) is internal
    /// scaffolding spawned during a `Dispatch` node's run.
    pub fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        self.add(NodeWork::Dispatch(expr), scope)
    }

    fn add(&mut self, work: NodeWork<'a>, scope: &'a Scope<'a>) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Some(Node { work, scope, frame: None }));
        self.results.push(None);
        self.queue.push_back(id.index());
        id
    }

    /// Drain the FIFO queue. A `Bind`/`Aggregate` whose subs forward through to a not-yet-
    /// resolved node gets re-queued at the back. A node whose work returns
    /// `NodeStep::Replace` (the tail-call path) gets its work rewritten and re-enqueued at
    /// the *front* so the same slot runs again with the new work — no new allocation.
    /// `Replace { frame: Some(f) }` also installs `f` on the slot, dropping the slot's
    /// previous frame; the new frame's `scope()` becomes the slot's scope and its `arena()`
    /// owns the per-call allocations.
    ///
    /// On `Done`: if the slot owned a frame, the body's return `Value` references memory
    /// inside the per-call arena that's about to drop. Lift the value into the captured
    /// scope's arena (= the per-call scope's `outer.arena`, which by lexical scoping is the
    /// FN's definition arena and outlives the call) by deep-cloning it. Forwards need no
    /// lift — the forwarded slot does its own lift on its own Done.
    ///
    /// Use `read(id)` to retrieve a top-level dispatch's result after `execute` returns.
    /// Internal Bind/Aggregate/sub-Dispatch slots' results may point into per-call arenas
    /// that finalization has freed; the public API keeps those internals out of reach.
    pub fn execute(&mut self) -> Result<(), String> {
        while let Some(idx) = self.queue.pop_front() {
            let node = self.nodes[idx]
                .take()
                .expect("scheduler must not revisit a completed node");
            let scope = node.scope;
            let work = node.work;
            let prev_frame = node.frame;
            // Bind/Aggregate may need their dep results to be fully resolved (forward chain
            // ending in a `Value`). If any forward-chains to an unresolved slot, requeue.
            if let Some(deps) = work_deps(&work) {
                if !deps.iter().all(|d| self.is_result_ready(*d)) {
                    self.nodes[idx] = Some(Node { work, scope, frame: prev_frame });
                    self.queue.push_back(idx);
                    continue;
                }
            }
            let step = match work {
                NodeWork::Dispatch(expr) => self.run_dispatch(expr, scope)?,
                NodeWork::Bind { expr, subs } => self.run_bind(expr, subs, scope)?,
                NodeWork::Aggregate { elements } => NodeStep::Done(self.run_aggregate(elements, scope)),
            };
            match step {
                NodeStep::Done(output) => {
                    match (output, prev_frame) {
                        (NodeOutput::Value(v), Some(frame)) => {
                            // Body produced a Value directly — lift into the captured
                            // arena. By lexical scoping, the per-call scope's `outer` IS
                            // the captured scope (run-root for top-level FNs), whose arena
                            // outlives the call. If the lifted value carries a KFunction
                            // reference into the dying frame's arena, `lift_kobject`
                            // attaches an Rc clone so the arena stays alive past the
                            // slot's frame drop.
                            let dest = scope
                                .outer
                                .expect("per-call scope must have an outer (its captured scope)")
                                .arena;
                            let lifted_obj = Self::lift_kobject(v, &frame);
                            let lifted = dest.alloc_object(lifted_obj);
                            self.results[idx] = Some(NodeOutput::Value(lifted));
                            // `frame` drops here. If the lifted value cloned an Rc, the
                            // arena lives on; otherwise this is the last reference and
                            // the per-call arena frees.
                        }
                        (NodeOutput::Forward(target), Some(frame)) => {
                            // Body forwarded into sub-slots whose scopes live in the
                            // per-call arena. Keep the frame alive on this slot until the
                            // forward chain resolves; a finalize pass below promotes the
                            // chain's terminal Value into the captured arena and drops
                            // the frame. The slot is no longer in the queue, so its
                            // `work` is unused — store a stub. Track the slot so
                            // `finalize_ready_frames` can find it without scanning all
                            // nodes.
                            self.results[idx] = Some(NodeOutput::Forward(target));
                            self.nodes[idx] = Some(Node {
                                work: NodeWork::Dispatch(KExpression { parts: Vec::new() }),
                                scope,
                                frame: Some(frame),
                            });
                            self.frame_holding_slots.push(idx);
                        }
                        (other, None) => {
                            self.results[idx] = Some(other);
                        }
                    }
                }
                NodeStep::Replace { work: new_work, frame: new_frame } => {
                    // TCO: drop the slot's previous frame immediately. Lexical scoping
                    // means the new frame's child scope's `outer` is the captured scope,
                    // not the previous frame's, so this is safe.
                    drop(prev_frame);
                    let (next_scope, next_frame) = match new_frame {
                        Some(f) => {
                            // SAFETY: `f.scope()` borrows from `f`, but `f` is owned by the
                            // slot once installed. The `&'a` we hand to the next iteration
                            // is anchored to `self.nodes[idx]`'s storage, which lives until
                            // the slot drops or its frame is replaced again.
                            let s: &'a Scope<'a> = unsafe {
                                std::mem::transmute::<&Scope<'_>, &'a Scope<'a>>(f.scope())
                            };
                            (s, Some(f))
                        }
                        None => (scope, None),
                    };
                    self.nodes[idx] = Some(Node { work: new_work, scope: next_scope, frame: next_frame });
                    self.queue.push_front(idx);
                }
            }
            // Drain `scope`'s pending writes — `Scope::add` queues writes that hit a borrow
            // conflict (a builtin iterating `data`/`functions` while a re-entrant write tries
            // to mutate). Drain runs here, between dispatch nodes, so the next node's reads
            // see them. The hot path is the no-op early-return inside `drain_pending` (queue
            // is empty in the typical case); only the rare re-entrant-write path does work.
            scope.drain_pending();
            // Finalize any frame-holding slots whose forward chain has now resolved:
            // lift the terminal Value into the captured arena, store as Value at the
            // slot, and drop the frame. Most iterations have nothing to do here; only
            // user-fn slots that returned `Done(Forward)` carry frames awaiting resolution.
            self.finalize_ready_frames();
        }
        Ok(())
    }

    /// Lift a KObject value out of the dying frame's arena into the destination arena.
    /// Owned variants (Number, KString, Bool, Null) `deep_clone` cleanly because their
    /// content is owned. `KObject::KFunction(&f, frame)` is the special case: `&f` may
    /// point into the dying frame's arena (an escaping closure). If so, we carry a clone
    /// of the dying frame's `Rc<CallArena>` in the lifted value's frame field, so the
    /// arena stays alive past the slot's frame drop and the `&f` reference remains valid.
    /// If the function lives in a longer-lived arena (run-root or another live frame), no
    /// Rc is needed and the lifted value's frame field stays `None`.
    ///
    /// `KObject::KFuture` is handled conservatively: any unanchored KFuture lifted from
    /// the dying frame gets the dying-frame Rc attached, regardless of where its `function`
    /// was defined. The KFuture's `bundle.args` and `parsed.parts`' `Future(&KObject)` refs
    /// can independently point into the dying arena, and we have no per-descendant arena
    /// tracking to tell us whether they do — anchoring unconditionally is safe and the
    /// over-keep is theoretical until KFutures escape as values (they currently don't;
    /// kept for the planned async features).
    ///
    /// Pre-existing `Some(rc)` on the input value is preserved (the value is already
    /// keeping some arena alive; we don't overwrite that with the current dying frame's).
    ///
    /// Composite variants (`List`, `Dict`) recurse to find embedded closures that need an
    /// Rc attach, but memoize via `needs_lift`: when no descendant needs lifting, the
    /// payload's existing `Rc` is cloned instead of rebuilding the `Vec`/`HashMap`. This
    /// makes a value's second-and-later lifts through a return chain O(N) walk + O(1)
    /// rebuild for the unchanged composites — Koan's collection-immutability contract is
    /// what makes the structural sharing safe.
    ///
    /// Whole-tree fast path: if the dying arena has zero `KFunction`s allocated in it, no
    /// descendant `&KFunction` can point into it (per `alloc_function`'s invariant). This
    /// is sound *today* because KFutures don't escape as values — the only way a lifted
    /// `v` could need anchoring under this condition is via a KFuture descendant, and
    /// none exist in current usage. When KFutures begin escaping (planned async), this
    /// gate must add a no-unanchored-KFuture-descendant clause; the slow path's KFuture
    /// arm is already correct. The check is one O(1) emptiness query on the arena.
    fn lift_kobject<'b>(v: &KObject<'b>, dying_frame: &Rc<CallArena>) -> KObject<'b> {
        if dying_frame.arena().functions_is_empty() {
            return v.deep_clone();
        }
        match v {
            KObject::KFunction(f, existing) => {
                let new_frame = if existing.is_some() {
                    existing.clone()
                } else {
                    let dying_runtime: *const RuntimeArena = dying_frame.arena();
                    let captured_runtime: *const RuntimeArena = f.captured_scope().arena;
                    if std::ptr::eq(captured_runtime, dying_runtime) {
                        Some(Rc::clone(dying_frame))
                    } else {
                        None
                    }
                };
                KObject::KFunction(*f, new_frame)
            }
            KObject::KFuture(t, existing) => {
                let new_frame = existing.clone().or_else(|| Some(Rc::clone(dying_frame)));
                KObject::KFuture(t.deep_clone(), new_frame)
            }
            KObject::List(items) => {
                if items.iter().any(|x| Self::needs_lift(x, dying_frame)) {
                    let lifted: Vec<KObject<'b>> = items
                        .iter()
                        .map(|x| Self::lift_kobject(x, dying_frame))
                        .collect();
                    KObject::List(Rc::new(lifted))
                } else {
                    KObject::List(Rc::clone(items))
                }
            }
            KObject::Dict(entries) => {
                if entries.values().any(|x| Self::needs_lift(x, dying_frame)) {
                    let lifted: HashMap<_, _> = entries
                        .iter()
                        .map(|(k, v)| (k.clone_box(), Self::lift_kobject(v, dying_frame)))
                        .collect();
                    KObject::Dict(Rc::new(lifted))
                } else {
                    KObject::Dict(Rc::clone(entries))
                }
            }
            other => other.deep_clone(),
        }
    }

    /// True iff lifting `v` against `dying_frame` would attach an `Rc` to some descendant.
    /// Drives both `lift_kobject`'s top-level fast-path skip and the per-composite rebuild
    /// decision: when this returns false, the existing `Rc<Vec>`/`Rc<HashMap>` can be cloned
    /// instead of allocating a fresh one. Walks composites recursively but bottoms out on
    /// the first match (`any`-style).
    ///
    /// `KFuture(_, None)` returns true unconditionally, mirroring `lift_kobject`'s
    /// conservative anchor for KFutures — we can't cheaply tell whether the bundle/parsed
    /// borrows reach into the dying arena, so we treat any unanchored KFuture as if they
    /// might.
    fn needs_lift<'b>(v: &KObject<'b>, dying_frame: &Rc<CallArena>) -> bool {
        match v {
            KObject::KFunction(_, Some(_)) => false,
            KObject::KFunction(f, None) => {
                let dying_runtime: *const RuntimeArena = dying_frame.arena();
                let captured_runtime: *const RuntimeArena = f.captured_scope().arena;
                std::ptr::eq(captured_runtime, dying_runtime)
            }
            KObject::KFuture(_, Some(_)) => false,
            KObject::KFuture(_, None) => true,
            KObject::List(items) => items.iter().any(|x| Self::needs_lift(x, dying_frame)),
            KObject::Dict(entries) => entries.values().any(|x| Self::needs_lift(x, dying_frame)),
            _ => false,
        }
    }

    /// Walk slots that returned `Done(Forward)` while owning a per-call frame; for each
    /// whose forward chain has resolved to a Value, lift the Value into the captured arena
    /// (the per-call scope's `outer.arena`) and drop the frame's slot-Rc. Called after
    /// every iteration of `execute`'s main loop.
    ///
    /// Reads the sidecar `frame_holding_slots` rather than scanning all `nodes`. Slots
    /// whose chain hasn't resolved yet stay in the sidecar for a future iteration; slots
    /// that finalize get removed.
    fn finalize_ready_frames(&mut self) {
        let mut still_waiting: Vec<usize> = Vec::with_capacity(self.frame_holding_slots.len());
        for idx in std::mem::take(&mut self.frame_holding_slots) {
            if !self.is_result_ready(NodeId(idx)) {
                still_waiting.push(idx);
                continue;
            }
            let value = self.read(NodeId(idx));
            let (dest, lifted_obj) = {
                let node = self.nodes[idx].as_ref().unwrap();
                let frame = node
                    .frame
                    .as_ref()
                    .expect("frame_holding_slot must own a frame");
                let dest = node
                    .scope
                    .outer
                    .expect("per-call scope must have an outer (its captured scope)")
                    .arena;
                let lifted_obj = Self::lift_kobject(value, frame);
                (dest, lifted_obj)
            };
            let lifted = dest.alloc_object(lifted_obj);
            self.results[idx] = Some(NodeOutput::Value(lifted));
            // Drop the slot's frame and clear the node. If the lifted value cloned an Rc,
            // the per-call arena lives on (closure escape); otherwise this is the last
            // strong reference and the arena frees.
            self.nodes[idx] = None;
        }
        self.frame_holding_slots = still_waiting;
    }

    /// True iff `id`'s `Forward` chain ends in a stored `Value`. Used by the execute loop to
    /// decide whether a `Bind`/`Aggregate` whose subs depend on `id` is safe to run yet.
    fn is_result_ready(&self, id: NodeId) -> bool {
        let mut cur = id;
        loop {
            match self.results.get(cur.index()).and_then(|o| o.as_ref()) {
                Some(NodeOutput::Value(_)) => return true,
                Some(NodeOutput::Forward(next)) => cur = *next,
                None => return false,
            }
        }
    }

    /// Walk an unresolved expression. If `lazy_candidate` matches, only schedule the
    /// eager-position `Expression` parts; the lazy positions ride through as `KExpression`
    /// data into a builtin slot typed `KExpression` (`if_then`, `FN`). Otherwise schedule
    /// every `Expression` (and `ListLiteral`) part as a sub-dispatch / aggregate dep.
    /// Returns a `NodeStep`: `Done(Value)` for an inline-dispatched body that produced a
    /// value, `Done(Forward(bind_id))` when it spawned a `Bind` to wait on subs, or
    /// `Replace { work: Dispatch(expr), .. }` when the body was a tail call (the slot gets
    /// rewritten in place by the execute loop).
    fn run_dispatch(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
    ) -> Result<NodeStep<'a>, String> {
        if let Some(eager_indices) = scope.lazy_candidate(&expr) {
            let mut parts = expr.parts;
            let mut subs = Vec::with_capacity(eager_indices.len());
            for i in eager_indices {
                let inner = match std::mem::replace(
                    &mut parts[i],
                    ExpressionPart::Identifier(String::new()),
                ) {
                    ExpressionPart::Expression(boxed) => *boxed,
                    _ => unreachable!("lazy_candidate only flags Expression parts"),
                };
                let sub_id = self.add(NodeWork::Dispatch(inner), scope);
                subs.push((i, sub_id));
            }
            let parent = KExpression { parts };
            if subs.is_empty() {
                let future = scope.dispatch(parent)?;
                return Ok(self.invoke_to_step(future, scope));
            }
            let bind_id = self.add(NodeWork::Bind { expr: parent, subs }, scope);
            return Ok(NodeStep::Done(NodeOutput::Forward(bind_id)));
        }

        let mut new_parts = Vec::with_capacity(expr.parts.len());
        let mut subs: Vec<(usize, NodeId)> = Vec::new();
        for (i, part) in expr.parts.into_iter().enumerate() {
            match part {
                ExpressionPart::Expression(boxed) => {
                    let sub_id = self.add(NodeWork::Dispatch(*boxed), scope);
                    subs.push((i, sub_id));
                    // Placeholder — overwritten with `Future(result)` at Bind time.
                    new_parts.push(ExpressionPart::Identifier(String::new()));
                }
                ExpressionPart::ListLiteral(items) => {
                    let agg_id = self.schedule_list_literal(items, scope);
                    subs.push((i, agg_id));
                    new_parts.push(ExpressionPart::Identifier(String::new()));
                }
                other => new_parts.push(other),
            }
        }
        let new_expr = KExpression { parts: new_parts };
        if subs.is_empty() {
            let future = scope.dispatch(new_expr)?;
            return Ok(self.invoke_to_step(future, scope));
        }
        let bind_id = self.add(NodeWork::Bind { expr: new_expr, subs }, scope);
        Ok(NodeStep::Done(NodeOutput::Forward(bind_id)))
    }

    fn run_bind(
        &mut self,
        mut expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
        scope: &'a Scope<'a>,
    ) -> Result<NodeStep<'a>, String> {
        for (part_idx, dep_id) in subs {
            let value = self.read(dep_id);
            expr.parts[part_idx] = ExpressionPart::Future(value);
        }
        let future = scope.dispatch(expr)?;
        Ok(self.invoke_to_step(future, scope))
    }

    fn run_aggregate(&self, elements: Vec<AggregateElement<'a>>, scope: &'a Scope<'a>) -> NodeOutput<'a> {
        let items: Vec<KObject<'a>> = elements
            .into_iter()
            .map(|e| match e {
                AggregateElement::Static(obj) => obj,
                AggregateElement::Dep(dep) => self.read(dep).deep_clone(),
            })
            .collect();
        let arena = scope.arena;
        let allocated: &'a KObject<'a> = arena.alloc_object(KObject::List(Rc::new(items)));
        NodeOutput::Value(allocated)
    }

    fn schedule_list_literal(&mut self, items: Vec<ExpressionPart<'a>>, scope: &'a Scope<'a>) -> NodeId {
        let mut elements: Vec<AggregateElement<'a>> = Vec::with_capacity(items.len());
        for item in items {
            match item {
                ExpressionPart::Expression(boxed) => {
                    let sub_id = self.add(NodeWork::Dispatch(*boxed), scope);
                    elements.push(AggregateElement::Dep(sub_id));
                }
                ExpressionPart::ListLiteral(inner) => {
                    let nested_id = self.schedule_list_literal(inner, scope);
                    elements.push(AggregateElement::Dep(nested_id));
                }
                other => elements.push(AggregateElement::Static(other.resolve())),
            }
        }
        self.add(NodeWork::Aggregate { elements }, scope)
    }

    /// Run a bound future's body and translate its `BodyResult` into a `NodeStep`. `Value`
    /// becomes `Done(Value)` — the slot stores the result. `Tail { expr, scope }` becomes
    /// `Replace { work: Dispatch(expr), scope }` — the execute loop rewrites the current
    /// slot's work (and optionally rebinds scope) and re-runs it, producing the tail-call
    /// slot reuse that keeps recursion at constant scheduler memory.
    fn invoke_to_step(&mut self, future: KFuture<'a>, scope: &'a Scope<'a>) -> NodeStep<'a> {
        match future.function.invoke(scope, self, future.bundle) {
            BodyResult::Value(v) => NodeStep::Done(NodeOutput::Value(v)),
            BodyResult::Tail { expr, frame } => NodeStep::Replace {
                work: NodeWork::Dispatch(expr),
                frame,
            },
        }
    }

    /// Retrieve the resolved `KObject` for a top-level dispatch (a `NodeId` returned from
    /// `add_dispatch`). Walks `Forward` chains to a stored `Value`. Only safe to call on
    /// IDs returned by `add_dispatch` — internal Bind/Aggregate/sub-Dispatch slots' results
    /// may have been freed by `finalize_ready_frames` when their parent user-fn slot's
    /// per-call frame dropped, so reading them would be UAF.
    pub fn read(&self, id: NodeId) -> &'a KObject<'a> {
        let mut cur = id;
        loop {
            match self.results[cur.index()]
                .as_ref()
                .expect("result must be ready by the time it's read")
            {
                NodeOutput::Value(v) => return v,
                NodeOutput::Forward(next) => cur = *next,
            }
        }
    }
}

impl<'a> Default for Scheduler<'a> {
    fn default() -> Self { Self::new() }
}

/// Dep `NodeId`s whose results a node needs to read before it can run, or `None` if the node
/// can run with no resolved deps. `Dispatch` itself has none — its job is to *spawn* deps; it
/// reads no results. `Bind` reads each `(_, dep)` in its subs; `Aggregate` reads each `Dep`
/// element.
fn work_deps<'a>(work: &NodeWork<'a>) -> Option<Vec<NodeId>> {
    match work {
        NodeWork::Dispatch(_) => None,
        NodeWork::Bind { subs, .. } => Some(subs.iter().map(|(_, d)| *d).collect()),
        NodeWork::Aggregate { elements } => Some(
            elements
                .iter()
                .filter_map(|e| match e {
                    AggregateElement::Dep(d) => Some(*d),
                    AggregateElement::Static(_) => None,
                })
                .collect(),
        ),
    }
}

impl<'a> SchedulerHandle<'a> for Scheduler<'a> {
    /// Schedule a fresh `Dispatch` node against `scope`. Used by builtin bodies that want
    /// to spawn sub-work — currently no in-tree builtin reaches for it (TCO covers the
    /// prior `if_then` use), but the lever is preserved.
    fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        Scheduler::add(self, NodeWork::Dispatch(expr), scope)
    }
}

impl<'a> KFunction<'a> {
    /// Run this function's body for an already-bound call. Builtins call straight through to
    /// their `fn` pointer with the scheduler handle. User-defined functions:
    ///   1. Allocate a per-call child `Scope` parented to `scope` (the call site) and bind
    ///      each parameter value into the child's `data`. Future closure work and any body
    ///      that uses parameters in Identifier-typed slots (e.g. `(LET y = x)` reading x by
    ///      name) will rely on these bindings.
    ///   2. Substitute every parameter `Identifier` in a clone of the body with
    ///      `Future(arena.alloc(value))` so the parameters match typed slots at dispatch
    ///      time (`(PRINT x)` needs `x` to surface as a `Future(KString)`, not an
    ///      `Identifier`). The cloned value is arena-allocated, freeing on run-end.
    ///   3. Return the substituted body as `BodyResult::tail_with_scope(body, child)` so the
    ///      scheduler rewrites the caller's own slot to a fresh `Dispatch(body)` against the
    ///      child scope — a tail call reuses the caller's slot in place. Recursive user-fns
    ///      therefore run in constant scheduler memory.
    ///
    /// The child scope and substitution are complementary: substitution covers parameter
    /// references in typed-slot positions, the child scope covers Identifier-slot lookups
    /// (`(x)` parens-wrapped) and is the substrate for future closure capture. The
    /// substitution carrying its own arena means we don't depend on the child scope's
    /// `data` map for parameter values inside typed-slot dispatch.
    pub fn invoke(
        &'a self,
        scope: &'a Scope<'a>,
        sched: &mut dyn SchedulerHandle<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        match &self.body {
            Body::Builtin(f) => f(scope, sched, bundle),
            Body::UserDefined(expr) => {
                // Build a fresh per-call frame whose arena owns the per-call allocations
                // (child scope, parameter clones, substituted body's Future rewrites).
                // `outer` is the FN's captured definition scope (lexical scoping); for top-
                // level FNs that's run-root.
                let outer = self.captured_scope();
                let frame: Rc<CallArena> = CallArena::new(outer);
                // Re-borrow through raw pointers so the borrow ends before the `frame` move
                // below. SAFETY: heap-pinning makes `arena_ptr` and `scope_ptr` valid for
                // the box's life; allocations into the arena live until `frame` drops.
                let arena_ptr: *const RuntimeArena = frame.arena();
                let scope_ptr: *const Scope<'_> = frame.scope();
                let inner_arena: &'a RuntimeArena = unsafe { &*(arena_ptr as *const _) };
                let child: &'a Scope<'a> = unsafe { &*(scope_ptr as *const _) };
                for (name, rc) in bundle.args.iter() {
                    let cloned = rc.deep_clone();
                    let allocated = inner_arena.alloc_object(cloned);
                    child.add(name.clone(), allocated);
                }
                let substituted = substitute_params(expr.clone(), &bundle, inner_arena);
                BodyResult::tail_with_frame(substituted, frame)
            }
        }
    }
}

/// Replace every `Identifier(name)` in `expr` whose name is a key in `bundle.args` with a
/// `Future(value)` carrying that arg's arena-allocated value. Recurses into nested
/// `Expression` and `ListLiteral` parts so a body like `(PRINT (x))` substitutes the inner
/// `(x)` correctly. `Keyword`, `Literal`, and `Future` parts pass through unchanged. Each
/// substituted value is allocated via the arena (replacing the prior `Box::leak`-per-call
/// model from before the leak fix).
fn substitute_params<'a>(
    expr: KExpression<'a>,
    bundle: &ArgumentBundle<'a>,
    arena: &'a crate::dispatch::arena::RuntimeArena,
) -> KExpression<'a> {
    KExpression {
        parts: expr
            .parts
            .into_iter()
            .map(|p| substitute_part(p, bundle, arena))
            .collect(),
    }
}

fn substitute_part<'a>(
    part: ExpressionPart<'a>,
    bundle: &ArgumentBundle<'a>,
    arena: &'a crate::dispatch::arena::RuntimeArena,
) -> ExpressionPart<'a> {
    match part {
        ExpressionPart::Identifier(name) => match bundle.get(&name) {
            Some(value) => {
                let allocated: &'a KObject<'a> = arena.alloc_object(value.deep_clone());
                ExpressionPart::Future(allocated)
            }
            None => ExpressionPart::Identifier(name),
        },
        ExpressionPart::Expression(boxed) => {
            ExpressionPart::Expression(Box::new(substitute_params(*boxed, bundle, arena)))
        }
        ExpressionPart::ListLiteral(items) => ExpressionPart::ListLiteral(
            items
                .into_iter()
                .map(|p| substitute_part(p, bundle, arena))
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::arena::RuntimeArena;
    use crate::dispatch::builtins::default_scope;
    use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

    fn let_expr<'a>(name: &str, value: f64) -> KExpression<'a> {
        KExpression {
            parts: vec![
                ExpressionPart::Keyword("LET".into()),
                ExpressionPart::Identifier(name.into()),
                ExpressionPart::Keyword("=".into()),
                ExpressionPart::Literal(KLiteral::Number(value)),
            ],
        }
    }

    #[test]
    fn dispatches_independent_expressions_in_order() {
        let arena = RuntimeArena::new();
        let root = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let id1 = sched.add_dispatch(let_expr("x", 1.0), root);
        let id2 = sched.add_dispatch(let_expr("y", 2.0), root);

        sched.execute().unwrap();

        assert!(matches!(sched.read(id1), KObject::Number(n) if *n == 1.0));
        assert!(matches!(sched.read(id2), KObject::Number(n) if *n == 2.0));
        let data = root.data.borrow();
        assert!(data.contains_key("x"));
        assert!(data.contains_key("y"));
    }

    #[test]
    fn later_expression_sees_earlier_binding_via_lookup() {
        // `(x)` parses as a sub-Expression; the scheduler walks the second top-level
        // expression, spawns a sub-Dispatch for `(x)`, and the LET node above runs first
        // because its NodeId is smaller. Tests the in-order processing invariant.
        let arena = RuntimeArena::new();
        let root = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        sched.add_dispatch(let_expr("a", 10.0), root);

        let lookup_a = KExpression {
            parts: vec![
                ExpressionPart::Keyword("LET".into()),
                ExpressionPart::Identifier("b".into()),
                ExpressionPart::Keyword("=".into()),
                ExpressionPart::Expression(Box::new(KExpression {
                    parts: vec![ExpressionPart::Identifier("a".into())],
                })),
            ],
        };
        sched.add_dispatch(lookup_a, root);

        sched.execute().unwrap();
        let data = root.data.borrow();
        assert!(matches!(data.get("b"), Some(KObject::Number(n)) if *n == 10.0));
    }

    #[test]
    fn tail_call_reuses_node_slot_in_place() {
        // `IF true THEN ("hi")` — when the predicate is true, `if_then` returns
        // `BodyResult::Tail(value_expr)`. The scheduler should rewrite the if_then's own
        // slot to a `Dispatch(value_expr)` and re-run, NOT spawn a fresh slot and forward.
        // Without TCO this would land at len() == 2 (one slot for if_then, one for the
        // value evaluation). With TCO, len() == 1 — the if_then's slot was reused.
        let arena = RuntimeArena::new();
        let root = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let value = KExpression {
            parts: vec![ExpressionPart::Literal(KLiteral::String("hi".into()))],
        };
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("IF".into()),
                ExpressionPart::Literal(KLiteral::Boolean(true)),
                ExpressionPart::Keyword("THEN".into()),
                ExpressionPart::Expression(Box::new(value)),
            ],
        };
        let id = sched.add_dispatch(expr, root);

        sched.execute().unwrap();

        assert!(matches!(sched.read(id), KObject::KString(s) if s == "hi"));
        assert_eq!(
            sched.len(),
            1,
            "tail-call slot reuse: the if_then's original slot should have been rewritten \
             to evaluate `(\"hi\")`, not allocate a new slot",
        );
    }
}
