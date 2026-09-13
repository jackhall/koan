//! The scheduler's per-dispatch expression form.
//!
//! A [`KExpression`] is parsed AST and never changes; dispatch writes resolved sub-results back into
//! an expression's slots. A [`WorkingExpression`] is where that happens: a node in the executing
//! cell's region whose parts either point at the AST ([`WorkingPart::Ast`], a pointer copy at
//! `'graph`) or hold what only the scheduler makes — a resolved value, a nested node it synthesized,
//! or a staging hole. It is never a value and never crosses a cell: a continuation captures it at
//! the cell's own lifetime.

use std::fmt;

use crate::memory::{BumpAllocator, Writer};
use crate::parse::ast::RunIter;
use crate::parse::{
    BinderSymbol, DispatchShape, ExpressionPart, KExpression, KeyElement, KeywordSymbol,
    LabelInterner, LazyKinds, NodeCache, PartClass, StoredBinderKey,
};
use crate::source::{FileId, SourceRef, Span, Spanned};
use crate::type_lattice::{TypeRegistry, display_name};

use super::{Value, collect, part_ktype};

/// One slot of a working expression.
#[derive(Clone, Copy, Debug)]
pub enum WorkingPart<'graph, 'cell> {
    /// A part of the AST this node was made from — a pointer copy, never a rebuild.
    Ast(ExpressionPart<'graph>),
    /// A nested node the scheduler synthesized, as an operator-chain fold's accumulator is. Distinct
    /// from an `Ast` expression part, which points at a parsed sub-node.
    Expression(&'cell WorkingExpression<'graph, 'cell>),
    /// A `:{…}` record-type body whose co-declared references are threaded. Its own arm because a
    /// record-type body is a field list its handler elaborates, never an expression to dispatch, so
    /// the slot keeps classifying as a record type.
    RecordType(&'cell WorkingExpression<'graph, 'cell>),
    /// A resolved sub-result: a value reachable at this node's cell. `from_name` is the bare name the
    /// slot held before the splice — `Some` for a wrapped operand or a threaded sibling reference,
    /// `None` for a sub-dispatch's result — so a diagnostic quotes the operand as the source spelled
    /// it.
    Spliced {
        value: Value<'graph, 'cell>,
        from_name: Option<BinderSymbol>,
    },
    /// A positional slot whose eager value a sibling dispatch is producing. It keeps the run's length
    /// and alignment until the splice replaces it, and is never a value.
    StagedSlot,
}

impl<'graph, 'cell> WorkingPart<'graph, 'cell> {
    /// The structural family this part belongs to — what dispatch-shape classification reads.
    pub fn class(&self) -> PartClass {
        match self {
            WorkingPart::Ast(part) => part.class(),
            WorkingPart::Expression(_) => PartClass::Expression,
            WorkingPart::RecordType(_) => PartClass::RecordType,
            WorkingPart::Spliced { .. } => PartClass::Spliced,
            WorkingPart::StagedSlot => PartClass::StagedSlot,
        }
    }

    /// This part's position in a bucket key: a keyword's symbol, `Slot` for every other part.
    pub fn key_element(&self) -> KeyElement {
        match self.class() {
            PartClass::Keyword(symbol) => KeyElement::Keyword(symbol),
            _ => KeyElement::Slot,
        }
    }

    /// The AST part this slot holds, if it holds one.
    pub fn as_ast(&self) -> Option<ExpressionPart<'graph>> {
        match self {
            WorkingPart::Ast(part) => Some(*part),
            _ => None,
        }
    }

    /// The value this slot was spliced with, if it was.
    pub fn as_value(&self) -> Option<&Value<'graph, 'cell>> {
        match self {
            WorkingPart::Spliced { value, .. } => Some(value),
            _ => None,
        }
    }

    /// The part as a diagnostic names it: the type dispatch matched the slot on, never the value or
    /// the spelling that produced it. A keyword fills no slot and renders as itself; a synthesized
    /// node and a staging hole denote no value yet and say so.
    pub fn write_summary(
        &self,
        f: &mut fmt::Formatter<'_>,
        types: &TypeRegistry<'_>,
        labels: &LabelInterner,
        scratch: BumpAllocator<'_>,
    ) -> fmt::Result {
        match self {
            WorkingPart::Ast(part) => match part_ktype(part, types, scratch) {
                Some(slot) => write!(f, "{}", display_name(slot, types, labels)),
                None => part.write_summary(f, labels),
            },
            WorkingPart::Spliced { value, .. } => {
                write!(f, "{}", display_name(value.ktype(), types, labels))
            }
            WorkingPart::Expression(_) | WorkingPart::RecordType(_) | WorkingPart::StagedSlot => {
                f.write_str("<staged>")
            }
        }
    }

    /// The part's own spelling, for a position no type was matched on — a binder's name.
    fn write_spelling(&self, f: &mut fmt::Formatter<'_>, labels: &LabelInterner) -> fmt::Result {
        match self {
            WorkingPart::Ast(part) => part.write_summary(f, labels),
            WorkingPart::Expression(_)
            | WorkingPart::RecordType(_)
            | WorkingPart::Spliced { .. }
            | WorkingPart::StagedSlot => f.write_str("<staged>"),
        }
    }
}

/// A dispatch's own working copy of an expression, its parts run in the executing cell's region.
///
/// It carries the [`NodeCache`] a [`KExpression`] does — copied over when the node is made from
/// one, since a splice leaves it unchanged, and computed from the key for a node the scheduler
/// built. A built node has no binder plan: a binder is always parsed AST.
///
/// One fact is not structural: [`under_type_sigil`](Self::under_type_sigil), the stamp the `:(…)`
/// handler sets on the body it re-dispatches, which a splice leaves unchanged for the same reason.
#[derive(Clone, Copy)]
pub struct WorkingExpression<'graph, 'cell> {
    pub parts: &'cell [Spanned<WorkingPart<'graph, 'cell>>],
    pub span: Option<Span>,
    pub file: Option<FileId>,
    /// At `'cell`: a cache carried over from the AST shortens from `'graph`, one built here borrows
    /// the region.
    cache: NodeCache<'cell>,
    under_type_sigil: bool,
}

impl<'graph, 'cell> WorkingExpression<'graph, 'cell> {
    /// The working copy of a parsed node: its parts wrapped as [`WorkingPart::Ast`] in one run, the
    /// cache carried over whole. Shallow — a nested node stays AST until it is itself dispatched.
    pub fn from_ast(writer: Writer<'cell>, ast: &KExpression<'graph>) -> Self {
        WorkingExpression {
            parts: collect(
                writer,
                ast.parts.iter().map(|part| Spanned {
                    value: WorkingPart::Ast(part.value),
                    span: part.span,
                }),
            ),
            span: ast.span,
            file: ast.file,
            cache: *ast.cache(),
            under_type_sigil: false,
        }
    }

    /// A run the scheduler built, copied into the region, its cache computed from the key.
    pub fn build(
        writer: Writer<'cell>,
        parts: &[Spanned<WorkingPart<'graph, 'cell>>],
        span: Option<Span>,
        file: Option<FileId>,
    ) -> Self {
        Self::from_run(writer, collect(writer, parts.iter().copied()), span, file)
    }

    /// [`build`](Self::build) over a run whose slots are computed — see [`RunIter`].
    pub fn build_from_iter<I>(
        writer: Writer<'cell>,
        parts: I,
        span: Option<Span>,
        file: Option<FileId>,
    ) -> Self
    where
        I: IntoIterator<Item = Spanned<WorkingPart<'graph, 'cell>>>,
        RunIter<I>: ExactSizeIterator,
    {
        Self::from_run(writer, collect(writer, parts.into_iter()), span, file)
    }

    /// A run synthesized out of `origin` — a chain reduction, an extracted head. It takes `origin`'s
    /// file and the extent its own parts span, or `origin`'s extent when no part carries one.
    pub fn synthesized(
        writer: Writer<'cell>,
        parts: &[Spanned<WorkingPart<'graph, 'cell>>],
        origin: &WorkingExpression<'graph, 'cell>,
    ) -> Self {
        let extent = parts
            .iter()
            .filter_map(|part| part.span)
            .reduce(|left, right| Span {
                start: left.start.min(right.start),
                end: left.end.max(right.end),
            });
        Self::build(writer, parts, extent.or(origin.span), origin.file)
    }

    /// The node over a run already resident in the region: the key laid down beside it and the
    /// cache built from the key and the head.
    fn from_run(
        writer: Writer<'cell>,
        parts: &'cell [Spanned<WorkingPart<'graph, 'cell>>],
        span: Option<Span>,
        file: Option<FileId>,
    ) -> Self {
        let key = writer.fill(parts.len(), |at| parts[at].value.key_element());
        WorkingExpression {
            parts,
            span,
            file,
            cache: NodeCache::build(key, parts.first().map(|part| part.value.class())),
            under_type_sigil: false,
        }
    }

    /// The splice path: a new run under the same key. A splice substitutes slots one for one and
    /// writes no keyword, so the key and everything settled by it ride through; only the head class
    /// is read again.
    pub fn respliced<I>(&self, writer: Writer<'cell>, parts: I) -> Self
    where
        I: IntoIterator<Item = Spanned<WorkingPart<'graph, 'cell>>>,
        RunIter<I>: ExactSizeIterator,
    {
        let parts = collect(writer, parts.into_iter());
        WorkingExpression {
            parts,
            span: self.span,
            file: self.file,
            cache: self
                .cache
                .resplice(parts.first().map(|part| part.value.class())),
            under_type_sigil: self.under_type_sigil,
        }
    }

    /// Stamp this node as the body of a `:(…)` type expression.
    pub fn in_type_context(mut self) -> Self {
        self.under_type_sigil = true;
        self
    }

    /// Whether this node was reached through the type sigil, so a body answers in the type universe.
    pub fn under_type_sigil(&self) -> bool {
        self.under_type_sigil
    }

    /// The structural facts this node cached or carried over.
    pub fn cache(&self) -> &NodeCache<'cell> {
        &self.cache
    }

    pub fn shape(&self) -> DispatchShape {
        self.cache.shape()
    }

    pub fn operator_probe(&self) -> Option<KeywordSymbol> {
        self.cache.operator_probe()
    }

    /// What the parsed node this copy was made from installs; `None` for a node the scheduler built.
    pub fn binder_plan(&self) -> Option<StoredBinderKey<'cell>> {
        self.cache.binder_plan()
    }

    pub fn binder_name_slot(&self) -> Option<usize> {
        self.cache.binder_name_slot()
    }

    pub fn lazy_kinds_at(&self, index: usize) -> LazyKinds {
        self.cache.lazy_kinds_at(index)
    }

    pub fn stored_key(&self) -> &'cell [KeyElement] {
        self.cache.stored_key()
    }

    /// This node's source extent, when both span and file are known.
    pub fn source_ref(&self) -> Option<SourceRef> {
        self.span
            .zip(self.file)
            .map(|(span, file)| SourceRef { span, file })
    }

    /// The whole node as a diagnostic names it: each slot by the type dispatch matched it on, and a
    /// binder's name slot by its spelling, since it is the name being installed rather than an
    /// argument.
    pub fn write_summary(
        &self,
        f: &mut fmt::Formatter<'_>,
        types: &TypeRegistry<'_>,
        labels: &LabelInterner,
        scratch: BumpAllocator<'_>,
    ) -> fmt::Result {
        for (index, part) in self.parts.iter().enumerate() {
            if index > 0 {
                f.write_str(" ")?;
            }
            if Some(index) == self.binder_name_slot() {
                part.value.write_spelling(f, labels)?;
            } else {
                part.value.write_summary(f, types, labels, scratch)?;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for WorkingExpression<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkingExpression")
            .field("parts", &self.parts)
            .finish()
    }
}
