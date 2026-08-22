//! The raw AST: what the parser produces and what a function body, a quote, and a stored signature
//! hold. Every node borrows its storage — parts, keyword text, and the structural cache alike — so a
//! node is `Copy`, `Drop`-free, and copying one to another region is a slice copy rather than a
//! rebuild.
//!
//! The scheduler's own per-call form is [`WorkingExpression`], a distinct type in [`working`]. A
//! resolved sub-result and a staging hole live only there, which is what keeps this type
//! structurally splice-free: an AST node names no producer region, so nothing here has a reach to
//! describe.

use crate::source::{FileId, Span, Spanned};

use crate::machine::core::{ProgramBrand, RegionBrand};
use crate::machine::model::labels::{KeywordSymbol, LabelInterner};
use crate::machine::model::{Held, KObject, Parseable, StoredBinderKey};
use crate::machine::model::{KeyElement, UntypedKey};
use crate::witnessed::reattachable;

pub mod program;
mod shape;
pub mod working;

pub use program::{ProgramExpression, ProgramNode};
pub use shape::{
    DispatchShape, FieldSlot, Part, PartClass, classify_dispatch_shape, operator_probe_for,
    stored_untyped_key,
};
pub use working::{WorkingExpression, WorkingPart};

#[cfg(test)]
mod tests;

/// A keyword occurrence: the token's program-storage text beside the [`KeywordSymbol`] minted for
/// it. Keyword identity travels as the symbol — every key, probe and comparison reads
/// [`symbol`](Self::symbol) — and the text rides along for rendering, diagnostics, and the
/// operator-probe join.
///
/// The two constructors mirror the classified-symbol pair: [`declared`](Self::declared) at a site
/// whose text should be resolvable later (the parser, signature mint), [`of`](Self::of) at a probe
/// or a hand-built node. Both return `None` for text that is not keyword-class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeywordToken<'a> {
    text: &'a str,
    symbol: KeywordSymbol,
}

impl<'a> KeywordToken<'a> {
    /// Classify, mint, and intern — the declaration door. The interner is what lets a diagnostic
    /// naming a bucket render the keyword back out of a symbol-only key.
    pub fn declared(text: &'a str, labels: &LabelInterner) -> Option<Self> {
        KeywordSymbol::declared(text, labels).map(|symbol| KeywordToken { text, symbol })
    }

    /// Classify and mint, recording nothing — the probe door, for a lookup key or a hand-built node
    /// whose text no diagnostic resolves through the interner.
    pub fn of(text: &'a str) -> Option<Self> {
        KeywordSymbol::of(text).map(|symbol| KeywordToken { text, symbol })
    }

    /// A draft keyword whose symbol is that of `normalized` rather than of `text` — the one door
    /// where the pairing is not text-for-text.
    ///
    /// A signature draft may spell a fixed token lowercase, and the bucket such a shape keys is the
    /// uppercased spelling [`ExpressionSignature::mint`] re-homes. `text` stays as drafted so a
    /// pre-mint render shows what the caller wrote; mint replaces the whole token with the
    /// normalized pairing, so the split lives only inside an unminted draft.
    ///
    /// [`ExpressionSignature::mint`]: crate::machine::model::ExpressionSignature::mint
    pub(crate) fn drafted(text: &'a str, normalized: &str) -> Option<Self> {
        KeywordSymbol::of(normalized).map(|symbol| KeywordToken { text, symbol })
    }

    /// The token's identity. Comparisons and keys read this.
    pub fn symbol(self) -> KeywordSymbol {
        self.symbol
    }

    /// The spelling, as the borrow of program storage it arrived as.
    pub fn text(self) -> &'a str {
        self.text
    }

    /// The same token with its text re-homed into `brand`'s region. A text move only — the symbol
    /// rides through, so nothing is re-classified and nothing is re-hashed.
    pub fn rehomed<'b>(self, brand: RegionBrand<'b>) -> KeywordToken<'b> {
        KeywordToken {
            text: brand.allocator().text(self.text),
            symbol: self.symbol,
        }
    }
}

impl std::fmt::Display for KeywordToken<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.text)
    }
}

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

/// A bare type identifier as written in source (`Number`, `Point`, `Mo.Ty`) — a single name
/// token, never compound syntax.
///
/// A thin borrow of the source name: `Deref`s to `str`, derives eq/hash by string. The
/// identifier stays a flat name even when it *denotes* a compound type (a `NEWTYPE` / `UNION`
/// name resolves to a record / tagged type); compound *syntax* (`:(LIST OF …)`, `:(FN … -> …)`)
/// is a `SigiledTypeExpr`, not a `TypeIdentifier`. The position tag rides on the carrier variant
/// (`ExpressionPart::Type`), not on this struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeIdentifier<'a>(&'a str);

impl<'a> std::ops::Deref for TypeIdentifier<'a> {
    type Target = str;
    fn deref(&self) -> &str {
        self.0
    }
}

impl<'a> TypeIdentifier<'a> {
    /// Name a leaf type from text already resident at `'a` — a parser token bumped into program
    /// storage, or a name bumped through [`RegionBrand::allocator`] at a construction site.
    pub fn leaf(name: &'a str) -> TypeIdentifier<'a> {
        TypeIdentifier(name)
    }

    pub fn as_str(&self) -> &'a str {
        self.0
    }

    /// Render in surface syntax so the output round-trips through the parser unchanged.
    pub fn render(&self) -> String {
        self.0.to_string()
    }
}

/// One element of a parsed expression. Every arm borrows at `'a`: a name is a run of bytes in the
/// storage that parsed it, a nested node is a pointer to a sibling node in the same storage, and a
/// literal run is a bumped slice.
#[derive(Debug, Clone, Copy)]
pub enum ExpressionPart<'a> {
    Keyword(KeywordToken<'a>),
    Identifier(&'a str),
    Type(TypeIdentifier<'a>),
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
    /// `DictLiteral`. Field names are syntactic identifiers (never name-resolved).
    RecordLiteral(&'a [(&'a str, ExpressionPart<'a>)]),
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
    fn class(&self) -> PartClass<'a> {
        match self {
            ExpressionPart::Keyword(kw) => PartClass::Keyword(*kw),
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
            ExpressionPart::Identifier(s) => FieldSlot::Name(s),
            ExpressionPart::Type(t) => FieldSlot::Type(*t),
            ExpressionPart::SigiledTypeExpr(body) => FieldSlot::AstSigil(body.reference()),
            ExpressionPart::RecordType(body) => FieldSlot::AstRecord(body.reference()),
            _ => FieldSlot::Other,
        }
    }

    fn summarize(&self) -> String {
        ExpressionPart::summarize(self)
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

    /// Per-part subset of [`KExpression::summarize`].
    pub fn summarize(&self) -> String {
        match self {
            ExpressionPart::Keyword(kw) => kw.text().to_string(),
            ExpressionPart::Identifier(s) => (*s).to_string(),
            ExpressionPart::Type(t) => t.render(),
            ExpressionPart::Expression(e) => e.summarize(),
            ExpressionPart::SigiledTypeExpr(e) => format!(":({})", e.summarize()),
            ExpressionPart::RecordType(e) => format!(":{{{}}}", e.summarize()),
            ExpressionPart::QuotedExpression(e) => format!("#({})", e.summarize()),
            ExpressionPart::ListLiteral(items) => {
                let inner: Vec<String> = items.iter().map(|p| p.summarize()).collect();
                format!("[{}]", inner.join(" "))
            }
            ExpressionPart::DictLiteral(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k.summarize(), v.summarize()))
                    .collect();
                format!("{{{}}}", inner.join(", "))
            }
            ExpressionPart::RecordLiteral(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("{} = {}", k, v.summarize()))
                    .collect();
                format!("{{{}}}", inner.join(", "))
            }
            ExpressionPart::Literal(lit) => match lit {
                KLiteral::Number(n) => n.to_string(),
                KLiteral::String(s) => (*s).to_string(),
                KLiteral::Boolean(b) => b.to_string(),
                KLiteral::Null => "null".to_string(),
            },
        }
    }

    /// Slot-aware resolve producing an owned [`Held`] cell, run at [`KFunction::bind_args`] time. A
    /// type rides the `Type` arm; a runtime value rides the `Object` arm. A `Type`-name token in a
    /// proper-type slot lowers through the builtin table ([`KType::from_name`]), falling back to
    /// the [`Held::UnresolvedType`] carrier for every other name — no type handle ever denotes an
    /// unresolved name, so the surface [`TypeIdentifier`] rides through verbatim and scope-aware
    /// elaboration defers to
    /// [`Scope::resolve_type_identifier`](crate::machine::core::Scope::resolve_type_identifier).
    ///
    /// [`KFunction::bind_args`]: crate::machine::KFunction::bind_args
    pub fn resolve_for(
        &self,
        slot: &crate::machine::model::KType,
        scope: &'a crate::machine::core::Scope<'a>,
    ) -> Held<'a> {
        use crate::machine::model::types::KType;
        if let (ExpressionPart::Type(t), KType::PROPER_TYPE | KType::ANY_TYPE) = (self, *slot) {
            return match KType::from_name(t.as_str()) {
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
        Held::Object(self.resolve(scope.brand()))
    }

    /// The [`KObject`] this part denotes, built into `brand`'s region — the string arms bump their
    /// bytes there, so the product is dest-resident and holds no allocation of its own.
    pub fn resolve(&self, brand: RegionBrand<'a>) -> KObject<'a> {
        match self {
            ExpressionPart::Keyword(kw) => KObject::KString(brand.allocator().text(kw.text())),
            ExpressionPart::Identifier(s) => KObject::KString(brand.allocator().text(s)),
            ExpressionPart::Type(t) => KObject::KString(brand.allocator().text(t.as_str())),
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
    /// of [`resolve`](Self::resolve) for static-cell sites that fold. The region-pure variants
    /// (keyword, bare identifier, type name, literal) reach nothing outside `brand`'s own region, so
    /// the value is constructible at the caller's fold brand, where [`resolve`](Self::resolve)'s
    /// invariant `KObject<'a>` cannot go. Borrow-bearing variants are classified to owned
    /// sub-dispatches before any static cell, so they never reach here.
    pub fn resolve_region_pure<'b>(&self, brand: RegionBrand<'b>) -> KObject<'b> {
        match self {
            ExpressionPart::Keyword(kw) => KObject::KString(brand.allocator().text(kw.text())),
            ExpressionPart::Identifier(s) => KObject::KString(brand.allocator().text(s)),
            ExpressionPart::Type(t) => KObject::KString(brand.allocator().text(t.as_str())),
            ExpressionPart::Literal(lit) => lit.to_kobject(brand),
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
                 (keyword / bare identifier / type name / literal); borrow-bearing parts are \
                 classified to sub-dispatches before any static cell"
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
        let untyped_key = stored_untyped_key(brand, parts);
        let shape = classify_dispatch_shape(parts);
        let mut expression = KExpression {
            parts,
            span,
            file,
            untyped_key,
            shape,
            operator_probe: operator_probe_for(parts, shape),
            binder_plan: None,
            binder_name_slot: None,
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

    /// The declared-name position of the binder form this node's bucket key matches
    /// ([`BinderSpec::name_slot`](crate::machine::model::binder::BinderSpec)); `None` when the node
    /// is not a binder form, or the form's spine carries no declared name (`FN`, `OP`).
    pub fn binder_name_slot(&self) -> Option<usize> {
        self.binder_name_slot
    }

    /// True when this expression is a statement block: two or more parts, all of them
    /// `Expression`. The single definition the body splitters ([`split_body_statements`] /
    /// [`body_statement_refs`]) and the binder-install aggregation share, so the multi-statement
    /// cutoff is stated once.
    ///
    /// [`split_body_statements`]: crate::machine::split_body_statements
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
    /// if `parts[1]` is a single `Type(t)`, returns its bare name; `None` on shape
    /// mismatch. The builtin body surfaces the structured error.
    pub fn binder_name_from_type_part(&self) -> Option<&'a str> {
        match &self.parts.get(1)?.value {
            ExpressionPart::Type(t) => Some(t.as_str()),
            _ => None,
        }
    }

    /// If every part is `Expression(_)`, return the inner expressions; otherwise `None`. The
    /// returned `Vec` encodes the all-`Expression` shape — callers iterate the nodes directly
    /// without re-matching the variant.
    pub fn borrow_inner_expressions(&self) -> Option<Vec<KExpression<'a>>> {
        let mut out = Vec::with_capacity(self.parts.len());
        for part in self.parts {
            match &part.value {
                ExpressionPart::Expression(inner) => out.push(**inner),
                _ => return None,
            }
        }
        Some(out)
    }

    /// Right-fold counterpart of [`Self::borrow_inner_expressions`]: `(preceding, last)` with both
    /// unwrapped from [`ExpressionPart::Expression`]. On any shape mismatch returns `self` back so
    /// the caller can pass through.
    pub fn try_split_inner_expressions(
        self,
    ) -> Result<(Vec<KExpression<'a>>, KExpression<'a>), Self> {
        let Some(mut inner) = self.borrow_inner_expressions() else {
            return Err(self);
        };
        match inner.pop() {
            Some(last) => Ok((inner, last)),
            None => Err(self),
        }
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
