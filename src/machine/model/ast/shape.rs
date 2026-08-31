//! Structural classification shared by the two expression nodes.
//!
//! [`KExpression`](super::KExpression) holds raw AST parts and
//! [`WorkingExpression`](super::WorkingExpression) holds the scheduler's per-call parts, but both
//! answer the same structural questions: which dispatch shape is this, what is its bucket key, and
//! which operator group does it probe. Each part family reports itself in one shared vocabulary
//! ([`PartClass`]) and the readers below are written once against that vocabulary, so a shape rule
//! is stated in exactly one place.

use smallvec::SmallVec;

use crate::machine::SplicedCell;
use crate::machine::core::RegionBrand;
use crate::machine::model::KeyElement;
use crate::machine::model::RunRegistries;
use crate::machine::model::labels::{KeywordSymbol, TypeSymbol, ValueSymbol};
use crate::source::Spanned;

use super::KExpression;
use super::working::WorkingExpression;

/// The structural family a part belongs to — the axis shape classification, the bucket key and the
/// operator probe read. A keyword carries the symbol its parse minted, which is what all three
/// readers key by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartClass {
    Keyword(KeywordSymbol),
    Identifier,
    Type,
    Expression,
    SigiledTypeExpr,
    RecordType,
    ListLiteral,
    DictLiteral,
    RecordLiteral,
    Literal,
    QuotedExpression,
    /// A resolved sub-result the scheduler wrote in. Only a working part reports this.
    Spliced,
    /// A staging hole awaiting its sibling's carrier. Only a working part reports this.
    StagedSlot,
}

/// One `<name> <slot>` position of a field list, viewed through what the field-list elaborator does
/// with it. The two part families answer in this one vocabulary, so the walker in
/// [`typed_field_list`](crate::machine::model::types) is written once and reads a parsed field list
/// and a self-reference-threaded one through the same arms.
///
/// The `Ast*` and `Threaded*` pairs are the same syntax at two stages: a `:(…)` or `:{…}` whose
/// co-declared references are still bare names, and one whose references the sigil-body rewrite has
/// already sealed into [`Resolved`](FieldSlot::Resolved) cells.
pub enum FieldSlot<'a> {
    /// A bare identifier — a field or parameter name, minted by the parse that classified it.
    Name(ValueSymbol),
    /// A type name token: a field's declared type, or a capitalized field / variant name.
    Type(TypeSymbol),
    /// `:(…)` still holding parsed AST — thread its co-declared references, then sub-dispatch it.
    AstSigil(&'a KExpression<'a>),
    /// `:{…}` still holding parsed AST — elaborate its field list inline.
    AstRecord(&'a KExpression<'a>),
    /// A sigil body already threaded — sub-dispatch it as it stands.
    ThreadedSigil(&'a WorkingExpression<'a>),
    /// A record body already threaded — elaborate its field list inline.
    ThreadedRecord(&'a WorkingExpression<'a>),
    /// A resolved carrier the threading wrote in: a co-declared sibling's handle.
    Resolved(SplicedCell<'a>),
    /// Any other shape, which no field-list position accepts.
    Other,
}

/// A part of either expression family, viewed through the classifications the shared readers need:
/// [`class`](Part::class) for dispatch shape, bucket key and operator probe, and
/// [`field_slot`](Part::field_slot) for the field-list walk. Implemented by
/// [`ExpressionPart`](super::ExpressionPart) and [`WorkingPart`](super::WorkingPart).
pub trait Part<'a>: Copy {
    fn class(&self) -> PartClass;

    /// This part read as a field-list position. See [`FieldSlot`].
    fn field_slot(&self) -> FieldSlot<'a>;

    /// Surface rendering, for the field walker's shape diagnostics, written straight into `f`.
    /// Takes the whole run bundle, not just its interner: a resolved
    /// [`Spliced`](super::WorkingPart::Spliced) slot renders the *type* of the argument it
    /// carries, and a type names itself through the type registry.
    ///
    /// The AST family renders through the interner alone, so
    /// [`ExpressionPart::write_summary`](super::ExpressionPart::write_summary) keeps that narrower
    /// signature and its impl here forwards `&registries.labels`. Parse reaches that inherent view
    /// before a run frame exists — the interner it fills is the one the run frame later adopts —
    /// so the AST view cannot take the bundle.
    ///
    /// A write signature rather than a view-returning one because the trait is used through
    /// `&dyn`-shaped generic walkers: a borrowed view would need an associated type per
    /// implementor, where one formatter argument stays object-safe and composes the same.
    fn write_summary(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        registries: &RunRegistries,
    ) -> std::fmt::Result;
}

/// A [`Part::write_summary`] view — the `format!`-embeddable form the field walker's diagnostics
/// use, so a part's surface lands in the message's own buffer.
pub fn part_summary<'x, 'a, P: Part<'a>>(
    part: &'x P,
    registries: &'x RunRegistries,
) -> PartSummary<'x, P> {
    PartSummary { part, registries }
}

pub struct PartSummary<'x, P> {
    part: &'x P,
    registries: &'x RunRegistries,
}

impl<'a, P: Part<'a>> std::fmt::Display for PartSummary<'_, P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.part.write_summary(f, self.registries)
    }
}

/// Pure-structural classification of an expression into the no-keyword fast-lane shapes, the
/// chainable operator shape, and the keyword-bearing shape.
///
/// A function of expression structure only (no scope, no types), so it is computed once when the
/// parts run is complete and cached on the node. The dispatch driver reads the cache rather than
/// re-deriving per call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchShape {
    BareIdentifier,
    BareTypeLeaf,
    /// Bare-`Type`-head call: head is a leaf `Type` and `parts[1..]` is non-empty.
    /// Resolves the name synchronously and launches type construction via the shared
    /// apply-a-callable tail.
    TypeCall,
    /// Function-value call: head is a lowercase `Identifier`, followed by ≥1
    /// non-keyword parts.
    FunctionValueCall,
    /// Single-part `:(...)` sigiled type-expression wrapper.
    SigiledTypeExpr,
    /// Single-part `:{…}` record-type sigil. The handler folds the field list straight
    /// to a record `KType` (deferring through a dep-finish when a field type sub-dispatches
    /// or forward-references), with no internal type-constructor builtin behind it.
    RecordType,
    /// Single-part literal-shaped expression — `Literal`, `Spliced`, nested
    /// `Expression`, `ListLiteral`, `DictLiteral`, or `RecordLiteral`. Surfaces the
    /// inner value without a bucket lookup.
    LiteralPassThrough,
    /// Chainable operator run: a slot-led key whose keywords alternate with slots,
    /// with two or more keyword positions (`Slot (Keyword Slot)+`, first keyword at
    /// index 1). A refinement of `Keyworded` the classifier carves out as its own track,
    /// which the fold pre-pass folds into nested binary sub-dispatches.
    OperatorChain,
    /// Head-deferred call: head is a nested `Expression` followed by ≥1 non-keyword
    /// parts. The head is evaluated first; its resulting value (a function or a
    /// constructible type) is then applied to `parts[1..]` via the shared
    /// apply-a-callable tail.
    HeadDeferred,
    /// Type-position head-deferred call: head is a `:(...)` sigiled type expression
    /// followed by ≥1 non-keyword parts. Like `HeadDeferred`, but the resumed value
    /// is admitted only when it is a constructible type; a function or any other value
    /// surfaces a type-shaped `TypeMismatch`.
    TypeHeadDeferred,
    /// A keyword appears anywhere in the parts run (and the chain shape did not match).
    Keyworded,
    /// Head is a non-callable surface — a literal, list, dict, or record — in a
    /// multi-part expression. Heads are always eager and must resolve to something
    /// callable; this shape surfaces a loud `DispatchFailed` from the dispatch entry.
    NonCallableHead,
}

/// Sweeps every part for `Keyword` first so a mixed shape like `(f IF x)` goes to
/// `Keyworded`; only with the no-keyword precondition established does it branch on
/// head shape. A keyword-bearing expression is refined to `OperatorChain` when it
/// matches the `Slot (Keyword Slot)+` shape with ≥2 keyword positions.
pub fn classify_dispatch_shape<'a, P: Part<'a>>(parts: &[Spanned<P>]) -> DispatchShape {
    if parts
        .iter()
        .any(|p| matches!(p.value.class(), PartClass::Keyword(_)))
    {
        if is_operator_chain_shape(parts) {
            return DispatchShape::OperatorChain;
        }
        return DispatchShape::Keyworded;
    }
    if let [only] = parts {
        return match only.value.class() {
            PartClass::Identifier => DispatchShape::BareIdentifier,
            PartClass::Type => DispatchShape::BareTypeLeaf,
            PartClass::SigiledTypeExpr => DispatchShape::SigiledTypeExpr,
            PartClass::RecordType => DispatchShape::RecordType,
            PartClass::Literal
            | PartClass::Spliced
            | PartClass::Expression
            | PartClass::QuotedExpression
            | PartClass::ListLiteral
            | PartClass::DictLiteral
            | PartClass::RecordLiteral => DispatchShape::LiteralPassThrough,
            // A lone hole classifies as a bare identifier — the shape a resolvable single part
            // takes.
            PartClass::StagedSlot => DispatchShape::BareIdentifier,
            PartClass::Keyword(_) => {
                unreachable!("no-keyword precondition: the sweep above caught every Keyword part")
            }
        };
    }
    // `len >= 2` here: the keyword sweep passed and the single-part block did not
    // match, so an empty parts run falls through as the explicit `NonCallableHead`.
    let Some(head) = parts.first() else {
        return DispatchShape::NonCallableHead;
    };
    match head.value.class() {
        PartClass::Type => DispatchShape::TypeCall,
        PartClass::Identifier => DispatchShape::FunctionValueCall,
        PartClass::Expression => DispatchShape::HeadDeferred,
        PartClass::SigiledTypeExpr => DispatchShape::TypeHeadDeferred,
        // A literal / list / dict / record-literal / record-type / quote / resolved head in a
        // multi-part expression: heads are always eager and must resolve to something callable, so
        // a non-callable head surfaces a loud `DispatchFailed`. A record *type* and a quoted
        // expression are values, not callables, so they join them here.
        PartClass::Literal
        | PartClass::Spliced
        | PartClass::ListLiteral
        | PartClass::DictLiteral
        | PartClass::RecordLiteral
        | PartClass::RecordType
        | PartClass::QuotedExpression => DispatchShape::NonCallableHead,
        // A staged slot at the head position is reachable the same way as the single-part case
        // above. A hole head classifies as a function-value call — the shape a resolvable
        // identifier head takes.
        PartClass::StagedSlot => DispatchShape::FunctionValueCall,
        PartClass::Keyword(_) => {
            unreachable!("no-keyword precondition: the sweep above caught every Keyword part")
        }
    }
}

/// True iff `parts` is the `Slot (Keyword Slot)+` chainable-operator shape: odd
/// length ≥ 5 (slot, keyword, slot, …), every odd index a `Keyword`, every even
/// index a non-keyword slot, with ≥2 keyword positions. The first keyword sits at
/// index 1, so no keyword-led builtin (`LET …`) collides with it.
fn is_operator_chain_shape<'a, P: Part<'a>>(parts: &[Spanned<P>]) -> bool {
    // Need slot, keyword, slot, keyword, slot — at least 5 parts (2 keywords).
    if parts.len() < 5 || parts.len().is_multiple_of(2) {
        return false;
    }
    parts.iter().enumerate().all(|(index, part)| {
        let is_keyword = matches!(part.value.class(), PartClass::Keyword(_));
        // Odd indices must be keywords; even indices must be non-keyword slots.
        (index % 2 == 1) == is_keyword
    })
}

/// The probe key an `OperatorChain` looks the per-scope operator registry up by: the digest of the
/// run of its operator keywords, minted by [`KeywordSymbol::of_run`]. `None` for any other shape.
///
/// The group registration mints its powerset keys through the same constructor, so a registered key
/// and this probe agree by construction and neither side touches text. The node carries `u128`
/// bits, and a registry probe compares them.
pub fn operator_probe_for<'a, P: Part<'a>>(
    parts: &[Spanned<P>],
    shape: DispatchShape,
) -> Option<KeywordSymbol> {
    if shape != DispatchShape::OperatorChain {
        return None;
    }
    // Distinct operators, in a stack buffer. `of_run` reads its members as a set, so dropping a
    // repeat here mints the same digest — what it buys is the bound. A chain holds one entry per
    // operator it names, not one per term, so the buffer is sized by the member count of the group
    // the chain must resolve against rather than by the length of an arbitrarily long run.
    let mut operators: SmallVec<[KeywordSymbol; 8]> = SmallVec::new();
    for part in parts {
        if let PartClass::Keyword(symbol) = part.value.class()
            && !operators.contains(&symbol)
        {
            operators.push(symbol);
        }
    }
    Some(KeywordSymbol::of_run(&operators))
}

/// The stored bucket key: `Keyword` parts contribute the symbol they carry, every other part a
/// `Slot`. Bumped once at construction, so reading it is a slice borrow and nothing is hashed —
/// the parse already minted every symbol in the run.
pub fn stored_untyped_key<'a, P: Part<'a>>(
    brand: RegionBrand<'a>,
    parts: &[Spanned<P>],
) -> &'a [KeyElement] {
    brand
        .allocator()
        .slice_from_iter(parts.iter().map(|part| match part.value.class() {
            PartClass::Keyword(symbol) => KeyElement::Keyword(symbol),
            _ => KeyElement::Slot,
        }))
}
