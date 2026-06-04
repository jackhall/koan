use std::collections::HashMap;

use crate::machine::core::source::Spanned;
use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::{KKey, KObject, Record, Serializable};
use crate::machine::{
    BodyResult, CombineFinish, Frame, KError, KErrorKind, NameOutcome, NodeId, Scope,
};

use super::super::dispatch::resolve_name_part;
use super::super::nodes::NodeWork;
use super::Scheduler;

/// One element of a list literal or one side of a dict-literal pair. Indices are into the
/// Combine's results: `Park(i)` reads position `i` of the park-producer prefix; `Owned(j)`
/// reads position `park_count + j` of the owned-sub suffix.
enum Slot<'a> {
    Static(KObject<'a>),
    Park(usize),
    Owned(usize),
}

impl<'a> Slot<'a> {
    /// Deep-clones `Park` / `Owned` results because the produced `KList` / `KDict` owns its
    /// elements and can't borrow `&'a KObject` into `Rc<Vec<KObject>>`.
    fn materialize(self, results: &[&'a KObject<'a>], park_count: usize) -> KObject<'a> {
        match self {
            Slot::Static(obj) => obj,
            Slot::Park(i) => results[i].deep_clone(),
            Slot::Owned(j) => results[park_count + j].deep_clone(),
        }
    }
}

impl<'a> Scheduler<'a> {
    pub(in crate::machine::execute) fn schedule_list_literal(
        &mut self,
        items: Vec<ExpressionPart<'a>>,
        scope: &'a Scope<'a>,
    ) -> NodeId {
        let mut layout: Vec<Slot<'a>> = Vec::with_capacity(items.len());
        let mut deps: Vec<NodeId> = Vec::new();
        let mut park_producers: Vec<NodeId> = Vec::new();
        for part in items {
            // List elements are not name-resolved; bare identifiers stay `Static`.
            let slot =
                self.classify_aggregate_part(part, scope, &mut deps, &mut park_producers, false);
            layout.push(slot);
        }
        let park_count = park_producers.len();
        let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
            let items: Vec<KObject<'a>> = layout
                .into_iter()
                .map(|slot| slot.materialize(results, park_count))
                .collect();
            let allocated: &'a KObject<'a> = scope.arena.alloc_object(KObject::list(items));
            BodyResult::Value(allocated)
        });
        self.add_combine(deps, park_producers, scope, finish)
    }

    /// Bare identifiers on either side are name-resolved (Python-like: keys are
    /// expressions, not symbols). Non-scalar keys produce `KErrorKind::ShapeError` at
    /// finish-time via the `KKey` conversion.
    pub(in crate::machine::execute) fn schedule_dict_literal(
        &mut self,
        pairs: Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>,
        scope: &'a Scope<'a>,
    ) -> NodeId {
        let mut layout: Vec<(Slot<'a>, Slot<'a>)> = Vec::with_capacity(pairs.len());
        let mut deps: Vec<NodeId> = Vec::new();
        let mut park_producers: Vec<NodeId> = Vec::new();
        for (k, v) in pairs {
            let key_slot =
                self.classify_aggregate_part(k, scope, &mut deps, &mut park_producers, true);
            let val_slot =
                self.classify_aggregate_part(v, scope, &mut deps, &mut park_producers, true);
            layout.push((key_slot, val_slot));
        }
        let frame_label = || Frame::bare("<dict>", "dict literal");
        let park_count = park_producers.len();
        let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
            let mut map: HashMap<Box<dyn Serializable<'a> + 'a>, KObject<'a>> = HashMap::new();
            for (k_slot, v_slot) in layout {
                let key_obj = k_slot.materialize(results, park_count);
                let value_obj = v_slot.materialize(results, park_count);
                let kkey = match KKey::try_from_kobject(&key_obj) {
                    Ok(k) => k,
                    Err(msg) => {
                        return BodyResult::Err(
                            KError::new(KErrorKind::ShapeError(msg)).with_frame(frame_label()),
                        )
                    }
                };
                map.insert(Box::new(kkey), value_obj);
            }
            let allocated: &'a KObject<'a> = scope.arena.alloc_object(KObject::dict(map));
            BodyResult::Value(allocated)
        });
        self.add_combine(deps, park_producers, scope, finish)
    }

    /// Record literal (`{x = 1, y = "a"}`). Field *names* are literal schema keys (never
    /// resolved); field *values* are name-resolved like dict values. Materializes a
    /// `KObject::Record`, which memoizes the per-field type record at construction.
    pub(in crate::machine::execute) fn schedule_record_literal(
        &mut self,
        fields: Vec<(String, ExpressionPart<'a>)>,
        scope: &'a Scope<'a>,
    ) -> NodeId {
        let mut names: Vec<String> = Vec::with_capacity(fields.len());
        let mut layout: Vec<Slot<'a>> = Vec::with_capacity(fields.len());
        let mut deps: Vec<NodeId> = Vec::new();
        let mut park_producers: Vec<NodeId> = Vec::new();
        for (name, value) in fields {
            let val_slot =
                self.classify_aggregate_part(value, scope, &mut deps, &mut park_producers, true);
            names.push(name);
            layout.push(val_slot);
        }
        let park_count = park_producers.len();
        let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
            let record: Record<KObject<'a>> = names
                .into_iter()
                .zip(layout)
                .map(|(name, slot)| (name, slot.materialize(results, park_count)))
                .collect();
            let allocated: &'a KObject<'a> = scope.arena.alloc_object(KObject::record(record));
            BodyResult::Value(allocated)
        });
        self.add_combine(deps, park_producers, scope, finish)
    }

    /// Plan one slot of a list / dict literal. The cycle check in the bare-name path is
    /// suppressed (`consumer = None` to `resolve_name_part`) because the Combine slot
    /// does not yet exist; cycles are caught post-submission against the Combine ID.
    fn classify_aggregate_part(
        &mut self,
        part: ExpressionPart<'a>,
        scope: &'a Scope<'a>,
        deps: &mut Vec<NodeId>,
        park_producers: &mut Vec<NodeId>,
        wrap_identifiers: bool,
    ) -> Slot<'a> {
        match part {
            ExpressionPart::ListLiteral(inner) => {
                let nested_id = self.schedule_list_literal(inner, scope);
                let pos = deps.len();
                deps.push(nested_id);
                Slot::Owned(pos)
            }
            ExpressionPart::DictLiteral(inner) => {
                let nested_id = self.schedule_dict_literal(inner, scope);
                let pos = deps.len();
                deps.push(nested_id);
                Slot::Owned(pos)
            }
            ExpressionPart::RecordLiteral(inner) => {
                let nested_id = self.schedule_record_literal(inner, scope);
                let pos = deps.len();
                deps.push(nested_id);
                Slot::Owned(pos)
            }
            ExpressionPart::Expression(boxed) => {
                let sub_id = self.add(NodeWork::dispatch(*boxed), scope);
                let pos = deps.len();
                deps.push(sub_id);
                Slot::Owned(pos)
            }
            ref p @ ExpressionPart::Identifier(_) if wrap_identifiers => {
                self.resolve_aggregate_bare_name(p, scope, deps, park_producers)
            }
            ref p @ ExpressionPart::Type(_) if wrap_identifiers => {
                self.resolve_aggregate_bare_name(p, scope, deps, park_producers)
            }
            other => Slot::Static(other.resolve()),
        }
    }

    /// Shared eager-resolve for the Identifier and leaf-Type branches. Unbound /
    /// ProducerErrored / Cycle outcomes fall back to a sub-Dispatch so the
    /// `BareIdentifier` fast lane's error path (and the Combine's dep-error
    /// short-circuit) handles them uniformly.
    fn resolve_aggregate_bare_name(
        &mut self,
        part: &ExpressionPart<'a>,
        scope: &'a Scope<'a>,
        deps: &mut Vec<NodeId>,
        park_producers: &mut Vec<NodeId>,
    ) -> Slot<'a> {
        match resolve_name_part(scope, part, self, None) {
            NameOutcome::Resolved(obj) => Slot::Static(obj.deep_clone()),
            NameOutcome::Parked(producer) => {
                let pos = park_producers.len();
                park_producers.push(producer);
                Slot::Park(pos)
            }
            NameOutcome::Unbound(_) | NameOutcome::ProducerErrored(_) | NameOutcome::Cycle(_) => {
                let expr =
                    crate::machine::model::ast::KExpression::new(vec![Spanned::bare(part.clone())]);
                let sub_id = self.add(NodeWork::dispatch(expr), scope);
                let pos = deps.len();
                deps.push(sub_id);
                Slot::Owned(pos)
            }
        }
    }
}
