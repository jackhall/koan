use std::collections::HashMap;

use smallvec::SmallVec;

use crate::machine::core::{
    FoldingBrand, KoanRegionExt, KoanStorageProfile, RegionBrand, SubstrateDoor,
};
use crate::machine::model::CarriedFamily;
use crate::machine::model::{Carried, Held, KKey, KObject, TypeRegistry};
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::machine::{
    CarrierWitness, DeliveredCarried, FrameStorage, KError, KErrorKind, KoanRegion, NodeId,
    TraceFrame,
};
use crate::source::Spanned;
use crate::witnessed::{BumpVec, Delivered, RegionHandle, reattachable};

use super::super::harness::{Host, KoanWorkload};
use super::super::lift::{HeldFamily, copy_held_from_carried, relocated_cell_still_borrows};
use super::super::outcome::DepTerminal;
use super::super::{StepCarried, WitnessedDepFinish};
use super::SubmitContext;
use super::ctx::{DecideCtx, current_dest_frame, with_current_node_scope};
use super::resolve::{Resolution, TypeLeafChannels, resolve_name};
use super::stage_eager_part;
use crate::machine::Scope;
use crate::machine::model::RunRegistries;
use crate::machine::model::Symbol;
use crate::scheduler::{Deps, Scheduler};
use crate::witnessed::RegionHandleFamily;

/// Build-time product family for an aggregate relocation. Layout-invariant in `'r`: a thin region
/// pointer and a slice of layout-invariant cells.
///
/// The cells ride a **region-bumped slice** rather than an owned `Vec`, which keeps the family on
/// the Copy tier: that tier's dormant slot is glue-free, so an owned buffer resting there would be
/// dropped by nobody. The run is bumped once, inside the relocation's single brand, so its bytes
/// are proportional to the aggregate rather than to the walk that built it.
struct AggBuildFamily;
reattachable!(AggBuildFamily => (RegionHandle<'r, KoanStorageProfile>, &'r [Held<'r>]));

/// One cell of a list / dict / record literal. A `Static` cell is wrapped into a delivery envelope
/// **at its source**, so the layout is lifetime-free and every cell folds uniformly, each carrying
/// the frame owner its value lives under. A `Dep` cell carries no index: the classifier appends one
/// dep per such cell as it walks the rows, so cell order *is* dep order and the finish's walk reads
/// results off a [`ResultFeed`] cursor.
enum Slot {
    Static(DeliveredCarried),
    Dep,
}

impl Slot {
    fn spawned(deps: &mut Deps<NodeId>, id: NodeId) -> Self {
        deps.request(id);
        Slot::Dep
    }
}

/// Pops resolved dep terminals in dep order. The classify walk and the finish walk visit the rows
/// in the same order (a dict row's key before its value), so popping in that order *is* the
/// alignment — no cell stores an index.
struct ResultFeed<'t, 'd> {
    terminals: &'t [DepTerminal<'d>],
    next: usize,
}

impl<'t, 'd> ResultFeed<'t, 'd> {
    fn new(terminals: &'t [DepTerminal<'d>]) -> Self {
        ResultFeed { terminals, next: 0 }
    }

    fn pop(&mut self) -> &'t DepTerminal<'d> {
        let terminal = &self.terminals[self.next];
        self.next += 1;
        terminal
    }
}

/// The relocation needs the reach *owned* — it moves each cell into the aggregate's region while
/// minting its reach and residence host onto the product's carrier — so the dep arm lifts the
/// terminal's resident into an envelope rather than pairing a read-out value with a separately-read
/// reach.
fn cell_carrier(
    slot: Slot,
    terminals: &mut ResultFeed<'_, '_>,
    scope: &Scope<'_>,
) -> DeliveredCarried {
    match slot {
        Slot::Static(delivered) => delivered,
        Slot::Dep => scope.lift_spliced(&terminals.pop().cell),
    }
}

/// Relocate a run of cell envelopes into a witnessed `(region, &[Held])` product over the consumer
/// scope's region: one `transfer_all_into` stages every cell against a bare destination handle, so
/// [`copy_held_from_carried`] rebuilds all of them at a single brand and the run is bumped once. A
/// cell rebuilt through the container door releases its producer when it is plain data, while a
/// cell that still borrows its producer (a closure's captured environment) materializes the host —
/// the same copied-adoption rule the param binds apply, asked per cell by
/// [`relocated_cell_still_borrows`].
fn fold_cells(
    view: &DecideCtx<'_, '_, '_>,
    cells: &[DeliveredCarried],
) -> Delivered<AggBuildFamily, CarrierWitness, FrameStorage> {
    DeliveredCarried::transfer_all_into::<
        RegionHandleFamily<KoanStorageProfile>,
        AggBuildFamily,
        HeldFamily,
        KoanStorageProfile,
    >(
        cells,
        Delivered::destination(view.dest_frame()),
        relocated_cell_still_borrows,
        |run, dest_handle, placement| {
            // `run[i]` is `cells[i]`'s live form, so each cell rebuilds under **its own** source
            // coverage — the holder-rule proof for whatever part of it stays foreign — rather than
            // under a union across the run.
            let helds: SmallVec<[Held<'_>; 8]> = run
                .iter()
                .zip(cells)
                .map(|(carried, envelope)| {
                    copy_held_from_carried(
                        *carried,
                        FoldingBrand::in_fold_closure(placement).with_holder(envelope.coverage()),
                    )
                })
                .collect();
            // Bumped once: the cells the relocation built are also the per-source cells the door
            // pairs back with the envelopes, so the same slice serves both.
            let slice = placement.allocator().slice(&helds);
            ((dest_handle, slice), slice)
        },
    )
}

/// A key is never folded (it is a scalar, reaching no region), so it is read out and converted in
/// place.
///
/// **Dict keys are owned data by language rule** — a function or module key is meaningless — so
/// turning a carrier into a key is where the rule is enforced. The check is O(1) on the key's own
/// **stored envelope**: a carrier naming any reach member is
/// rejected outright, no walk over the value. [`KKey`] then admits only `String` / `Number` /
/// `Bool`, so the two together leave a borrow-carrying key unrepresentable downstream.
fn scalar_key(
    slot: &Slot,
    terminals: &mut ResultFeed<'_, '_>,
    registries: &RunRegistries,
) -> Result<PendingKey, String> {
    // Each arm answers the reach probe and the key read inside its own borrow.
    let (borrows, key) = match slot {
        Slot::Static(delivered) => (
            delivered.open_at().has_reach_members(),
            delivered.open(|c| key_from_carried(c, registries)),
        ),
        Slot::Dep => {
            let cell = &terminals.pop().cell;
            (
                cell.open_at().has_reach_members(),
                cell.open(|c| key_from_carried(c, registries)),
            )
        }
    };
    if borrows {
        return Err("dict key must be owned data, but its value borrows a region".to_string());
    }
    key
}

fn key_from_carried(c: Carried<'_>, registries: &RunRegistries) -> Result<PendingKey, String> {
    match c {
        Carried::Object(o) => KKey::try_from_kobject(o, registries).map(PendingKey::staged),
        Carried::Type(_) | Carried::UnresolvedType(_) => {
            Err("dict key must be a value, not a type".to_string())
        }
    }
}

/// A resolved dict key held **owned** across the gap between reading it and assembling the dict:
/// the key is read inside its producer envelope's own open, whose lifetime ends there, and the dict
/// is built later at a fold brand no earlier borrow can reach, so a string key's bytes have to be
/// staged rather than carried. Transient by construction — the dict door bumps the bytes into the
/// dict's own region — and it never reaches a region, so owning a `String` here costs no teardown.
enum PendingKey {
    String(String),
    Number(f64),
    Bool(bool),
}

impl PendingKey {
    fn staged(key: KKey<'_>) -> PendingKey {
        match key {
            KKey::String(s) => PendingKey::String(s.to_string()),
            KKey::Number(n) => PendingKey::Number(n),
            KKey::Bool(b) => PendingKey::Bool(b),
        }
    }

    fn as_key(&self) -> KKey<'_> {
        match self {
            PendingKey::String(s) => KKey::String(s),
            PendingKey::Number(n) => KKey::Number(*n),
            PendingKey::Bool(b) => KKey::Bool(*b),
        }
    }
}

/// One layout row of an aggregate literal: the value cell, plus — for a dict — the key slot resolved
/// to a scalar [`KKey`] at finish time (list and record rows carry no key).
struct AggRow {
    key: Option<Slot>,
    value: Slot,
}

/// Finish-side assemble hook — the keys are empty unless the rows carry key slots. Boxed
/// higher-ranked so the record variant can capture its field names and each shape builds its own
/// `KObject` at the substrate door.
type AggAssemble = Box<
    dyn for<'r, 'h> FnOnce(
        SubstrateDoor<'r, 'h>,
        Vec<PendingKey>,
        &'r [Held<'r>],
        &TypeRegistry,
    ) -> KObject<'r>,
>;

impl<'step> Host<'step> {
    /// Schedule an aggregate literal's element deps and its assembling fold. A non-scalar dict key
    /// errors before the fold, under the dict-literal frame (only a dict row carries a key slot), and
    /// `assemble` runs inside the witness closure so the aggregate names every region it reaches by
    /// construction.
    fn schedule_aggregate(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        deps: Deps<NodeId>,
        rows: Vec<AggRow>,
        assemble: AggAssemble,
    ) -> NodeId {
        let finish: WitnessedDepFinish<'step> = Box::new(move |view, terminals| {
            let n = rows.len();
            // Keys stay scalar (reaching no region): read out eagerly, erroring before the fold.
            let mut keys: Vec<PendingKey> = Vec::new();
            let mut cells: Vec<DeliveredCarried> = Vec::with_capacity(n);
            let mut feed = ResultFeed::new(terminals);
            for row in rows {
                if let Some(key_slot) = row.key {
                    let kkey =
                        scalar_key(&key_slot, &mut feed, view.registries()).map_err(|msg| {
                            KError::new(KErrorKind::ShapeError(msg))
                                .with_frame(TraceFrame::bare("<dict>", "dict literal"))
                        })?;
                    keys.push(kkey);
                }
                cells.push(cell_carrier(row.value, &mut feed, view.current_scope()));
            }
            let acc = fold_cells(view, &cells);
            // The relocated envelope's coverage names every region the `Held` views point into and
            // its home is the destination frame the relocation minted into, so merging into a bare
            // handle on that region assembles the aggregate and mints its description in one step:
            // a record literal's fresh substrate borrows into the very region it was built in,
            // which the relocated envelope's own pins name.
            let dest_frame = view.dest_frame();
            let types = view.types();
            // The holder for the **aggregate's own** birth is the union across the run — the
            // relocated envelope's coverage, pinning every region any cell reaches. Union rather
            // than per-cell because the value being born here is the container, whose stored run
            // spans all of them: the door reads that run's reach under a proof covering every
            // region it could name, and it is also the declared reach for a cell carrying no
            // stored description of its own (a spliced expression). The per-cell rule applies one
            // level down, at each cell's own rebuild inside `fold_cells`.
            let holder = acc.coverage().clone();
            let built = acc
                .merge_into::<RegionHandleFamily<KoanStorageProfile>, CarriedFamily, KoanStorageProfile>(
                Delivered::destination(dest_frame),
                move |(_region, value_helds), _dest_handle, placement| {
                    let region = FoldingBrand::in_fold_closure(placement);
                    Carried::Object(region.alloc_object_folded(assemble(
                        region.with_holder(&holder),
                        keys,
                        value_helds,
                        types,
                    )))
                },
            );
            Ok(StepCarried::born_delivered(built))
        });
        self.submit_dep_finish_witnessed_in_own_scope(sched, deps, finish)
    }

    /// Schedule a list-literal materialization as a witnessed dep-finish over its element producers.
    /// Bare identifier elements are name-resolved like dict values, so `[n, n]` holds `n`'s bound
    /// value and the memoized element type joins the resolved values' types.
    pub(in crate::machine::execute) fn schedule_list_literal<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        brand: RegionBrand<'a>,
        items: &[ExpressionPart<'a>],
    ) -> NodeId {
        let mut deps = Deps::new();
        let mut rows = Vec::with_capacity(items.len());
        for &part in items {
            let value = self.classify_aggregate_part(sched, brand, part, &mut deps);
            rows.push(AggRow { key: None, value });
        }
        self.schedule_aggregate(
            sched,
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
        sched: &mut Scheduler<KoanWorkload>,
        brand: RegionBrand<'a>,
        pairs: &[(ExpressionPart<'a>, ExpressionPart<'a>)],
    ) -> NodeId {
        let mut deps = Deps::new();
        let mut rows = Vec::with_capacity(pairs.len());
        for &(k, v) in pairs {
            let key = self.classify_aggregate_part(sched, brand, k, &mut deps);
            let value = self.classify_aggregate_part(sched, brand, v, &mut deps);
            rows.push(AggRow {
                key: Some(key),
                value,
            });
        }
        self.schedule_aggregate(
            sched,
            deps,
            rows,
            Box::new(|door, keys, value_helds, types| {
                let map: HashMap<KKey, Held<'_>> = keys
                    .iter()
                    .map(PendingKey::as_key)
                    .zip(value_helds.iter().copied())
                    .collect();
                KObject::dict_of_held(door, map, types)
            }),
        )
    }

    /// Schedule a record-literal materialization (`{x = 1, y = "a"}`). Field *names* are literal
    /// schema keys (never resolved); field *values* are name-resolved like dict values. Materializes
    /// a `KObject::Record`, which memoizes the per-field type record at construction.
    pub(in crate::machine::execute) fn schedule_record_literal<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        brand: RegionBrand<'a>,
        fields: &[(&'a str, ExpressionPart<'a>)],
    ) -> NodeId {
        let mut names: Vec<Symbol> = Vec::with_capacity(fields.len());
        let mut deps = Deps::new();
        let mut rows = Vec::with_capacity(fields.len());
        for &(name, value) in fields {
            let value = self.classify_aggregate_part(sched, brand, value, &mut deps);
            names.push(self.ambient.registries().labels.intern(name));
            rows.push(AggRow { key: None, value });
        }
        self.schedule_aggregate(
            sched,
            deps,
            rows,
            Box::new(move |door, _keys, value_helds, types| {
                // The field pairs are assembled in the destination region's own construction
                // storage: the schedule-time name slice zipped against the delivered cells, with
                // no owned record in between.
                let mut pairs: BumpVec<'_, (Symbol, Held<'_>)> =
                    BumpVec::with_capacity_in(names.len(), door.allocator());
                pairs.extend(names.iter().copied().zip(value_helds.iter().copied()));
                KObject::record_of_held(door, &pairs, types)
            }),
        )
    }

    /// Plan one slot of a list / dict / record literal.
    fn classify_aggregate_part<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        brand: RegionBrand<'a>,
        part: ExpressionPart<'a>,
        deps: &mut Deps<NodeId>,
    ) -> Slot {
        let part = match stage_eager_part(brand, part) {
            Ok(dep) => return Slot::spawned(deps, self.realize_dep(sched, brand, dep)),
            Err(part) => part,
        };
        match part {
            ExpressionPart::QuotedExpression(_) => {
                // A quote rides its own one-part sub-dispatch (the `LiteralPassThrough` lane, which
                // seals it through the expression door) rather than a static cell: a
                // `KObject::KExpression` is invariant in its region lifetime with no `'static`
                // rebuild, so `resolve_region_pure` cannot build it at the fold brand below.
                let wrapped =
                    WorkingExpression::new(brand, &[Spanned::bare(WorkingPart::Ast(part))]);
                Slot::spawned(
                    deps,
                    self.dispatch_in_own_scope(sched, wrapped, SubmitContext::SubDispatch),
                )
            }
            ref p @ ExpressionPart::Identifier(_) => {
                self.resolve_aggregate_bare_name(sched, brand, p, deps)
            }
            ref p @ ExpressionPart::Type(_) => {
                self.resolve_aggregate_bare_name(sched, brand, p, deps)
            }
            other => {
                // A static literal (keyword / literal) is region-pure — every borrow it carries
                // points into the classify scope's own frame, a string literal's bumped bytes
                // included — so the cell is built **inside** a zero-dep fold, born co-located with
                // that frame as its reach.
                let frame = current_dest_frame(&self.ambient);
                Slot::Static(KoanRegion::fold_witnessed(frame, move |brand| {
                    Carried::Object(brand.alloc_object_folded(other.resolve_region_pure(*brand)))
                }))
            }
        }
    }

    /// Shared eager-resolve for the Identifier and leaf-Type branches. A bound name seals its
    /// binding-scope carrier — value and reach as one cell, witnessed by its binding scope's home
    /// frame — straight into a static slot; a still-finalizing name parks on its claim edge. An
    /// unbound name falls back to a sub-Dispatch so the `BareIdentifier` fast lane's error path
    /// handles it uniformly.
    fn resolve_aggregate_bare_name<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        brand: RegionBrand<'a>,
        part: &ExpressionPart<'a>,
        deps: &mut Deps<NodeId>,
    ) -> Slot {
        let active_chain = self.ambient.active_payload().map(|p| &p.chain);
        // `Resolution` is lifetime-free, so the whole result escapes the branded-scope closure and
        // the `&mut self` fallback runs after the read closes.
        let resolved = with_current_node_scope(&self.ambient, |s| {
            resolve_name(
                s,
                part,
                active_chain,
                self.ambient.types(),
                TypeLeafChannels::TypeChannel,
            )
        });
        match resolved {
            Resolution::Resolved(cell) => Slot::Static(cell),
            Resolution::Parked(source) => {
                deps.on(source.scheduler_edge());
                Slot::Dep
            }
            Resolution::Unbound(_) => {
                let expr = WorkingExpression::new(brand, &[Spanned::bare(WorkingPart::Ast(*part))]);
                Slot::spawned(
                    deps,
                    self.dispatch_in_own_scope(sched, expr, SubmitContext::SubDispatch),
                )
            }
        }
    }
}
