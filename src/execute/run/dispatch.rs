use crate::dispatch::{Frame, KError, NodeId, Parseable, Resolution, Scope};
use crate::parse::{ExpressionPart, KExpression};

use super::super::nodes::{DepEdge, NodeOutput, NodeStep, NodeWork};
use super::super::scheduler::Scheduler;

/// Idempotent on a §8 replay-park re-dispatch. Errors with `Rebind` if `data` or
/// `placeholders` already holds `name` and the existing entry doesn't match `idx`.
fn install_dispatch_placeholder<'a>(
    expr: &KExpression<'a>,
    scope: &'a Scope<'a>,
    idx: usize,
) -> Result<(), KError> {
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
    pub(in crate::execute) fn run_dispatch(
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
                    // Notify edge, not Owned: the producer is a sibling slot this Lift
                    // only parks on for a wake — it is not part of this slot's reclaim
                    // subtree. Bookkeeping: `register_slot_deps` will still install the
                    // forward wake on `notify_list[producer]`; `free` will skip past it.
                    self.dep_edges[idx].push(DepEdge::Notify(producer_id));
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
                // §7 wrap: bare-Identifier or bare leaf Type-token in a value slot becomes
                // a single-name sub-Expression so it re-enters via §1 and routes through
                // the Identifier or TypeExprRef overload of `value_lookup`.
                let mut parts = expr.parts;
                for i in pick.wrap_indices {
                    let placeholder = ExpressionPart::Identifier(String::new());
                    let original = std::mem::replace(&mut parts[i], placeholder);
                    parts[i] = match original {
                        ExpressionPart::Identifier(name) => {
                            ExpressionPart::Expression(Box::new(KExpression {
                                parts: vec![ExpressionPart::Identifier(name)],
                            }))
                        }
                        ExpressionPart::Type(t) => {
                            ExpressionPart::Expression(Box::new(KExpression {
                                parts: vec![ExpressionPart::Type(t)],
                            }))
                        }
                        // wrap_indices is built from is_bare_name parts; any other variant
                        // is a classifier bug. Restore the part rather than panic.
                        other => other,
                    };
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
                        // Bare leaf Type-tokens in literal-name slots park on the same
                        // placeholder rails as Identifier — `IntOrd :| OrderedSig` waits
                        // on a forward-declared `MODULE IntOrd` the same way `LET y = (x)`
                        // waits on `LET x = …`. Parameterized Type parts (List<…>, etc.)
                        // are structural type-syntax, not look-up targets.
                        Some(ExpressionPart::Type(t))
                            if matches!(t.params, crate::parse::TypeParams::None) =>
                        {
                            t.name.as_str()
                        }
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
                    // Notify edges: §8 replay-park parks on sibling producers (often
                    // top-level slots) the rewritten Dispatch does not own. `free` must
                    // not transit through these into the producer's subtree.
                    for p in &producers_to_wait {
                        self.dep_edges[idx].push(DepEdge::Notify(*p));
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
}
