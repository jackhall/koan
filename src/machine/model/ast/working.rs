//! The scheduler's per-call expression form.
//!
//! A [`KExpression`] is raw AST and never changes; dispatch, though, needs to write resolved
//! sub-results back into an expression's slots. [`WorkingExpression`] is where that happens — a
//! per-call node in the destination frame's region whose parts either point at the AST
//! ([`WorkingPart::Ast`], a pointer copy) or carry something only the scheduler makes: a resolved
//! sub-result at rest, a staging hole, or a nested node the reducer synthesized.
//!
//! Keeping the two apart is what keeps koan out of reach arithmetic. A value cell holds a
//! [`KExpression`] (see [`KObject::KExpression`]), which has no arm that can name a producer region,
//! so no expression entering a sectioned container has a reach to describe. A working node cannot
//! reach the value channel at all — not by audit, but because no constructor takes one.

use crate::machine::core::{RegionBrand, read_resting};
use crate::machine::model::labels::{BinderSymbol, KeywordSymbol, LabelInterner};
use crate::machine::model::{Carried, Held, KObject, RunRegistries};
use crate::machine::model::{KeyElement, UntypedKey};
use crate::machine::{AdoptSeam, SplicedCell};
use crate::source::{FileId, SourceRef, Span, Spanned};

use super::shape::{
    DispatchShape, FieldSlot, Part, PartClass, PartSummary, classify_dispatch_shape,
    operator_probe_for, part_summary, stored_untyped_key,
};
use super::{ExpressionPart, KExpression, RunIter};
use crate::machine::model::StoredBinderKey;
use crate::machine::model::lazy_slots::{LazyKinds, LazySlotSpec};

#[cfg(test)]
mod tests;

/// One slot of a working expression.
#[derive(Clone, Copy)]
pub enum WorkingPart<'a> {
    /// A part of the AST this node was made from — a pointer copy into the storage that parsed it,
    /// never a rebuild.
    Ast(ExpressionPart<'a>),
    /// A nested node the scheduler synthesized, as the operator-chain fold's right-associated
    /// accumulator does. Distinct from `Ast(ExpressionPart::Expression(_))`, which points at a
    /// parsed sub-node.
    Expression(&'a WorkingExpression<'a>),
    /// A `:{…}` record-type body whose co-declared references are already threaded. Distinct from
    /// [`Expression`](WorkingPart::Expression) because a record-type sigil is not transparent: its
    /// body is a field list its own handler elaborates, never an expression to dispatch, so the slot
    /// must keep classifying as [`PartClass::RecordType`].
    RecordType(&'a WorkingExpression<'a>),
    /// A resolved sub-result **at rest**: its producer's sealed carrier — value and reach description
    /// as one unit — and nothing else. `Copy` and `Drop`-free; the pins that keep its backing alive
    /// live one level down, in the region the splice site rested it into
    /// ([`Scope::rest_delivered`](crate::machine::core::Scope::rest_delivered)), exactly as a binding
    /// entry's pins do.
    ///
    /// The cell rests on the working expression across steps and is read under that region's owner:
    /// [`Scope::lift_spliced`](crate::machine::core::Scope::lift_spliced) re-owns its reach for a
    /// consuming adoption, and a verdict-only probe opens it at its own brand
    /// ([`KType::accepts_cell`](crate::machine::model::KType::accepts_cell)) with nothing minted.
    ///
    /// `from_name` is the bare name this slot held before the splice replaced it — `Some` for an
    /// auto-wrapped operand and a threaded sibling reference, `None` for a sub-dispatch's result,
    /// which was an expression and never a name. A body reads it back through
    /// [`BoundArgs::surface_name`](crate::machine::BoundArgs::surface_name) to quote the operand as
    /// the source spelled it: a [`BinderSymbol`] is a fixed-width content digest, so carrying it
    /// costs the part no allocation and no borrow, and the diagnostic never re-derives a class
    /// from text. Not every named type can be recovered from its handle — a `UNION` or `SIG`
    /// binding interns structurally — which is why the spelling rides here rather than being
    /// looked up after the fact.
    Spliced {
        cell: SplicedCell<'a>,
        from_name: Option<BinderSymbol>,
    },
    /// A positional argument slot whose eager value is being produced by a sibling dispatch,
    /// awaiting its resolved carrier. The keyworded part walk stages an eager part as an owned
    /// [`DepRequest`](crate::machine::core::DepRequest) and leaves this marker in its slot so the
    /// parts run keeps its length and index alignment; `install_eager_subs`'s finish rebuilds the
    /// run with each marked slot replaced by its resolved [`Spliced`](WorkingPart::Spliced) cell. It
    /// is a scheduler-internal hole, never a language-level value — it exists only between staging
    /// and splice and is never name-resolved.
    StagedSlot,
}

impl<'a> Part<'a> for WorkingPart<'a> {
    fn class(&self) -> PartClass {
        match self {
            WorkingPart::Ast(part) => part.class(),
            WorkingPart::Expression(_) => PartClass::Expression,
            WorkingPart::RecordType(_) => PartClass::RecordType,
            WorkingPart::Spliced { .. } => PartClass::Spliced,
            WorkingPart::StagedSlot => PartClass::StagedSlot,
        }
    }

    fn field_slot(&self) -> FieldSlot<'a> {
        match self {
            WorkingPart::Ast(part) => part.field_slot(),
            // A threaded sigil body rides the `Expression` arm: the rewrite drops the transparent
            // `:(…)` wrapper, since the wrapper's handler does no more than dispatch its body.
            WorkingPart::Expression(body) => FieldSlot::ThreadedSigil(body),
            WorkingPart::RecordType(body) => FieldSlot::ThreadedRecord(body),
            WorkingPart::Spliced { cell, .. } => FieldSlot::Resolved(*cell),
            WorkingPart::StagedSlot => FieldSlot::Other,
        }
    }

    fn write_summary(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        registries: &RunRegistries,
    ) -> std::fmt::Result {
        WorkingPart::write_summary(self, f, registries)
    }
}

/// Registry-free rendering of a spliced cell's carried value, for `Debug`, which carries no run
/// state to resolve a name or a type node through. Every arm renders a bare handle — a type's own
/// digest, an unlowered name's symbol, an object's *type* digest — so two values of one type read
/// alike here. `Debug` is the only path that renders one; the summary path names the type.
///
/// Reached through [`read_resting`], which states the coverage a pin-less probe stands under.
fn spliced_debug(carried: Carried<'_>) -> String {
    match carried {
        Carried::Type(kt) => format!("0x{:032x}", kt.digest().0),
        Carried::UnresolvedType(name) => format!("0x{:032x}", name.symbol().0),
        Carried::Object(object) => format!("0x{:032x}", object.ktype().digest().0),
    }
}

impl<'a> std::fmt::Debug for WorkingPart<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkingPart::Ast(part) => part.fmt(f),
            WorkingPart::Expression(e) => f.debug_tuple("Expression").field(e).finish(),
            WorkingPart::RecordType(e) => f.debug_tuple("RecordType").field(e).finish(),
            WorkingPart::Spliced { cell, .. } => {
                write!(f, "Spliced({})", read_resting(cell, spliced_debug))
            }
            WorkingPart::StagedSlot => write!(f, "StagedSlot"),
        }
    }
}

impl<'a> WorkingPart<'a> {
    /// Per-part subset of [`WorkingExpression::write_summary`], written straight into `f`.
    ///
    /// Every argument slot renders **the type dispatch matched it on**, never the value nor the
    /// spelling that produced it: a summary is read inside a diagnostic explaining a dispatch
    /// outcome, and the type is the whole of what that decision saw. The dispatch error kinds own
    /// the expression's site — not only the trace frames an enclosing call pushes — so a reader
    /// locates the spelling from the rendered site rather than from an echo here.
    ///
    /// The arms mirror [`KType::accepts_working_part`], which is what decided the outcome being
    /// explained. A keyword fills no slot, so it stays its own spelling.
    /// [`KType::slot_ktype`] answers the AST arm and [`Carried::ktype`] the resolved one; the
    /// scheduler's own unfilled arms denote no value yet — only an `Any` slot admits one — so they
    /// say that rather than name a type they do not have.
    ///
    /// Naming a type is what widens this path from the interner to the whole bundle:
    /// [`KType::write_name`] reads the type registry.
    pub fn write_summary(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        registries: &RunRegistries,
    ) -> std::fmt::Result {
        match self {
            WorkingPart::Ast(part) => {
                match crate::machine::model::KType::slot_ktype(part, &registries.types) {
                    Some(slot) => slot.write_name(f, registries),
                    // A keyword is fixed syntax filling no slot — it renders as itself.
                    None => part.write_summary(f, &registries.labels),
                }
            }
            // Reached through `read_resting`, which states the coverage a pin-less probe stands
            // under.
            WorkingPart::Spliced { cell, .. } => read_resting(cell, |carried| {
                carried.ktype(&registries.types).write_name(f, registries)
            }),
            // A node the scheduler synthesized and will dispatch, and a hole awaiting a sibling's
            // carrier, both denote no value yet — only an `Any` slot admits one, so neither
            // narrowed the candidate set that missed. They say so rather than name a type they do
            // not have.
            WorkingPart::Expression(_) | WorkingPart::RecordType(_) | WorkingPart::StagedSlot => {
                f.write_str("<staged>")
            }
        }
    }

    /// The part's own surface spelling, for a position that is **not** an argument slot and so was
    /// never matched on a type — a binder's name. Every other position renders through
    /// [`write_summary`](Self::write_summary), which names the type instead.
    pub fn write_spelling(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        labels: &LabelInterner,
    ) -> std::fmt::Result {
        match self {
            WorkingPart::Ast(part) => part.write_summary(f, labels),
            // A name is an AST token wherever a binder declares one; the scheduler's own arms reach
            // here only if a synthesized node claims a name slot, which nothing does.
            WorkingPart::Expression(_)
            | WorkingPart::RecordType(_)
            | WorkingPart::Spliced { .. }
            | WorkingPart::StagedSlot => f.write_str("<staged>"),
        }
    }

    /// [`write_summary`](Self::write_summary) as a `Display` view.
    pub fn summary<'x>(
        &'x self,
        registries: &'x RunRegistries,
    ) -> PartSummary<'x, WorkingPart<'a>> {
        part_summary(self, registries)
    }

    /// The part's surface as an owned `String`.
    pub fn summarize(&self, registries: &RunRegistries) -> String {
        self.summary(registries).to_string()
    }

    /// The AST part this slot holds, if it holds one. The scheduler's own arms answer `None`.
    pub fn as_ast(&self) -> Option<ExpressionPart<'a>> {
        match self {
            WorkingPart::Ast(part) => Some(*part),
            _ => None,
        }
    }

    /// Slot-aware resolve producing an owned [`Held`] cell, run at
    /// [`KFunction::bind_args`](crate::machine::KFunction::bind_args) time. A spliced cell is first
    /// adopted into `scope` (reach folded, value re-anchored at the scope brand) so a cloned type
    /// that still borrows the producer region stays pinned before it is owned-ified; every AST arm
    /// defers to [`ExpressionPart::resolve_for`].
    pub fn resolve_for(
        &self,
        slot: &crate::machine::model::KType,
        scope: &'a crate::machine::core::Scope<'a>,
        types: &crate::machine::model::TypeRegistry,
    ) -> Held<'a> {
        match self {
            WorkingPart::Spliced { cell, .. } => {
                // The cell rests in this scope's region (the splice site put it there), so the
                // scope's own owner covers the lift that re-owns its reach for the adoption below.
                let delivered = scope.lift_spliced(cell);
                match scope.adopt_carried(&delivered, AdoptSeam::Retaining) {
                    Carried::Type(kt) => Held::Type(kt),
                    Carried::UnresolvedType(ti) => Held::UnresolvedType(ti),
                    Carried::Object(obj) => Held::Object(obj.deep_clone()),
                }
            }
            WorkingPart::Ast(part) => part.resolve_for(slot, scope, types),
            _ => Held::Object(self.resolve(scope.brand())),
        }
    }

    /// The [`KObject`] this slot denotes, built into `brand`'s region.
    pub fn resolve(&self, brand: RegionBrand<'a>) -> KObject<'a> {
        match self {
            WorkingPart::Ast(part) => part.resolve(brand),
            // A value cell holds a `KExpression`, so a node the scheduler synthesized has no way to
            // become one — and never needs to: a synthesized node is always on its way to
            // `become_dispatch`, while every raw capture a `:KExpression` slot makes is of parsed
            // AST, which rides the `Ast` arm above.
            WorkingPart::Expression(_) | WorkingPart::RecordType(_) => unreachable!(
                "a synthesized nested node is dispatched, never captured as a value; \
                 a raw `:KExpression` capture is always of parsed AST"
            ),
            // A spliced cell is opened / adopted at the consuming scope's brand before resolution,
            // so its value never reaches the region-less `resolve()`.
            WorkingPart::Spliced { .. } => unreachable!(
                "a spliced cell is adopted at the binding scope before resolve(); \
                 resolve() runs only on region-pure parts"
            ),
            // A staged slot is a scheduler-internal hole: `install_eager_subs`'s finish splices
            // every marked slot into a `Spliced` cell before anything binds or resolves it.
            WorkingPart::StagedSlot => unreachable!(
                "StagedSlot is a transient staging hole; install_eager_subs splices it before \
                 resolve() runs"
            ),
        }
    }

    /// The [`KObject`] a **region-pure** slot denotes, at *any* lifetime — the lifetime-generic peer
    /// of [`resolve`](Self::resolve) for static-cell sites that fold. Only an AST arm is ever
    /// region-pure; the scheduler's own arms are classified to sub-dispatches before any
    /// static cell.
    pub fn resolve_region_pure<'b>(&self, brand: RegionBrand<'b>) -> KObject<'b> {
        match self {
            WorkingPart::Ast(part) => part.resolve_region_pure(brand),
            _ => unreachable!(
                "resolve_region_pure is only called on a region-pure static-cell part; \
                 synthesized nodes, spliced cells and staging holes are classified to owned \
                 sub-dispatches before any static cell"
            ),
        }
    }
}

/// A dispatch's own working copy of an expression: the parts run lives in the destination frame's
/// region, so the scheduler can write a resolved sub-result into a slot by rebuilding the run
/// through a door rather than mutating frozen storage.
///
/// Carries the same structural cache a [`KExpression`] does — copied over verbatim when the node is
/// made from one, since the cache is invariant under splice, and computed outright for a node the
/// scheduler synthesized — except the two binder caches (the plan and the declared-name position),
/// which are copied over and never computed here: a binder is always parsed AST, so a synthesized
/// node carries `None` for both and installs nothing. The lazy-slot stamp is filled at every door,
/// synthesized runs included: it is a fact about the bucket key, which a synthesized run carries as
/// plainly as a parsed one.
///
/// One fact here is not structural: [`under_type_sigil`](Self::under_type_sigil), the type-context
/// stamp the `:(…)` handler sets on the body it re-dispatches. It rides beside the cache because it
/// is invariant under splice for the same reason the cache is — a splice substitutes slots, and the
/// node's own context does not change with what fills them.
#[derive(Clone, Copy)]
pub struct WorkingExpression<'a> {
    pub parts: &'a [Spanned<WorkingPart<'a>>],
    pub span: Option<Span>,
    pub file: Option<FileId>,
    untyped_key: &'a [KeyElement],
    shape: DispatchShape,
    operator_probe: Option<KeywordSymbol>,
    binder_plan: Option<&'a StoredBinderKey<'a>>,
    lazy_slots: Option<&'static LazySlotSpec>,
    binder_name_slot: Option<usize>,
    under_type_sigil: bool,
}

/// The source extent a parts run covers: the union of the extents its parts carry, or `None` when
/// no part carries one.
fn parts_extent(parts: &[Spanned<WorkingPart<'_>>]) -> Option<Span> {
    parts
        .iter()
        .filter_map(|part| part.span)
        .reduce(|a, b| Span {
            start: a.start.min(b.start),
            end: a.end.max(b.end),
        })
}

impl<'a> WorkingExpression<'a> {
    /// Spanless construction door for a borrowed run the scheduler built itself.
    pub fn new(brand: RegionBrand<'a>, parts: &[Spanned<WorkingPart<'a>>]) -> Self {
        Self::build(brand, parts, None, None)
    }

    /// [`new`](Self::new)'s peer for a run whose slots are computed — see [`RunIter`].
    pub fn new_from_iter<I>(brand: RegionBrand<'a>, parts: I) -> Self
    where
        I: IntoIterator<Item = Spanned<WorkingPart<'a>>>,
        RunIter<I>: ExactSizeIterator,
    {
        Self::build_from_iter(brand, parts, None, None)
    }

    /// Construction door for a run the machine synthesized **out of** `origin` — an operator-chain
    /// reduction, an extracted head, a wrapped literal element. The node takes `origin`'s file and
    /// the extent its own parts span, so a synthesized node still names the source it came from
    /// rather than reporting as location-free.
    ///
    /// Parts carry their own extents down from the parse, so the union is exact where they survive
    /// the synthesis; a run assembled entirely from spanless parts falls back to `origin`'s extent,
    /// which is the enclosing expression the synthesis is standing in for either way.
    pub fn synthesized(
        brand: RegionBrand<'a>,
        parts: &[Spanned<WorkingPart<'a>>],
        origin: &WorkingExpression<'a>,
    ) -> Self {
        let parts = brand.allocator().slice(parts);
        Self::from_run(
            brand,
            parts,
            parts_extent(parts).or(origin.span),
            origin.file,
        )
    }

    /// Construction door for a borrowed run: copy it into `brand`'s region, then fill the cache. A
    /// node built here is not a binder — binders are parsed statements.
    pub fn build(
        brand: RegionBrand<'a>,
        parts: &[Spanned<WorkingPart<'a>>],
        span: Option<Span>,
        file: Option<FileId>,
    ) -> Self {
        Self::from_run(brand, brand.allocator().slice(parts), span, file)
    }

    /// [`build`](Self::build)'s peer for a run whose slots are computed — see [`RunIter`].
    pub fn build_from_iter<I>(
        brand: RegionBrand<'a>,
        parts: I,
        span: Option<Span>,
        file: Option<FileId>,
    ) -> Self
    where
        I: IntoIterator<Item = Spanned<WorkingPart<'a>>>,
        RunIter<I>: ExactSizeIterator,
    {
        Self::from_run(brand, brand.allocator().slice_from_iter(parts), span, file)
    }

    /// Construction chokepoint, over a parts run **already resident** in `brand`'s region: fills the
    /// structural cache from it and does nothing else. Every door above lands here, differing only
    /// in how the run reached the region; [`respliced`](Self::respliced) is the one door that does
    /// not, since it inherits the cache it would otherwise rebuild.
    fn from_run(
        brand: RegionBrand<'a>,
        parts: &'a [Spanned<WorkingPart<'a>>],
        span: Option<Span>,
        file: Option<FileId>,
    ) -> Self {
        let shape = classify_dispatch_shape(parts);
        WorkingExpression {
            parts,
            span,
            file,
            untyped_key: stored_untyped_key(brand, parts),
            shape,
            operator_probe: operator_probe_for(parts, shape),
            binder_plan: None,
            lazy_slots: crate::machine::model::lazy_slots::lazy_slot_spec_for(parts),
            binder_name_slot: None,
            under_type_sigil: false,
        }
    }

    /// The working copy of a parsed node: one slice copy of the parts run with each part wrapped as
    /// [`WorkingPart::Ast`], and the whole structural cache carried over as pointer copies. Shallow
    /// by construction — a nested sub-node stays AST until it is itself dispatched.
    pub fn from_ast(brand: RegionBrand<'a>, ast: KExpression<'a>) -> Self {
        WorkingExpression {
            parts: brand
                .allocator()
                .slice_from_iter(ast.parts.iter().map(|part| Spanned {
                    value: WorkingPart::Ast(part.value),
                    span: part.span,
                })),
            span: ast.span,
            file: ast.file,
            untyped_key: ast.stored_key(),
            shape: ast.shape(),
            operator_probe: ast.operator_probe(),
            binder_plan: ast.binder_plan_ref(),
            lazy_slots: ast.lazy_slots(),
            binder_name_slot: ast.binder_name_slot(),
            under_type_sigil: false,
        }
    }

    /// Rebuild this node with a new parts run — the splice path: a part walk replaces each wrap slot
    /// with its resolved cell and each eager slot with a staging hole, and `install_eager_subs`'s
    /// finish replaces each hole with the cell the dep delivered. `span`, `file` and the binder
    /// caches ride through.
    ///
    /// The one door that does not land in [`from_run`](Self::from_run): a splice substitutes slots
    /// one for one and writes no keyword position, so the bucket key and the operator probe are
    /// invariant under it and ride through as the handles they already are. Re-deriving them would
    /// bump a second identical key run per splice, and a chain splices once per reduction step.
    ///
    /// Every caller's run is computed slot by slot, so this door only takes the iterator form —
    /// see [`RunIter`].
    pub fn respliced<I>(&self, brand: RegionBrand<'a>, parts: I) -> Self
    where
        I: IntoIterator<Item = Spanned<WorkingPart<'a>>>,
        RunIter<I>: ExactSizeIterator,
    {
        let parts = brand.allocator().slice_from_iter(parts);
        WorkingExpression {
            parts,
            span: self.span,
            file: self.file,
            untyped_key: self.untyped_key,
            shape: classify_dispatch_shape(parts),
            operator_probe: self.operator_probe,
            binder_plan: self.binder_plan,
            lazy_slots: self.lazy_slots,
            binder_name_slot: self.binder_name_slot,
            under_type_sigil: self.under_type_sigil,
        }
    }

    /// Stamp this node as the body of a `:(…)` type expression — the one door, taken by the sigil
    /// handler on the body it re-dispatches. See [`under_type_sigil`](Self::under_type_sigil).
    pub fn in_type_context(mut self) -> Self {
        self.under_type_sigil = true;
        self
    }

    /// Was this node reached through the type sigil? A builtin body reads it off its
    /// [`BodyCtx`](crate::machine::BodyCtx) to answer in the type universe where the bare
    /// expression answers in the value universe — `:(Ordered.compare)` names the `VAL` slot's
    /// declared type, `Ordered.compare` names no member at all.
    ///
    /// The stamp marks one node, not a subtree: the sigil handler re-labels each qualifying
    /// value-context part of the body it dispatches, so a nested projection re-enters that handler
    /// and stamps itself. A part that is not a type-lhs projection is left alone and keeps reading
    /// on the value channel.
    pub fn under_type_sigil(&self) -> bool {
        self.under_type_sigil
    }

    /// Cached dispatch shape (see [`classify_dispatch_shape`]).
    pub fn shape(&self) -> DispatchShape {
        self.shape
    }

    /// Cached operator-registry probe key: `Some` only for an `OperatorChain`.
    pub fn operator_probe(&self) -> Option<KeywordSymbol> {
        self.operator_probe
    }

    /// What the parsed node this working copy was made from installs when submitted as a
    /// statement. `None` for a node the scheduler synthesized — a binder is always a parsed
    /// statement.
    pub fn binder_plan(&self) -> Option<StoredBinderKey<'a>> {
        self.binder_plan.copied()
    }

    /// The kinds of part that stay raw at slot `index` — the seal-time lazy-slot stamp, empty at
    /// every slot of every form that has none. See
    /// [`lazy_slots`](crate::machine::model::lazy_slots).
    pub fn lazy_kinds_at(&self, index: usize) -> LazyKinds {
        self.lazy_slots
            .map_or(LazyKinds::EMPTY, |spec| spec.kinds_at(index))
    }

    /// The declared-name position of the binder form this node matches — see
    /// [`KExpression::binder_name_slot`]. `None` for a node the scheduler synthesized.
    pub fn binder_name_slot(&self) -> Option<usize> {
        self.binder_name_slot
    }

    /// The stored bucket key, as a borrow of the run bumped at construction.
    pub fn stored_key(&self) -> &'a [KeyElement] {
        self.untyped_key
    }

    /// Bucket key, materialized owned for a bucket-table lookup. See
    /// [`KExpression::untyped_key`].
    pub fn untyped_key(&self) -> UntypedKey {
        self.untyped_key.to_vec()
    }

    /// Surface rendering of the whole expression, written straight into `f`, resolving each
    /// symbol-carrying part through the run's registries.
    pub fn write_summary(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        registries: &RunRegistries,
    ) -> std::fmt::Result {
        for (index, part) in self.parts.iter().enumerate() {
            if index > 0 {
                f.write_str(" ")?;
            }
            // A binder's name slot is not an argument: nothing dispatches on it, it is the name
            // being installed. Naming its type would render every `LET` alike, so it keeps its
            // spelling; every other slot renders the type dispatch matched it on.
            if Some(index) == self.binder_name_slot {
                part.value.write_spelling(f, &registries.labels)?;
            } else {
                part.value.write_summary(f, registries)?;
            }
        }
        Ok(())
    }

    /// [`write_summary`](Self::write_summary) as a `Display` view.
    pub fn summary<'x>(&'x self, registries: &'x RunRegistries) -> WorkingSummary<'x, 'a> {
        WorkingSummary {
            expression: self,
            registries,
        }
    }

    /// The expression's surface as an owned `String`.
    pub fn summarize(&self, registries: &RunRegistries) -> String {
        self.summary(registries).to_string()
    }

    /// This expression's source extent, `Some` when both span and file are populated.
    pub fn source_ref(&self) -> Option<SourceRef> {
        self.span
            .zip(self.file)
            .map(|(span, file)| SourceRef { span, file })
    }
}

/// A [`WorkingExpression::summary`] view: one expression plus the registries its parts resolve
/// through.
pub struct WorkingSummary<'x, 'a> {
    expression: &'x WorkingExpression<'a>,
    registries: &'x RunRegistries,
}

impl std::fmt::Display for WorkingSummary<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.expression.write_summary(f, self.registries)
    }
}

impl<'a> std::fmt::Debug for WorkingExpression<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkingExpression")
            .field("parts", &self.parts)
            .finish()
    }
}
