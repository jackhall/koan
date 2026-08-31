//! The raw AST: what the parser produces and what a function body, a quote, and a stored signature
//! hold. Every node borrows its storage — the parts and the structural cache alike — so a node is
//! `Copy`, `Drop`-free, and copying one to another region is a slice copy rather than a rebuild.
//!
//! The scheduler's own per-call form is [`WorkingExpression`], a distinct type in [`working`]. A
//! resolved sub-result and a staging hole live only there, which is what keeps this type
//! structurally splice-free: an AST node names no producer region, so nothing here has a reach to
//! describe.

use crate::source::{FileId, Span, Spanned};

use crate::machine::core::{ProgramBrand, RegionBrand};
use crate::machine::model::labels::{
    BinderSymbol, KeywordSymbol, LabelInterner, TypeSymbol, ValueSymbol,
};
use crate::machine::model::lazy_slots::{LazyKinds, LazySlotSpec};
use crate::machine::model::{Held, KObject, Parseable, RunRegistries, StoredBinderKey};
use crate::machine::model::{KeyElement, UntypedKey};
use crate::witnessed::reattachable;

pub mod program;
mod shape;
pub mod working;

pub use program::{ProgramExpression, ProgramNode};
pub use shape::{
    DispatchShape, FieldSlot, Part, PartClass, PartSummary, classify_dispatch_shape,
    operator_probe_for, part_summary, stored_untyped_key,
};
pub use working::{WorkingExpression, WorkingPart, WorkingSummary};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KLiteral<'a> {
    Number(f64),
    String(&'a str),
    Boolean(bool),
    Null,
}

impl<'a> KLiteral<'a> {
    /// The [`KObject`] this literal denotes, built into `brand`'s region: the scalar arms own their
    /// data outright, and a string literal's bytes are bumped into that region
    /// ([`RegionBrand::allocator`]). Generic in `'b` (rather than `'a`) despite [`KObject`] being
    /// invariant in its lifetime — the value is constructible at whatever lifetime the brand
    /// carries, in particular a construction site's fold brand.
    ///
    /// Region-pure in the sense the construction sites need: every borrow the product carries points
    /// into `brand`'s own region, so a fold door stores it with no audit at all.
    pub fn to_kobject<'b>(&self, brand: RegionBrand<'b>) -> KObject<'b> {
        match self {
            KLiteral::Number(n) => KObject::Number(*n),
            KLiteral::String(s) => KObject::KString(brand.allocator().text(s)),
            KLiteral::Boolean(b) => KObject::Bool(*b),
            KLiteral::Null => KObject::Null,
        }
    }
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
    /// [design/typing/type-language-via-dispatch.md](../../../design/typing/type-language-via-dispatch.md).
    SigiledTypeExpr(ProgramNode<'a>),
    /// First-class record type `:{x :Number, y :Str}`. The nested `KExpression` is the
    /// field-list `(x :Number, y :Str)` — the same `<name> :<Type>` pair shape a SIG member
    /// or FN parameter list uses. Unlike `SigiledTypeExpr`, this is matched
    /// structurally (the elaborator folds it straight to a record `KType`); there is no
    /// internal type-constructor builtin behind it. See
    /// [design/typing/type-language-via-dispatch.md](../../../design/typing/type-language-via-dispatch.md).
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
    /// [design/expressions-and-parsing.md](../../../design/expressions-and-parsing.md).
    QuotedExpression(ProgramNode<'a>),
}

impl<'a> Part<'a> for ExpressionPart<'a> {
    fn class(&self) -> PartClass {
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

    fn field_slot(&self) -> FieldSlot<'a> {
        match self {
            ExpressionPart::Identifier(v) => FieldSlot::Name(*v),
            ExpressionPart::Type(t) => FieldSlot::Type(*t),
            ExpressionPart::SigiledTypeExpr(body) => FieldSlot::AstSigil(body.reference()),
            ExpressionPart::RecordType(body) => FieldSlot::AstRecord(body.reference()),
            _ => FieldSlot::Other,
        }
    }

    /// The AST view renders through the interner alone, so the bundle narrows here. Parse fills
    /// that interner before a run frame exists, which is why the inherent method below keeps the
    /// narrower parameter rather than matching its working-family peer.
    fn write_summary(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        registries: &RunRegistries,
    ) -> std::fmt::Result {
        ExpressionPart::write_summary(self, f, &registries.labels)
    }
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
    /// Its own view rather than the generic [`PartSummary`], which resolves through the whole run
    /// bundle: parse renders a part here — a record literal's field-name error names the token it
    /// rejected — while still filling the interner a run frame has yet to adopt.
    pub fn summary<'x>(&'x self, labels: &'x LabelInterner) -> AstPartSummary<'x, 'a> {
        AstPartSummary { part: self, labels }
    }

    /// The part's surface as an owned `String`.
    pub fn summarize(&self, labels: &LabelInterner) -> String {
        self.summary(labels).to_string()
    }

    /// Slot-aware resolve producing an owned [`Held`] cell, run at [`KFunction::bind_args`] time. A
    /// type rides the `Type` arm; a runtime value rides the `Object` arm. A `Type`-name token in a
    /// proper-type slot lowers through the builtin table ([`KType::from_symbol`]), falling back to
    /// the [`Held::UnresolvedType`] carrier for every other name — no type handle ever denotes an
    /// unresolved name, so the token's symbol rides through verbatim and scope-aware
    /// elaboration defers to
    /// [`Scope::resolve_type_identifier`](crate::machine::core::Scope::resolve_type_identifier).
    /// An `:Identifier` slot is that carrier's value-channel mirror: the token's symbol rides
    /// through on [`Held::Identifier`], so a captured name is never rendered to a string to be
    /// bound.
    ///
    /// [`KFunction::bind_args`]: crate::machine::KFunction::bind_args
    pub fn resolve_for(
        &self,
        slot: &crate::machine::model::KType,
        scope: &'a crate::machine::core::Scope<'a>,
    ) -> Held<'a> {
        use crate::machine::model::types::KType;
        if let (ExpressionPart::Type(t), KType::PROPER_TYPE | KType::ANY_TYPE) = (self, *slot) {
            return match KType::from_symbol(*t) {
                Some(kt) => Held::Type(kt),
                None => Held::UnresolvedType(*t),
            };
        }
        if let (ExpressionPart::SigiledTypeExpr(inner), KType::SIGILED_TYPE_EXPR) = (self, *slot) {
            return Held::Object(KObject::KExpression(inner.expression()));
        }
        if let (ExpressionPart::RecordType(inner), KType::RECORD_TYPE) = (self, *slot) {
            return Held::Object(KObject::KExpression(inner.expression()));
        }
        if let (ExpressionPart::Identifier(name), KType::IDENTIFIER) = (self, *slot) {
            return Held::Identifier(*name);
        }
        Held::Object(self.resolve(scope.brand()))
    }

    /// The [`KObject`] this part denotes, built into `brand`'s region — the string arms bump their
    /// bytes there, so the product is dest-resident and holds no allocation of its own.
    pub fn resolve(&self, brand: RegionBrand<'a>) -> KObject<'a> {
        match self {
            // A keyword part is fixed syntax, never data: a literal rejects one at parse
            // (`parse_stack::push_part`), and dispatch consumes a fixed token positionally against
            // its bucket key rather than resolving it.
            ExpressionPart::Keyword(_) => {
                unreachable!("a keyword part is fixed syntax and never resolves to a value")
            }
            // A name part carries a symbol, not text, and never becomes a string on the way to a
            // slot. `:Identifier` is part-kind-exact — it admits this part shape and no resolved
            // cell (`accepts_part` / `accepts_carried`) — so an identifier reaches the bind seam
            // only through the `Held::Identifier` arm of `resolve_for`; a `Type` part likewise
            // reaches only a `PROPER_TYPE` / `ANY_TYPE` slot, taken at the top of the same
            // function. Every other slot is served by an eagerly-resolved carrier, not a raw name.
            ExpressionPart::Identifier(_) | ExpressionPart::Type(_) => unreachable!(
                "a name part is captured as its symbol by resolve_for, never resolved to a string"
            ),
            ExpressionPart::Literal(KLiteral::Number(n)) => KObject::Number(*n),
            ExpressionPart::Literal(KLiteral::String(s)) => {
                KObject::KString(brand.allocator().text(s))
            }
            ExpressionPart::Literal(KLiteral::Boolean(b)) => KObject::Bool(*b),
            ExpressionPart::Literal(KLiteral::Null) => KObject::Null,
            ExpressionPart::Expression(e) => KObject::KExpression(e.expression()),
            // A quote denotes its body as data — the same `KObject` an `Expression` part in a
            // `:KExpression` slot denotes, reached from any slot a literal reaches.
            ExpressionPart::QuotedExpression(e) => KObject::KExpression(e.expression()),
            // Reaches a value only through the dispatcher's type-context fast lane or sub-Dispatch,
            // both of which unwrap it; hitting `resolve()` means a builtin lost the marker.
            ExpressionPart::SigiledTypeExpr(_) => {
                unreachable!("SigiledTypeExpr only valid in type-context dispatch")
            }
            // Like SigiledTypeExpr: a record type reaches a value through the dispatcher's
            // `RecordType` fast lane or a raw `:RecordType`-slot capture, never `resolve()`.
            ExpressionPart::RecordType(_) => {
                unreachable!("RecordType only valid in type-context dispatch")
            }
            // A container literal's substrate is born only through the fold door, which `resolve()`
            // has no brand to reach — and it never needs one: eager staging
            // (`eager_shape`/`stage_eager_part`, `dispatch.rs`) routes every literal part through
            // its scheduled path (`schedule_list_literal` / `_dict_` / `_record_`) before any
            // resolve site reaches it, replacing it with a spliced cell first. Non-scalar dict keys
            // are surfaced as a structured `ShapeError` on that scheduled path, never here.
            ExpressionPart::ListLiteral(_)
            | ExpressionPart::DictLiteral(_)
            | ExpressionPart::RecordLiteral(_) => {
                unreachable!(
                    "a container-literal part is always staged (schedule_*_literal) before any \
                     resolve() site reaches it"
                )
            }
        }
    }

    /// The [`KObject`] a **region-pure** part denotes, at *any* lifetime — the lifetime-generic peer
    /// of [`resolve`](Self::resolve) for static-cell sites that fold. The region-pure variant
    /// (a literal) reaches nothing outside `brand`'s own region, so the value is constructible
    /// at the caller's fold brand, where [`resolve`](Self::resolve)'s invariant `KObject<'a>` cannot
    /// go. Borrow-bearing variants are classified to owned sub-dispatches before any static cell, so
    /// they never reach here.
    pub fn resolve_region_pure<'b>(&self, brand: RegionBrand<'b>) -> KObject<'b> {
        match self {
            ExpressionPart::Literal(lit) => lit.to_kobject(brand),
            // Fixed syntax, never data — a literal rejects a keyword at parse and dispatch
            // consumes one positionally.
            ExpressionPart::Keyword(_) => {
                unreachable!("a keyword part is fixed syntax and never resolves to a value")
            }
            // A name part in an aggregate is eagerly resolved against the scope chain
            // (`classify_aggregate_part`), so it becomes a resolved cell before any static cell
            // folds — a raw name never reaches a fold brand.
            ExpressionPart::Identifier(_) | ExpressionPart::Type(_) => unreachable!(
                "a bare name in an aggregate is resolved to a cell before any static-cell fold"
            ),
            // A quote's `KObject::KExpression` is invariant in `'a` with no `'static` rebuild, so it
            // cannot be constructed at the caller's fold brand — the classifier routes a quote to
            // its own sub-dispatch (which seals it through the expression door) before any static cell.
            ExpressionPart::Expression(_)
            | ExpressionPart::SigiledTypeExpr(_)
            | ExpressionPart::RecordType(_)
            | ExpressionPart::QuotedExpression(_)
            | ExpressionPart::ListLiteral(_)
            | ExpressionPart::DictLiteral(_)
            | ExpressionPart::RecordLiteral(_) => unreachable!(
                "resolve_region_pure is only called on a region-pure static-cell part \
                 (keyword / literal); borrow-bearing parts are classified to sub-dispatches \
                 before any static cell"
            ),
        }
    }
}

/// A parsed Koan expression: an ordered run of [`ExpressionPart`]s borrowed from the storage that
/// parsed them.
///
/// `span` and `file` are `None` for hand-built ASTs.
///
/// `untyped_key`, `shape`, and `operator_probe` are a structural cache filled by the construction
/// doors once the parts run is complete, so the dispatch driver reads the cache rather than
/// re-deriving on every call of the enclosing function. `binder_plan` is the binder-position cache:
/// what this node installs when it is submitted as a statement, and `None` when it is not itself a
/// binder. It is per-node only — a statement's namespace is legible from its own spine, never from
/// what its slots contain. `binder_name_slot` is the matched binder form's declared-name position
/// ([`BinderSpec::name_slot`](crate::machine::model::binder::BinderSpec)), cached separately from
/// the plan because `VAL` and the anonymous `FN :{…}` match binder keys — and their declaration
/// slots need declaration treatment in dispatch — while installing nothing.
///
/// Every field is a shared borrow at `'a` or a `Copy` handle, so the node is covariant in `'a`: a
/// program-storage node flows into shorter-lived code by ordinary subtyping, with no reattach and no
/// witness.
#[derive(Clone, Copy)]
pub struct KExpression<'a> {
    pub parts: &'a [Spanned<ExpressionPart<'a>>],
    pub span: Option<Span>,
    pub file: Option<FileId>,
    untyped_key: &'a [KeyElement],
    shape: DispatchShape,
    operator_probe: Option<KeywordSymbol>,
    binder_plan: Option<&'a StoredBinderKey<'a>>,
    binder_name_slot: Option<usize>,
    lazy_slots: Option<&'static LazySlotSpec>,
}

// Lifetimes do not affect layout, so this retype is a no-op transmute. The witness's `'b: 'w` bound
// is what makes a reattach a shortening; nothing here weakens it.
reattachable! { KExpression<'static> => KExpression<'r> }

/// The three structural facts a node caches at construction, as one value so the doors that
/// compute them and the door that carries them from a peeled source hand the same thing to the
/// chokepoint.
#[derive(Clone, Copy)]
struct StructuralCache<'a> {
    untyped_key: &'a [KeyElement],
    shape: DispatchShape,
    operator_probe: Option<KeywordSymbol>,
}

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

    /// **Rebuild** door for a run whose parts were rewritten *without* changing what the structural
    /// cache reads: the peel pass ([`peel_redundant`](crate::parse::peel_redundant)) drops redundant
    /// wrappers from the innards of nested parts, so every surviving node keeps its part-kind
    /// sequence and every keyword symbol in its run. The three cached facts — bucket key, dispatch
    /// shape, operator probe — are exactly those two things read, so they carry over from `cache_of`
    /// instead of being recomputed, which is what keeps peel from re-bumping a bucket key and
    /// re-minting a probe digest per nested node. The `debug_assert` is the contract: a caller whose
    /// rewrite changes the shape has no business here.
    ///
    /// `cache_of` must be the node the run was rewritten *from* and must live in the same region, so
    /// the carried key slice stays resident where the new node does.
    pub(crate) fn rebuild_from_iter<I>(
        brand: RegionBrand<'a>,
        parts: I,
        span: Option<Span>,
        file: Option<FileId>,
        cache_of: &KExpression<'a>,
    ) -> Self
    where
        I: IntoIterator<Item = Spanned<ExpressionPart<'a>>>,
        RunIter<I>: ExactSizeIterator,
    {
        let parts = brand.allocator().slice_from_iter(parts);
        debug_assert_eq!(
            cache_of.shape,
            classify_dispatch_shape(parts),
            "a rebuild carries its source's structural cache, so the rewrite must preserve the shape"
        );
        Self::seal(
            brand,
            parts,
            span,
            file,
            StructuralCache {
                untyped_key: cache_of.untyped_key,
                shape: cache_of.shape,
                operator_probe: cache_of.operator_probe,
            },
        )
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
        let shape = classify_dispatch_shape(parts);
        Self::seal(
            brand,
            parts,
            span,
            file,
            StructuralCache {
                untyped_key: stored_untyped_key(brand, parts),
                shape,
                operator_probe: operator_probe_for(parts, shape),
            },
        )
    }

    /// The node itself, over a resident run and a settled structural cache: fills the two binder
    /// caches and freezes. The one place a `KExpression` is written, so neither door above can ship
    /// a node whose binder caches disagree with its parts.
    fn seal(
        brand: RegionBrand<'a>,
        parts: &'a [Spanned<ExpressionPart<'a>>],
        span: Option<Span>,
        file: Option<FileId>,
        cache: StructuralCache<'a>,
    ) -> Self {
        let mut expression = KExpression {
            parts,
            span,
            file,
            untyped_key: cache.untyped_key,
            shape: cache.shape,
            operator_probe: cache.operator_probe,
            binder_plan: None,
            binder_name_slot: None,
            lazy_slots: crate::machine::model::lazy_slots::lazy_slot_spec_for(parts),
        };
        // One spec-table probe fills both binder caches. The plan is bumped behind a reference
        // rather than stored inline: it is the widest thing a node would carry, and `KExpression`
        // is copied on every part walk.
        if let Some(spec) = crate::machine::model::binder::binder_spec_for(&expression) {
            expression.binder_name_slot = spec.name_slot;
            expression.binder_plan =
                crate::machine::model::binder::binder_plan_from_spec(brand, spec, &expression)
                    .map(|key| brand.allocator().value(key));
        }
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

    /// This node's own binder plan — `Some` iff this node is itself a binder.
    pub fn binder_plan(&self) -> Option<StoredBinderKey<'a>> {
        self.binder_plan.copied()
    }

    /// The plan as the bumped borrow it is stored as, for the working copy that carries it through
    /// a splice unchanged.
    pub(crate) fn binder_plan_ref(&self) -> Option<&'a StoredBinderKey<'a>> {
        self.binder_plan
    }

    /// This node's lazy-slot stamp — the [`LazySlotSpec`] its bucket key matches, `None` for every
    /// form with no lazy slot. Read by the scheduler to decide which children submit.
    pub(crate) fn lazy_slots(&self) -> Option<&'static LazySlotSpec> {
        self.lazy_slots
    }

    /// The kinds of part that stay raw at slot `index`, empty when the slot evaluates.
    pub fn lazy_kinds_at(&self, index: usize) -> LazyKinds {
        self.lazy_slots
            .map_or(LazyKinds::EMPTY, |spec| spec.kinds_at(index))
    }

    /// The declared-name position of the binder form this node's bucket key matches
    /// ([`BinderSpec::name_slot`](crate::machine::model::binder::BinderSpec)); `None` when the node
    /// is not a binder form, or the form's spine carries no declared name (`FN`, `OP`).
    pub fn binder_name_slot(&self) -> Option<usize> {
        self.binder_name_slot
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
        self.shape
    }

    /// Cached operator-registry probe key: `Some` only for an `OperatorChain`, holding the symbol
    /// of its sorted-joined unique operator keywords.
    pub fn operator_probe(&self) -> Option<KeywordSymbol> {
        self.operator_probe
    }

    /// The stored bucket key, as a borrow of the run bumped at construction.
    pub fn stored_key(&self) -> &'a [KeyElement] {
        self.untyped_key
    }

    /// Bucket key: `Keyword` parts contribute `Keyword(symbol)`; every other variant contributes
    /// `Slot`. Must agree with `ExpressionSignature::untyped_key` for any signature that
    /// should match. Copies the stored run into the owned key a caller passes onward as plain data.
    pub fn untyped_key(&self) -> UntypedKey {
        self.untyped_key.to_vec()
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

impl<'a> Parseable for KExpression<'a> {
    fn ktype(&self) -> crate::machine::model::KType {
        crate::machine::model::KType::KEXPRESSION
    }
}
