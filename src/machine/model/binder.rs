//! Binder discovery model: the pure, structural reading of which AST forms introduce a binder and
//! which name and bucket keys they declare.
//!
//! Everything here is a pure `&KExpression -> Option<…>` reader plus a static spec table
//! ([`BINDER_SPECS`]) that is the single source of truth for the binder-introducing forms. The
//! table is derived from the builtin registration sites and pinned against them by the
//! spec⟺registration consistency test, so a new binder builtin that forgets its spec entry (or a
//! spec entry with no live registration) fails the suite.

use crate::machine::core::{KError, KErrorKind, RegionBrand};
#[cfg(test)]
use crate::machine::model::UntypedElement;
use crate::machine::model::UntypedKey;
use crate::machine::model::{owned_untyped_key, StoredElement};
use crate::machine::model::{ExpressionPart, KExpression};
use crate::source::Spanned;

/// Whether a binding — committed or an in-flight placeholder — lives in the value
/// language or the type language. The `data`/`types` partition is mutually exclusive
/// (a name is one xor the other; see the cross-kind check in the write paths), and a
/// forward-reference placeholder is tagged with its kind so a type placeholder is
/// never satisfied by a value bind, nor the reverse.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum BindKind {
    Value,
    Type,
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
/// the driver claims a pending slot in each `bindings.functions` bucket so a sibling call
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
    /// for the bindings tables.
    pub fn to_owned_key(self) -> BinderKey {
        BinderKey {
            name: self.name.map(|(name, kind)| (name.to_string(), kind)),
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
    pub name: Option<(String, BindKind)>,
    /// 0..=2 entries, in declaration order.
    pub buckets: Vec<UntypedKey>,
}

// ---------- extractors (pure structural readers) ----------

/// Shared [`BinderNameFn`] for typed-binder builtins (SIG / UNION / RECURSIVE TYPES / NEWTYPE):
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
    let mut key: Vec<StoredElement<'a>> = Vec::with_capacity(parts.len());
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
    Some(BucketKeys::one(brand.alloc_slice(&key)))
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
    brand.alloc_slice(&[
        StoredElement::Slot,
        StoredElement::Keyword(symbol),
        StoredElement::Slot,
    ])
}

/// Stored twin of [`unary_key`](crate::machine::model::unary_key): the `[Keyword(sym), Slot]` run a
/// reduced unary run computes.
fn stored_unary_key<'a>(brand: RegionBrand<'a>, symbol: &'a str) -> &'a [StoredElement<'a>] {
    brand.alloc_slice(&[StoredElement::Keyword(symbol), StoredElement::Slot])
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
/// bucket key; keys and extractors are pinned against the live builtin registration table by the
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
    },
    // TYPE <name> — SIG-body-only abstract-type declarator (bare and higher-kinded share the key).
    BinderSpec {
        key: &[Kw("TYPE"), Slot],
        names: &[(type_decl_binder_name, BindKind::Type)],
        bucket: None,
    },
    // MODULE <name> = <body> (identifier overload; the type-named overload has no hooks).
    BinderSpec {
        key: &[Kw("MODULE"), Slot, Kw("="), Slot],
        names: &[(identifier_part_binder_name, BindKind::Value)],
        bucket: None,
    },
    // GROUP <name> FOLD LEFT = <body>.
    BinderSpec {
        key: &[Kw("GROUP"), Slot, Kw("FOLD"), Kw("LEFT"), Kw("="), Slot],
        names: &[(identifier_part_binder_name, BindKind::Value)],
        bucket: None,
    },
    // GROUP <name> FOLD RIGHT = <body>.
    BinderSpec {
        key: &[Kw("GROUP"), Slot, Kw("FOLD"), Kw("RIGHT"), Kw("="), Slot],
        names: &[(identifier_part_binder_name, BindKind::Value)],
        bucket: None,
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
    },
    // SIG <name> = <body>.
    BinderSpec {
        key: &[Kw("SIG"), Slot, Kw("="), Slot],
        names: &[(type_part_binder_name, BindKind::Type)],
        bucket: None,
    },
    // UNION <name> = <schema>.
    BinderSpec {
        key: &[Kw("UNION"), Slot, Kw("="), Slot],
        names: &[(type_part_binder_name, BindKind::Type)],
        bucket: None,
    },
    // NEWTYPE <name> = <repr> (scalar / sigil / record reprs share the key).
    BinderSpec {
        key: &[Kw("NEWTYPE"), Slot, Kw("="), Slot],
        names: &[(type_part_binder_name, BindKind::Type)],
        bucket: None,
    },
    // NEWTYPE <decl> — constructor family (keyword set {NEWTYPE}, disjoint from the `= _` forms).
    BinderSpec {
        key: &[Kw("NEWTYPE"), Slot],
        names: &[(type_decl_binder_name, BindKind::Type)],
        bucket: None,
    },
    // RECURSIVE TYPES <name> = <body>.
    BinderSpec {
        key: &[Kw("RECURSIVE"), Kw("TYPES"), Slot, Kw("="), Slot],
        names: &[(type_part_binder_name, BindKind::Type)],
        bucket: None,
    },
    // FN <signature> -> <return_type> = <body> (three hook-bearing overloads share this key; the
    // anonymous record-schema overload has no hooks).
    BinderSpec {
        key: &[Kw("FN"), Slot, Kw("->"), Slot, Kw("="), Slot],
        names: &[],
        bucket: Some(fn_def_binder_bucket),
    },
    // OP <symbol> OVER <operand> = <body>.
    BinderSpec {
        key: &[Kw("OP"), Slot, Kw("OVER"), Slot, Kw("="), Slot],
        names: &[],
        bucket: Some(op_def_binder_bucket),
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
    },
    // VAL <name> <ty> — a declaration form with no install channel. It records
    // into the decl scope's slot collector, not a binding map any name lookup can see, so it
    // installs nothing; it appears here so the one-place specification of the declaration forms is
    // complete.
    BinderSpec {
        key: &[Kw("VAL"), Slot, Slot],
        names: &[],
        bucket: None,
    },
];

/// What `expression` installs, if its bucket key matches a [`BINDER_SPECS`] entry whose extractors
/// read a binder out of the AST. Both channels are read — a combined form fills them together. The
/// key is read off the node's own stored run, and a synthesized bucket key is bumped into `brand`'s
/// region — the node's, since this runs from the construction door. Returns `None` for a non-binder
/// shape, and for a form whose extractors install nothing (`VAL`, and the anonymous `FN :{…}` whose
/// signature part names no bucket).
pub(crate) fn binder_plan_for<'a>(
    brand: RegionBrand<'a>,
    expression: &KExpression<'a>,
) -> Option<StoredBinderKey<'a>> {
    let spec = BINDER_SPECS
        .iter()
        .find(|spec| spec.matches_stored_key(expression.stored_key()))?;
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
