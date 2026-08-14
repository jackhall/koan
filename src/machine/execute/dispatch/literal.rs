use std::collections::HashMap;

use crate::machine::core::{
    FoldingBrand, KoanRegionExt, KoanStorageProfile, RegionBrand, SubstrateDoor,
};
use crate::machine::model::CarriedFamily;
use crate::machine::model::{Carried, Held, KKey, KObject, Record, TypeRegistry};
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::machine::{
    CarrierWitness, DeliveredCarried, FrameStorage, KError, KErrorKind, KoanRegion, NodeId,
    TraceFrame,
};
use crate::source::Spanned;
use crate::witnessed::{Delivered, RegionHandle, reattachable};

use super::super::lift::{cell_still_borrows, copy_held_from_carried};
use super::super::outcome::DepTerminal;
use super::super::run_loop::{DestHandleFamily, dest_brand};
use super::super::runtime::KoanRuntime;
use super::super::{StepCarried, WitnessedDepFinish};
use super::SubmitContext;
use super::ctx::{SchedulerView, current_dest_frame, with_current_node_scope};
use super::stage_eager_part;
use super::{BareCarrier, resolve_bare_carrier};
use crate::machine::Scope;
use crate::scheduler::{DepResults, Deps};

/// Build-time accumulator family for an aggregate fold: the destination region plus the cells folded
/// in so far. Each cell carrier is `transfer_into`-folded in — relocating the value and unioning its
/// reach onto the accumulator's witness — then the final `map` allocates the aggregate from the
/// region. Layout-invariant in `'r`: a thin region pointer and a slice of layout-invariant cells.
///
/// The cells ride a **region-bumped slice** rather than an owned `Vec`, which is what keeps the
/// accumulator on the Copy tier: it rests between fold steps (each `transfer_into` seals it), and
/// that tier's dormant slot is glue-free, so an owned buffer resting there would be dropped by
/// nobody. Each step re-bumps the accumulated cells plus the new one — the same shape
/// `StepContext::alloc_with`'s own dep-view fold takes, and for the same reason.
struct AggBuildFamily;
reattachable!(AggBuildFamily => (RegionHandle<'r, KoanStorageProfile>, &'r [Held<'r>]));

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
    fn owned(deps: &mut Deps<NodeId>, id: NodeId) -> Self {
        Slot::Owned(deps.own(id))
    }
}

/// The per-cell envelope the fold consumes: a static cell's source-built envelope, or a dep
/// terminal's resident lifted back into one. The fold needs the reach *owned* — `transfer_into`
/// relocates each cell into the aggregate's region while minting its reach and residence host onto
/// the accumulator's carrier — so a dep arm lifts rather than reads, and the envelope it hands back
/// is the cell's own description upgraded, never a fresh bundle pairing a read-out value with a
/// separately-read reach.
fn cell_carrier(
    slot: Slot,
    terminals: DepResults<'_, &DepTerminal<'_>>,
    scope: &Scope<'_>,
) -> DeliveredCarried {
    match slot {
        Slot::Static(delivered) => delivered,
        Slot::Park(i) => scope.lift_spliced(&terminals.park(i).cell),
        Slot::Owned(j) => scope.lift_spliced(&terminals.owned(j).cell),
    }
}

/// Fold a sequence of cell envelopes into a witnessed `(region, Vec<Held>)` accumulator over the
/// consumer scope's region: `yoke` an empty accumulator under the consumer frame, then
/// `transfer_into` each envelope under [`cell_still_borrows`] — [`copy_held_from_carried`]
/// relocates each cell into the aggregate region (a top-level record totally rebuilt through the
/// record door so its substrate is container-resident), so a plain-data record cell releases its
/// producer while a cell that still borrows its producer (a closure's captured environment)
/// materializes the host — the same copied-adoption rule the param binds apply. The final aggregate
/// shape (`list_of_held` / `dict_of_held` / `record_of_held`) is built by the caller's pinned map.
fn fold_cells(
    view: &SchedulerView<'_, '_, '_>,
    cells: impl Iterator<Item = DeliveredCarried>,
) -> Delivered<AggBuildFamily, CarrierWitness, FrameStorage> {
    let dest_frame = view.dest_frame();
    // The accumulator crosses as an envelope homed in the aggregate's own destination frame, so each
    // cell's transfer composes into it directly and the accumulated coverage rides the envelope
    // rather than being threaded beside it. A bare handle plus an empty slice reaches nothing, so
    // the seed's coverage is empty.
    let acc0 = KoanRegion::yoke_branded::<AggBuildFamily, _>(dest_frame, |region| {
        (region.handle(), &[][..])
    });
    cells.fold(acc0, |acc, cell| {
        // The cell rebuilds through the container door, so its own cells' stored reach is read at
        // that door; this envelope's coverage is the holder-rule proof for any part of it that stays
        // foreign, captured before the fold closure.
        let holder = cell.coverage().clone();
        cell.transfer_into::<AggBuildFamily, AggBuildFamily, _>(
            acc,
            // The cell always rebuilds through the container door, so the retention predicate walks
            // the cell the fold just pushed — the exact answer for what this relocation left
            // pointing back at its source.
            cell_still_borrows(&cell),
            |value, (region, cells), placement| {
                let cell = copy_held_from_carried(
                    value,
                    FoldingBrand::in_fold_closure(placement).with_holder(&holder),
                );
                // Re-bump the accumulated cells plus this one into the destination region: the
                // relocations each fold performs interleave with the accumulator's own allocations,
                // so extending the previous slice in place is not available.
                let mut grown = Vec::with_capacity(cells.len() + 1);
                grown.extend_from_slice(cells);
                grown.push(cell);
                (region, placement.allocator().slice(&grown))
            },
        )
    })
}

/// Read a dict key cell as a scalar [`KKey`]: a key is never folded (it is a scalar, reaching no
/// region), so it is read out and converted in place. A `Type` arm or a non-scalar value errors.
///
/// **Dict keys are owned data by language rule** — a function or module key is meaningless — and
/// this is the one site that turns a carrier into a key, so it is where the rule is enforced. The
/// check is O(1) on the key's own **stored envelope**: a carrier naming any reach member is
/// rejected outright, no walk over the value. [`KKey`] then admits only `String` / `Number` /
/// `Bool`, so the two together leave a borrow-carrying key unrepresentable downstream of here.
fn scalar_key(
    slot: &Slot,
    terminals: DepResults<'_, &DepTerminal<'_>>,
    types: &TypeRegistry,
) -> Result<PendingKey, String> {
    // The reach probe and the key read are the same two verbs on either carrier — a static cell's
    // envelope or a dep's resident cell — so each arm answers both inside its own borrow.
    let (borrows, key) = match slot {
        Slot::Static(delivered) => (
            delivered.open_at().has_reach_members(),
            delivered.open(|c| key_from_carried(c, types)),
        ),
        Slot::Park(i) => {
            let cell = &terminals.park(*i).cell;
            (
                cell.open_at().has_reach_members(),
                cell.open(|c| key_from_carried(c, types)),
            )
        }
        Slot::Owned(j) => {
            let cell = &terminals.owned(*j).cell;
            (
                cell.open_at().has_reach_members(),
                cell.open(|c| key_from_carried(c, types)),
            )
        }
    };
    if borrows {
        return Err("dict key must be owned data, but its value borrows a region".to_string());
    }
    key
}

fn key_from_carried(c: Carried<'_>, types: &TypeRegistry) -> Result<PendingKey, String> {
    match c {
        Carried::Object(o) => KKey::try_from_kobject(o, types).map(PendingKey::staged),
        Carried::Type(_) | Carried::UnresolvedType(_) => {
            Err("dict key must be a value, not a type".to_string())
        }
    }
}

/// A resolved dict key held **owned** across the gap between reading it and assembling the dict.
/// The key is read inside its producer envelope's own open, whose lifetime ends there, and the dict
/// is built later at a fold brand no earlier borrow can reach — so a string key's bytes have to be
/// staged rather than carried. Transient by construction: [`Self::as_key`] hands the dict door a
/// borrow and the door bumps the bytes into the dict's own region, after which this is dropped. It
/// is not a value-family slot and never reaches a region, so owning a `String` here costs no
/// teardown.
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

    /// This key as a [`KKey`] borrowing the staged bytes — what the dict door re-homes.
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

/// Finish-side assemble hook: the resolved keys (empty unless the rows carry key slots) and the folded
/// value cells become the aggregate object. Boxed higher-ranked so the record variant captures its
/// field names and each shape builds its own `KObject` at the substrate door. Most cells [`fold_cells`]
/// pushed were rebuilt at this very brand, but a borrow leaf rides its source borrow verbatim, so the
/// door carries the accumulator's own coverage as its holder — the pins every folded cell's reach was
/// composed from.
type AggAssemble = Box<
    dyn for<'r, 'h> FnOnce(
        SubstrateDoor<'r, 'h>,
        Vec<PendingKey>,
        &'r [Held<'r>],
        &TypeRegistry,
    ) -> KObject<'r>,
>;

impl<'step> KoanRuntime<'step> {
    /// The one scheduling path behind the three aggregate literals: park a witnessed dep-finish on
    /// `deps`; on resolve, read each row's key (a non-scalar dict key errors before the fold, under the
    /// dict-literal frame — only a dict row carries a key slot), fold the value cells into the consumer
    /// region, and `assemble` the aggregate inside the witness closure so it names every region it
    /// reaches by construction.
    fn schedule_aggregate(
        &mut self,
        deps: Deps<NodeId>,
        rows: Vec<AggRow>,
        assemble: AggAssemble,
    ) -> NodeId {
        let finish: WitnessedDepFinish<'step> = Box::new(move |view, terminals| {
            let n = rows.len();
            // Keys stay scalar (reaching no region): read them out eagerly, erroring before the fold.
            // The value cells fold into the witnessed accumulator, paired back with the keys at `map`.
            let mut keys: Vec<PendingKey> = Vec::new();
            let mut cells: Vec<DeliveredCarried> = Vec::with_capacity(n);
            for row in rows {
                if let Some(key_slot) = row.key {
                    let kkey = scalar_key(&key_slot, terminals, view.types()).map_err(|msg| {
                        KError::new(KErrorKind::ShapeError(msg))
                            .with_frame(TraceFrame::bare("<dict>", "dict literal"))
                    })?;
                    keys.push(kkey);
                }
                cells.push(cell_carrier(row.value, terminals, view.current_scope()));
            }
            let acc = fold_cells(view, cells.into_iter());
            // The accumulated envelope's coverage carries every region the folded `Held` views point
            // into, and its home is the destination frame the folds minted into. Merging it into a
            // bare handle on that same region assembles the aggregate at a fold door and mints its
            // description there in one step: `dest_frame`'s region is the product's host, and it
            // rides the members too — a record literal's fresh substrate borrows into the very
            // region it was built in, which the accumulator's own pins name.
            let dest_frame = view.dest_frame();
            let types = view.types();
            // The accumulated coverage pins every region the folded cells reach — the holder-rule
            // proof the substrate door reads their stored reach under, and the declared reach for a
            // cell that carries no stored description of its own (a spliced expression).
            let holder = acc.coverage().clone();
            let built = acc.merge_into::<DestHandleFamily, CarriedFamily, KoanStorageProfile>(
                dest_brand(dest_frame),
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
        self.submit_dep_finish_witnessed_in_own_scope(deps, finish)
    }

    /// Schedule a list-literal materialization as a witnessed dep-finish over its element producers.
    /// Bare identifier elements are name-resolved like dict values, so `[n, n]` holds `n`'s bound
    /// value and the memoized element type joins the resolved values' types.
    pub(in crate::machine::execute) fn schedule_list_literal<'a>(
        &mut self,
        brand: RegionBrand<'a>,
        items: &[ExpressionPart<'a>],
    ) -> NodeId {
        let mut deps = Deps::new();
        let mut rows = Vec::with_capacity(items.len());
        for &part in items {
            let value = self.classify_aggregate_part(brand, part, &mut deps);
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
        brand: RegionBrand<'a>,
        pairs: &[(ExpressionPart<'a>, ExpressionPart<'a>)],
    ) -> NodeId {
        let mut deps = Deps::new();
        let mut rows = Vec::with_capacity(pairs.len());
        for &(k, v) in pairs {
            let key = self.classify_aggregate_part(brand, k, &mut deps);
            let value = self.classify_aggregate_part(brand, v, &mut deps);
            rows.push(AggRow {
                key: Some(key),
                value,
            });
        }
        self.schedule_aggregate(
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
        brand: RegionBrand<'a>,
        fields: &[(&'a str, ExpressionPart<'a>)],
    ) -> NodeId {
        let mut names: Vec<String> = Vec::with_capacity(fields.len());
        let mut deps = Deps::new();
        let mut rows = Vec::with_capacity(fields.len());
        for &(name, value) in fields {
            let value = self.classify_aggregate_part(brand, value, &mut deps);
            names.push(name.to_string());
            rows.push(AggRow { key: None, value });
        }
        self.schedule_aggregate(
            deps,
            rows,
            Box::new(move |door, _keys, value_helds, types| {
                let record: Record<Held<'_>> =
                    names.into_iter().zip(value_helds.iter().copied()).collect();
                KObject::record_of_held(door, record, types)
            }),
        )
    }

    /// Plan one slot of a list / dict literal. The bare-name ladder does no cycle check — the
    /// dep-finish slot does not yet exist, so a still-finalizing name parks and cycles are caught
    /// post-submission against the dep-finish ID.
    fn classify_aggregate_part<'a>(
        &mut self,
        brand: RegionBrand<'a>,
        part: ExpressionPart<'a>,
        deps: &mut Deps<NodeId>,
    ) -> Slot {
        let part = match stage_eager_part(brand, part) {
            Ok(dep) => return Slot::owned(deps, self.realize_eager_dep(brand, dep)),
            Err(part) => part,
        };
        match part {
            ExpressionPart::QuotedExpression(_) => {
                // A quote rides its own one-part sub-dispatch (the `LiteralPassThrough` lane, which
                // seals it through the expression door) rather than a static cell: a
                // `KObject::KExpression` is invariant in its region lifetime with no `'static`
                // rebuild, so `resolve_region_pure` cannot build it at the `yoke` brand below.
                let wrapped =
                    WorkingExpression::new(brand, vec![Spanned::bare(WorkingPart::Ast(part))]);
                Slot::owned(
                    deps,
                    self.dispatch_in_own_scope(wrapped, SubmitContext::SubDispatch {}),
                )
            }
            ref p @ ExpressionPart::Identifier(_) => {
                self.resolve_aggregate_bare_name(brand, p, deps)
            }
            ref p @ ExpressionPart::Type(_) => self.resolve_aggregate_bare_name(brand, p, deps),
            other => {
                // A static literal (keyword / literal): region-pure — every borrow it carries
                // points into the classify scope's own frame, a string literal's bumped bytes
                // included — so the cell is built **inside** a zero-dep fold, born co-located with
                // that frame as its reach rather than resolved at the ambient lifetime and bundled
                // under an asserted witness. The cell then folds uniformly with the dep cells.
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
    /// (and the dep-finish's dep-error short-circuit) handles it uniformly.
    fn resolve_aggregate_bare_name<'a>(
        &mut self,
        brand: RegionBrand<'a>,
        part: &ExpressionPart<'a>,
        deps: &mut Deps<NodeId>,
    ) -> Slot {
        let active_chain = self.ambient.active_payload().map(|p| &p.chain);
        // `BareCarrier` is lifetime-free, so the whole result escapes the branded-scope closure and
        // the `&mut self` fallback runs after the read closes.
        let resolved = with_current_node_scope(&self.ambient, |s| {
            resolve_bare_carrier(s, part, active_chain, self.ambient.type_registry())
        });
        match resolved {
            BareCarrier::Sealed(cell) => Slot::Static(cell),
            BareCarrier::Parked(source) => Slot::Park(deps.park_on(source.scheduler_edge())),
            // Unbound: fall back to a sub-Dispatch so the `BareIdentifier` fast lane's error path
            // surfaces it uniformly.
            BareCarrier::Unbound(_) => {
                let expr =
                    WorkingExpression::new(brand, vec![Spanned::bare(WorkingPart::Ast(*part))]);
                Slot::owned(
                    deps,
                    self.dispatch_in_own_scope(expr, SubmitContext::SubDispatch {}),
                )
            }
        }
    }
}
