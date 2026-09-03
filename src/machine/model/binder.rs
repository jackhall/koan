//! Binder discovery model: the pure, structural reading of which AST forms introduce a binder and
//! which name and bucket keys they declare.
//!
//! Everything here is a pure `&KExpression -> Option<…>` reader plus a static spec table
//! ([`BINDER_SPECS`]) that is the single source of truth for the binder-introducing forms — a form
//! is a binder because it has an entry here, and nothing else declares it. The keys are pinned
//! against the live builtin registration table by the spec⟺registration consistency test, so an
//! entry whose builtin was renamed, re-shaped, or dropped fails the suite.

use smallvec::SmallVec;

pub(crate) mod signature;

use crate::machine::core::{KError, KErrorKind, RegionBrand, body_statement_refs};
use crate::machine::model::KeyElement;
#[cfg(test)]
use crate::machine::model::UntypedKey;
use crate::machine::model::ast::Part;
#[cfg(test)]
use crate::machine::model::key_spec::key_matches_untyped;
use crate::machine::model::key_spec::{KEYWORDS, KeyElementSpec, key_matches_parts};
use crate::machine::model::labels::{
    BinderSymbol, KeywordSymbol, LabelInterner, StaticName, ValueSymbol,
};
use crate::machine::model::registries::RunRegistries;
use crate::machine::model::types::{AnnouncedData, display_label, pair_list_names};
use crate::machine::model::{ExpressionPart, KExpression};
use crate::source::Spanned;

/// Whether a binding — committed or an in-flight placeholder — lives in the value
/// language or the type language. The `data`/`types` partition is mutually exclusive: the two
/// tables key by disjoint classified symbol types, so a name is one xor the other by
/// construction. A forward-reference placeholder carries its name's own class
/// ([`BinderSymbol`](crate::machine::model::BinderSymbol)), so a type placeholder is never
/// satisfied by a value bind, nor the reverse.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum BindKind {
    Value,
    Type,
}

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

    pub fn len(&self) -> usize {
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

/// The signature slot of an `FN` declaration: the part right after the `FN` keyword. Read by
/// position relative to that keyword rather than at a fixed index, so the bare form and the
/// combined `LET <name> = FN …` statement share one extractor. A `RecordType` there is the
/// anonymous form, which registers no bucket — `None`, and the statement installs nothing on this
/// channel.
fn signature_expr_part<'a>(expr: &KExpression<'a>) -> Option<&'a KExpression<'a>> {
    let fn_index = expr
        .parts
        .iter()
        .position(|part| {
            matches!(part.value, ExpressionPart::Keyword(symbol) if symbol == KEYWORDS.fn_.symbol())
        })?;
    match expr.parts.get(fn_index + 1)?.value {
        ExpressionPart::Expression(inner) => Some(inner.reference()),
        _ => None,
    }
}

/// The names the machine itself fixes in Rust source for binders no program spells a declaration
/// for. Each declares as a [`StaticName`] and is minted once for the process, so a form binds by
/// loading the symbol rather than by classifying the spelling again per evaluation.
///
/// They live here, beside [`BINDER_SPECS`], because they answer the same question that table does
/// — what a form binds — for the forms whose binder is implicit in the surface rather than written
/// in it. The builtins that install them read them back from here, so there is one spelling of
/// each.
pub(crate) struct MachineBinders {
    /// What every `MATCH` and `TRY` arm binds its scrutinee under.
    pub(crate) arm: StaticName<ValueSymbol>,
    /// The binary `OP` body's two operands, named by the surface rather than declared as
    /// parameters.
    pub(crate) operand_left: StaticName<ValueSymbol>,
    pub(crate) operand_right: StaticName<ValueSymbol>,
    /// The unary `OP` body's single parameter: the whole operand run as one list.
    pub(crate) operands: StaticName<ValueSymbol>,
}

pub(crate) static MACHINE_BINDERS: MachineBinders = MachineBinders {
    arm: crate::static_name!(ValueSymbol, "it"),
    operand_left: crate::static_name!(ValueSymbol, "left"),
    operand_right: crate::static_name!(ValueSymbol, "right"),
    operands: crate::static_name!(ValueSymbol, "operands"),
};

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
    Reserved(KeywordSymbol),
}

impl SymbolError {
    /// The diagnostic, rendered against the run that interned the glyph.
    pub(crate) fn into_error(self, labels: &LabelInterner) -> KError {
        KError::new(KErrorKind::ShapeError(match self {
            SymbolError::Shape => {
                "operator symbol must be one quoted token: `OP #(+) OVER Number = (…)`".to_string()
            }
            SymbolError::Reserved(symbol) => format!(
                "`{}` is reserved by the operator-declaration surface and cannot name an operator",
                labels.display(symbol.symbol()),
            ),
        }))
    }
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

// ---------- the spec table ----------

/// Which declaration surface a spec entry belongs to. Binder discovery itself never branches on
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

/// A binder-introducing form: the untyped bucket key it dispatches under and the extractors that
/// read its declared name and bucket keys out of the AST.
///
/// The two channels are separate fields rather than one extractor list because a combined form
/// (`LET <name> = FN …`) fills both at once — a name *and* the bucket keys its body registers.
pub struct BinderSpec {
    /// Full untyped bucket key — ALL keywords in position, never just the lead keyword.
    pub key: &'static [KeyElementSpec],
    /// Name extractors tried in order; first `Some` wins. Empty for the bucket-only and
    /// declaration forms (`FN`, `OP`, `VAL`). Each extractor's [`BinderSymbol`] variant carries the
    /// channel the name binds in, so the spec states no separate kind.
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
    /// spine carries no declared name. Dispatch resolution reads this off the node's cached copy
    /// ([`KExpression::binder_name_slot`]) to exempt a declaration slot from parking on a
    /// still-finalizing same-named outer binder. Pinned against `names` by the
    /// name-slot⟺extractor consistency test.
    pub name_slot: Option<usize>,
    /// Parts-run positions where a bare parenthesized part is a **type expression**: the parser
    /// rewrites a plain `Expression` part at each listed index to `SigiledTypeExpr`, making `(…)` ≡
    /// `:(…)` in exactly those slots (see [`admit_bare_type_slots`]). Indices are
    /// element-for-element with `key`, like `name_slot`.
    ///
    /// The mask is opt-in, not derived: a slot may take a type without wanting the flip.
    /// `NEWTYPE <name> = <repr>` is the standing case — a bare `(…)` there already works by
    /// evaluation, so it stays unmasked. The consistency test pins that every masked index is a
    /// slot its bucket's live registrations type as a raw type-expression carrier and never as
    /// code.
    pub type_slots: &'static [usize],
}

impl BinderSpec {
    /// True iff this form declares no install channel at all — the `VAL` declaration form, which
    /// records into the decl scope's slot collector rather than a binding map.
    #[cfg(test)]
    pub fn installs_nothing(&self) -> bool {
        self.names.is_empty() && self.bucket.is_none()
    }

    /// True iff this spec's key matches the runtime bucket key element-for-element.
    #[cfg(test)]
    pub fn matches_key(&self, key: &UntypedKey) -> bool {
        key_matches_untyped(self.key, key)
    }

    /// [`Self::matches_key`] read straight off an expression's parts, materializing no key at all —
    /// the parts already carry every token this compares.
    pub fn matches_parts<'a, P: Part<'a>>(&self, parts: &[Spanned<P>]) -> bool {
        key_matches_parts(self.key, parts)
    }
}

use KeyElementSpec::{Keyword as Kw, Slot};

/// The single source of truth for the binder-introducing forms. One entry per distinct untyped
/// bucket key; the keys are pinned against the live builtin registration table by the
/// spec⟺registration consistency test.
pub static BINDER_SPECS: &[BinderSpec] = &[
    // LET <name> = <value>: value-name overload then type-alias overload.
    BinderSpec {
        key: &[Kw(&KEYWORDS.let_), Slot, Kw(&KEYWORDS.equals), Slot],
        names: &[identifier_part_binder_name, type_part_binder_name],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
        type_slots: &[],
    },
    // TYPE <name> — SIG-body-only abstract-type declarator (bare and higher-kinded share the key).
    BinderSpec {
        key: &[Kw(&KEYWORDS.type_), Slot],
        names: &[type_decl_binder_name],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
        type_slots: &[],
    },
    // MODULE <name> = <body> (a module is a value, so the name slot is an `Identifier`; a
    // Type-token name registers nothing and takes the miss table's respelling diagnostic).
    BinderSpec {
        key: &[Kw(&KEYWORDS.module), Slot, Kw(&KEYWORDS.equals), Slot],
        names: &[identifier_part_binder_name],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
        type_slots: &[],
    },
    // GROUP <name> FOLD LEFT = <body>.
    BinderSpec {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        names: &[identifier_part_binder_name],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
        type_slots: &[],
    },
    // GROUP <name> FOLD RIGHT = <body>.
    BinderSpec {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        names: &[identifier_part_binder_name],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
        type_slots: &[],
    },
    // GROUP <name> PAIRWISE FOLD <combiner> LEFT = <body>.
    BinderSpec {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.pairwise),
            Kw(&KEYWORDS.fold),
            Slot,
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        names: &[identifier_part_binder_name],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
        type_slots: &[],
    },
    // GROUP <name> PAIRWISE FOLD <combiner> RIGHT = <body>.
    BinderSpec {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.pairwise),
            Kw(&KEYWORDS.fold),
            Slot,
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        names: &[identifier_part_binder_name],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
        type_slots: &[],
    },
    // SIG <name> = <body>.
    BinderSpec {
        key: &[Kw(&KEYWORDS.sig), Slot, Kw(&KEYWORDS.equals), Slot],
        names: &[type_part_binder_name],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
        type_slots: &[],
    },
    // UNION <name> = <schema>.
    BinderSpec {
        key: &[Kw(&KEYWORDS.union), Slot, Kw(&KEYWORDS.equals), Slot],
        names: &[type_part_binder_name],
        bucket: None,
        surface: BinderSurface::UnionDef,
        name_slot: Some(1),
        type_slots: &[],
    },
    // NEWTYPE <name> = <repr> (scalar / sigil / record reprs share the key). The repr slot takes a
    // type but is deliberately unmasked: a bare `(…)` there already works by evaluation, so flipping
    // it would change a working spelling's route for nothing.
    BinderSpec {
        key: &[Kw(&KEYWORDS.newtype), Slot, Kw(&KEYWORDS.equals), Slot],
        names: &[type_part_binder_name],
        bucket: None,
        surface: BinderSurface::NewTypeDef,
        name_slot: Some(1),
        type_slots: &[],
    },
    // NEWTYPE <decl> — constructor family (keyword set {NEWTYPE}, disjoint from the `= _` forms).
    BinderSpec {
        key: &[Kw(&KEYWORDS.newtype), Slot],
        names: &[type_decl_binder_name],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
        type_slots: &[],
    },
    // FN <signature> -> <return_type> = <body> (every FN overload shares this key; the anonymous
    // record-schema form claims no bucket because the extractor rejects its signature operand).
    BinderSpec {
        key: &[
            Kw(&KEYWORDS.fn_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        names: &[],
        bucket: Some(fn_def_binder_bucket),
        surface: BinderSurface::Other,
        name_slot: None,
        type_slots: &[3],
    },
    // OP <symbol> OVER <operand> = <body>.
    BinderSpec {
        key: &[
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        names: &[],
        bucket: Some(op_def_binder_bucket),
        surface: BinderSurface::OperatorDef,
        name_slot: None,
        type_slots: &[3],
    },
    // OP <symbol> OVER <operand> -> <return_type> = <body>.
    BinderSpec {
        key: &[
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        names: &[],
        bucket: Some(op_def_binder_bucket),
        surface: BinderSurface::OperatorDef,
        name_slot: None,
        type_slots: &[3, 5],
    },
    // UNARY OP <symbol> OVER <operand> -> <return_type> = <body>.
    BinderSpec {
        key: &[
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        names: &[],
        bucket: Some(op_def_binder_bucket),
        surface: BinderSurface::OperatorDef,
        name_slot: None,
        type_slots: &[4, 6],
    },
    // The combined statement forms: one binder filling both channels — the LET value name and the
    // bucket key(s) the declaration's body registers under. `LET <name> = UNARY OP …` is the
    // two-bucket maximum.
    //
    // LET <name> = FN <signature> -> <return_type> = <body>.
    BinderSpec {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.fn_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        names: &[identifier_part_binder_name],
        bucket: Some(fn_def_binder_bucket),
        surface: BinderSurface::Other,
        name_slot: Some(1),
        type_slots: &[6],
    },
    // LET <name> = OP <symbol> OVER <operand> = <body>.
    BinderSpec {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        names: &[identifier_part_binder_name],
        bucket: Some(op_def_binder_bucket),
        surface: BinderSurface::OperatorDef,
        name_slot: Some(1),
        type_slots: &[6],
    },
    // LET <name> = OP <symbol> OVER <operand> -> <return_type> = <body>.
    BinderSpec {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        names: &[identifier_part_binder_name],
        bucket: Some(op_def_binder_bucket),
        surface: BinderSurface::OperatorDef,
        name_slot: Some(1),
        type_slots: &[6, 8],
    },
    // LET <name> = UNARY OP <symbol> OVER <operand> -> <return_type> = <body>.
    BinderSpec {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        names: &[identifier_part_binder_name],
        bucket: Some(op_def_binder_bucket),
        surface: BinderSurface::OperatorDef,
        name_slot: Some(1),
        type_slots: &[7, 9],
    },
    // VAL <name> <ty> — a declaration form with no install channel. It records
    // into the decl scope's slot collector, not a binding map any name lookup can see, so it
    // installs nothing; it appears here so the one-place specification of the declaration forms is
    // complete.
    BinderSpec {
        key: &[Kw(&KEYWORDS.val), Slot, Slot],
        names: &[],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
        type_slots: &[],
    },
];

/// Parse-side admission of the bare parenthesized type spelling. If `parts` matches a binder form's
/// key, every plain `Expression` part at one of that form's [`type_slots`](BinderSpec::type_slots)
/// is rewritten to `SigiledTypeExpr` — the same `ProgramNode` payload under a different
/// parse-context marker, so `(LIST OF Str)` ≡ `:(LIST OF Str)` in exactly those positions and
/// nowhere else.
///
/// A variant change is the whole of it, and everything downstream follows by construction: the
/// statement's untyped key is unchanged (both variants are slots), the form's `LAZY_SLOT_SPECS`
/// entry already stamps `TYPE_EXPR` at each masked index so the part is captured raw instead of
/// staged, and the return/operand slot's carrier union already lists `SIGILED_TYPE_EXPR`. The two
/// spellings are the same part by the time anything semantic looks at them, so parity is exact.
///
/// Any other part kind at a masked index — a `Type` token, a `:(…)`, a `:{…}`, an identifier — is
/// left alone, and a run matching no binder key is untouched. Idempotent.
///
/// Called from the parse frames, on a run that is still an unfrozen `Vec`: a node's parts and its
/// structural cache are bumped together and never touched again, so this must run before the freeze.
pub(crate) fn admit_bare_type_slots(parts: &mut [Spanned<ExpressionPart<'_>>]) {
    let Some(spec) = BINDER_SPECS.iter().find(|spec| spec.matches_parts(parts)) else {
        return;
    };
    for &index in spec.type_slots {
        // `matches_parts` pinned `key.len() == parts.len()`, and the consistency test pins every
        // masked index to a slot position of that key, so the index is in range.
        if let ExpressionPart::Expression(node) = parts[index].value {
            parts[index].value = ExpressionPart::SigiledTypeExpr(node);
        }
    }
}

/// The nominal-type declaration surfaces a module body pre-announces, as
/// [`announced_type_declaration`] classifies them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeDeclarationSurface {
    NewType,
    Union,
}

/// What `expression` announces to its module body's declaration window, or `None` if it announces
/// nothing.
///
/// Recognition is by **full bucket key** against the [`BINDER_SPECS`] entries — every keyword
/// pinned in position — so a user overload that merely shares a head keyword announces nothing, and
/// the constructor-family key `NEWTYPE <decl>` is excluded structurally rather than by inspecting
/// what its extractor would return. Only a statement at the body's top level is offered here; a
/// declaration nested inside another statement's slot keeps ordinary dataflow order.
pub(crate) fn announced_type_declaration(
    expression: &KExpression<'_>,
) -> Option<TypeDeclarationSurface> {
    match binder_spec_for(expression)?.surface {
        BinderSurface::NewTypeDef => Some(TypeDeclarationSurface::NewType),
        BinderSurface::UnionDef => Some(TypeDeclarationSurface::Union),
        BinderSurface::OperatorDef | BinderSurface::Other => None,
    }
}

/// Pre-scan `body`'s **top-level** statements for the type declarations the body announces, so
/// every one of their names is visible to every statement regardless of order — which is what lets
/// a plain module host a mutually-recursive group.
///
/// A `NEWTYPE` announces one standalone member. A `UNION` announces one member per statically
/// scannable variant tag, each **owned** by the union's binder: a variant is never
/// bare-name-resolvable and never lands in `bindings.types`, so it is reached only through the
/// binder or by member projection off it (`:(Tree.Node)`). A `UNION` whose schema does not scan announces nothing at all —
/// its own dispatch surfaces the real diagnostic.
///
/// Nested and computed declarations are untouched by construction: the scan sees only the statement
/// split [`body_statement_refs`] draws, the same boundary `GROUP` reads its members off.
pub(crate) fn announce_type_members(
    body: &KExpression<'_>,
    module: ValueSymbol,
    registries: &RunRegistries,
) -> Result<Option<AnnouncedData>, KError> {
    let mut announced = AnnouncedData::default();
    for statement in body_statement_refs(body) {
        let Some(surface) = announced_type_declaration(statement) else {
            continue;
        };
        let Some(name) = statement.binder_name_from_type_part() else {
            continue;
        };
        // The parser classified and interned the binder token, so the window, the members it
        // seals and every diagnostic naming one already share one currency.
        let binder = name;
        if announced.declares(binder) || announced.binds(binder) {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "module `{}` declares type `{}` twice",
                display_label(module.symbol(), registries),
                display_label(binder.symbol(), registries),
            ))));
        }
        match surface {
            TypeDeclarationSurface::NewType => {
                announced.announce(binder);
            }
            TypeDeclarationSurface::Union => {
                // The variant tags are the union's announced members. A schema this scan cannot
                // read is left entirely unannounced rather than half-announced.
                let Some(schema) = union_schema(statement) else {
                    continue;
                };
                match pair_list_names(&schema, "UNION schema", registries) {
                    Ok(tags) => {
                        announced.announce_binder(binder, tags);
                    }
                    Err(_) => continue,
                }
            }
        }
    }
    Ok((!announced.is_empty()).then_some(announced))
}

/// The schema expression of a `UNION <name> = (<schema>)` statement — its final slot.
pub(crate) fn union_schema<'a>(statement: &KExpression<'a>) -> Option<KExpression<'a>> {
    match statement.parts.last()?.value {
        ExpressionPart::Expression(schema) => Some(*schema),
        _ => None,
    }
}

/// The arity of the operator declaration `expression` is, or `None` if it is not one. Recognition
/// is by **full bucket key** against the [`BINDER_SPECS`] entries marked
/// [`BinderSurface::OperatorDef`] — every keyword pinned in position — so a statement that merely
/// spells the `OP` token (a call to a user `FN` whose signature names it as a keyword) is not an
/// operator declaration, and neither is an `OP` nested inside some other statement's slot. `GROUP`
/// reads its members' symbols off exactly the statements this admits.
pub(crate) fn op_declaration_arity(expression: &KExpression<'_>) -> Option<OpArity> {
    let spec = binder_spec_for(expression)?;
    if spec.surface != BinderSurface::OperatorDef {
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

/// The [`BINDER_SPECS`] entry `expression`'s bucket key matches, or `None` for a non-binder shape.
/// The one spec-table probe — every reader of the table's per-form facts (install plan, surface
/// classification, declared-name position) resolves its entry through here.
pub(crate) fn binder_spec_for(expression: &KExpression<'_>) -> Option<&'static BinderSpec> {
    BINDER_SPECS
        .iter()
        .find(|spec| spec.matches_parts(expression.parts))
}

/// What `expression` installs under its matched `spec`. Both channels are read — a combined form
/// fills them together. The key is read off the node's own stored run, and a synthesized bucket key
/// is bumped into `brand`'s region — the node's, since this runs from the construction door.
/// Returns `None` for a form whose extractors install nothing (`VAL`, and the anonymous `FN :{…}`
/// whose signature part names no bucket).
pub(crate) fn binder_plan_from_spec<'a>(
    brand: RegionBrand<'a>,
    spec: &BinderSpec,
    expression: &KExpression<'a>,
) -> Option<StoredBinderKey<'a>> {
    let name = spec.names.iter().find_map(|extract| extract(expression));
    let buckets = spec.bucket.and_then(|extract| extract(brand, expression));
    if name.is_none() && buckets.is_none() {
        return None;
    }
    Some(StoredBinderKey { name, buckets })
}

#[cfg(test)]
mod tests;
