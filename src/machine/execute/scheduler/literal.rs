use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::model::{KKey, KObject, Serializable};
use crate::machine::{BodyResult, CombineFinish, Frame, NodeId, Scope};
use crate::machine::model::ast::{ExpressionPart, KExpression};

use super::super::nodes::NodeWork;
use super::Scheduler;

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

impl<'a> Scheduler<'a> {
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
            ExpressionPart::Type(t)
                if wrap_identifiers
                    && matches!(t.params, crate::machine::model::ast::TypeParams::None) =>
            {
                // Auto-wrap for bare leaf Type-tokens in value slots: `MAKESET IntOrd`
                // sub-dispatches `(IntOrd)` through the TypeExprRef overload of
                // `value_lookup`, which surfaces the bound `KModule`/`KSignature`.
                let expr = KExpression {
                    parts: vec![ExpressionPart::Type(t)],
                };
                let sub_id = self.add(NodeWork::Dispatch(expr), scope);
                let pos = deps.len();
                deps.push(sub_id);
                Slot::Dep(pos)
            }
            other => Slot::Static(other.resolve()),
        }
    }
}
