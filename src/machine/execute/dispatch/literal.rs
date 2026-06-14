use std::collections::HashMap;

use crate::machine::core::source::Spanned;
use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::{Carried, Held, KKey, KObject, Record, Serializable};
use crate::machine::{KError, KErrorKind, NameOutcome, NodeId, TraceFrame};

use super::super::nodes::NodeOutput;
use super::super::outcome::Outcome;
use super::super::scheduler::Scheduler;
use super::super::CombineFinish;
use super::resolve_name_part;

/// One element of a list literal or one side of a dict-literal pair. Indices are into the
/// Combine's results: `Park(i)` reads position `i` of the park-producer prefix; `Owned(j)`
/// reads position `park_count + j` of the owned-sub suffix.
enum Slot<'run> {
    Static(Held<'run>),
    Park(usize),
    Owned(usize),
}

impl<'run> Slot<'run> {
    /// Deep-clones `Park` / `Owned` results because the produced container owns its cells
    /// and can't borrow a `&'run` carrier into its `Rc<…>`. A literal element may be a runtime
    /// value *or* a first-class type, so each carrier widens to a [`Held`] cell.
    fn materialize(self, results: &[Carried<'run>], park_count: usize) -> Held<'run> {
        match self {
            Slot::Static(held) => held,
            Slot::Park(i) => Held::from_carried(results[i]),
            Slot::Owned(j) => Held::from_carried(results[park_count + j]),
        }
    }
}

impl<'run> Scheduler<'run> {
    pub(in crate::machine::execute) fn schedule_list_literal(
        &mut self,
        items: Vec<ExpressionPart<'run>>,
    ) -> NodeId {
        let mut layout: Vec<Slot<'run>> = Vec::with_capacity(items.len());
        let mut deps: Vec<NodeId> = Vec::new();
        let mut park_producers: Vec<NodeId> = Vec::new();
        for part in items {
            // List elements are not name-resolved; bare identifiers stay `Static`.
            let slot = self.classify_aggregate_part(part, &mut deps, &mut park_producers, false);
            layout.push(slot);
        }
        let park_count = park_producers.len();
        let finish: CombineFinish<'run> = Box::new(move |_sched, results| {
            let items: Vec<Held<'run>> = layout
                .into_iter()
                .map(|slot| slot.materialize(results, park_count))
                .collect();
            let allocated: &'run KObject<'run> = _sched
                .current_scope()
                .arena
                .alloc_object(KObject::list_of_held(items));
            Outcome::Done(NodeOutput::Value(Carried::Object(allocated)))
        });
        self.combine_here(deps, park_producers, finish)
    }

    /// Bare identifiers on either side are name-resolved (Python-like: keys are
    /// expressions, not symbols). Non-scalar keys produce `KErrorKind::ShapeError` at
    /// finish-time via the `KKey` conversion.
    pub(in crate::machine::execute) fn schedule_dict_literal(
        &mut self,
        pairs: Vec<(ExpressionPart<'run>, ExpressionPart<'run>)>,
    ) -> NodeId {
        let mut layout: Vec<(Slot<'run>, Slot<'run>)> = Vec::with_capacity(pairs.len());
        let mut deps: Vec<NodeId> = Vec::new();
        let mut park_producers: Vec<NodeId> = Vec::new();
        for (k, v) in pairs {
            let key_slot = self.classify_aggregate_part(k, &mut deps, &mut park_producers, true);
            let val_slot = self.classify_aggregate_part(v, &mut deps, &mut park_producers, true);
            layout.push((key_slot, val_slot));
        }
        let frame_label = || TraceFrame::bare("<dict>", "dict literal");
        let park_count = park_producers.len();
        let finish: CombineFinish<'run> = Box::new(move |_sched, results| {
            let mut map: HashMap<Box<dyn Serializable<'run> + 'run>, Held<'run>> = HashMap::new();
            for (k_slot, v_slot) in layout {
                let key_held = k_slot.materialize(results, park_count);
                let value_held = v_slot.materialize(results, park_count);
                // Keys stay scalar: only a value can be a `KKey`, never a first-class type.
                let key_obj = match key_held.as_object() {
                    Some(obj) => obj,
                    None => {
                        return Outcome::Done(NodeOutput::Err(
                            KError::new(KErrorKind::ShapeError(
                                "dict key must be a value, not a type".to_string(),
                            ))
                            .with_frame(frame_label()),
                        ))
                    }
                };
                let kkey = match KKey::try_from_kobject(key_obj) {
                    Ok(k) => k,
                    Err(msg) => {
                        return Outcome::Done(NodeOutput::Err(
                            KError::new(KErrorKind::ShapeError(msg)).with_frame(frame_label()),
                        ))
                    }
                };
                map.insert(Box::new(kkey), value_held);
            }
            let allocated: &'run KObject<'run> = _sched
                .current_scope()
                .arena
                .alloc_object(KObject::dict_of_held(map));
            Outcome::Done(NodeOutput::Value(Carried::Object(allocated)))
        });
        self.combine_here(deps, park_producers, finish)
    }

    /// Record literal (`{x = 1, y = "a"}`). Field *names* are literal schema keys (never
    /// resolved); field *values* are name-resolved like dict values. Materializes a
    /// `KObject::Record`, which memoizes the per-field type record at construction.
    pub(in crate::machine::execute) fn schedule_record_literal(
        &mut self,
        fields: Vec<(String, ExpressionPart<'run>)>,
    ) -> NodeId {
        let mut names: Vec<String> = Vec::with_capacity(fields.len());
        let mut layout: Vec<Slot<'run>> = Vec::with_capacity(fields.len());
        let mut deps: Vec<NodeId> = Vec::new();
        let mut park_producers: Vec<NodeId> = Vec::new();
        for (name, value) in fields {
            let val_slot =
                self.classify_aggregate_part(value, &mut deps, &mut park_producers, true);
            names.push(name);
            layout.push(val_slot);
        }
        let park_count = park_producers.len();
        let finish: CombineFinish<'run> = Box::new(move |_sched, results| {
            let record: Record<Held<'run>> = names
                .into_iter()
                .zip(layout)
                .map(|(name, slot)| (name, slot.materialize(results, park_count)))
                .collect();
            let allocated: &'run KObject<'run> = _sched
                .current_scope()
                .arena
                .alloc_object(KObject::record_of_held(record));
            Outcome::Done(NodeOutput::Value(Carried::Object(allocated)))
        });
        self.combine_here(deps, park_producers, finish)
    }

    /// Plan one slot of a list / dict literal. The cycle check in the bare-name path is
    /// suppressed (`consumer = None` to `resolve_name_part`) because the Combine slot
    /// does not yet exist; cycles are caught post-submission against the Combine ID.
    fn classify_aggregate_part(
        &mut self,
        part: ExpressionPart<'run>,
        deps: &mut Vec<NodeId>,
        park_producers: &mut Vec<NodeId>,
        wrap_identifiers: bool,
    ) -> Slot<'run> {
        match part {
            ExpressionPart::ListLiteral(inner) => {
                let nested_id = self.schedule_list_literal(inner);
                let pos = deps.len();
                deps.push(nested_id);
                Slot::Owned(pos)
            }
            ExpressionPart::DictLiteral(inner) => {
                let nested_id = self.schedule_dict_literal(inner);
                let pos = deps.len();
                deps.push(nested_id);
                Slot::Owned(pos)
            }
            ExpressionPart::RecordLiteral(inner) => {
                let nested_id = self.schedule_record_literal(inner);
                let pos = deps.len();
                deps.push(nested_id);
                Slot::Owned(pos)
            }
            ExpressionPart::Expression(boxed) => {
                let sub_id = self.dispatch_here(*boxed);
                let pos = deps.len();
                deps.push(sub_id);
                Slot::Owned(pos)
            }
            ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
                // A `:(...)` / `:{…}` type value is a type-context sub-Dispatch to a
                // `KTypeValue`, like the keyworded eager-subs path — it cannot `resolve()`.
                let wrapped =
                    crate::machine::model::ast::KExpression::new(vec![Spanned::bare(part)]);
                let sub_id = self.dispatch_here(wrapped);
                let pos = deps.len();
                deps.push(sub_id);
                Slot::Owned(pos)
            }
            ref p @ ExpressionPart::Identifier(_) if wrap_identifiers => {
                self.resolve_aggregate_bare_name(p, deps, park_producers)
            }
            ref p @ ExpressionPart::Type(_) if wrap_identifiers => {
                self.resolve_aggregate_bare_name(p, deps, park_producers)
            }
            other => Slot::Static(Held::Object(other.resolve())),
        }
    }

    /// Shared eager-resolve for the Identifier and leaf-Type branches. Unbound /
    /// ProducerErrored / Cycle outcomes fall back to a sub-Dispatch so the
    /// `BareIdentifier` fast lane's error path (and the Combine's dep-error
    /// short-circuit) handles them uniformly.
    fn resolve_aggregate_bare_name(
        &mut self,
        part: &ExpressionPart<'run>,
        deps: &mut Vec<NodeId>,
        park_producers: &mut Vec<NodeId>,
    ) -> Slot<'run> {
        match resolve_name_part(self.current_scope(), part, self, None) {
            // An aggregate literal element may resolve to a value or a first-class type;
            // both ride into the cell as a `Held`.
            NameOutcome::Resolved(c) => Slot::Static(Held::from_carried(c)),
            NameOutcome::Parked(producer) => {
                let pos = park_producers.len();
                park_producers.push(producer);
                Slot::Park(pos)
            }
            NameOutcome::Unbound(_) | NameOutcome::ProducerErrored(_) | NameOutcome::Cycle(_) => {
                let expr =
                    crate::machine::model::ast::KExpression::new(vec![Spanned::bare(part.clone())]);
                let sub_id = self.dispatch_here(expr);
                let pos = deps.len();
                deps.push(sub_id);
                Slot::Owned(pos)
            }
        }
    }
}
