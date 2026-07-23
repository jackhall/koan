use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::{FoldingBrand, KoanRegionExt, KoanStorageProfile};
use crate::machine::model::CarriedFamily;
use crate::machine::model::ExpressionPart;
use crate::machine::model::{Carried, Held, KKey, KObject, Record, TypeRegistry};
use crate::machine::{
    force_substrate_borrows_host, CarrierWitness, DeliveredCarried, KError, KErrorKind, KoanRegion,
    NodeId, TraceFrame,
};
use crate::source::Spanned;
use crate::witnessed::{reattachable, Delivered, RegionHandle, Witnessed};

use super::super::lift::{copied_seam_mode, copy_held_from_carried};
use super::super::outcome::DepTerminal;
use super::super::runtime::KoanRuntime;
use super::super::{StepCarried, WitnessedDepFinish};
use super::ctx::{current_dest_frame, with_current_node_scope, SchedulerView};
use super::stage_eager_part;
use super::SubmitContext;
use super::{resolve_bare_carrier, BareCarrier};
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
/// `transfer_into_placing` each envelope at its own [`copied_seam_mode`] — [`copy_held_from_carried`]
/// relocates each cell into the aggregate region (a top-level record totally rebuilt through the
/// record door so its substrate is container-resident), so a plain-data record cell releases its
/// producer while a cell that still borrows its producer (a closure's captured environment)
/// materializes the host — the same copied-adoption rule the param binds apply. The final aggregate
/// shape (`list_of_held` / `dict_of_held` / `record_of_held`) is built by the caller's pinned map.
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
        let mode = copied_seam_mode(&cell);
        cell.transfer_into_placing::<AggBuildFamily, AggBuildFamily, _>(
            acc,
            mode,
            |value, (region, mut cells), placement| {
                cells.push(copy_held_from_carried(
                    value,
                    FoldingBrand::in_fold_closure(placement),
                ));
                (region, cells)
            },
        )
    })
}

/// Read a dict key cell as a scalar [`KKey`]: a key is never folded (it is a scalar, reaching no
/// region), so it is read out and converted in place. A `Type` arm or a non-scalar value errors.
fn scalar_key(
    slot: &Slot,
    terminals: DepResults<'_, &DepTerminal<'_>>,
    types: &TypeRegistry,
) -> Result<KKey, String> {
    match slot {
        Slot::Static(delivered) => delivered.open(|c| key_from_carried(c, types)),
        Slot::Park(i) => key_from_carried(terminals.park(*i).value, types),
        Slot::Owned(j) => key_from_carried(terminals.owned(*j).value, types),
    }
}

fn key_from_carried(c: Carried<'_>, types: &TypeRegistry) -> Result<KKey, String> {
    match c {
        Carried::Object(o) => KKey::try_from_kobject(o, types),
        Carried::Type(_) | Carried::UnresolvedType(_) => {
            Err("dict key must be a value, not a type".to_string())
        }
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
/// field names and each shape builds its own `KObject` at the fold brand. Every shape threads the
/// fold's own `FoldingBrand` into its `*_of_held` constructor — the door each substrate (record,
/// list, dict) is born through.
type AggAssemble = Box<
    dyn for<'r> FnOnce(FoldingBrand<'r>, Vec<KKey>, Vec<Held<'r>>, &TypeRegistry) -> KObject<'r>,
>;

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
                    let kkey = scalar_key(&key_slot, terminals, view.types()).map_err(|msg| {
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
            let types = view.types();
            let witnessed = acc.map_pinned_placing::<CarriedFamily, KoanStorageProfile, _>(
                &dest_frame,
                move |(_region, value_helds), placement| {
                    let region = FoldingBrand::in_fold_closure(placement);
                    Carried::Object(region.alloc_object_folded(assemble(
                        region,
                        keys,
                        value_helds,
                        types,
                    )))
                },
            );
            // Step-terminal seal: a record literal's fresh substrate always borrows into this
            // same `dest_frame` it was just built into — the fold above composes the witness
            // from the accumulator alone, blind to that fact, so force it here rather than
            // under-report the value's own self-borrow.
            let witnessed = force_substrate_borrows_host(witnessed, &dest_frame);
            Ok(StepCarried::born(witnessed))
        });
        self.submit_dep_finish_witnessed_in_own_scope(deps, finish)
    }

    /// Schedule a list-literal materialization as a witnessed dep-finish over its element producers.
    /// Bare identifier elements are name-resolved like dict values, so `[n, n]` holds `n`'s bound
    /// value and the memoized element type joins the resolved values' types.
    pub(in crate::machine::execute) fn schedule_list_literal<'a>(
        &mut self,
        items: Vec<ExpressionPart<'a>>,
    ) -> NodeId {
        let mut deps = ResolvedDeps::new();
        let mut rows = Vec::with_capacity(items.len());
        for part in items {
            let value = self.classify_aggregate_part(part, &mut deps);
            rows.push(AggRow { key: None, value });
        }
        self.schedule_aggregate(
            deps,
            rows,
            Box::new(|door, _keys, cells, types| KObject::list_of_held(door, cells, types)),
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
            let key = self.classify_aggregate_part(k, &mut deps);
            let value = self.classify_aggregate_part(v, &mut deps);
            rows.push(AggRow {
                key: Some(key),
                value,
            });
        }
        self.schedule_aggregate(
            deps,
            rows,
            Box::new(|door, keys, value_helds, types| {
                let map: HashMap<KKey, Held<'_>> = keys.into_iter().zip(value_helds).collect();
                KObject::dict_of_held(door, map, types)
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
            let value = self.classify_aggregate_part(value, &mut deps);
            names.push(name);
            rows.push(AggRow { key: None, value });
        }
        self.schedule_aggregate(
            deps,
            rows,
            Box::new(move |door, _keys, value_helds, types| {
                let record: Record<Held<'_>> = names.into_iter().zip(value_helds).collect();
                KObject::record_of_held(door, record, types)
            }),
        )
    }

    /// Plan one slot of a list / dict literal. The bare-name ladder does no cycle check — the
    /// dep-finish slot does not yet exist, so a still-finalizing name parks and cycles are caught
    /// post-submission against the dep-finish ID.
    fn classify_aggregate_part<'a>(
        &mut self,
        part: ExpressionPart<'a>,
        deps: &mut ResolvedDeps,
    ) -> Slot {
        let part = match stage_eager_part(part) {
            Ok(dep) => return Slot::owned(deps, self.realize_eager_dep(dep)),
            Err(part) => part,
        };
        match part {
            ExpressionPart::QuotedExpression(_) => {
                // A quote rides its own one-part sub-dispatch (the `LiteralPassThrough` lane, which
                // seals it through the checked door) rather than a static cell: a
                // `KObject::KExpression` is invariant in its region lifetime with no `'static`
                // rebuild, so `resolve_region_pure` cannot build it at the `yoke` brand below.
                let wrapped = crate::machine::model::KExpression::new(vec![Spanned::bare(part)]);
                Slot::owned(
                    deps,
                    self.dispatch_in_own_scope(
                        wrapped,
                        SubmitContext::SubDispatch {
                            binder_covered: false,
                        },
                    ),
                )
            }
            ref p @ ExpressionPart::Identifier(_) => self.resolve_aggregate_bare_name(p, deps),
            ref p @ ExpressionPart::Type(_) => self.resolve_aggregate_bare_name(p, deps),
            other => {
                // A static literal (keyword / literal): region-pure owned
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

    /// Shared eager-resolve for the Identifier and leaf-Type branches. A bound name seals its
    /// binding-scope carrier — value and reach as one cell, witnessed by its binding scope's home
    /// frame — straight into a static slot; a still-finalizing name parks. Unbound / producer-errored
    /// names fall back to a sub-Dispatch so the `BareIdentifier` fast lane's error path (and the
    /// dep-finish's dep-error short-circuit) handles them uniformly.
    fn resolve_aggregate_bare_name<'a>(
        &mut self,
        part: &ExpressionPart<'a>,
        deps: &mut ResolvedDeps,
    ) -> Slot {
        let active_chain = self.ambient.active_payload().map(|p| &p.chain);
        // `BareCarrier` is lifetime-free, so the whole result escapes the branded-scope closure and
        // the `&mut self` fallback runs after the read closes.
        let resolved = with_current_node_scope(&self.ambient, |s| {
            resolve_bare_carrier(
                s,
                part,
                active_chain,
                &self.sched,
                self.ambient.type_registry(),
            )
        });
        match resolved {
            Ok(BareCarrier::Sealed(cell)) => Slot::Static(cell),
            Ok(BareCarrier::Parked(producer)) => Slot::Park(deps.park_on(producer)),
            // Unbound / producer-errored: fall back to a sub-Dispatch so the `BareIdentifier` fast
            // lane's error path surfaces them uniformly.
            Ok(BareCarrier::Unbound(_)) | Err(_) => {
                let expr =
                    crate::machine::model::KExpression::new(vec![Spanned::bare(part.clone())]);
                Slot::owned(
                    deps,
                    self.dispatch_in_own_scope(
                        expr,
                        SubmitContext::SubDispatch {
                            binder_covered: false,
                        },
                    ),
                )
            }
        }
    }
}
