use std::collections::HashMap;

use crate::machine::core::source::Spanned;
use crate::machine::model::{KKey, KObject, Serializable};
use crate::machine::{BodyResult, CombineFinish, Frame, KError, KErrorKind, NodeId, Scope};
use crate::machine::model::ast::ExpressionPart;

use super::dispatch::{resolve_name_part, NameOutcome};
use super::super::nodes::NodeWork;
use super::Scheduler;

/// One element of a list literal or one side of a dict-literal pair, captured by the
/// `Combine` closure. `Static` carries an already-resolved value (e.g. a literal scalar);
/// `Park(i)` indexes into the Combine's park-producer prefix (final position `i`); and
/// `Owned(j)` indexes into the owned-sub suffix (final position `park_count + j`). Kept
/// private to the planner — the scheduler doesn't see it.
enum Slot<'a> {
    Static(KObject<'a>),
    Park(usize),
    Owned(usize),
}

impl<'a> Slot<'a> {
    /// Materialize this slot into an owned `KObject` for the literal under construction.
    /// `Park` / `Owned` results are deep-cloned because the resulting `KList` / `KDict`
    /// owns its elements (you can't store `&'a KObject` into `Rc<Vec<KObject>>`).
    /// Infallible: `run_combine` short-circuits on errored deps before invoking the
    /// closure.
    fn materialize(self, results: &[&'a KObject<'a>], park_count: usize) -> KObject<'a> {
        match self {
            Slot::Static(obj) => obj,
            Slot::Park(i) => results[i].deep_clone(),
            Slot::Owned(j) => results[park_count + j].deep_clone(),
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
        let mut park_producers: Vec<NodeId> = Vec::new();
        for part in items {
            // Bare `Identifier` parts in a list stay `Static` (parser surfaces them as
            // already-resolved values via `resolve()`); list elements are not
            // name-resolved like dict keys/values are.
            let slot = self.classify_aggregate_part(
                part,
                scope,
                &mut deps,
                &mut park_producers,
                false,
            );
            layout.push(slot);
        }
        let park_count = park_producers.len();
        let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
            let items: Vec<KObject<'a>> = layout
                .into_iter()
                .map(|slot| slot.materialize(results, park_count))
                .collect();
            let allocated: &'a KObject<'a> =
                scope.arena.alloc(KObject::list(items));
            BodyResult::Value(allocated)
        });
        self.add_combine(deps, park_producers, scope, finish)
    }

    /// Schedule a dict literal as a `Combine`. Bare identifiers on either side route
    /// through the shared `resolve_name_part` helper (Python-like name resolution applies
    /// to both keys and values): a value-side hit is captured as a `Slot::Static`; a
    /// still-pending placeholder is wired as a park-producer on the Combine slot; an
    /// unbound name falls back to a sub-Dispatch so the receiving `value_lookup` body
    /// surfaces `UnboundName` through the Combine's dep-error short-circuit. The closure
    /// performs `KKey` conversion on each key — non-scalar keys produce
    /// `KErrorKind::ShapeError`.
    pub(super) fn schedule_dict_literal(
        &mut self,
        pairs: Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>,
        scope: &'a Scope<'a>,
    ) -> NodeId {
        let mut layout: Vec<(Slot<'a>, Slot<'a>)> = Vec::with_capacity(pairs.len());
        let mut deps: Vec<NodeId> = Vec::new();
        let mut park_producers: Vec<NodeId> = Vec::new();
        for (k, v) in pairs {
            let key_slot = self.classify_aggregate_part(
                k,
                scope,
                &mut deps,
                &mut park_producers,
                true,
            );
            let val_slot = self.classify_aggregate_part(
                v,
                scope,
                &mut deps,
                &mut park_producers,
                true,
            );
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
                    Err(msg) => return BodyResult::Err(
                        KError::new(KErrorKind::ShapeError(msg)).with_frame(frame_label()),
                    ),
                };
                map.insert(Box::new(kkey), value_obj);
            }
            let allocated: &'a KObject<'a> =
                scope.arena.alloc(KObject::dict(map));
            BodyResult::Value(allocated)
        });
        self.add_combine(deps, park_producers, scope, finish)
    }

    /// Plan one slot of a list / dict literal: nested literals recurse via their own
    /// schedulers, `Expression` parts spawn sub-Dispatches, and (when `wrap_identifiers`
    /// is set) bare-name parts route through the shared eager-resolve helper —
    /// `Resolved` is captured as a `Slot::Static`, `Parked` adds the producer to the
    /// Combine's `park_producers` and the slot's value is read at finish via the dep
    /// position. The cycle check is suppressed (`consumer = None`) because the Combine
    /// slot does not yet exist; cycles are detectable post-submission against the
    /// pre-existing Combine ID.
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
            ExpressionPart::Expression(boxed) => {
                let sub_id = self.add(NodeWork::dispatch(*boxed), scope);
                let pos = deps.len();
                deps.push(sub_id);
                Slot::Owned(pos)
            }
            ref p @ ExpressionPart::Identifier(_) if wrap_identifiers => {
                // Eager-resolve a bare Identifier dict key/value: a `Resolved` hit
                // captures the carrier directly as a static; a `Parked` outcome attaches
                // a park producer on the Combine; `Unbound` falls back to a sub-Dispatch
                // so the receiving `value_lookup` body surfaces the `UnboundName` error
                // through the Combine's dep-error short-circuit. `ProducerErrored` and
                // `Cycle` fall back the same way — the sub-Dispatch path's terminalized
                // error / cycle detection still applies.
                self.resolve_aggregate_bare_name(p, scope, deps, park_producers)
            }
            ref p @ ExpressionPart::Type(ref t)
                if wrap_identifiers
                    && matches!(t.params, crate::machine::model::ast::TypeParams::None) =>
            {
                // Eager-resolve a bare leaf Type-token dict key/value the same way
                // `MAKESET IntOrd` resolves its wrap-slot in the dispatcher: a
                // `Resolved` hit produces the paired `KModule`/`KSignature` carrier;
                // forward references attach as park producers.
                self.resolve_aggregate_bare_name(p, scope, deps, park_producers)
            }
            other => Slot::Static(other.resolve()),
        }
    }

    /// Bare-name eager-resolve for `classify_aggregate_part`. Splits out so the
    /// Identifier and leaf-Type branches share their fallback shape (sub-Dispatch on
    /// Unbound / ProducerErrored / Cycle). The caller has already confirmed the part is
    /// a bare-name shape eligible for `resolve_name_part`. Slots route as `Park(i)` for
    /// the resolved-but-pending case (the Combine reads the producer's terminal at
    /// finish-time without owning the slot) and `Owned(j)` for the sub-Dispatch
    /// fallback (cascade-freed by the Combine's success path).
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
            NameOutcome::Unbound(_)
            | NameOutcome::ProducerErrored(_)
            | NameOutcome::Cycle(_) => {
                // Fall back to a sub-Dispatch so the existing error-propagation surface
                // (Combine's dep-error short-circuit, frame attachment) is preserved.
                let expr = crate::machine::model::ast::KExpression::new(vec![
                    Spanned::bare(part.clone()),
                ]);
                let sub_id = self.add(NodeWork::dispatch(expr), scope);
                let pos = deps.len();
                deps.push(sub_id);
                Slot::Owned(pos)
            }
        }
    }
}
