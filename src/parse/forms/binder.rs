//! Binder discovery: the pure, structural reading of which forms introduce a binder and which name
//! and bucket keys they declare.
//!
//! Everything here is a pure `&KExpression -> Option<…>` reader plus the [`BinderFacts`] that ride
//! a [`FORMS`](super::FORMS) entry — a form is a binder because its entry carries them, and nothing
//! else declares it. The keys are pinned against the live builtin registration table by the
//! table⟺registration property, so an entry whose builtin was renamed, re-shaped, or dropped fails
//! the suite.
//!
//! What a binder *does* with what is read — installing into a scope, rendering a refusal, announcing
//! a module body's declarations — is the machine's, in
//! [`machine::model::binder`](crate::machine::model::binder).

use smallvec::SmallVec;

use crate::memory::RegionBrand;
use crate::parse::ast::{ExpressionPart, KExpression, KeyElement};
use crate::parse::forms::{Form, KEYWORDS, form_for};
use crate::parse::labels::{BinderSymbol, KeywordSymbol, StaticName, WILDCARD};
use crate::source::Spanned;

/// Structural name extractor for a binder builtin. Returning `Some(name)` names the placeholder a
/// forward reference parks on while the binder's body is in flight. Both channels' names are `Copy`
/// symbols the parser minted when it classified the token, so the read allocates nothing and the
/// variant *is* the channel — a name reads as `Value` xor `Type` by construction, so the
/// placeholder is tagged by the same read that found the name.
pub type BinderNameFn = fn(&KExpression<'_>) -> Option<BinderSymbol>;

/// Structural bucket-key extractor for a binder that registers a callable
/// (`FN`, `OP`). Returns every bucket key a *call* to the to-be-registered
/// overloads would compute (e.g. `(MAKESET er :Ordered)` → one key
/// `[Keyword("MAKESET"), Slot]`; a `UNARY OP` → both the keyword-first list key
/// `[Keyword(sym), Slot]` and the binary bridge key `[Slot, Keyword(sym), Slot]`);
/// the driver stamps a claim on each inner-call bucket key so a sibling call
/// form parks on the producer instead of failing dispatch.
///
/// Separate from [`BinderNameFn`] because the two key different resolvers:
/// `BinderNameFn` for `Scope::resolve`, `BinderBucketFn` for the no-bucket fallback
/// in `resolve_dispatch`. Keying on the full bucket (not just the lead keyword)
/// keeps overloads sharing a head keyword but differing in later keywords
/// (`MAKESET _` vs `MAKESET _ USING _`) from colliding on the park edge.
///
/// A bucket key is synthesized rather than read straight out of the parts run, so the extractor
/// takes the brand that bumps each key into the node's own region.
pub type BinderBucketFn = for<'a> fn(RegionBrand<'a>, &KExpression<'a>) -> Option<BucketKeys<'a>>;

/// The bucket keys one binder's body registers overloads under: one for `FN` and binary `OP`, two
/// for `UNARY OP` (the keyword-first list key plus the binary bridge key). Two is the maximum any
/// declaration form reaches, so the pair is inline and `Copy` — each key is a run bumped into the
/// node's own region.
#[derive(Clone, Copy, Debug)]
pub struct BucketKeys<'a> {
    pub first: &'a [KeyElement],
    pub second: Option<&'a [KeyElement]>,
}

impl<'a> BucketKeys<'a> {
    /// One key, the `FN` / binary-`OP` shape.
    pub(crate) fn one(first: &'a [KeyElement]) -> Self {
        BucketKeys {
            first,
            second: None,
        }
    }

    /// Both keys in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = &'a [KeyElement]> + '_ {
        std::iter::once(self.first).chain(self.second)
    }

    /// How many keys this names — one or two, never none.
    pub fn count(&self) -> usize {
        1 + usize::from(self.second.is_some())
    }
}

/// What one statement installs into the enclosing scope: at most one name and at most two bucket
/// keys (the `LET … = UNARY OP …` maximum, both channels at once). Fixed-size and `Copy` — every
/// string and key run is a borrow at the node's own lifetime, so the stored form owns no heap.
#[derive(Clone, Copy, Debug)]
pub struct StoredBinderKey<'a> {
    pub name: Option<BinderSymbol>,
    pub buckets: Option<BucketKeys<'a>>,
}

// ---------- extractors (pure structural readers) ----------

/// Shared [`BinderNameFn`] for typed-binder builtins (SIG / UNION / NEWTYPE):
/// the binder name is `parts[1]`'s `Type(t)` token. A free function (not the
/// `KExpression::binder_name_from_type_part` method reference) so it wraps the symbol in the
/// channel tag [`BinderNameFn`] hands back.
pub(crate) fn type_part_binder_name(expr: &KExpression<'_>) -> Option<BinderSymbol> {
    expr.binder_name_from_type_part().map(BinderSymbol::Type)
}

/// Shared [`BinderNameFn`] for value-binder builtins (`LET <name> = …`, `MODULE <name> = …`): the
/// binder name is `parts[1]`'s `Identifier` token. The Identifier-part twin of
/// [`type_part_binder_name`], so each overload's extractor matches exactly its own name-part kind
/// and the placeholder is tagged `Value` xor `Type` to match where the bind lands.
pub(crate) fn identifier_part_binder_name(expr: &KExpression<'_>) -> Option<BinderSymbol> {
    match expr.parts.get(1)?.value {
        ExpressionPart::Identifier(v) => Some(BinderSymbol::Value(v)),
        _ => None,
    }
}

/// Placeholder extractor covering both `TYPE` overloads: the bare form's name is the `Type` part at
/// `parts[1]`; the higher-kinded form's name is the *last* inner part of the parenthesized
/// `(Param AS Name)` expression.
pub(crate) fn type_decl_binder_name(expr: &KExpression<'_>) -> Option<BinderSymbol> {
    match expr.parts.get(1)?.value {
        ExpressionPart::Type(t) => Some(BinderSymbol::Type(t)),
        ExpressionPart::Expression(inner) => match inner.parts.last()?.value {
            ExpressionPart::Type(t) => Some(BinderSymbol::Type(t)),
            _ => None,
        },
        _ => None,
    }
}

/// Bucket-key extractor for FN. The key must match what a future call would compute via
/// `KExpression::stored_key`: each Keyword maps to `KeyElement::Keyword`, and each
/// `<name> :<Type>` pair collapses to one `KeyElement::Slot`.
///
/// Unknown shapes advance silently — the body's full parse surfaces `ShapeError` on real
/// malformations, so we err toward producing the bucket key for well-formed signatures. An FN
/// registers exactly one overload, so the result names one key. Returns `None` only when the
/// signature slot itself is missing.
pub(crate) fn fn_def_binder_bucket<'a>(
    brand: RegionBrand<'a>,
    expr: &KExpression<'a>,
) -> Option<BucketKeys<'a>> {
    let signature_expr = signature_expr_part(expr)?;
    let parts = signature_expr.parts;
    // Staged on the stack, not the heap: the stride is data-dependent (`+= 2` collapses a
    // `<name> :<Type>` pair), so no exact-length iterator spells the run and the fill needs a length
    // before its first element. A signature longer than the inline capacity spills, which is the one
    // case that allocates.
    let mut key: SmallVec<[KeyElement; 8]> = SmallVec::new();
    let mut i = 0;
    while i < parts.len() {
        match parts[i].value {
            // `_` lexes keyword-class, but a `_ :<Type>` pair is an unnamed slot — the one keyword
            // this walk reads as a slot rather than as a fixed token, so a wildcard head keys the
            // same bucket the call spelling it will compute.
            ExpressionPart::Keyword(symbol)
                if symbol == WILDCARD.symbol() && next_is_type_slot(parts, i + 1) =>
            {
                key.push(KeyElement::Slot);
                i += 2;
            }
            ExpressionPart::Keyword(symbol) => {
                key.push(KeyElement::Keyword(symbol));
                i += 1;
            }
            ExpressionPart::Identifier(_) | ExpressionPart::Type(_)
                if next_is_type_slot(parts, i + 1) =>
            {
                key.push(KeyElement::Slot);
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }
    Some(BucketKeys::one(brand.allocator().slice_from_iter(key)))
}

/// True iff the part at `index` is a type ascription — the second half of a `<name> :<Type>` pair,
/// which collapses to one slot in the bucket key.
fn next_is_type_slot(parts: &[Spanned<ExpressionPart<'_>>], index: usize) -> bool {
    parts.get(index).is_some_and(|p| {
        matches!(
            p.value,
            ExpressionPart::Type(_)
                | ExpressionPart::Expression(_)
                | ExpressionPart::SigiledTypeExpr(_)
                | ExpressionPart::RecordType(_)
        )
    })
}

/// The head slot of an expression-shape definition: the part right after the keyword that opens
/// the head — `ALL` on a quantified form, whose group sits between `EXPR` and the head, and `EXPR`
/// otherwise. Read by position relative to that keyword rather than at a fixed index, so the bare
/// form and the combined `LET <name> = FN EXPR …` statement share one extractor. Anything but a
/// parenthesized group there is no head, so it keys no bucket.
fn signature_expr_part<'a>(expr: &KExpression<'a>) -> Option<&'a KExpression<'a>> {
    let head_keyword = |name: &StaticName<KeywordSymbol>| {
        expr.parts.iter().position(
            |part| matches!(part.value, ExpressionPart::Keyword(symbol) if symbol == name.symbol()),
        )
    };
    let head_index = head_keyword(&KEYWORDS.all).or_else(|| head_keyword(&KEYWORDS.expr))?;
    match expr.parts.get(head_index + 1)?.value {
        ExpressionPart::Expression(inner) => Some(inner.reference()),
        _ => None,
    }
}

/// Symbols the `OP` / `GROUP` surface spells with, plus the two ascription sigils. Declaring an
/// operator under one of these would make its own declaration form unreadable. Every other
/// keyword-classified token is a legal operator symbol, including an all-caps alphabetic name
/// (`OP #(MAX) OVER Number` is fine).
static RESERVED_SYMBOLS: [&StaticName<KeywordSymbol>; 12] = [
    &KEYWORDS.op,
    &KEYWORDS.unary,
    &KEYWORDS.over,
    &KEYWORDS.group,
    &KEYWORDS.fold,
    &KEYWORDS.pairwise,
    &KEYWORDS.left,
    &KEYWORDS.right,
    &KEYWORDS.equals,
    &KEYWORDS.arrow,
    &KEYWORDS.guard,
    &KEYWORDS.otherwise,
];

/// Why a quoted operator symbol will not do. The reason travels as data rather than as a rendered
/// message because the binder hook that reads a symbol runs inside node construction, where the
/// run's [`LabelInterner`] is out of reach; each surface that *reports* the refusal renders the
/// glyph itself ([`Self::into_error`]).
pub(crate) enum SymbolError {
    /// The quote body is not exactly one keyword token.
    Shape,
    /// A token the `OP` / `GROUP` surface spells with.
    #[cfg_attr(not(feature = "pending_rewrite"), allow(dead_code))]
    Reserved(KeywordSymbol),
}

/// The operator symbol a quote body carries: exactly one `Keyword` part. The `symbol` slot is
/// typed `:KExpression`, and a quote is data already — it never stages — so the part arrives raw
/// and un-dispatched, and its body is read here as data. A multi-part body, a non-keyword token, or
/// a reserved symbol is a shape error.
pub(crate) fn symbol_from_quote_body(
    inner: &KExpression<'_>,
) -> Result<KeywordSymbol, SymbolError> {
    let [part] = inner.parts else {
        return Err(SymbolError::Shape);
    };
    let ExpressionPart::Keyword(symbol) = part.value else {
        return Err(SymbolError::Shape);
    };
    if RESERVED_SYMBOLS
        .iter()
        .any(|reserved| reserved.symbol() == symbol)
    {
        return Err(SymbolError::Reserved(symbol));
    }
    Ok(symbol)
}

/// Statement-side symbol read: the declaration's first `QuotedExpression` part. `GROUP` scans its
/// unevaluated body block with this to collect its members; the binder hook uses it to decide
/// whether to install park edges (discarding the diagnostic — the body's own extraction surfaces
/// it).
pub(crate) fn symbol_from_parts(expr: &KExpression<'_>) -> Result<KeywordSymbol, SymbolError> {
    let quoted = expr
        .parts
        .iter()
        .find_map(|part| match part.value {
            ExpressionPart::QuotedExpression(inner) => Some(inner.reference()),
            _ => None,
        })
        .ok_or(SymbolError::Shape)?;
    symbol_from_quote_body(quoted)
}

/// True iff the declaration names `UNARY` — leading, for the bare form, or after the `LET <name> =`
/// prefix of the combined one. `UNARY` is a reserved symbol ([`RESERVED_SYMBOLS`]), so no operator
/// name can put the token anywhere else in the run.
fn is_unary_form(expr: &KExpression<'_>) -> bool {
    expr.parts.iter().any(|part| {
        matches!(part.value, ExpressionPart::Keyword(symbol) if symbol == KEYWORDS.unary.symbol())
    })
}

/// Park keys: every bucket this declaration's body registers an overload under, so a later sibling
/// statement using the operator parks on the `OP` slot instead of failing dispatch while the
/// declaration is still finalizing. A `UNARY OP` registers two bodies, so it names two keys.
pub(crate) fn op_def_binder_bucket<'a>(
    brand: RegionBrand<'a>,
    expr: &KExpression<'a>,
) -> Option<BucketKeys<'a>> {
    // The glyph's symbol is already minted on the quoted part, so the park keys are read off it.
    let sym = symbol_from_parts(expr).ok()?;
    if is_unary_form(expr) {
        Some(BucketKeys {
            first: stored_unary_key(brand, sym),
            second: Some(stored_binary_key(brand, sym)),
        })
    } else {
        Some(BucketKeys::one(stored_binary_key(brand, sym)))
    }
}

/// Region-bumped twin of [`binary_key`](crate::machine::model::binary_key): the `[Slot,
/// Keyword(sym), Slot]` run a reduced binary call computes. Agreeing with the owned builder on the
/// symbol is what lets a park edge installed here be found by a later call's key.
fn stored_binary_key<'a>(brand: RegionBrand<'a>, symbol: KeywordSymbol) -> &'a [KeyElement] {
    brand.allocator().slice(&[
        KeyElement::Slot,
        KeyElement::Keyword(symbol),
        KeyElement::Slot,
    ])
}

/// Region-bumped twin of [`unary_key`](crate::machine::model::unary_key): the `[Keyword(sym),
/// Slot]` run a reduced unary run computes.
fn stored_unary_key<'a>(brand: RegionBrand<'a>, symbol: KeywordSymbol) -> &'a [KeyElement] {
    brand
        .allocator()
        .slice(&[KeyElement::Keyword(symbol), KeyElement::Slot])
}

/// this; it exists so a consumer outside binder discovery can recognize a surface by *full bucket
/// key*, a structural read that a statement merely spelling one of the surface's keywords cannot
/// fool. An entry names its surface here rather than a reader re-matching the key run, because the
/// entry is already the answer to "which full key is this".
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BinderSurface {
    /// `OP #(…) OVER …` / `UNARY OP #(…) OVER …`, bare or in its combined `LET <name> =` spelling.
    OperatorDef,
    /// `NEWTYPE <name> = <representation>` — the declaration form, not the constructor family.
    NewTypeDef,
    /// `UNION <name> = <variants>`.
    UnionDef,
    /// Every other binder-introducing form.
    Other,
}

/// What a binder-introducing form installs: the extractors that read its declared name and bucket
/// keys out of the AST, plus the positions its spine fixes. Rides its [`Form`] entry, which is what
/// names the key.
///
/// The two channels are separate fields rather than one extractor list because a combined form
/// (`LET <name> = FN …`) fills both at once — a name *and* the bucket keys its body registers.
#[derive(Clone, Copy)]
pub struct BinderFacts {
    /// Name extractors tried in order; first `Some` wins. Empty for the bucket-only and
    /// declaration forms (`FN`, `OP`, `VAL`). Each extractor's [`BinderSymbol`] variant carries the
    /// channel the name binds in, so the entry states no separate kind.
    pub names: &'static [BinderNameFn],
    /// Bucket-key extractor for a form whose body registers overloads (`FN`, `OP`). `None` for the
    /// name-only forms.
    pub bucket: Option<BinderBucketFn>,
    /// The declaration surface this key belongs to (see [`BinderSurface`]).
    pub surface: BinderSurface,
    /// The parts-run position of the declared name, for the forms whose name is a direct part of
    /// the statement spine (`VAL` declares at this position even though it installs nothing;
    /// `TYPE`'s higher-kinded form nests its name inside the slot, so the position holds no bare
    /// name there and reads as vacuous). `None` for the bucket-only forms (`FN`, `OP`), whose
    /// spine carries no declared name. Dispatch resolution reads this off the node's cached form
    /// ([`KExpression::binder_name_slot`]) to exempt a declaration slot from parking on a
    /// still-finalizing same-named outer binder. Pinned against `names` by the
    /// name-slot⟺extractor property.
    pub name_slot: Option<usize>,
    /// Parts-run positions where a bare parenthesized part is a **type expression**: the parser
    /// rewrites a plain `Expression` part at each listed index to `SigiledTypeExpr`, making `(…)` ≡
    /// `:(…)` in exactly those slots (see [`admit_bare_type_slots`]). Indices are
    /// element-for-element with the entry's key, like `name_slot`.
    ///
    /// The mask is opt-in, not derived: a slot may take a type without wanting the flip.
    /// `NEWTYPE <name> = <repr>` is the standing case — a bare `(…)` there already works by
    /// evaluation, so it stays unmasked. The table⟺registration property pins that every masked
    /// index is a slot its bucket's live registrations type as a raw type-expression carrier and
    /// never as code.
    pub type_slots: &'static [usize],
}

impl BinderFacts {
    /// True iff this form declares no install channel at all — the `VAL` declaration form, which
    /// records into the decl scope's slot collector rather than a binding map.
    #[cfg(test)]
    pub fn installs_nothing(&self) -> bool {
        self.names.is_empty() && self.bucket.is_none()
    }
}

/// Parse-side admission of the bare parenthesized type spelling. If `parts` matches a builtin
/// form's key, every plain `Expression` part at one of that form's
/// [`type_slots`](BinderFacts::type_slots) is rewritten to `SigiledTypeExpr` — the same
/// `ProgramNode` payload under a different parse-context marker, so `(LIST OF Str)` ≡
/// `:(LIST OF Str)` in exactly those positions and nowhere else.
///
/// A variant change is the whole of it, and everything downstream follows by construction: the
/// statement's untyped key is unchanged (both variants are slots), the form's
/// [`lazy_slots`](crate::parse::forms::Form::lazy_slots) already stamp `TYPE_EXPR` at
/// each masked index so the part is captured raw instead of staged, and the return/operand slot's
/// carrier union already lists `SIGILED_TYPE_EXPR`. The two spellings are the same part by the time
/// anything semantic looks at them, so parity is exact.
///
/// Any other part kind at a masked index — a `Type` token, a `:(…)`, a `:{…}`, an identifier — is
/// left alone, and a run matching no form key is untouched. Idempotent.
///
/// Called from the parse frames, on a run that is still an unfrozen `Vec`: a node's parts and its
/// structural cache are bumped together and never touched again, so this must run before the
/// freeze. The run has no stored key yet, so it feeds the matcher the key elements its parts spell.
pub(crate) fn admit_bare_type_slots(parts: &mut [Spanned<ExpressionPart<'_>>]) {
    let Some(binder) =
        form_for(parts.iter().map(|part| part.value.key_element())).and_then(|form| form.binder)
    else {
        return;
    };
    for &index in binder.type_slots {
        // The key match pinned `key.len() == parts.len()`, and the table-shape property pins every
        // masked index to a slot position of that key, so the index is in range.
        if let ExpressionPart::Expression(node) = parts[index].value {
            parts[index].value = ExpressionPart::SigiledTypeExpr(node);
        }
    }
}

/// The schema expression of a `UNION <name> = (<schema>)` statement — its final slot.
#[cfg_attr(not(feature = "pending_rewrite"), allow(dead_code))]
pub(crate) fn union_schema<'a>(statement: &KExpression<'a>) -> Option<KExpression<'a>> {
    match statement.parts.last()?.value {
        ExpressionPart::Expression(schema) => Some(*schema),
        _ => None,
    }
}

/// The arity of the operator declaration `expression` is, or `None` if it is not one. Recognition
/// is by the node's cached form, admitted only when its binder facts are marked
/// [`BinderSurface::OperatorDef`] — a full bucket key, every keyword pinned in position — so a
/// statement that merely spells the `OP` token (a call to a user `FN` whose signature names it as a
/// keyword) is not an operator declaration, and neither is an `OP` nested inside some other
/// statement's slot. `GROUP` reads its members' symbols off exactly the statements this admits.
#[cfg_attr(not(feature = "pending_rewrite"), allow(dead_code))]
pub(crate) fn op_declaration_arity(expression: &KExpression<'_>) -> Option<OpArity> {
    let binder = expression.cache().form()?.binder?;
    if binder.surface != BinderSurface::OperatorDef {
        return None;
    }
    Some(if is_unary_form(expression) {
        OpArity::Unary
    } else {
        OpArity::Binary
    })
}

/// The two operator-declaration surfaces, as [`op_declaration_arity`] classifies them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OpArity {
    Binary,
    Unary,
}

/// What `expression` installs under `form`. Both channels are read — a combined form fills them
/// together. The key is read off the node's own stored run, and a synthesized bucket key is bumped
/// into `brand`'s region — the node's, since this runs from the construction door. Returns `None`
/// for a form with no binder facts, and for one whose extractors install nothing (`VAL`, and the
/// anonymous `FN :{…}` whose signature part names no bucket).
pub(crate) fn binder_plan_for<'a>(
    brand: RegionBrand<'a>,
    form: Option<&'static Form>,
    expression: &KExpression<'a>,
) -> Option<StoredBinderKey<'a>> {
    let binder = form?.binder?;
    let name = binder.names.iter().find_map(|extract| extract(expression));
    let buckets = binder.bucket.and_then(|extract| extract(brand, expression));
    if name.is_none() && buckets.is_none() {
        return None;
    }
    Some(StoredBinderKey { name, buckets })
}
