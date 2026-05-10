use std::collections::HashMap;
use std::rc::Rc;

use crate::dispatch::runtime::{Frame, KError, Resolution};
use crate::dispatch::kfunction::{BodyResult, CombineFinish, NodeId};
use crate::dispatch::values::KKey;
use crate::dispatch::values::KObject;
use crate::dispatch::types::{Parseable, Serializable};
use crate::dispatch::runtime::{KFuture, Scope};
use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::nodes::{NodeOutput, NodeStep, NodeWork};
use super::scheduler::Scheduler;

/// One element of a list literal or one side of a dict-literal pair, captured by the
/// `Combine` closure. `Static` carries an already-resolved value (e.g. a literal scalar);
/// `Dep(pos)` indexes into the `Combine`'s dep-results slice. Kept private to the planner
/// — the scheduler doesn't see it.
enum Slot<'a> {
    Static(KObject<'a>),
    Dep(usize),
}

impl<'a> Slot<'a> {
    /// Materialize this slot into an owned `KObject` for the literal under construction.
    /// `Dep` results are deep-cloned because the resulting `KList` / `KDict` owns its
    /// elements (you can't store `&'a KObject` into `Rc<Vec<KObject>>`). Infallible:
    /// `run_combine` short-circuits on errored deps before invoking the closure.
    fn materialize(self, results: &[&'a KObject<'a>]) -> KObject<'a> {
        match self {
            Slot::Static(obj) => obj,
            // `results` mirrors `deps` order, so `Dep(pos)` indexes directly.
            Slot::Dep(pos) => results[pos].deep_clone(),
        }
    }
}

/// Idempotent on a §8 replay-park re-dispatch. Errors with `Rebind` if `data` or
/// `placeholders` already holds `name` and the existing entry doesn't match `idx`.
fn install_dispatch_placeholder<'a>(
    expr: &KExpression<'a>,
    scope: &'a Scope<'a>,
    idx: usize,
) -> Result<(), KError> {
    use crate::dispatch::kfunction::NodeId;
    let key = expr.untyped_key();
    // Placeholder installs land in the dispatching scope, not the scope the candidate
    // was found in — placeholders track dispatch-time intent local to the call site.
    let mut current: Option<&Scope<'a>> = Some(scope);
    while let Some(s) = current {
        let candidate = {
            let functions = s.functions.borrow();
            let mut found: Option<String> = None;
            if let Some(bucket) = functions.get(&key) {
                for f in bucket.iter() {
                    if let Some(extractor) = f.pre_run {
                        if let Some(name) = extractor(expr) {
                            found = Some(name);
                            break;
                        }
                    }
                }
            }
            found
        };
        if let Some(name) = candidate {
            return scope.install_placeholder(name, NodeId(idx));
        }
        current = s.outer;
    }
    Ok(())
}

impl<'a> Scheduler<'a> {
    /// See design/execution-model.md for the dispatch-time placeholder rules
    /// (§1 short-circuit, §4 install, §7 auto-wrap, §8 replay-park) referenced below.
    pub(super) fn run_dispatch(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        // §4: a `Rebind` here surfaces as Done(Err) so other slots keep draining.
        if let Err(e) = install_dispatch_placeholder(&expr, scope, idx) {
            return Ok(NodeStep::Done(NodeOutput::Err(e)));
        }

        // §1: single-Identifier short-circuit. Unbound falls through so `value_lookup`'s
        // body produces the structured `UnboundName` error.
        if let [ExpressionPart::Identifier(name)] = expr.parts.as_slice() {
            match scope.resolve(name) {
                Resolution::Value(obj) => {
                    return Ok(NodeStep::Done(NodeOutput::Value(obj)));
                }
                Resolution::Placeholder(producer_id) => {
                    self.node_dependencies[idx].push(producer_id.index());
                    return Ok(NodeStep::Replace {
                        work: NodeWork::Lift { from: producer_id },
                        frame: None,
                        function: None,
                    });
                }
                Resolution::Unbound => {}
            }
        }

        let expr = match scope.shape_pick(&expr) {
            Some(pick) => {
                // §7 wrap: bare-Identifier in a value slot becomes a single-Identifier
                // sub-Expression so it re-enters via §1.
                let mut parts = expr.parts;
                for i in pick.wrap_indices {
                    if let ExpressionPart::Identifier(name) =
                        std::mem::replace(&mut parts[i], ExpressionPart::Identifier(String::new()))
                    {
                        parts[i] = ExpressionPart::Expression(Box::new(KExpression {
                            parts: vec![ExpressionPart::Identifier(name)],
                        }));
                    }
                }
                let rewritten = KExpression { parts };

                // §8 replay-park check. A `ref_name_indices` slot whose producer has
                // already terminalized but whose placeholder is still set means the
                // producer errored (success would have cleared the placeholder via
                // `bind_value`); propagate the error rather than parking on a dead slot.
                let mut producers_to_wait: Vec<NodeId> = Vec::new();
                for i in pick.ref_name_indices {
                    let name = match rewritten.parts.get(i) {
                        Some(ExpressionPart::Identifier(n)) => n.as_str(),
                        // wrap_indices and ref_name_indices are disjoint by construction.
                        _ => continue,
                    };
                    match scope.resolve(name) {
                        Resolution::Placeholder(producer_id) => {
                            if self.is_result_ready(producer_id) {
                                if let Err(e) = self.read_result(producer_id) {
                                    let frame = Frame {
                                        function: "<replay-park>".to_string(),
                                        expression: rewritten.summarize(),
                                    };
                                    let propagated = e.clone_for_propagation().with_frame(frame);
                                    return Ok(NodeStep::Done(NodeOutput::Err(propagated)));
                                }
                            } else {
                                producers_to_wait.push(producer_id);
                            }
                        }
                        Resolution::Value(_) | Resolution::Unbound => {}
                    }
                }

                if !producers_to_wait.is_empty() {
                    for p in &producers_to_wait {
                        self.node_dependencies[idx].push(p.index());
                    }
                    return Ok(NodeStep::Replace {
                        work: NodeWork::Dispatch(rewritten),
                        frame: None,
                        function: None,
                    });
                }

                rewritten
            }
            None => expr,
        };

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
                return Ok(self.invoke_to_step(future, scope, idx));
            }
            let bind_id = self.add(NodeWork::Bind { expr: parent, subs }, scope);
            return Ok(self.defer_to_lift(idx, bind_id));
        }

        let mut new_parts = Vec::with_capacity(expr.parts.len());
        let mut subs: Vec<(usize, NodeId)> = Vec::new();
        for (i, part) in expr.parts.into_iter().enumerate() {
            match part {
                ExpressionPart::Expression(boxed) => {
                    let sub_id = self.add(NodeWork::Dispatch(*boxed), scope);
                    subs.push((i, sub_id));
                    // Slot overwritten with `Future(result)` at Bind time.
                    new_parts.push(ExpressionPart::Identifier(String::new()));
                }
                ExpressionPart::ListLiteral(items) => {
                    let agg_id = self.schedule_list_literal(items, scope);
                    subs.push((i, agg_id));
                    new_parts.push(ExpressionPart::Identifier(String::new()));
                }
                ExpressionPart::DictLiteral(pairs) => {
                    let agg_id = self.schedule_dict_literal(pairs, scope);
                    subs.push((i, agg_id));
                    new_parts.push(ExpressionPart::Identifier(String::new()));
                }
                other => new_parts.push(other),
            }
        }
        let new_expr = KExpression { parts: new_parts };
        if subs.is_empty() {
            let future = scope.dispatch(new_expr)?;
            return Ok(self.invoke_to_step(future, scope, idx));
        }
        let bind_id = self.add(NodeWork::Bind { expr: new_expr, subs }, scope);
        Ok(self.defer_to_lift(idx, bind_id))
    }

    /// Frame / function are left as `None` so the slot's existing per-call frame and
    /// function label stay attached when the Lift writes its terminal.
    fn defer_to_lift(&mut self, idx: usize, bind_id: NodeId) -> NodeStep<'a> {
        self.node_dependencies[idx].push(bind_id.index());
        NodeStep::Replace {
            work: NodeWork::Lift { from: bind_id },
            frame: None,
            function: None,
        }
    }

    pub(super) fn run_bind(
        &mut self,
        mut expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        // Sub slots stay in `node_dependencies[idx]` on the error path so chain-free at
        // finalize reclaims them; eager free is the success-path optimization.
        for (_, dep_id) in &subs {
            if let Err(e) = self.read_result(*dep_id) {
                let frame = Frame {
                    function: "<bind>".to_string(),
                    expression: expr.summarize(),
                };
                let propagated = e.clone_for_propagation().with_frame(frame);
                return Ok(NodeStep::Done(NodeOutput::Err(propagated)));
            }
        }
        let dep_indices: Vec<usize> = subs.iter().map(|(_, d)| d.index()).collect();
        for (part_idx, dep_id) in subs {
            let value = self.read(dep_id);
            expr.parts[part_idx] = ExpressionPart::Future(value);
        }
        // Spliced `Future(&'a KObject)` references survive `results[dep] = None`
        // because the objects live in arenas tied to lexical scope. Reclaim happens
        // before `scope.dispatch` so the dispatched body's `add()` calls can recycle
        // the indices immediately.
        self.reclaim_deps(idx, dep_indices);
        let future = scope.dispatch(expr)?;
        Ok(self.invoke_to_step(future, scope, idx))
    }

    /// Success-path eager free; the error path leaves deps for chain-free at slot drop.
    fn reclaim_deps(&mut self, idx: usize, dep_indices: Vec<usize>) {
        self.node_dependencies[idx].clear();
        for d in dep_indices {
            self.free(d);
        }
    }

    /// Run a `Combine` slot: short-circuit on the first errored dep using the same
    /// frame-attached propagation as `run_bind`, then call `finish` over the dep values
    /// and decode the returned `BodyResult` (Value, Tail, or Err) into a `NodeStep`
    /// using the same dispatch as `invoke_to_step`. Deps are eagerly freed on the
    /// success path; the error path leaves them in `node_dependencies[idx]` for
    /// chain-free at slot drop.
    pub(super) fn run_combine(
        &mut self,
        deps: Vec<NodeId>,
        finish: CombineFinish<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        // The closure carries its own framing context (e.g. "<list>", "<dict>") via its
        // capture; the Combine machinery only handles dep-error propagation, which uses
        // the generic "<combine>" frame to match `run_bind`'s "<bind>" convention.
        let make_frame = || Frame {
            function: "<combine>".to_string(),
            expression: "combine".to_string(),
        };
        for dep in &deps {
            if let Err(e) = self.read_result(*dep) {
                let propagated = e.clone_for_propagation().with_frame(make_frame());
                return NodeStep::Done(NodeOutput::Err(propagated));
            }
        }
        // Pre-collect refs so `finish` (which holds `&mut self` via the trait object)
        // doesn't reborrow `self` for reads.
        let values: Vec<&'a KObject<'a>> = deps.iter().map(|d| self.read(*d)).collect();
        let dep_indices: Vec<usize> = deps.iter().map(|d| d.index()).collect();
        let body = finish(scope, self, &values);
        self.reclaim_deps(idx, dep_indices);
        match body {
            BodyResult::Value(v) => NodeStep::Done(NodeOutput::Value(v)),
            BodyResult::Tail { expr, frame, function } => NodeStep::Replace {
                work: NodeWork::Dispatch(expr),
                frame,
                function,
            },
            BodyResult::DeferTo(id) => self.defer_to_lift(idx, id),
            BodyResult::Err(e) => NodeStep::Done(NodeOutput::Err(e)),
        }
    }

    /// Schedule a list literal as a `Combine`: every `Expression` part becomes a sub
    /// `Dispatch`; bare identifiers and other already-resolved parts stay as captured
    /// `KObject` statics. The closure interleaves dep results with statics in source
    /// order to build the final `KObject::List`.
    pub(super) fn schedule_list_literal(
        &mut self,
        items: Vec<ExpressionPart<'a>>,
        scope: &'a Scope<'a>,
    ) -> NodeId {
        let mut layout: Vec<Slot<'a>> = Vec::with_capacity(items.len());
        let mut deps: Vec<NodeId> = Vec::new();
        for part in items {
            // Bare `Identifier` parts in a list stay `Static` (parser surfaces them as
            // already-resolved values via `resolve()`); list elements are not
            // name-resolved like dict keys/values are.
            let slot = self.classify_aggregate_part(part, scope, &mut deps, false);
            layout.push(slot);
        }
        let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
            let items: Vec<KObject<'a>> = layout
                .into_iter()
                .map(|slot| slot.materialize(results))
                .collect();
            let allocated: &'a KObject<'a> =
                scope.arena.alloc_object(KObject::List(Rc::new(items)));
            BodyResult::Value(allocated)
        });
        self.add_combine(deps, scope, finish)
    }

    /// Schedule a dict literal as a `Combine`. Bare identifiers on either side are
    /// scheduled as sub-Dispatches (Python-like name resolution applies to both keys and
    /// values). Identifier wrapping happens here, not at parse time, so the AST stays
    /// faithful to the source. The closure performs `KKey` conversion on each key —
    /// non-scalar keys produce `KErrorKind::ShapeError`.
    pub(super) fn schedule_dict_literal(
        &mut self,
        pairs: Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>,
        scope: &'a Scope<'a>,
    ) -> NodeId {
        let mut layout: Vec<(Slot<'a>, Slot<'a>)> = Vec::with_capacity(pairs.len());
        let mut deps: Vec<NodeId> = Vec::new();
        for (k, v) in pairs {
            let key_slot = self.classify_aggregate_part(k, scope, &mut deps, true);
            let val_slot = self.classify_aggregate_part(v, scope, &mut deps, true);
            layout.push((key_slot, val_slot));
        }
        let frame_label = || Frame {
            function: "<dict>".to_string(),
            expression: "dict literal".to_string(),
        };
        let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
            let mut map: HashMap<Box<dyn Serializable + 'a>, KObject<'a>> = HashMap::new();
            for (k_slot, v_slot) in layout {
                let key_obj = k_slot.materialize(results);
                let value_obj = v_slot.materialize(results);
                let kkey = match KKey::try_from_kobject(&key_obj) {
                    Ok(k) => k,
                    Err(e) => return BodyResult::Err(e.with_frame(frame_label())),
                };
                map.insert(Box::new(kkey), value_obj);
            }
            let allocated: &'a KObject<'a> =
                scope.arena.alloc_object(KObject::Dict(Rc::new(map)));
            BodyResult::Value(allocated)
        });
        self.add_combine(deps, scope, finish)
    }

    /// Plan one slot of a list / dict literal: nested literals recurse via their own
    /// schedulers, `Expression` parts spawn sub-Dispatches, and bare identifiers either
    /// stay static (list) or wrap as sub-Dispatches (dict, when `wrap_identifiers` is
    /// set). Sub-Dispatch ids are pushed onto `deps` and tracked in the returned `Slot`
    /// by their position in `deps`.
    fn classify_aggregate_part(
        &mut self,
        part: ExpressionPart<'a>,
        scope: &'a Scope<'a>,
        deps: &mut Vec<NodeId>,
        wrap_identifiers: bool,
    ) -> Slot<'a> {
        match part {
            ExpressionPart::ListLiteral(inner) => {
                let nested_id = self.schedule_list_literal(inner, scope);
                let pos = deps.len();
                deps.push(nested_id);
                Slot::Dep(pos)
            }
            ExpressionPart::DictLiteral(inner) => {
                let nested_id = self.schedule_dict_literal(inner, scope);
                let pos = deps.len();
                deps.push(nested_id);
                Slot::Dep(pos)
            }
            ExpressionPart::Expression(boxed) => {
                let sub_id = self.add(NodeWork::Dispatch(*boxed), scope);
                let pos = deps.len();
                deps.push(sub_id);
                Slot::Dep(pos)
            }
            ExpressionPart::Identifier(name) if wrap_identifiers => {
                let expr = KExpression {
                    parts: vec![ExpressionPart::Identifier(name)],
                };
                let sub_id = self.add(NodeWork::Dispatch(expr), scope);
                let pos = deps.len();
                deps.push(sub_id);
                Slot::Dep(pos)
            }
            other => Slot::Static(other.resolve()),
        }
    }

    /// Returns a fresh `NodeOutput` referencing `results[from]`'s terminal value. The
    /// `&KObject<'a>` is the same reference the producer wrote, not a clone — the arena
    /// lifetime contract must hold across notify-wake and re-run. The execute loop's
    /// Done arm handles frame-aware deep-cloning into the outer arena.
    ///
    /// Invariant: when notify-walk wakes a Lift, `results[from]` is `Some` (Value or Err).
    /// A `None` would mean the wake fired without a terminal write, which is impossible
    /// by construction.
    pub(super) fn run_lift(&self, from: NodeId) -> NodeOutput<'a> {
        match self.results[from.index()]
            .as_ref()
            .expect("Lift only runs after notify wakes it from `from`'s terminal write")
        {
            NodeOutput::Value(v) => NodeOutput::Value(v),
            NodeOutput::Err(e) => NodeOutput::Err(e.clone_for_propagation()),
        }
    }

    /// `BodyResult::Tail` rewrites the current slot's work in place — this is what gives
    /// recursion constant scheduler memory. `BodyResult::DeferTo(id)` rewrites to a Lift
    /// off `id`, so the slot's terminal becomes whatever `id` produces; matches
    /// `defer_to_lift`'s post-Bind shape but for body-driven combinator planning (MODULE
    /// and SIG body wrap-up via `add_combine`).
    ///
    /// `idx` is the executing slot. Needed so the `DeferTo` arm can push `id` into
    /// `node_dependencies[idx]` before returning the `Replace` — without that push,
    /// `register_slot_deps` sees an empty dep list and re-enqueues the Lift before the
    /// producer runs (the same shape `defer_to_lift` already handles).
    pub(super) fn invoke_to_step(
        &mut self,
        future: KFuture<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        match future.function.invoke(scope, self, future.bundle) {
            BodyResult::Value(v) => NodeStep::Done(NodeOutput::Value(v)),
            BodyResult::Tail { expr, frame, function } => NodeStep::Replace {
                work: NodeWork::Dispatch(expr),
                frame,
                function,
            },
            BodyResult::DeferTo(id) => self.defer_to_lift(idx, id),
            BodyResult::Err(e) => NodeStep::Done(NodeOutput::Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    //! End-to-end coverage for the §1/§7/§8 dispatch-time placeholder routing in
    //! `run_dispatch` (see design/execution-model.md).
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::runtime::{KErrorKind, RuntimeArena};
    use crate::dispatch::values::KObject;
    use crate::execute::scheduler::Scheduler;
    use crate::parse::expression_tree::parse;

    fn parse_one(src: &str) -> crate::parse::kexpression::KExpression<'static> {
        let mut exprs = parse(src).expect("parse should succeed");
        assert_eq!(exprs.len(), 1, "test helper expects a single expression");
        exprs.remove(0)
    }

    fn parse_all(src: &str) -> Vec<crate::parse::kexpression::KExpression<'static>> {
        parse(src).expect("parse should succeed")
    }

    #[test]
    fn single_identifier_short_circuit_returns_value_when_bound() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all("LET x = 42") {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        let id = sched.add_dispatch(parse_one("(x)"), scope);
        sched.execute().unwrap();
        assert!(matches!(sched.read(id), KObject::Number(n) if *n == 42.0));
    }

    /// Submission order matters: `LET y = (x)` dispatches first and parks on `x`'s
    /// placeholder; `LET x = 1` then wakes the parked sub.
    #[test]
    fn single_identifier_short_circuit_lift_parks_on_placeholder() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all("LET y = (x)\nLET x = 1") {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 1.0));
    }

    #[test]
    fn single_identifier_short_circuit_falls_through_when_unbound() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("(missing)"), scope);
        sched.execute().unwrap();
        let err = match sched.read_result(id) {
            Err(e) => e.clone(),
            Ok(_) => panic!("missing should error"),
        };
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(name) if name == "missing"),
            "expected UnboundName, got {err}",
        );
    }

    #[test]
    fn bare_identifier_in_value_slot_auto_wraps_and_resolves() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all("LET z = 7\nLET y = z") {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 7.0));
    }

    #[test]
    fn bare_identifier_in_value_slot_parks_when_forward_referenced() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all("LET y = z\nLET z = 9") {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 9.0));
    }

    #[test]
    fn multiple_value_slot_placeholders_park_on_distinct_producers() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all(
            "FN (ADD a: Number BY b: Number) -> Number = (a)\n\
             LET out = (ADD aa BY bb)\n\
             LET aa = 3\n\
             LET bb = 4",
        ) {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 3.0));
    }

    /// The FN binder skips the placeholder install when the name is already a function in
    /// scope (overload model), so the callee must not yet be in `data` when the caller
    /// dispatches for a true forward-reference park.
    #[test]
    fn call_by_name_replay_parks_on_forward_function_reference() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all(
            "LET out = (DOUBLE 7)\n\
             FN (DOUBLE x: Number) -> Number = (x)",
        ) {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 7.0));
    }

    #[test]
    fn multi_producer_replay_park_waits_for_all_then_re_dispatches() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all(
            "FN (ADD a: Number BY b: Number) -> Number = (b)\n\
             LET out = (ADD aa BY bb)\n\
             LET aa = 11\n\
             LET bb = 22",
        ) {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 22.0));
    }

    /// Miri audit-slate: pins the §1 Lift-park lifetime contract. The `&KObject<'a>` the
    /// Lift returns is the producer's reference, not a clone — the arena must outlive
    /// the wake and re-run.
    #[test]
    fn lift_park_minimal_program_for_miri() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all("LET y = z\nLET z = 11") {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 11.0));
    }

    /// Miri audit-slate: pins the §8 replay-park scope-lifetime contract — the parked
    /// slot's scope must stay valid across the wake and the re-dispatch.
    #[test]
    fn replay_park_minimal_program_for_miri() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all(
            "LET out = (DOUBLE 7)\n\
             FN (DOUBLE x: Number) -> Number = (x)",
        ) {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 7.0));
    }

    /// A producer that errors at dispatch time aborts `execute` via `?` propagation.
    /// Rerouting sub-Dispatch failures into the consumer's slot is a follow-up.
    #[test]
    fn replay_park_propagates_producer_error() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all(
            "LET y = (x)\n\
             LET x = (UNDEFINED_FN)",
        ) {
            sched.add_dispatch(e, scope);
        }
        let exec_result = sched.execute();
        assert!(
            exec_result.is_err(),
            "UNDEFINED_FN dispatch failure should surface via execute",
        );
        assert!(scope.lookup("y").is_none(), "y should not bind when its dependency errors");
    }
}
