use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::{FoldingBrand, KoanRegionExt, KoanStorageProfile};
use crate::machine::model::CarriedFamily;
use crate::machine::model::ExpressionPart;
use crate::machine::model::{Carried, Held, KKey, KObject, Record, Serializable};
use crate::machine::{
    CarrierWitness, DeliveredCarried, KError, KErrorKind, KoanRegion, NameLookup, NameOutcome,
    NodeId, TraceFrame,
};
use crate::source::Spanned;
use crate::witnessed::{reattachable, Delivered, RegionHandle, Residence, Witnessed};

use super::super::outcome::DepTerminal;
use super::super::runtime::KoanRuntime;
use super::super::{StepCarried, WitnessedDepFinish};
use super::ctx::{current_dest_frame, with_current_node_scope, SchedulerView};
use super::resolve_name_part;
use crate::scheduler::{DepResults, ResolvedDeps};

/// Build-time accumulator family for an aggregate fold: the destination region plus the partial cell
/// vector. Each cell carrier is `transfer_into`-folded in — relocating the value and unioning its
/// reach onto the accumulator's witness — then the final `map` allocates the aggregate from the
/// region. Layout-invariant in `'r`: a thin region pointer and a `Vec` of layout-invariant cells.
struct AggBuildFamily;
reattachable!(AggBuildFamily => (RegionHandle<'r, KoanStorageProfile>, Vec<Held<'r>>));

/// One cell of a list / dict / record literal. A `Static` cell is wrapped into a delivery envelope
/// **at its source** (when the literal is classified), so the layout is lifetime-free and every cell
/// — static or dep — folds uniformly, each carrying the frame owner its value lives under. `Park(i)`
/// / `Owned(j)` are the dep-finish's park / owned [`DepResults`] indices — the
/// [`Deps`](crate::scheduler::Deps) builder hands them back at classify time and they read straight
/// back through the view.
enum Slot {
    Static(DeliveredCarried),
    Park(usize),
    Owned(usize),
}

impl Slot {
    /// Add `id` as an owned sub-dependency and return the `Owned` slot that reads its result.
    fn owned(deps: &mut ResolvedDeps, id: NodeId) -> Self {
        Slot::Owned(deps.own(id))
    }
}

/// The per-cell envelope the fold consumes: a static cell's source-built envelope, or a dep
/// terminal's own delivery envelope (arriving witnessed from the pull, un-relocated —
/// `transfer_into` relocates it once into the aggregate's region while minting its reach and
/// residence host onto the accumulator's carrier). The dep arm hands back a
/// [`duplicate`](crate::witnessed::Delivered::duplicate) of the terminal's envelope, never a fresh
/// bundle pairing the read-out value with a separately-read reach.
fn cell_carrier(slot: Slot, terminals: DepResults<'_, &DepTerminal<'_>>) -> DeliveredCarried {
    match slot {
        Slot::Static(delivered) => delivered,
        Slot::Park(i) => terminals.park(i).delivered.duplicate(),
        Slot::Owned(j) => terminals.owned(j).delivered.duplicate(),
    }
}

/// Fold a sequence of cell envelopes into a witnessed `(region, Vec<Held>)` accumulator over the
/// consumer scope's region: `yoke` an empty accumulator under the consumer frame, then
/// `transfer_into` each envelope at [`Residence::Copied`] — [`Held::from_carried`] deep-clones each
/// cell into the aggregate, so the producer host materializes as a member of the minted set only
/// when the copy's borrows genuinely reach it (a closure's captured environment), the same
/// copied-adoption rule the param binds apply; a residence-only producer releases at retention
/// discharge instead of riding the aggregate. The final aggregate shape (`list_of_held` /
/// `dict_of_held` / `record_of_held`) is built by the caller's pinned map.
fn fold_cells(
    view: &SchedulerView<'_, '_>,
    cells: impl Iterator<Item = DeliveredCarried>,
    capacity: usize,
) -> Witnessed<AggBuildFamily, CarrierWitness> {
    let dest_frame = view.dest_frame();
    let acc0 = KoanRegion::yoke_branded::<AggBuildFamily, _>(dest_frame, |region| {
        (region.handle(), Vec::with_capacity(capacity))
    });
    cells.fold(acc0, |acc, cell| {
        cell.transfer_into::<AggBuildFamily, AggBuildFamily, _>(
            acc,
            Residence::Copied,
            |cell, (region, mut cells), _brand| {
                cells.push(Held::from_carried(cell));
                (region, cells)
            },
        )
    })
}

/// Read a dict key cell as a scalar [`KKey`]: a key is never folded (it is a scalar, reaching no
/// region), so it is read out and converted in place. A `Type` arm or a non-scalar value errors.
fn scalar_key(slot: &Slot, terminals: DepResults<'_, &DepTerminal<'_>>) -> Result<KKey, String> {
    match slot {
        Slot::Static(delivered) => delivered.open(key_from_carried),
        Slot::Park(i) => key_from_carried(terminals.park(*i).value),
        Slot::Owned(j) => key_from_carried(terminals.owned(*j).value),
    }
}

fn key_from_carried(c: Carried<'_>) -> Result<KKey, String> {
    match c {
        Carried::Object(o) => KKey::try_from_kobject(o),
        Carried::Type(_) => Err("dict key must be a value, not a type".to_string()),
    }
}

/// One layout row of an aggregate literal: the value cell, plus — for a dict — the key slot resolved
/// to a scalar [`KKey`] at finish time (list and record rows carry no key).
struct AggRow {
    key: Option<Slot>,
    value: Slot,
}

/// Finish-side assemble hook: the resolved keys (empty unless the rows carry key slots) and the folded
/// value cells become the aggregate object. Boxed higher-ranked so the record variant captures its
/// field names and each shape builds its own `KObject` at the fold brand.
type AggAssemble = Box<dyn for<'r> FnOnce(Vec<KKey>, Vec<Held<'r>>) -> KObject<'r>>;

impl<'step> KoanRuntime<'step> {
    /// The one scheduling path behind the three aggregate literals: park a witnessed dep-finish on
    /// `deps`; on resolve, read each row's key (a non-scalar dict key errors before the fold, under the
    /// dict-literal frame — only a dict row carries a key slot), fold the value cells into the consumer
    /// region, and `assemble` the aggregate inside the witness closure so it names every region it
    /// reaches by construction.
    fn schedule_aggregate(
        &mut self,
        deps: ResolvedDeps,
        rows: Vec<AggRow>,
        assemble: AggAssemble,
    ) -> NodeId {
        let finish: WitnessedDepFinish<'step> = Box::new(move |view, terminals| {
            let n = rows.len();
            // Keys stay scalar (reaching no region): read them out eagerly, erroring before the fold.
            // The value cells fold into the witnessed accumulator, paired back with the keys at `map`.
            let mut keys: Vec<KKey> = Vec::new();
            let mut cells: Vec<DeliveredCarried> = Vec::with_capacity(n);
            for row in rows {
                if let Some(key_slot) = row.key {
                    let kkey = scalar_key(&key_slot, terminals).map_err(|msg| {
                        KError::new(KErrorKind::ShapeError(msg))
                            .with_frame(TraceFrame::bare("<dict>", "dict literal"))
                    })?;
                    keys.push(kkey);
                }
                cells.push(cell_carrier(row.value, terminals));
            }
            let acc = fold_cells(view, cells.into_iter(), n);
            // The pin: the destination frame, whose arena holds the set the folds minted — through
            // it every producer the accumulated `Held` views point into.
            let dest_frame = view.dest_frame();
            Ok(StepCarried::born(
                acc.map_pinned_placing::<CarriedFamily, KoanStorageProfile, _>(
                    &dest_frame,
                    move |(_region, value_helds), placement| {
                        let region = FoldingBrand::in_fold_closure(placement);
                        Carried::Object(region.alloc_object_folded(assemble(keys, value_helds)))
                    },
                ),
            ))
        });
        self.submit_dep_finish_witnessed_in_own_scope(deps, finish)
    }

    /// Schedule a list-literal materialization as a witnessed dep-finish over its element producers.
    pub(in crate::machine::execute) fn schedule_list_literal<'a>(
        &mut self,
        items: Vec<ExpressionPart<'a>>,
    ) -> NodeId {
        let mut deps = ResolvedDeps::new();
        let mut rows = Vec::with_capacity(items.len());
        for part in items {
            // List elements are not name-resolved; bare identifiers stay `Static`.
            let value = self.classify_aggregate_part(part, &mut deps, false);
            rows.push(AggRow { key: None, value });
        }
        self.schedule_aggregate(
            deps,
            rows,
            Box::new(|_keys, cells| KObject::list_of_held(cells)),
        )
    }

    /// Schedule a dict-literal materialization as a witnessed dep-finish over its key/value producers.
    /// Bare identifiers on either side are name-resolved (Python-like: keys are expressions, not
    /// symbols). Non-scalar keys produce `KErrorKind::ShapeError`, raised before the value fold.
    pub(in crate::machine::execute) fn schedule_dict_literal<'a>(
        &mut self,
        pairs: Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>,
    ) -> NodeId {
        let mut deps = ResolvedDeps::new();
        let mut rows = Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            let key = self.classify_aggregate_part(k, &mut deps, true);
            let value = self.classify_aggregate_part(v, &mut deps, true);
            rows.push(AggRow {
                key: Some(key),
                value,
            });
        }
        self.schedule_aggregate(
            deps,
            rows,
            Box::new(|keys, value_helds| {
                let map: HashMap<Box<dyn Serializable + '_>, Held<'_>> = keys
                    .into_iter()
                    .zip(value_helds)
                    .map(|(k, v)| (Box::new(k) as Box<dyn Serializable + '_>, v))
                    .collect();
                KObject::dict_of_held(map)
            }),
        )
    }

    /// Schedule a record-literal materialization (`{x = 1, y = "a"}`). Field *names* are literal
    /// schema keys (never resolved); field *values* are name-resolved like dict values. Materializes
    /// a `KObject::Record`, which memoizes the per-field type record at construction.
    pub(in crate::machine::execute) fn schedule_record_literal<'a>(
        &mut self,
        fields: Vec<(String, ExpressionPart<'a>)>,
    ) -> NodeId {
        let mut names: Vec<String> = Vec::with_capacity(fields.len());
        let mut deps = ResolvedDeps::new();
        let mut rows = Vec::with_capacity(fields.len());
        for (name, value) in fields {
            let value = self.classify_aggregate_part(value, &mut deps, true);
            names.push(name);
            rows.push(AggRow { key: None, value });
        }
        self.schedule_aggregate(
            deps,
            rows,
            Box::new(move |_keys, value_helds| {
                let record: Record<Held<'_>> = names.into_iter().zip(value_helds).collect();
                KObject::record_of_held(record)
            }),
        )
    }

    /// Plan one slot of a list / dict literal. The cycle check in the bare-name path is suppressed
    /// (`consumer = None` to `resolve_name_part`) because the dep-finish slot does not yet exist;
    /// cycles are caught post-submission against the dep-finish ID.
    fn classify_aggregate_part<'a>(
        &mut self,
        part: ExpressionPart<'a>,
        deps: &mut ResolvedDeps,
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
                let wrapped = crate::machine::model::KExpression::new(vec![Spanned::bare(part)]);
                Slot::owned(deps, self.dispatch_in_own_scope(wrapped))
            }
            ExpressionPart::QuotedExpression(_) => {
                // A quote rides its own one-part sub-dispatch (the `LiteralPassThrough` lane, which
                // seals it through the checked door) rather than a static cell: a
                // `KObject::KExpression` is invariant in its region lifetime with no `'static`
                // rebuild, so `resolve_region_pure` cannot build it at the `yoke` brand below.
                let wrapped = crate::machine::model::KExpression::new(vec![Spanned::bare(part)]);
                Slot::owned(deps, self.dispatch_in_own_scope(wrapped))
            }
            ref p @ ExpressionPart::Identifier(_) if wrap_identifiers => {
                self.resolve_aggregate_bare_name(p, deps)
            }
            ref p @ ExpressionPart::Type(_) if wrap_identifiers => {
                self.resolve_aggregate_bare_name(p, deps)
            }
            other => {
                // A static literal (keyword / bare identifier / type name / literal): region-pure owned
                // data, so the cell is built **inside** the witness closure — `yoke`d into the classify
                // scope's frame, born co-located with that frame as its reach rather than resolved at
                // the ambient lifetime and bundled under an asserted witness. The cell is then lifetime-free
                // and folds uniformly with the dep cells.
                let frame = current_dest_frame(&self.ambient);
                let carrier = KoanRegion::alloc_witnessed(Rc::clone(&frame), move |region| {
                    Carried::Object(region.alloc_object(other.resolve_region_pure()))
                });
                Slot::Static(Delivered::seal(carrier, frame))
            }
        }
    }

    /// Shared eager-resolve for the Identifier and leaf-Type branches. Unbound / ProducerErrored /
    /// Cycle outcomes fall back to a sub-Dispatch so the `BareIdentifier` fast lane's error path (and
    /// the dep-finish's dep-error short-circuit) handles them uniformly.
    fn resolve_aggregate_bare_name<'a>(
        &mut self,
        part: &ExpressionPart<'a>,
        deps: &mut ResolvedDeps,
    ) -> Slot {
        let active_chain = self.ambient.active_payload().map(|p| &p.chain);
        // A value-bound Identifier element rides into the cell on a carrier witnessed by its binding
        // scope's home frame, which transitively pins that scope's reach-set — so the cell names the
        // value's reach by construction, never an asserted co-location bundle. Type leaves and unbound /
        // pending names fall to the shared `resolve_name_part` path below.
        if let ExpressionPart::Identifier(name) = part {
            let resolved = with_current_node_scope(&self.ambient, |s| {
                s.resolve_value_carrier(name, active_chain.map(|c| &**c))
            });
            if let Some(NameLookup::Bound(carrier)) = resolved {
                let delivered =
                    with_current_node_scope(&self.ambient, |s| s.seal_resident_delivered(carrier));
                return Slot::Static(delivered);
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
            let leaf_name = match &part_b {
                ExpressionPart::Identifier(n) => n.as_str(),
                ExpressionPart::Type(t) => t.as_str(),
                _ => unreachable!(),
            };
            match resolve_name_part(s, &part_b, &self.sched, active_chain, None) {
                // A first-class **type** resolved into the cell is witnessed in place from its binding's
                // stored reach (recomputed here, since `resolve_name_part` returns only the `&KType`):
                // the read scope's home frame pins the type's (ancestor) region via its `outer` chain,
                // and `reach` names any genuinely-foreign region (a module's child scope).
                NameOutcome::Resolved(Carried::Type(kt)) => {
                    // The resolved type is witnessed in place from its binding's stored token
                    // (recomputed here, since `resolve_name_part` returns only the `&KType`): the read
                    // scope's home frame pins the type's (ancestor) region via its `outer` chain, the
                    // token's foreign reach names any genuinely-foreign region (a module's child scope),
                    // and its home-borrow bit rides too — replayed whole, never re-asserted.
                    let stored = s.type_reach(leaf_name, active_chain.map(|c| &**c));
                    Some(Slot::Static(s.seal_resident_delivered(
                        s.resident_type_carrier(kt, stored),
                    )))
                }
                // The value case is handled above via the reach-carrying binding-scope carrier
                // (`resolve_value_carrier`). A bare `Carried::Object` reaching here carries no reach to
                // build a correct carrier from, so it falls through to the sub-dispatch fallback rather
                // than wrapping a reachless value.
                NameOutcome::Resolved(Carried::Object(_)) => None,
                NameOutcome::Parked(producer) => Some(Slot::Park(deps.park_on(producer))),
                NameOutcome::Unbound(_)
                | NameOutcome::ProducerErrored(_)
                | NameOutcome::Cycle(_) => None,
            }
        });
        match resolved {
            Some(slot) => slot,
            None => {
                let expr =
                    crate::machine::model::KExpression::new(vec![Spanned::bare(part.clone())]);
                Slot::owned(deps, self.dispatch_in_own_scope(expr))
            }
        }
    }
}
