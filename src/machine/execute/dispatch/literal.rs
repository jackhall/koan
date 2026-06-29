use std::collections::HashMap;

use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::values::CarriedFamily;
use crate::machine::model::{Carried, Held, KKey, KObject, Record, Serializable};
use crate::machine::{
    FrameSet, KError, KErrorKind, KoanRegion, NameOutcome, NodeId, TraceFrame,
    ValueCarrierResolution,
};
use crate::source::Spanned;
use crate::witnessed::{reattachable, Sealed, Witnessed};

use super::super::outcome::DepTerminal;
use super::super::runtime::KoanRuntime;
use super::super::WitnessedDepFinish;
use super::ctx::{with_current_node_scope, SchedulerView};
use super::resolve_name_part;

/// Build-time accumulator family for an aggregate fold: the destination region plus the partial cell
/// vector. Each cell carrier is `transfer_into`-folded in — relocating the value and unioning its
/// reach onto the accumulator's witness — then the final `map` allocates the aggregate from the
/// region. Layout-invariant in `'r`: a thin region pointer and a `Vec` of layout-invariant cells.
struct AggBuildFamily;
reattachable!(AggBuildFamily => (&'r KoanRegion, Vec<Held<'r>>));

/// One cell of a list / dict / record literal. A `Static` cell is wrapped into a witnessed carrier
/// **at its source** (when the literal is classified), so the layout is lifetime-free and every cell
/// — static or dep — folds uniformly. `Park(i)` / `Owned(j)` index the dep-finish's resolved
/// terminals: position `i` of the park prefix, or `park_count + j` of the owned suffix.
enum Slot {
    Static(Sealed<CarriedFamily, FrameSet>),
    Park(usize),
    Owned(usize),
}

impl Slot {
    /// Append `id` as an owned sub-dependency and return the `Owned` slot that reads its result.
    fn owned(deps: &mut Vec<NodeId>, id: NodeId) -> Self {
        let pos = deps.len();
        deps.push(id);
        Slot::Owned(pos)
    }
}

/// The per-cell carrier the fold consumes: a static cell's source-built carrier, or a dep terminal's
/// own `Sealed` carrier (arriving witnessed from the lift, un-relocated — `transfer_into` relocates it
/// once into the aggregate's region while unioning its reach onto the carrier). The dep arm hands back
/// a [`duplicate`](crate::witnessed::Sealed::duplicate) of the terminal's carrier, never a fresh
/// `Witnessed::new` over the read-out value + a separately-read reach.
fn cell_carrier(
    slot: Slot,
    terminals: &[&DepTerminal<'_>],
    park_count: usize,
) -> Sealed<CarriedFamily, FrameSet> {
    match slot {
        Slot::Static(sealed) => sealed,
        Slot::Park(i) => terminals[i].carrier.duplicate(),
        Slot::Owned(j) => terminals[park_count + j].carrier.duplicate(),
    }
}

/// Fold a sequence of cell carriers into a witnessed `(region, Vec<Held>)` accumulator over the
/// consumer scope's region: `yoke` an empty accumulator under the consumer frame, then
/// `transfer_into` each cell so the result names the union of every cell's reach. The final aggregate
/// shape (`list_of_held` / `dict_of_held` / `record_of_held`) is built by the caller's `map`.
fn fold_cells(
    view: &SchedulerView<'_, '_>,
    cells: impl Iterator<Item = Sealed<CarriedFamily, FrameSet>>,
    capacity: usize,
) -> Witnessed<AggBuildFamily, FrameSet> {
    let dest_frame = view
        .current_scope()
        .region_owner()
        .upgrade()
        .expect("the consumer scope's region owner is held for the step");
    let acc0 =
        Witnessed::<AggBuildFamily, FrameSet>::yoke(FrameSet::singleton(dest_frame), |region| {
            (region, Vec::with_capacity(capacity))
        });
    cells.fold(acc0, |acc, cell| {
        cell.transfer_into::<AggBuildFamily, AggBuildFamily>(
            acc,
            |cell, (region, mut cells), _brand| {
                cells.push(Held::from_carried(cell));
                (region, cells)
            },
        )
        .expect("a FrameSet set witness always represents the union")
    })
}

/// Read a dict key cell as a scalar [`KKey`]: a key is never folded (it is a scalar, reaching no
/// region), so it is read out and converted in place. A `Type` arm or a non-scalar value errors.
fn scalar_key(
    slot: &Slot,
    terminals: &[&DepTerminal<'_>],
    park_count: usize,
) -> Result<KKey, String> {
    match slot {
        Slot::Static(sealed) => sealed.open(key_from_carried),
        Slot::Park(i) => key_from_carried(terminals[*i].value),
        Slot::Owned(j) => key_from_carried(terminals[park_count + *j].value),
    }
}

fn key_from_carried(c: Carried<'_>) -> Result<KKey, String> {
    match c {
        Carried::Object(o) => KKey::try_from_kobject(o),
        Carried::Type(_) => Err("dict key must be a value, not a type".to_string()),
    }
}

impl<'step> KoanRuntime<'step> {
    /// Schedule a list-literal materialization as a witnessed dep-finish over its element producers.
    pub(in crate::machine::execute) fn schedule_list_literal<'a>(
        &mut self,
        items: Vec<ExpressionPart<'a>>,
    ) -> NodeId {
        let mut layout: Vec<Slot> = Vec::with_capacity(items.len());
        let mut deps: Vec<NodeId> = Vec::new();
        let mut park_producers: Vec<NodeId> = Vec::new();
        for part in items {
            // List elements are not name-resolved; bare identifiers stay `Static`.
            let slot = self.classify_aggregate_part(part, &mut deps, &mut park_producers, false);
            layout.push(slot);
        }
        let park_count = park_producers.len();
        let finish: WitnessedDepFinish<'a> = Box::new(move |view, terminals| {
            let n = layout.len();
            let cells = layout
                .into_iter()
                .map(|slot| cell_carrier(slot, terminals, park_count));
            let acc = fold_cells(view, cells, n);
            Ok(acc.map(|(region, cells), _brand| {
                Carried::Object(region.alloc_object(KObject::list_of_held(cells)))
            }))
        });
        self.submit_dep_finish_witnessed_in_own_scope(deps, park_producers, finish)
    }

    /// Schedule a dict-literal materialization as a witnessed dep-finish over its key/value producers.
    /// Bare identifiers on either side are name-resolved (Python-like: keys are expressions, not
    /// symbols). Non-scalar keys produce `KErrorKind::ShapeError`, raised before the value fold.
    pub(in crate::machine::execute) fn schedule_dict_literal<'a>(
        &mut self,
        pairs: Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>,
    ) -> NodeId {
        let mut layout: Vec<(Slot, Slot)> = Vec::with_capacity(pairs.len());
        let mut deps: Vec<NodeId> = Vec::new();
        let mut park_producers: Vec<NodeId> = Vec::new();
        for (k, v) in pairs {
            let key_slot = self.classify_aggregate_part(k, &mut deps, &mut park_producers, true);
            let val_slot = self.classify_aggregate_part(v, &mut deps, &mut park_producers, true);
            layout.push((key_slot, val_slot));
        }
        let park_count = park_producers.len();
        let finish: WitnessedDepFinish<'a> = Box::new(move |view, terminals| {
            let frame_label = || TraceFrame::bare("<dict>", "dict literal");
            let n = layout.len();
            // Keys stay scalar (reaching no region): read them out eagerly, erroring before the fold.
            // The value cells fold into the witnessed accumulator, paired back with the keys at `map`.
            let mut keys: Vec<KKey> = Vec::with_capacity(n);
            let mut value_cells: Vec<Sealed<CarriedFamily, FrameSet>> = Vec::with_capacity(n);
            for (k_slot, v_slot) in layout {
                let kkey = scalar_key(&k_slot, terminals, park_count).map_err(|msg| {
                    KError::new(KErrorKind::ShapeError(msg)).with_frame(frame_label())
                })?;
                keys.push(kkey);
                value_cells.push(cell_carrier(v_slot, terminals, park_count));
            }
            let acc = fold_cells(view, value_cells.into_iter(), n);
            Ok(acc.map(|(region, value_helds), _brand| {
                let map: HashMap<Box<dyn Serializable + '_>, Held<'_>> = keys
                    .into_iter()
                    .zip(value_helds)
                    .map(|(k, v)| (Box::new(k) as Box<dyn Serializable + '_>, v))
                    .collect();
                Carried::Object(region.alloc_object(KObject::dict_of_held(map)))
            }))
        });
        self.submit_dep_finish_witnessed_in_own_scope(deps, park_producers, finish)
    }

    /// Schedule a record-literal materialization (`{x = 1, y = "a"}`). Field *names* are literal
    /// schema keys (never resolved); field *values* are name-resolved like dict values. Materializes
    /// a `KObject::Record`, which memoizes the per-field type record at construction.
    pub(in crate::machine::execute) fn schedule_record_literal<'a>(
        &mut self,
        fields: Vec<(String, ExpressionPart<'a>)>,
    ) -> NodeId {
        let mut names: Vec<String> = Vec::with_capacity(fields.len());
        let mut layout: Vec<Slot> = Vec::with_capacity(fields.len());
        let mut deps: Vec<NodeId> = Vec::new();
        let mut park_producers: Vec<NodeId> = Vec::new();
        for (name, value) in fields {
            let val_slot =
                self.classify_aggregate_part(value, &mut deps, &mut park_producers, true);
            names.push(name);
            layout.push(val_slot);
        }
        let park_count = park_producers.len();
        let finish: WitnessedDepFinish<'a> = Box::new(move |view, terminals| {
            let n = layout.len();
            let cells = layout
                .into_iter()
                .map(|slot| cell_carrier(slot, terminals, park_count));
            let acc = fold_cells(view, cells, n);
            Ok(acc.map(|(region, value_helds), _brand| {
                let record: Record<Held<'_>> = names.into_iter().zip(value_helds).collect();
                Carried::Object(region.alloc_object(KObject::record_of_held(record)))
            }))
        });
        self.submit_dep_finish_witnessed_in_own_scope(deps, park_producers, finish)
    }

    /// Plan one slot of a list / dict literal. The cycle check in the bare-name path is suppressed
    /// (`consumer = None` to `resolve_name_part`) because the dep-finish slot does not yet exist;
    /// cycles are caught post-submission against the dep-finish ID.
    fn classify_aggregate_part<'a>(
        &mut self,
        part: ExpressionPart<'a>,
        deps: &mut Vec<NodeId>,
        park_producers: &mut Vec<NodeId>,
        wrap_identifiers: bool,
    ) -> Slot {
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
                // `Carried::Type`, like the keyworded eager-subs path — it cannot `resolve()`.
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
            other => {
                // A static literal (keyword / bare identifier / type name / literal): region-pure owned
                // data, so the cell is built **inside** the witness closure — `yoke`d into the classify
                // scope's frame, born co-located with that frame as its reach rather than resolved at
                // the ambient lifetime and bundled via `Witnessed::new`. The cell is then lifetime-free
                // and folds uniformly with the dep cells.
                let frame = with_current_node_scope(&self.ambient, |s| {
                    s.region_owner()
                        .upgrade()
                        .expect("the classify scope's region owner is held for the step")
                });
                let carrier =
                    KoanRegion::alloc_witnessed(FrameSet::singleton(frame), move |region| {
                        Carried::Object(region.alloc_object(other.resolve_region_pure()))
                    });
                Slot::Static(Sealed::seal(carrier))
            }
        }
    }

    /// Shared eager-resolve for the Identifier and leaf-Type branches. Unbound / ProducerErrored /
    /// Cycle outcomes fall back to a sub-Dispatch so the `BareIdentifier` fast lane's error path (and
    /// the dep-finish's dep-error short-circuit) handles them uniformly.
    fn resolve_aggregate_bare_name<'a>(
        &mut self,
        part: &ExpressionPart<'a>,
        deps: &mut Vec<NodeId>,
        park_producers: &mut Vec<NodeId>,
    ) -> Slot {
        let active_chain = self.ambient.active_payload().map(|p| &p.chain);
        // A value-bound Identifier element rides into the cell on a carrier witnessed by its binding
        // scope's home frame, which transitively pins that scope's reach-set — so the cell names the
        // value's reach by construction, never an asserted `Witnessed::new`. Type leaves and unbound /
        // pending names fall to the shared `resolve_name_part` path below.
        if let ExpressionPart::Identifier(name) = part {
            let resolved = with_current_node_scope(&self.ambient, |s| {
                s.resolve_value_carrier(name, active_chain.map(|c| &**c))
            });
            if let ValueCarrierResolution::Value(carrier) = resolved {
                return Slot::Static(Sealed::seal(carrier));
            }
        }
        // Resolve + seal inside the brand (the scope and its `NameOutcome` are branded); the rebuilt
        // owned `part_b` matches the scope's `'b`. The unbound / errored / cycle fallback needs
        // `&mut self`, so it runs after the read closes — `None` signals it.
        let resolved = with_current_node_scope(&self.ambient, |s| {
            let part_b = match part {
                ExpressionPart::Identifier(n) => ExpressionPart::Identifier(n.clone()),
                ExpressionPart::Type(t) => ExpressionPart::Type(t.clone()),
                _ => unreachable!("resolve_aggregate_bare_name only sees Identifier / Type parts"),
            };
            match resolve_name_part(s, &part_b, &self.sched, active_chain, None) {
                // A first-class **type** resolved into the cell rides the type channel sealed under the
                // classify scope's home frame, which pins the type's (ancestor) region via its `outer`
                // chain — `seal_type` yokes it (a `KType::Module` folds its child reach), co-located by
                // the brand, never an asserted bundle. The value case is handled above via the
                // binding-scope carrier, so this is reached only for a `Type` carrier.
                NameOutcome::Resolved(c) => Some(Slot::Static(Sealed::seal(s.seal_type(c)))),
                NameOutcome::Parked(producer) => {
                    let pos = park_producers.len();
                    park_producers.push(producer);
                    Some(Slot::Park(pos))
                }
                NameOutcome::Unbound(_)
                | NameOutcome::ProducerErrored(_)
                | NameOutcome::Cycle(_) => None,
            }
        });
        match resolved {
            Some(slot) => slot,
            None => {
                let expr =
                    crate::machine::model::ast::KExpression::new(vec![Spanned::bare(part.clone())]);
                Slot::owned(deps, self.dispatch_in_own_scope(expr))
            }
        }
    }
}
