use std::collections::HashMap;

use crate::machine::core::source::Spanned;
use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::{Carried, Held, KKey, KObject, Record, Serializable};
use crate::machine::{KError, KErrorKind, NameOutcome, NodeId, TraceFrame};

use super::super::outcome::Outcome;
use super::super::runtime::KoanRuntime;
use super::super::DepFinish;
use super::ctx::{current_scope, SchedulerView};
use super::resolve_name_part;

/// One element of a list literal or one side of a dict-literal pair. Indices are into the
/// the dep-finish's results: `Park(i)` reads position `i` of the park-producer prefix; `Owned(j)`
/// reads position `park_count + j` of the owned-sub suffix.
enum Slot<'run> {
    Static(Held<'run>),
    Park(usize),
    Owned(usize),
}

impl<'run> Slot<'run> {
    /// Append `id` as an owned sub-dependency and return the `Owned` slot that reads its result.
    /// The one place the "push a sub-dispatch, point a slot at it" tail lives.
    fn owned(deps: &mut Vec<NodeId>, id: NodeId) -> Self {
        let pos = deps.len();
        deps.push(id);
        Slot::Owned(pos)
    }

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

/// Allocate `obj` in the executing slot's arena and wrap it as a successful combine result — the
/// shared tail of every aggregate-literal finish.
fn done_object<'run>(view: &SchedulerView<'run, '_>, obj: KObject<'run>) -> Outcome<'run, 'run> {
    Outcome::Done(Ok(Carried::Object(
        view.current_scope().arena.alloc_object(obj),
    )))
}

impl<'run> KoanRuntime<'run> {
    /// Schedule a list-literal materialization as a dep-finish over its element producers.
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
        let finish: DepFinish<'run> = Box::new(move |_sched, results| {
            let items: Vec<Held<'run>> = layout
                .into_iter()
                .map(|slot| slot.materialize(results, park_count))
                .collect();
            done_object(_sched, KObject::list_of_held(items))
        });
        self.submit_dep_finish_in_own_scope(deps, park_producers, finish)
    }

    /// Schedule a dict-literal materialization as a dep-finish over its key/value producers.
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
        let finish: DepFinish<'run> = Box::new(move |_sched, results| {
            let mut map: HashMap<Box<dyn Serializable<'run> + 'run>, Held<'run>> = HashMap::new();
            for (k_slot, v_slot) in layout {
                let key_held = k_slot.materialize(results, park_count);
                let value_held = v_slot.materialize(results, park_count);
                // Keys stay scalar: only a value can be a `KKey`, never a first-class type.
                let key_obj = match key_held.as_object() {
                    Some(obj) => obj,
                    None => {
                        return Outcome::Done(Err(KError::new(KErrorKind::ShapeError(
                            "dict key must be a value, not a type".to_string(),
                        ))
                        .with_frame(frame_label())))
                    }
                };
                let kkey = match KKey::try_from_kobject(key_obj) {
                    Ok(k) => k,
                    Err(msg) => {
                        return Outcome::Done(Err(
                            KError::new(KErrorKind::ShapeError(msg)).with_frame(frame_label())
                        ))
                    }
                };
                map.insert(Box::new(kkey), value_held);
            }
            done_object(_sched, KObject::dict_of_held(map))
        });
        self.submit_dep_finish_in_own_scope(deps, park_producers, finish)
    }

    /// Schedule a record-literal materialization (`{x = 1, y = "a"}`). Field *names* are literal
    /// schema keys (never resolved); field *values* are name-resolved like dict values. Materializes
    /// a `KObject::Record`, which memoizes the per-field type record at construction.
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
        let finish: DepFinish<'run> = Box::new(move |_sched, results| {
            let record: Record<Held<'run>> = names
                .into_iter()
                .zip(layout)
                .map(|(name, slot)| (name, slot.materialize(results, park_count)))
                .collect();
            done_object(_sched, KObject::record_of_held(record))
        });
        self.submit_dep_finish_in_own_scope(deps, park_producers, finish)
    }

    /// Plan one slot of a list / dict literal. The cycle check in the bare-name path is
    /// suppressed (`consumer = None` to `resolve_name_part`) because the dep-finish slot
    /// does not yet exist; cycles are caught post-submission against the dep-finish ID.
    fn classify_aggregate_part(
        &mut self,
        part: ExpressionPart<'run>,
        deps: &mut Vec<NodeId>,
        park_producers: &mut Vec<NodeId>,
        wrap_identifiers: bool,
    ) -> Slot<'run> {
        match part {
            ExpressionPart::ListLiteral(inner) => {
                Slot::owned(deps, self.schedule_list_literal(inner))
            }
            ExpressionPart::DictLiteral(inner) => {
                Slot::owned(deps, self.schedule_dict_literal(inner))
            }
            ExpressionPart::RecordLiteral(inner) => {
                Slot::owned(deps, self.schedule_record_literal(inner))
            }
            ExpressionPart::Expression(boxed) => {
                Slot::owned(deps, self.dispatch_in_own_scope(*boxed))
            }
            ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
                // A `:(...)` / `:{…}` type value is a type-context sub-Dispatch to a
                // `KTypeValue`, like the keyworded eager-subs path — it cannot `resolve()`.
                let wrapped =
                    crate::machine::model::ast::KExpression::new(vec![Spanned::bare(part)]);
                Slot::owned(deps, self.dispatch_in_own_scope(wrapped))
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
    /// `BareIdentifier` fast lane's error path (and the dep-finish's dep-error
    /// short-circuit) handles them uniformly.
    fn resolve_aggregate_bare_name(
        &mut self,
        part: &ExpressionPart<'run>,
        deps: &mut Vec<NodeId>,
        park_producers: &mut Vec<NodeId>,
    ) -> Slot<'run> {
        let active_chain = self.ambient.active_payload().map(|p| &p.chain);
        match resolve_name_part(current_scope(&self.ambient), part, &self.sched, active_chain, None)
        {
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
                Slot::owned(deps, self.dispatch_in_own_scope(expr))
            }
        }
    }
}
