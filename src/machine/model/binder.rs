//! Binder discovery model: the pure, structural reading of which AST forms introduce a binder and
//! which name and bucket keys they declare.
//!
//! Everything here is a pure `&KExpression -> Option<…>` reader plus a static spec table
//! ([`BINDER_SPECS`]) that is the single source of truth for the binder-introducing forms — a form
//! is a binder because it has an entry here, and nothing else declares it. The keys are pinned
//! against the live builtin registration table by the spec⟺registration consistency test, so an
//! entry whose builtin was renamed, re-shaped, or dropped fails the suite.

use smallvec::SmallVec;

use crate::machine::core::{KError, KErrorKind, RegionBrand};
#[cfg(test)]
use crate::machine::model::UntypedElement;
use crate::machine::model::UntypedKey;
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::model::{StoredElement, owned_untyped_key};
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

/// Classify a declaration's binder name into the **type** channel and record it for rendering, or
/// the `ShapeError` naming the partition it crosses. This is the text→symbol declaration seam: past
/// it every table key carries its own class, so a value/type crossing is unrepresentable rather
/// than something a write door probes for.
pub fn type_binder(
    name: &str,
    registries: &crate::machine::model::RunRegistries,
) -> Result<crate::machine::model::TypeSymbol, KError> {
    crate::machine::model::TypeSymbol::declared(name, &registries.labels).ok_or_else(|| {
        KError::new(KErrorKind::ShapeError(
            crate::machine::model::wrong_binder_class(name, BindKind::Type),
        ))
    })
}

/// The **value** channel's half of [`type_binder`].
pub fn value_binder(
    name: &str,
    registries: &crate::machine::model::RunRegistries,
) -> Result<crate::machine::model::ValueSymbol, KError> {
    crate::machine::model::ValueSymbol::declared(name, &registries.labels).ok_or_else(|| {
        KError::new(KErrorKind::ShapeError(
            crate::machine::model::wrong_binder_class(name, BindKind::Value),
        ))
    })
}

/// Structural name extractor for a binder builtin. Returning `Some(name)` names the placeholder a
/// forward reference parks on while the binder's body is in flight. The name is a token of the
/// node's own parts run, so it is already resident at the node's lifetime and the read allocates
/// nothing.
pub type BinderNameFn = for<'a> fn(&KExpression<'a>) -> Option<&'a str>;

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
    pub first: &'a [StoredElement<'a>],
    pub second: Option<&'a [StoredElement<'a>]>,
}

impl<'a> BucketKeys<'a> {
    /// One key, the `FN` / binary-`OP` shape.
    pub(crate) fn one(first: &'a [StoredElement<'a>]) -> Self {
        BucketKeys {
            first,
            second: None,
        }
    }

    /// Both keys in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = &'a [StoredElement<'a>]> + '_ {
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
    pub name: Option<(&'a str, BindKind)>,
    pub buckets: Option<BucketKeys<'a>>,
}

impl<'a> StoredBinderKey<'a> {
    /// The owned [`BinderKey`] this stands for — materialized where an install path needs map keys
    /// for the bindings tables. The name is classified into the binding vocabulary here, at the one
    /// seam where the declaration's source text is still in hand, and interned so a `Rebind`
    /// naming it can render. A keyword-class name classifies to `None` — nothing binds to a
    /// keyword, so the statement claims no name.
    pub fn to_owned_key(self, labels: &crate::machine::model::LabelInterner) -> BinderKey {
        BinderKey {
            name: self.name.and_then(|(name, kind)| {
                let binder = crate::machine::model::BinderSymbol::declared(name, labels);
                debug_assert!(
                    binder.is_none_or(|binder| binder.bind_kind() == kind),
                    "a binder spec's declared kind agrees with its name's token class",
                );
                binder
            }),
            buckets: self
                .buckets
                .into_iter()
                .flat_map(|keys| keys.iter().map(owned_untyped_key).collect::<Vec<_>>())
                .collect(),
        }
    }
}

/// Owned twin of [`StoredBinderKey`]: the transient install currency the submission path hands the
/// bindings tables, whose keys are owned.
#[derive(Clone, Debug)]
pub struct BinderKey {
    pub name: Option<crate::machine::model::BinderSymbol>,
    /// 0..=2 entries, in declaration order.
    pub buckets: Vec<UntypedKey>,
}

// ---------- extractors (pure structural readers) ----------

/// Shared [`BinderNameFn`] for typed-binder builtins (SIG / UNION / NEWTYPE):
/// the binder name is `parts[1]`'s `Type(t)` token. A free function (not the
/// `KExpression::binder_name_from_type_part` method reference) so the signature is higher-ranked
/// over the expression lifetime, as `BinderNameFn` requires.
pub(crate) fn type_part_binder_name<'a>(expr: &KExpression<'a>) -> Option<&'a str> {
    expr.binder_name_from_type_part()
}

/// Shared [`BinderNameFn`] for value-binder builtins (`LET <name> = …`, `MODULE <name> = …`): the
/// binder name is `parts[1]`'s `Identifier` token. The Identifier-part twin of
/// [`type_part_binder_name`], so each overload's extractor matches exactly its own name-part kind
/// and the placeholder is tagged `Value` xor `Type` to match where the bind lands.
pub(crate) fn identifier_part_binder_name<'a>(expr: &KExpression<'a>) -> Option<&'a str> {
    match expr.parts.get(1)?.value {
        ExpressionPart::Identifier(s) => Some(s),
        _ => None,
    }
}

/// Placeholder extractor covering both `TYPE` overloads: the bare form's name is the `Type` part at
/// `parts[1]`; the higher-kinded form's name is the *last* inner part of the parenthesized
/// `(Param AS Name)` expression.
pub(crate) fn type_decl_binder_name<'a>(expr: &KExpression<'a>) -> Option<&'a str> {
    match expr.parts.get(1)?.value {
        ExpressionPart::Type(t) => Some(t.as_str()),
        ExpressionPart::Expression(inner) => match inner.parts.last()?.value {
            ExpressionPart::Type(t) => Some(t.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// Bucket-key extractor for FN. The key must match what a future call would compute via
/// `KExpression::stored_key`: each Keyword maps to `StoredElement::Keyword`, and each
/// `<name> :<Type>` pair collapses to one `StoredElement::Slot`.
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
    let mut key: SmallVec<[StoredElement<'a>; 8]> = SmallVec::new();
    let mut i = 0;
    while i < parts.len() {
        match parts[i].value {
            ExpressionPart::Keyword(s) => {
                key.push(StoredElement::Keyword(s));
                i += 1;
            }
            ExpressionPart::Identifier(_) | ExpressionPart::Type(_)
                if next_is_type_slot(parts, i + 1) =>
            {
                key.push(StoredElement::Slot);
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
        .position(|part| matches!(part.value, ExpressionPart::Keyword("FN")))?;
    match expr.parts.get(fn_index + 1)?.value {
        ExpressionPart::Expression(inner) => Some(inner.reference()),
        _ => None,
    }
}

/// Symbols the `OP` / `GROUP` surface spells with, plus the two ascription sigils. Declaring an
/// operator under one of these would make its own declaration form unreadable. Every other
/// keyword-classified token is a legal operator symbol, including an all-caps alphabetic name
/// (`OP #(MAX) OVER Number` is fine).
const RESERVED_SYMBOLS: [&str; 12] = [
    "OP", "UNARY", "OVER", "GROUP", "FOLD", "PAIRWISE", "LEFT", "RIGHT", "=", "->", ":|", ":!",
];

/// The operator symbol a quote body carries: exactly one `Keyword` part. The `symbol` slot is
/// typed `:KExpression`, so a `QuotedExpression` part arrives raw and un-dispatched (it makes the
/// declaration a lazy candidate) and its body is read here as data. A multi-part body, a
/// non-keyword token, or a reserved symbol is a shape error.
pub(crate) fn symbol_from_quote_body<'a>(inner: &KExpression<'a>) -> Result<&'a str, KError> {
    let [part] = inner.parts else {
        return Err(symbol_shape_error());
    };
    let ExpressionPart::Keyword(sym) = part.value else {
        return Err(symbol_shape_error());
    };
    if RESERVED_SYMBOLS.contains(&sym) {
        return Err(KError::new(KErrorKind::ShapeError(format!(
            "`{sym}` is reserved by the operator-declaration surface and cannot name an operator",
        ))));
    }
    Ok(sym)
}

fn symbol_shape_error() -> KError {
    KError::new(KErrorKind::ShapeError(
        "operator symbol must be one quoted token: `OP #(+) OVER Number = (…)`".to_string(),
    ))
}

/// Statement-side symbol read: the declaration's first `QuotedExpression` part. `GROUP` scans its
/// unevaluated body block with this to collect its members; the binder hook uses it to decide
/// whether to install park edges (discarding the diagnostic — the body's own extraction surfaces
/// it).
pub(crate) fn symbol_from_parts<'a>(expr: &KExpression<'a>) -> Result<&'a str, KError> {
    let quoted = expr
        .parts
        .iter()
        .find_map(|part| match part.value {
            ExpressionPart::QuotedExpression(inner) => Some(inner.reference()),
            _ => None,
        })
        .ok_or_else(symbol_shape_error)?;
    symbol_from_quote_body(quoted)
}

/// True iff the declaration names `UNARY` — leading, for the bare form, or after the `LET <name> =`
/// prefix of the combined one. `UNARY` is a reserved symbol ([`RESERVED_SYMBOLS`]), so no operator
/// name can put the token anywhere else in the run.
fn is_unary_form(expr: &KExpression<'_>) -> bool {
    expr.parts
        .iter()
        .any(|part| matches!(part.value, ExpressionPart::Keyword("UNARY")))
}

/// Park keys: every bucket this declaration's body registers an overload under, so a later sibling
/// statement using the operator parks on the `OP` slot instead of failing dispatch while the
/// declaration is still finalizing. A `UNARY OP` registers two bodies, so it names two keys.
pub(crate) fn op_def_binder_bucket<'a>(
    brand: RegionBrand<'a>,
    expr: &KExpression<'a>,
) -> Option<BucketKeys<'a>> {
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

/// Stored twin of [`binary_key`](crate::machine::model::binary_key): the `[Slot, Keyword(sym),
/// Slot]` run a reduced binary call computes, bumped into `brand`'s region. Byte-for-byte agreement
/// with the owned builder is what lets a park edge installed here be found by a later call's key.
fn stored_binary_key<'a>(brand: RegionBrand<'a>, symbol: &'a str) -> &'a [StoredElement<'a>] {
    brand.allocator().slice(&[
        StoredElement::Slot,
        StoredElement::Keyword(symbol),
        StoredElement::Slot,
    ])
}

/// Stored twin of [`unary_key`](crate::machine::model::unary_key): the `[Keyword(sym), Slot]` run a
/// reduced unary run computes.
fn stored_unary_key<'a>(brand: RegionBrand<'a>, symbol: &'a str) -> &'a [StoredElement<'a>] {
    brand
        .allocator()
        .slice(&[StoredElement::Keyword(symbol), StoredElement::Slot])
}

// ---------- the spec table ----------

/// One element of a [`BinderSpec`] bucket key: a fixed keyword token or a slot. The static-friendly
/// twin of [`UntypedElement`] (whose `Keyword` owns a `String`).
pub enum UntypedElementSpec {
    Keyword(&'static str),
    Slot,
}

impl UntypedElementSpec {
    #[cfg(test)]
    fn matches(&self, element: &UntypedElement) -> bool {
        match (self, element) {
            (UntypedElementSpec::Keyword(a), UntypedElement::Keyword(b)) => *a == b.as_str(),
            (UntypedElementSpec::Slot, UntypedElement::Slot) => true,
            _ => false,
        }
    }

    /// Stored-key peer of [`Self::matches`], so spec matching reads a node's bucket key straight
    /// off the run bumped at construction.
    fn matches_stored(&self, element: &StoredElement<'_>) -> bool {
        match (self, element) {
            (UntypedElementSpec::Keyword(a), StoredElement::Keyword(b)) => a == b,
            (UntypedElementSpec::Slot, StoredElement::Slot) => true,
            _ => false,
        }
    }
}

/// Which declaration surface a spec entry belongs to. Binder discovery itself never branches on
/// this; it exists so a consumer outside binder discovery can recognize a surface by *full bucket
/// key*, a structural read that a statement merely spelling one of the surface's keywords cannot
/// fool.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BinderSurface {
    /// `OP #(…) OVER …` / `UNARY OP #(…) OVER …`, bare or in its combined `LET <name> =` spelling.
    OperatorDef,
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
    pub key: &'static [UntypedElementSpec],
    /// Name extractors tried in order; first `Some` wins. Empty for the bucket-only and
    /// declaration forms (`FN`, `OP`, `VAL`).
    pub names: &'static [(BinderNameFn, BindKind)],
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
        self.key.len() == key.len()
            && self
                .key
                .iter()
                .zip(key.iter())
                .all(|(spec, element)| spec.matches(element))
    }

    /// [`Self::matches_key`] against a node's stored bucket key, with no owned key materialized.
    pub fn matches_stored_key(&self, key: &[StoredElement<'_>]) -> bool {
        self.key.len() == key.len()
            && self
                .key
                .iter()
                .zip(key.iter())
                .all(|(spec, element)| spec.matches_stored(element))
    }
}

use UntypedElementSpec::{Keyword as Kw, Slot};

/// The single source of truth for the binder-introducing forms. One entry per distinct untyped
/// bucket key; the keys are pinned against the live builtin registration table by the
/// spec⟺registration consistency test.
pub static BINDER_SPECS: &[BinderSpec] = &[
    // LET <name> = <value>: value-name overload then type-alias overload.
    BinderSpec {
        key: &[Kw("LET"), Slot, Kw("="), Slot],
        names: &[
            (identifier_part_binder_name, BindKind::Value),
            (type_part_binder_name, BindKind::Type),
        ],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
    },
    // TYPE <name> — SIG-body-only abstract-type declarator (bare and higher-kinded share the key).
    BinderSpec {
        key: &[Kw("TYPE"), Slot],
        names: &[(type_decl_binder_name, BindKind::Type)],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
    },
    // MODULE <name> = <body> (identifier overload; the type-named overload has no hooks).
    BinderSpec {
        key: &[Kw("MODULE"), Slot, Kw("="), Slot],
        names: &[(identifier_part_binder_name, BindKind::Value)],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
    },
    // GROUP <name> FOLD LEFT = <body>.
    BinderSpec {
        key: &[Kw("GROUP"), Slot, Kw("FOLD"), Kw("LEFT"), Kw("="), Slot],
        names: &[(identifier_part_binder_name, BindKind::Value)],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
    },
    // GROUP <name> FOLD RIGHT = <body>.
    BinderSpec {
        key: &[Kw("GROUP"), Slot, Kw("FOLD"), Kw("RIGHT"), Kw("="), Slot],
        names: &[(identifier_part_binder_name, BindKind::Value)],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
    },
    // GROUP <name> PAIRWISE FOLD <combiner> LEFT = <body>.
    BinderSpec {
        key: &[
            Kw("GROUP"),
            Slot,
            Kw("PAIRWISE"),
            Kw("FOLD"),
            Slot,
            Kw("LEFT"),
            Kw("="),
            Slot,
        ],
        names: &[(identifier_part_binder_name, BindKind::Value)],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
    },
    // GROUP <name> PAIRWISE FOLD <combiner> RIGHT = <body>.
    BinderSpec {
        key: &[
            Kw("GROUP"),
            Slot,
            Kw("PAIRWISE"),
            Kw("FOLD"),
            Slot,
            Kw("RIGHT"),
            Kw("="),
            Slot,
        ],
        names: &[(identifier_part_binder_name, BindKind::Value)],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
    },
    // SIG <name> = <body>.
    BinderSpec {
        key: &[Kw("SIG"), Slot, Kw("="), Slot],
        names: &[(type_part_binder_name, BindKind::Type)],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
    },
    // UNION <name> = <schema>.
    BinderSpec {
        key: &[Kw("UNION"), Slot, Kw("="), Slot],
        names: &[(type_part_binder_name, BindKind::Type)],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
    },
    // NEWTYPE <name> = <repr> (scalar / sigil / record reprs share the key).
    BinderSpec {
        key: &[Kw("NEWTYPE"), Slot, Kw("="), Slot],
        names: &[(type_part_binder_name, BindKind::Type)],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
    },
    // NEWTYPE <decl> — constructor family (keyword set {NEWTYPE}, disjoint from the `= _` forms).
    BinderSpec {
        key: &[Kw("NEWTYPE"), Slot],
        names: &[(type_decl_binder_name, BindKind::Type)],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
    },
    // FN <signature> -> <return_type> = <body> (every FN overload shares this key; the anonymous
    // record-schema form claims no bucket because the extractor rejects its signature operand).
    BinderSpec {
        key: &[Kw("FN"), Slot, Kw("->"), Slot, Kw("="), Slot],
        names: &[],
        bucket: Some(fn_def_binder_bucket),
        surface: BinderSurface::Other,
        name_slot: None,
    },
    // OP <symbol> OVER <operand> = <body>.
    BinderSpec {
        key: &[Kw("OP"), Slot, Kw("OVER"), Slot, Kw("="), Slot],
        names: &[],
        bucket: Some(op_def_binder_bucket),
        surface: BinderSurface::OperatorDef,
        name_slot: None,
    },
    // OP <symbol> OVER <operand> -> <return_type> = <body>.
    BinderSpec {
        key: &[
            Kw("OP"),
            Slot,
            Kw("OVER"),
            Slot,
            Kw("->"),
            Slot,
            Kw("="),
            Slot,
        ],
        names: &[],
        bucket: Some(op_def_binder_bucket),
        surface: BinderSurface::OperatorDef,
        name_slot: None,
    },
    // UNARY OP <symbol> OVER <operand> -> <return_type> = <body>.
    BinderSpec {
        key: &[
            Kw("UNARY"),
            Kw("OP"),
            Slot,
            Kw("OVER"),
            Slot,
            Kw("->"),
            Slot,
            Kw("="),
            Slot,
        ],
        names: &[],
        bucket: Some(op_def_binder_bucket),
        surface: BinderSurface::OperatorDef,
        name_slot: None,
    },
    // The combined statement forms: one binder filling both channels — the LET value name and the
    // bucket key(s) the declaration's body registers under. `LET <name> = UNARY OP …` is the
    // two-bucket maximum.
    //
    // LET <name> = FN <signature> -> <return_type> = <body>.
    BinderSpec {
        key: &[
            Kw("LET"),
            Slot,
            Kw("="),
            Kw("FN"),
            Slot,
            Kw("->"),
            Slot,
            Kw("="),
            Slot,
        ],
        names: &[(identifier_part_binder_name, BindKind::Value)],
        bucket: Some(fn_def_binder_bucket),
        surface: BinderSurface::Other,
        name_slot: Some(1),
    },
    // LET <name> = OP <symbol> OVER <operand> = <body>.
    BinderSpec {
        key: &[
            Kw("LET"),
            Slot,
            Kw("="),
            Kw("OP"),
            Slot,
            Kw("OVER"),
            Slot,
            Kw("="),
            Slot,
        ],
        names: &[(identifier_part_binder_name, BindKind::Value)],
        bucket: Some(op_def_binder_bucket),
        surface: BinderSurface::OperatorDef,
        name_slot: Some(1),
    },
    // LET <name> = OP <symbol> OVER <operand> -> <return_type> = <body>.
    BinderSpec {
        key: &[
            Kw("LET"),
            Slot,
            Kw("="),
            Kw("OP"),
            Slot,
            Kw("OVER"),
            Slot,
            Kw("->"),
            Slot,
            Kw("="),
            Slot,
        ],
        names: &[(identifier_part_binder_name, BindKind::Value)],
        bucket: Some(op_def_binder_bucket),
        surface: BinderSurface::OperatorDef,
        name_slot: Some(1),
    },
    // LET <name> = UNARY OP <symbol> OVER <operand> -> <return_type> = <body>.
    BinderSpec {
        key: &[
            Kw("LET"),
            Slot,
            Kw("="),
            Kw("UNARY"),
            Kw("OP"),
            Slot,
            Kw("OVER"),
            Slot,
            Kw("->"),
            Slot,
            Kw("="),
            Slot,
        ],
        names: &[(identifier_part_binder_name, BindKind::Value)],
        bucket: Some(op_def_binder_bucket),
        surface: BinderSurface::OperatorDef,
        name_slot: Some(1),
    },
    // VAL <name> <ty> — a declaration form with no install channel. It records
    // into the decl scope's slot collector, not a binding map any name lookup can see, so it
    // installs nothing; it appears here so the one-place specification of the declaration forms is
    // complete.
    BinderSpec {
        key: &[Kw("VAL"), Slot, Slot],
        names: &[],
        bucket: None,
        surface: BinderSurface::Other,
        name_slot: Some(1),
    },
];

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
    let spec = binder_spec_for(expression)?;
    match spec.key {
        [Kw("NEWTYPE"), Slot, Kw("="), Slot] => Some(TypeDeclarationSurface::NewType),
        [Kw("UNION"), Slot, Kw("="), Slot] => Some(TypeDeclarationSurface::Union),
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
        .find(|spec| spec.matches_stored_key(expression.stored_key()))
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
    let name = spec
        .names
        .iter()
        .find_map(|(extract, kind)| extract(expression).map(|name| (name, *kind)));
    let buckets = spec.bucket.and_then(|extract| extract(brand, expression));
    if name.is_none() && buckets.is_none() {
        return None;
    }
    Some(StoredBinderKey { name, buckets })
}

#[cfg(test)]
mod tests;
