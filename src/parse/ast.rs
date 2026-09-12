//! The syntax AST: what the parser produces and what a function body, a quote, and a stored
//! signature hold. Every node borrows its storage — the parts and the structural cache alike — so a
//! node is `Copy`, `Drop`-free, and copying one to another region is a slice copy rather than a
//! rebuild.
//!
//! A node's structural cache ([`NodeCache`]) is filled at construction from the parts run and the
//! form table, so a reader answers "which dispatch shape, which bucket key, which builtin form,
//! what does this install" without walking the run again.
//!
//! The scheduler's own per-call form is `WorkingExpression`, a distinct type in
//! [`machine::model::ast::working`](crate::machine::model::ast::working). A resolved sub-result and
//! a staging hole live only there, which is what keeps this type structurally splice-free: an AST
//! node names no producer region, so nothing here has a reach to describe. The runtime operations
//! on these types — lowering a literal, resolving a part to a cell — are inherent impls in
//! [`machine::model::ast`](crate::machine::model::ast).

use crate::source::{FileId, Span, Spanned};

use crate::memory::reattachable;
use crate::memory::{ProgramBrand, RegionBrand};
use crate::parse::forms::binder::{StoredBinderKey, binder_plan_for};
use crate::parse::forms::layout::SlotLayout;
use crate::parse::forms::lazy::LazyKinds;
use crate::parse::labels::{BinderSymbol, KeywordSymbol, LabelInterner, TypeSymbol, ValueSymbol};

pub mod program;
pub mod shape;

pub use program::{ProgramExpression, ProgramNode};
pub use shape::{
    DispatchShape, KeyElement, NodeCache, PartClass, UntypedKey, classify_dispatch_shape,
    operator_probe_for, stored_untyped_key,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KLiteral<'a> {
    Number(f64),
    String(&'a str),
    Boolean(bool),
    Null,
}

/// One element of a parsed expression. A keyword and a name — value-side or type-side — are each a
/// symbol the parse minted when it classified the token, so they carry no borrow at all. The arms
/// that do borrow do so at `'a`: a nested node is a pointer to a sibling node in the same storage,
/// and a literal run is a bumped slice.
#[derive(Debug, Clone, Copy)]
pub enum ExpressionPart<'a> {
    Keyword(KeywordSymbol),
    Identifier(ValueSymbol),
    Type(TypeSymbol),
    Expression(ProgramNode<'a>),
    /// Parse-context marker for a `:(...)` group: the wrapped `KExpression` must dispatch
    /// in type-context, returning a type-side carrier. Shape recognition is the
    /// dispatcher's responsibility — the parser does no folding here. See
    /// [old_design/typing/type-language-via-dispatch.md](../../old_design/typing/type-language-via-dispatch.md).
    SigiledTypeExpr(ProgramNode<'a>),
    /// First-class record type `:{x :Number, y :Str}`. The nested `KExpression` is the
    /// field-list `(x :Number, y :Str)` — the same `<name> :<Type>` pair shape a SIG member
    /// or FN parameter list uses. Unlike `SigiledTypeExpr`, this is matched
    /// structurally (the elaborator folds it straight to a record `KType`); there is no
    /// internal type-constructor builtin behind it. See
    /// [old_design/typing/type-language-via-dispatch.md](../../old_design/typing/type-language-via-dispatch.md).
    RecordType(ProgramNode<'a>),
    ListLiteral(&'a [ExpressionPart<'a>]),
    DictLiteral(&'a [(ExpressionPart<'a>, ExpressionPart<'a>)]),
    /// Anonymous record literal (`{x = 1, y = "a"}`) — identifier-keyed `=` pairs. The
    /// brace frame routes here when the first pair separator is `=`; `:` pairs stay a
    /// `DictLiteral`. A field name is the Identifier or Type token's own parse-minted
    /// symbol, so a key carries its class and no consumer re-derives one from text.
    RecordLiteral(&'a [(BinderSymbol, ExpressionPart<'a>)]),
    Literal(KLiteral<'a>),
    /// A `#(...)` quote: the parenthesized body captured at parse time as data. The parser folds
    /// the sigil and its group into this part, so quoting is static syntax — there is no runtime
    /// quoting operation and the body never dispatches. Behaves as a literal everywhere: it is a
    /// `Slot` in the untyped key, a single one classifies [`DispatchShape::LiteralPassThrough`],
    /// and it resolves to `KObject::KExpression(<body>)` — the value `$(...)` evaluates. See
    /// [old_design/expressions-and-parsing.md](../../old_design/expressions-and-parsing.md).
    QuotedExpression(ProgramNode<'a>),
}

/// A parts run on its way into a node's region, as a construction door takes it: either a borrowed
/// run to copy in, or an exact-length iterator to fill the region's bytes straight from. One alias
/// for both expression families — [`KExpression`] and
/// [`WorkingExpression`](working::WorkingExpression) split their doors the same way.
///
/// Both forms exist because both shapes of call site do. A fixed-length run — an operator chain's
/// `[left, op, right]`, a wrapped single operand — is a stack array the door copies; a run whose
/// slots are computed one at a time is an iterator, and staging it through an owned `Vec` first
/// would pay a heap allocation and a second copy for bytes the region was going to hold anyway.
pub(crate) type RunIter<I> = <I as IntoIterator>::IntoIter;

impl<'a> ExpressionPart<'a> {
    /// The structural family this part belongs to — what dispatch-shape classification reads.
    pub fn class(&self) -> PartClass {
        match self {
            ExpressionPart::Keyword(symbol) => PartClass::Keyword(*symbol),
            ExpressionPart::Identifier(_) => PartClass::Identifier,
            ExpressionPart::Type(_) => PartClass::Type,
            ExpressionPart::Expression(_) => PartClass::Expression,
            ExpressionPart::SigiledTypeExpr(_) => PartClass::SigiledTypeExpr,
            ExpressionPart::RecordType(_) => PartClass::RecordType,
            ExpressionPart::ListLiteral(_) => PartClass::ListLiteral,
            ExpressionPart::DictLiteral(_) => PartClass::DictLiteral,
            ExpressionPart::RecordLiteral(_) => PartClass::RecordLiteral,
            ExpressionPart::Literal(_) => PartClass::Literal,
            ExpressionPart::QuotedExpression(_) => PartClass::QuotedExpression,
        }
    }

    /// This part's position in a bucket key: the symbol a keyword carries, `Slot` for every other
    /// part. What the stored key is built from, and what the pre-freeze form match compares.
    pub fn key_element(&self) -> KeyElement {
        match self.class() {
            PartClass::Keyword(symbol) => KeyElement::Keyword(symbol),
            _ => KeyElement::Slot,
        }
    }

    /// Wrap a run of parts as a nested `Expression` part, bumping both the run and the node into
    /// the program storage `brand` names. Takes a [`ProgramBrand`] because the arm it builds is a
    /// value-channel conduit: the marker on its payload is the proof the cell doors cite.
    pub fn expression(
        brand: ProgramBrand<'a>,
        parts: &[Spanned<ExpressionPart<'a>>],
    ) -> ExpressionPart<'a> {
        ExpressionPart::Expression(brand.nested_node(parts))
    }

    /// [`expression`](Self::expression)'s peer for a run whose slots are computed — see [`RunIter`].
    pub fn expression_from_iter<I>(brand: ProgramBrand<'a>, parts: I) -> ExpressionPart<'a>
    where
        I: IntoIterator<Item = Spanned<ExpressionPart<'a>>>,
        RunIter<I>: ExactSizeIterator,
    {
        ExpressionPart::Expression(brand.nested_node_from_iter(parts))
    }

    /// Per-part subset of [`KExpression::write_summary`], written straight into `f`.
    pub fn write_summary(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        labels: &LabelInterner,
    ) -> std::fmt::Result {
        match self {
            ExpressionPart::Keyword(symbol) => write!(f, "{}", labels.display(symbol.symbol())),
            ExpressionPart::Identifier(v) => write!(f, "{}", labels.display(v.symbol())),
            ExpressionPart::Type(t) => write!(f, "{}", labels.display(t.symbol())),
            ExpressionPart::Expression(e) => e.write_summary(f, labels),
            ExpressionPart::SigiledTypeExpr(e) => {
                f.write_str(":(")?;
                e.write_summary(f, labels)?;
                f.write_str(")")
            }
            ExpressionPart::RecordType(e) => {
                f.write_str(":{")?;
                e.write_summary(f, labels)?;
                f.write_str("}")
            }
            ExpressionPart::QuotedExpression(e) => {
                f.write_str("#(")?;
                e.write_summary(f, labels)?;
                f.write_str(")")
            }
            ExpressionPart::ListLiteral(items) => {
                f.write_str("[")?;
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        f.write_str(" ")?;
                    }
                    item.write_summary(f, labels)?;
                }
                f.write_str("]")
            }
            ExpressionPart::DictLiteral(pairs) => {
                f.write_str("{")?;
                for (index, (k, v)) in pairs.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    k.write_summary(f, labels)?;
                    f.write_str(": ")?;
                    v.write_summary(f, labels)?;
                }
                f.write_str("}")
            }
            ExpressionPart::RecordLiteral(pairs) => {
                f.write_str("{")?;
                for (index, (k, v)) in pairs.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{} = ", labels.display(k.symbol()))?;
                    v.write_summary(f, labels)?;
                }
                f.write_str("}")
            }
            ExpressionPart::Literal(lit) => match lit {
                KLiteral::Number(n) => write!(f, "{n}"),
                KLiteral::String(s) => f.write_str(s),
                KLiteral::Boolean(b) => write!(f, "{b}"),
                KLiteral::Null => f.write_str("null"),
            },
        }
    }

    /// [`write_summary`](Self::write_summary) as a `Display` view — what a `format!` argument
    /// naming a part uses.
    ///
    /// Its own view rather than the generic
    /// [`PartSummary`](crate::machine::model::ast::PartSummary), which resolves through the whole run
    /// bundle: parse renders a part here — a record literal's field-name error names the token it
    /// rejected — while still filling the interner a run frame has yet to adopt.
    pub fn summary<'x>(&'x self, labels: &'x LabelInterner) -> AstPartSummary<'x, 'a> {
        AstPartSummary { part: self, labels }
    }

    /// The part's surface as an owned `String`.
    pub fn summarize(&self, labels: &LabelInterner) -> String {
        self.summary(labels).to_string()
    }
}

/// A parsed Koan expression: an ordered run of [`ExpressionPart`]s borrowed from the storage that
/// parsed them.
///
/// `span` and `file` are `None` for hand-built ASTs.
///
/// [`cache`](Self::cache) is the structural cache the construction doors fill once the parts run is
/// complete — the bucket key, the dispatch shape, the operator probe, the matched builtin form and
/// the binder plan — so the dispatch driver reads it rather than re-deriving on every call of the
/// enclosing function. The binder plan is per-node only: what this node installs when it is
/// submitted as a statement, and `None` when it is not itself a binder. A statement's namespace is
/// legible from its own spine, never from what its slots contain.
///
/// Every field is a shared borrow at `'a` or a `Copy` handle, so the node is covariant in `'a`: a
/// program-storage node flows into shorter-lived code by ordinary subtyping, with no reattach and no
/// witness.
#[derive(Clone, Copy)]
pub struct KExpression<'a> {
    pub parts: &'a [Spanned<ExpressionPart<'a>>],
    pub span: Option<Span>,
    pub file: Option<FileId>,
    cache: NodeCache<'a>,
    body_layout: &'a SlotLayout<'a>,
}

// Lifetimes do not affect layout, so this retype is a no-op transmute. The witness's `'b: 'w` bound
// is what makes a reattach a shortening; nothing here weakens it.
reattachable! { KExpression<'static> => KExpression<'r> }

impl<'a> KExpression<'a> {
    /// Spanless construction door for a borrowed run; `span`/`file` populated by later phases.
    pub fn new(brand: RegionBrand<'a>, parts: &[Spanned<ExpressionPart<'a>>]) -> Self {
        Self::build(brand, parts, None, None)
    }

    /// [`new`](Self::new)'s peer for a run whose slots are computed — see [`RunIter`].
    pub fn new_from_iter<I>(brand: RegionBrand<'a>, parts: I) -> Self
    where
        I: IntoIterator<Item = Spanned<ExpressionPart<'a>>>,
        RunIter<I>: ExactSizeIterator,
    {
        Self::build_from_iter(brand, parts, None, None)
    }

    /// Construction door for a borrowed run: copy it into `brand`'s region, then fill the
    /// structural cache.
    pub fn build(
        brand: RegionBrand<'a>,
        parts: &[Spanned<ExpressionPart<'a>>],
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
        I: IntoIterator<Item = Spanned<ExpressionPart<'a>>>,
        RunIter<I>: ExactSizeIterator,
    {
        Self::from_run(brand, brand.allocator().slice_from_iter(parts), span, file)
    }

    /// Construction chokepoint, over a parts run **already resident** in `brand`'s region: fills the
    /// structural cache from it and does nothing else. Every door above lands here, differing only
    /// in how the run reached the region — so none ships with a stale or unfilled cache and no part
    /// run is mutated after it is frozen.
    fn from_run(
        brand: RegionBrand<'a>,
        parts: &'a [Spanned<ExpressionPart<'a>>],
        span: Option<Span>,
        file: Option<FileId>,
    ) -> Self {
        let key = stored_untyped_key(brand, parts.iter().map(|part| part.value.key_element()));
        Self::seal(
            brand,
            parts,
            span,
            file,
            NodeCache::build(key, parts.first().map(|part| part.value.class())),
        )
    }

    /// The node itself, over a resident run and a settled structural cache: fills the binder plan
    /// and freezes. The one place a `KExpression` is written, so neither door above can ship a node
    /// whose plan disagrees with its parts.
    fn seal(
        brand: RegionBrand<'a>,
        parts: &'a [Spanned<ExpressionPart<'a>>],
        span: Option<Span>,
        file: Option<FileId>,
        cache: NodeCache<'a>,
    ) -> Self {
        let mut expression = KExpression {
            parts,
            span,
            file,
            cache,
            body_layout: SlotLayout::EMPTY,
        };
        // The extractors read the node, so the plan is filled once it stands. It is bumped behind a
        // reference rather than stored inline: it is the widest thing a node would carry, and
        // `KExpression` is copied on every part walk.
        let plan = binder_plan_for(brand, cache.form(), &expression)
            .map(|key| brand.allocator().value(key));
        expression.cache = cache.declaring(plan);
        // The value binders this node would open a frame over, read off the same statement plans
        // the claim stamp and the `CLOSE` capture walk read. Filled for every node — a node is a
        // body only where a callable names it as one, and the read is a walk of plans already
        // cached on the statements it wraps.
        expression.body_layout = SlotLayout::of_body(brand, &expression);
        expression
    }

    /// Build a node and bump it, for a part arm that nests one ([`ExpressionPart::Expression`] and
    /// its sigil siblings hold `&'a KExpression<'a>`).
    pub fn nested(
        brand: RegionBrand<'a>,
        parts: &[Spanned<ExpressionPart<'a>>],
    ) -> &'a KExpression<'a> {
        brand.allocator().value(Self::new(brand, parts))
    }

    /// [`nested`](Self::nested)'s peer for a run whose slots are computed — see [`RunIter`].
    pub fn nested_from_iter<I>(brand: RegionBrand<'a>, parts: I) -> &'a KExpression<'a>
    where
        I: IntoIterator<Item = Spanned<ExpressionPart<'a>>>,
        RunIter<I>: ExactSizeIterator,
    {
        brand.allocator().value(Self::new_from_iter(brand, parts))
    }

    /// The [`SlotLayout`] of this node **as a body**: the value binders its statements declare,
    /// each at the lexical position its statement submits at. Computed at construction beside the
    /// binder plan it is read from, and homed in this node's own region — never in a frame's, which
    /// a spliced-out array could outlive.
    pub fn body_layout(&self) -> &'a SlotLayout<'a> {
        self.body_layout
    }

    /// The structural facts this node cached at construction. Every accessor below reads it, and
    /// the working copy carries it over whole.
    pub fn cache(&self) -> &NodeCache<'a> {
        &self.cache
    }

    /// This node's own binder plan — `Some` iff this node is itself a binder.
    pub fn binder_plan(&self) -> Option<StoredBinderKey<'a>> {
        self.cache.binder_plan()
    }

    /// The statement this node stands for. A redundant single-`Expression` paren wrapper
    /// (`((…))`) is the same statement as its child, so it reads through; every other shape is its
    /// own statement.
    pub(crate) fn statement_spine(&self) -> &KExpression<'a> {
        if let [only] = self.parts
            && let ExpressionPart::Expression(child) = only.value
        {
            return child.reference();
        }
        self
    }

    /// What this statement installs. The statement's *own* plan key, never anything its slots
    /// contain — the namespace a block introduces is legible from its statement spines alone,
    /// which is what lets the block fan-out rule on duplicate declarations before any statement
    /// runs.
    ///
    /// Every key the plan names is a borrow into the declaring node's own region, so reading a
    /// block's whole namespace allocates nothing.
    pub(crate) fn statement_binder_plan(&self) -> Option<StoredBinderKey<'a>> {
        self.statement_spine().binder_plan()
    }

    /// The kinds of part that stay raw at slot `index`, empty when the slot evaluates. Read by the
    /// scheduler to decide which children submit.
    pub fn lazy_kinds_at(&self, index: usize) -> LazyKinds {
        self.cache.lazy_kinds_at(index)
    }

    /// The declared-name position of the binder form this node's bucket key matches
    /// ([`BinderFacts::name_slot`](crate::parse::forms::binder::BinderFacts::name_slot)); `None`
    /// when the node is not a binder form, or the form's spine carries no declared name (`FN`,
    /// `OP`).
    pub fn binder_name_slot(&self) -> Option<usize> {
        self.cache.binder_name_slot()
    }

    /// True when this expression is a statement block: two or more parts, all of them
    /// `Expression`. The single definition the body splitters (`split_leading_tail` /
    /// [`body_statement_refs`]) and the binder-install aggregation share, so the multi-statement
    /// cutoff is stated once.
    ///
    /// [`body_statement_refs`]: crate::machine::body_statement_refs
    pub fn is_statement_block(&self) -> bool {
        self.parts.len() >= 2
            && self
                .parts
                .iter()
                .all(|part| matches!(part.value, ExpressionPart::Expression(_)))
    }

    /// Cached dispatch shape (see [`classify_dispatch_shape`]).
    pub fn shape(&self) -> DispatchShape {
        self.cache.shape()
    }

    /// Cached operator-registry probe key: `Some` only for an `OperatorChain`, holding the symbol
    /// of its sorted-joined unique operator keywords.
    pub fn operator_probe(&self) -> Option<KeywordSymbol> {
        self.cache.operator_probe()
    }

    /// The stored bucket key, as a borrow of the run bumped at construction: `Keyword` parts
    /// contribute `Keyword(symbol)`, every other variant a `Slot`. Must agree with
    /// `ExpressionSignature::untyped_key` for any signature that should match.
    pub fn stored_key(&self) -> &'a [KeyElement] {
        self.cache.stored_key()
    }

    /// Binder-name extractor for typed-binder builtins (`SIG <Name> = …`, `UNION <Name> = …`):
    /// if `parts[1]` is a single `Type(t)`, returns its symbol; `None` on shape
    /// mismatch. The builtin body surfaces the structured error.
    pub fn binder_name_from_type_part(&self) -> Option<TypeSymbol> {
        match &self.parts.get(1)?.value {
            ExpressionPart::Type(t) => Some(*t),
            _ => None,
        }
    }

    /// Surface rendering of the whole expression, written straight into `f`, resolving each
    /// symbol-carrying part through the run's interner.
    pub fn write_summary(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        labels: &LabelInterner,
    ) -> std::fmt::Result {
        for (index, part) in self.parts.iter().enumerate() {
            if index > 0 {
                f.write_str(" ")?;
            }
            part.value.write_summary(f, labels)?;
        }
        Ok(())
    }

    /// [`write_summary`](Self::write_summary) as a `Display` view.
    pub fn summary<'x>(&'x self, labels: &'x LabelInterner) -> ExpressionSummary<'x, 'a> {
        ExpressionSummary {
            expression: self,
            labels,
        }
    }

    /// The expression's surface as an owned `String`.
    pub fn summarize(&self, labels: &LabelInterner) -> String {
        self.summary(labels).to_string()
    }
}

/// An [`ExpressionPart::summary`] view: one AST part plus the interner its symbols resolve
/// through.
pub struct AstPartSummary<'x, 'a> {
    part: &'x ExpressionPart<'a>,
    labels: &'x LabelInterner,
}

impl std::fmt::Display for AstPartSummary<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.part.write_summary(f, self.labels)
    }
}

/// A [`KExpression::summary`] view: one expression plus the interner its symbols resolve through.
pub struct ExpressionSummary<'x, 'a> {
    expression: &'x KExpression<'a>,
    labels: &'x LabelInterner,
}

impl std::fmt::Display for ExpressionSummary<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.expression.write_summary(f, self.labels)
    }
}

impl<'a> std::fmt::Debug for KExpression<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KExpression")
            .field("parts", &self.parts)
            .finish()
    }
}

#[cfg(test)]
mod tests;
