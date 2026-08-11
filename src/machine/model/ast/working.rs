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
use crate::machine::model::{Carried, Held, KObject};
use crate::machine::model::{StoredElement, UntypedElement, UntypedKey};
use crate::machine::{AdoptSeam, SplicedCell};
use crate::source::{FileId, Span, Spanned};

use super::shape::{
    DispatchShape, FieldSlot, Part, PartClass, classify_dispatch_shape, operator_probe_for,
    stored_untyped_key,
};
use super::{ExpressionPart, KExpression};
use crate::machine::model::StoredBinderKey;

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
    Spliced { cell: SplicedCell<'a> },
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
    fn class(&self) -> PartClass<'a> {
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
            WorkingPart::Spliced { cell } => FieldSlot::Resolved(*cell),
            WorkingPart::StagedSlot => FieldSlot::Other,
        }
    }

    fn summarize(&self) -> String {
        WorkingPart::summarize(self)
    }
}

/// Registry-free rendering of a spliced cell's carried value, for `Debug` and the registry-free
/// [`WorkingPart::summarize`]. A type name resolves through the registry, which neither signature
/// carries, so the type channel renders its content-digest hex — the value's own identity — and an
/// object renders its type's digest. An unlowered name is already a bare surface string.
///
/// Reached through [`read_resting`], which states the coverage a pin-less probe stands under.
fn spliced_summary(carried: Carried<'_>) -> String {
    match carried {
        Carried::Type(kt) => format!("0x{:032x}", kt.digest().0),
        Carried::UnresolvedType(ti) => ti.render(),
        Carried::Object(object) => format!("0x{:032x}", object.ktype().digest().0),
    }
}

impl<'a> std::fmt::Debug for WorkingPart<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkingPart::Ast(part) => part.fmt(f),
            WorkingPart::Expression(e) => f.debug_tuple("Expression").field(e).finish(),
            WorkingPart::RecordType(e) => f.debug_tuple("RecordType").field(e).finish(),
            WorkingPart::Spliced { cell } => {
                write!(f, "Spliced({})", read_resting(cell, spliced_summary))
            }
            WorkingPart::StagedSlot => write!(f, "StagedSlot"),
        }
    }
}

impl<'a> WorkingPart<'a> {
    /// Per-part subset of [`WorkingExpression::summarize`].
    pub fn summarize(&self) -> String {
        match self {
            WorkingPart::Ast(part) => part.summarize(),
            WorkingPart::Expression(e) => e.summarize(),
            WorkingPart::RecordType(e) => format!(":{{{}}}", e.summarize()),
            WorkingPart::Spliced { cell } => read_resting(cell, spliced_summary),
            WorkingPart::StagedSlot => "<staged>".to_string(),
        }
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
        types: &crate::machine::model::types::TypeRegistry,
    ) -> Held<'a> {
        match self {
            WorkingPart::Spliced { cell } => {
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
    /// region-pure; the scheduler's own arms are classified to owned sub-dispatches before any
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
/// scheduler synthesized — except the binder plan, which is copied over and never computed here: a
/// binder is always parsed AST, so a synthesized node carries `None` and installs nothing.
#[derive(Clone, Copy)]
pub struct WorkingExpression<'a> {
    pub parts: &'a [Spanned<WorkingPart<'a>>],
    pub span: Option<Span>,
    pub file: Option<FileId>,
    untyped_key: &'a [StoredElement<'a>],
    shape: DispatchShape,
    operator_probe: Option<&'a str>,
    binder_plan: Option<&'a StoredBinderKey<'a>>,
}

impl<'a> WorkingExpression<'a> {
    /// Spanless construction door for a run the scheduler built itself.
    pub fn new(brand: RegionBrand<'a>, parts: Vec<Spanned<WorkingPart<'a>>>) -> Self {
        Self::build(brand, parts, None, None)
    }

    /// Construction chokepoint: bumps the parts run into `brand`'s region and fills the structural
    /// cache from it. A node built here is not a binder — binders are parsed statements.
    pub fn build(
        brand: RegionBrand<'a>,
        parts: Vec<Spanned<WorkingPart<'a>>>,
        span: Option<Span>,
        file: Option<FileId>,
    ) -> Self {
        let parts = brand.allocator().slice(&parts);
        let shape = classify_dispatch_shape(parts);
        WorkingExpression {
            parts,
            span,
            file,
            untyped_key: stored_untyped_key(brand, parts),
            shape,
            operator_probe: operator_probe_for(brand, parts, shape),
            binder_plan: None,
        }
    }

    /// The working copy of a parsed node: one slice copy of the parts run with each part wrapped as
    /// [`WorkingPart::Ast`], and the whole structural cache carried over as pointer copies. Shallow
    /// by construction — a nested sub-node stays AST until it is itself dispatched.
    pub fn from_ast(brand: RegionBrand<'a>, ast: KExpression<'a>) -> Self {
        let parts: Vec<Spanned<WorkingPart<'a>>> = ast
            .parts
            .iter()
            .map(|part| Spanned {
                value: WorkingPart::Ast(part.value),
                span: part.span,
            })
            .collect();
        WorkingExpression {
            parts: brand.allocator().slice(&parts),
            span: ast.span,
            file: ast.file,
            untyped_key: ast.stored_key(),
            shape: ast.shape(),
            operator_probe: ast.operator_probe(),
            binder_plan: ast.binder_plan_ref(),
        }
    }

    /// Rebuild this node with a new parts run — the splice path: `install_eager_subs`'s finish
    /// replaces each staging hole with its resolved cell and freezes the result. `span` and `file`
    /// ride through, and the cache refills from the new run.
    pub fn respliced(&self, brand: RegionBrand<'a>, parts: Vec<Spanned<WorkingPart<'a>>>) -> Self {
        let mut rebuilt = Self::build(brand, parts, self.span, self.file);
        rebuilt.binder_plan = self.binder_plan;
        rebuilt
    }

    /// Build a node and bump it, for the [`WorkingPart::Expression`] arm that nests one.
    pub fn nested(
        brand: RegionBrand<'a>,
        parts: Vec<Spanned<WorkingPart<'a>>>,
    ) -> &'a WorkingExpression<'a> {
        brand.allocator().value(Self::new(brand, parts))
    }

    /// Cached dispatch shape (see [`classify_dispatch_shape`]).
    pub fn shape(&self) -> DispatchShape {
        self.shape
    }

    /// Cached operator-registry probe key: `Some` only for an `OperatorChain`.
    pub fn operator_probe(&self) -> Option<&'a str> {
        self.operator_probe
    }

    /// What the parsed node this working copy was made from installs when submitted as a
    /// statement. `None` for a node the scheduler synthesized — a binder is always a parsed
    /// statement.
    pub fn binder_plan(&self) -> Option<StoredBinderKey<'a>> {
        self.binder_plan.copied()
    }

    /// The stored bucket key, as a borrow of the run bumped at construction.
    pub fn stored_key(&self) -> &'a [StoredElement<'a>] {
        self.untyped_key
    }

    /// Bucket key, materialized owned for a bucket-table lookup. See
    /// [`KExpression::untyped_key`].
    pub fn untyped_key(&self) -> UntypedKey {
        self.untyped_key
            .iter()
            .map(|element| match element {
                StoredElement::Keyword(s) => UntypedElement::Keyword((*s).to_string()),
                StoredElement::Slot => UntypedElement::Slot,
            })
            .collect()
    }

    /// Surface rendering of the whole expression — parts only, so no registry is needed.
    pub fn summarize(&self) -> String {
        self.parts
            .iter()
            .map(|p| p.value.summarize())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl<'a> std::fmt::Debug for WorkingExpression<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkingExpression")
            .field("parts", &self.parts)
            .finish()
    }
}
