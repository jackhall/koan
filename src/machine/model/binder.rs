//! Binder discovery model: the pure, structural reading of which AST forms introduce a binder,
//! which name or bucket they declare, and which of their slots carry nested binders forward.
//!
//! Everything here is a pure `&KExpression -> Option<…>` reader plus a static spec table
//! ([`BINDER_SPECS`]) that is the single source of truth for the binder-introducing forms. The
//! table is derived from the builtin registration sites and pinned against them by the
//! spec⟺registration consistency test, so a new binder builtin that forgets its spec entry (or a
//! spec entry with no live registration) fails the suite.

use crate::machine::core::{KError, KErrorKind};
use crate::machine::model::{binary_key, unary_key};
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::model::{UntypedElement, UntypedKey};

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
/// forward reference parks on while the binder's body is in flight.
pub type BinderNameFn = for<'a> fn(&KExpression<'a>) -> Option<String>;

/// Structural bucket-key extractor for a binder that registers a callable
/// (`FN`, `OP`). Returns every `UntypedKey` a *call* to the to-be-registered
/// overloads would compute (e.g. `(MAKESET er :Ordered)` → one key
/// `[Keyword("MAKESET"), Slot]`; a `UNARY OP` → both the keyword-first list key
/// `[Keyword(sym), Slot]` and the binary bridge key `[Slot, Keyword(sym), Slot]`);
/// the driver installs each in `bindings.pending_overloads` so a sibling call form
/// parks on the producer instead of failing dispatch.
///
/// Separate from [`BinderNameFn`] because the two key different resolvers:
/// `BinderNameFn` for `Scope::resolve`, `BinderBucketFn` for the no-bucket fallback
/// in `resolve_dispatch`. Keying on the full bucket (not just the lead keyword)
/// keeps overloads sharing a head keyword but differing in later keywords
/// (`MAKESET _` vs `MAKESET _ USING _`) from colliding on the park edge.
pub type BinderBucketFn = for<'a> fn(&KExpression<'a>) -> Option<Vec<UntypedKey>>;

/// The two install channels a binder may use, mutually exclusive per binder. `Bucket` carries
/// every key the binder's body registers an overload under — a `UNARY OP` declares two.
#[derive(Clone, Debug)]
pub enum BinderKey {
    Name(String, BindKind),
    Bucket(Vec<UntypedKey>),
}

// ---------- extractors (pure structural readers) ----------

/// Shared [`BinderNameFn`] for typed-binder builtins (SIG / UNION / RECURSIVE TYPES / NEWTYPE):
/// the binder name is `parts[1]`'s `Type(t)` token. A free function (not the
/// `KExpression::binder_name_from_type_part` method reference) so the signature is higher-ranked
/// over the expression lifetime, as `BinderNameFn` requires.
pub(crate) fn type_part_binder_name(expr: &KExpression<'_>) -> Option<String> {
    expr.binder_name_from_type_part()
}

/// Shared [`BinderNameFn`] for value-binder builtins (`LET <name> = …`, `MODULE <name> = …`): the
/// binder name is `parts[1]`'s `Identifier` token. The Identifier-part twin of
/// [`type_part_binder_name`], so each overload's extractor matches exactly its own name-part kind
/// and the placeholder is tagged `Value` xor `Type` to match where the bind lands.
pub(crate) fn identifier_part_binder_name(expr: &KExpression<'_>) -> Option<String> {
    match &expr.parts.get(1)?.value {
        ExpressionPart::Identifier(s) => Some(s.clone()),
        _ => None,
    }
}

/// Placeholder extractor covering both `TYPE` overloads: the bare form's name is the `Type` part at
/// `parts[1]`; the higher-kinded form's name is the *last* inner part of the parenthesized
/// `(Param AS Name)` expression.
pub(crate) fn type_decl_binder_name(expr: &KExpression<'_>) -> Option<String> {
    match &expr.parts.get(1)?.value {
        ExpressionPart::Type(t) => Some(t.render()),
        ExpressionPart::Expression(inner) => match &inner.parts.last()?.value {
            ExpressionPart::Type(t) => Some(t.render()),
            _ => None,
        },
        _ => None,
    }
}

/// Bucket-key extractor for FN. The key must match what a future call would compute via
/// `KExpression::untyped_key`: each Keyword maps to `UntypedElement::Keyword`, and each
/// `<name> :<Type>` pair collapses to one `UntypedElement::Slot`.
///
/// Unknown shapes advance silently — the body's full parse surfaces `ShapeError` on real
/// malformations, so we err toward producing the bucket key for well-formed signatures. An FN
/// registers exactly one overload, so the returned vector holds one key. Returns `None` only when
/// the signature slot itself is missing.
pub(crate) fn fn_def_binder_bucket(expr: &KExpression<'_>) -> Option<Vec<UntypedKey>> {
    let signature_expr = signature_expr_part(expr)?;
    let parts = &signature_expr.parts;
    let mut key = Vec::with_capacity(parts.len());
    let mut i = 0;
    while i < parts.len() {
        match &parts[i].value {
            ExpressionPart::Keyword(s) => {
                key.push(UntypedElement::Keyword(s.clone()));
                i += 1;
            }
            ExpressionPart::Identifier(_) => {
                let next_is_type_slot = parts.get(i + 1).is_some_and(|p| {
                    matches!(
                        p.value,
                        ExpressionPart::Type(_)
                            | ExpressionPart::Expression(_)
                            | ExpressionPart::SigiledTypeExpr(_)
                            | ExpressionPart::RecordType(_)
                            | ExpressionPart::Spliced { .. }
                    )
                });
                if next_is_type_slot {
                    key.push(UntypedElement::Slot);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            ExpressionPart::Type(_) => {
                let next_is_type_slot = parts.get(i + 1).is_some_and(|p| {
                    matches!(
                        p.value,
                        ExpressionPart::Type(_)
                            | ExpressionPart::Expression(_)
                            | ExpressionPart::SigiledTypeExpr(_)
                            | ExpressionPart::RecordType(_)
                            | ExpressionPart::Spliced { .. }
                    )
                });
                if next_is_type_slot {
                    key.push(UntypedElement::Slot);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            _ => {
                i += 1;
            }
        }
    }
    Some(vec![key])
}

fn signature_expr_part<'a, 'b>(expr: &'b KExpression<'a>) -> Option<&'b KExpression<'a>> {
    let sig_part = expr.parts.get(1)?;
    match &sig_part.value {
        ExpressionPart::Expression(boxed) => Some(boxed.as_ref()),
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
pub(crate) fn symbol_from_quote_body(inner: &KExpression<'_>) -> Result<String, KError> {
    let [part] = inner.parts.as_slice() else {
        return Err(symbol_shape_error());
    };
    let ExpressionPart::Keyword(sym) = &part.value else {
        return Err(symbol_shape_error());
    };
    if RESERVED_SYMBOLS.contains(&sym.as_str()) {
        return Err(KError::new(KErrorKind::ShapeError(format!(
            "`{sym}` is reserved by the operator-declaration surface and cannot name an operator",
        ))));
    }
    Ok(sym.clone())
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
pub(crate) fn symbol_from_parts(expr: &KExpression<'_>) -> Result<String, KError> {
    let quoted = expr
        .parts
        .iter()
        .find_map(|part| match &part.value {
            ExpressionPart::QuotedExpression(inner) => Some(inner.as_ref()),
            _ => None,
        })
        .ok_or_else(symbol_shape_error)?;
    symbol_from_quote_body(quoted)
}

/// True iff the declaration leads with `UNARY`.
fn is_unary_form(expr: &KExpression<'_>) -> bool {
    matches!(
        expr.parts.first().map(|p| &p.value),
        Some(ExpressionPart::Keyword(k)) if k == "UNARY",
    )
}

/// Park keys: every bucket this declaration's body registers an overload under, so a later sibling
/// statement using the operator parks on the `OP` slot instead of failing dispatch while the
/// declaration is still finalizing. A `UNARY OP` registers two bodies, so it names two keys.
pub(crate) fn op_def_binder_bucket(expr: &KExpression<'_>) -> Option<Vec<UntypedKey>> {
    let sym = symbol_from_parts(expr).ok()?;
    if is_unary_form(expr) {
        Some(vec![unary_key(&sym), binary_key(&sym)])
    } else {
        Some(vec![binary_key(&sym)])
    }
}

// ---------- the spec table ----------

/// One element of a [`BinderSpec`] bucket key: a fixed keyword token or a slot. The static-friendly
/// twin of [`UntypedElement`] (whose `Keyword` owns a `String`).
pub enum UntypedElementSpec {
    Keyword(&'static str),
    Slot,
}

impl UntypedElementSpec {
    fn matches(&self, element: &UntypedElement) -> bool {
        match (self, element) {
            (UntypedElementSpec::Keyword(a), UntypedElement::Keyword(b)) => *a == b.as_str(),
            (UntypedElementSpec::Slot, UntypedElement::Slot) => true,
            _ => false,
        }
    }
}

/// One extractor of a [`BinderSpec`]: a name channel (with its bind kind) or a bucket channel.
pub enum BinderExtract {
    Name(BinderNameFn, BindKind),
    Bucket(BinderBucketFn),
}

impl BinderExtract {
    fn run(&self, expr: &KExpression<'_>) -> Option<BinderKey> {
        match self {
            BinderExtract::Name(extractor, kind) => {
                extractor(expr).map(|name| BinderKey::Name(name, *kind))
            }
            BinderExtract::Bucket(extractor) => extractor(expr).map(BinderKey::Bucket),
        }
    }
}

/// A binder-introducing form: the untyped bucket key it dispatches under, the extractors that read
/// its declared name/bucket out of the AST, and the chain-slot mask marking which of its slots
/// carry nested binders forward.
pub struct BinderSpec {
    /// Full untyped bucket key — ALL keywords in position, never just the lead keyword.
    pub key: &'static [UntypedElementSpec],
    /// Extractors tried in order; first `Some` wins. Empty for declaration forms (`VAL`).
    pub extractors: &'static [BinderExtract],
    /// Slots whose nested binders join the statement aggregate and whose staged deps are
    /// binder-covered. Derived from the binder overloads' signatures: an `Argument` slot is
    /// `true` iff its ktype != `KType::KEXPRESSION`, ANDed across every binder overload in the
    /// bucket (keyword positions false). Per-bucket constant.
    pub chain_slot_mask: &'static [bool],
}

impl BinderSpec {
    /// True iff this spec's key matches the runtime bucket key element-for-element.
    pub fn matches_key(&self, key: &UntypedKey) -> bool {
        self.key.len() == key.len()
            && self
                .key
                .iter()
                .zip(key.iter())
                .all(|(spec, element)| spec.matches(element))
    }
}

use BinderExtract::{Bucket, Name};
use UntypedElementSpec::{Keyword as Kw, Slot};

/// The single source of truth for the binder-introducing forms. One entry per distinct untyped
/// bucket key; keys, extractors, and masks are pinned against the live builtin registration table
/// by the spec⟺registration consistency test.
pub static BINDER_SPECS: &[BinderSpec] = &[
    // LET <name> = <value>: value-name overload then type-alias overload.
    BinderSpec {
        key: &[Kw("LET"), Slot, Kw("="), Slot],
        extractors: &[
            Name(identifier_part_binder_name, BindKind::Value),
            Name(type_part_binder_name, BindKind::Type),
        ],
        chain_slot_mask: &[false, true, false, true],
    },
    // TYPE <name> — SIG-body-only abstract-type declarator (bare and higher-kinded share the key).
    BinderSpec {
        key: &[Kw("TYPE"), Slot],
        extractors: &[Name(type_decl_binder_name, BindKind::Type)],
        chain_slot_mask: &[false, false],
    },
    // MODULE <name> = <body> (identifier overload; the type-named overload has no hooks).
    BinderSpec {
        key: &[Kw("MODULE"), Slot, Kw("="), Slot],
        extractors: &[Name(identifier_part_binder_name, BindKind::Value)],
        chain_slot_mask: &[false, true, false, false],
    },
    // GROUP <name> FOLD LEFT = <body>.
    BinderSpec {
        key: &[Kw("GROUP"), Slot, Kw("FOLD"), Kw("LEFT"), Kw("="), Slot],
        extractors: &[Name(identifier_part_binder_name, BindKind::Value)],
        chain_slot_mask: &[false, true, false, false, false, false],
    },
    // GROUP <name> FOLD RIGHT = <body>.
    BinderSpec {
        key: &[Kw("GROUP"), Slot, Kw("FOLD"), Kw("RIGHT"), Kw("="), Slot],
        extractors: &[Name(identifier_part_binder_name, BindKind::Value)],
        chain_slot_mask: &[false, true, false, false, false, false],
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
        extractors: &[Name(identifier_part_binder_name, BindKind::Value)],
        chain_slot_mask: &[false, true, false, false, false, false, false, false],
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
        extractors: &[Name(identifier_part_binder_name, BindKind::Value)],
        chain_slot_mask: &[false, true, false, false, false, false, false, false],
    },
    // SIG <name> = <body>.
    BinderSpec {
        key: &[Kw("SIG"), Slot, Kw("="), Slot],
        extractors: &[Name(type_part_binder_name, BindKind::Type)],
        chain_slot_mask: &[false, true, false, false],
    },
    // UNION <name> = <schema>.
    BinderSpec {
        key: &[Kw("UNION"), Slot, Kw("="), Slot],
        extractors: &[Name(type_part_binder_name, BindKind::Type)],
        chain_slot_mask: &[false, true, false, false],
    },
    // NEWTYPE <name> = <repr> (scalar / sigil / record reprs share the key).
    BinderSpec {
        key: &[Kw("NEWTYPE"), Slot, Kw("="), Slot],
        extractors: &[Name(type_part_binder_name, BindKind::Type)],
        chain_slot_mask: &[false, true, false, true],
    },
    // NEWTYPE <decl> — constructor family (keyword set {NEWTYPE}, disjoint from the `= _` forms).
    BinderSpec {
        key: &[Kw("NEWTYPE"), Slot],
        extractors: &[Name(type_decl_binder_name, BindKind::Type)],
        chain_slot_mask: &[false, false],
    },
    // RECURSIVE TYPES <name> = <body>.
    BinderSpec {
        key: &[Kw("RECURSIVE"), Kw("TYPES"), Slot, Kw("="), Slot],
        extractors: &[Name(type_part_binder_name, BindKind::Type)],
        chain_slot_mask: &[false, false, true, false, false],
    },
    // FN <signature> -> <return_type> = <body> (three hook-bearing overloads share this key; the
    // anonymous record-schema overload has no hooks).
    BinderSpec {
        key: &[Kw("FN"), Slot, Kw("->"), Slot, Kw("="), Slot],
        extractors: &[Bucket(fn_def_binder_bucket)],
        chain_slot_mask: &[false, false, false, true, false, false],
    },
    // OP <symbol> OVER <operand> = <body>.
    BinderSpec {
        key: &[Kw("OP"), Slot, Kw("OVER"), Slot, Kw("="), Slot],
        extractors: &[Bucket(op_def_binder_bucket)],
        chain_slot_mask: &[false, false, false, true, false, false],
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
        extractors: &[Bucket(op_def_binder_bucket)],
        chain_slot_mask: &[false, false, false, true, false, true, false, false],
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
        extractors: &[Bucket(op_def_binder_bucket)],
        chain_slot_mask: &[false, false, false, false, true, false, true, false, false],
    },
    // VAL <name> <ty> — a declaration form with no install channel and no chain slots. It records
    // into the decl scope's slot collector, not a binding map any name lookup can see, so it
    // installs nothing; it appears here so the one-place specification of the declaration forms is
    // complete.
    BinderSpec {
        key: &[Kw("VAL"), Slot, Slot],
        extractors: &[],
        chain_slot_mask: &[false, false, false],
    },
];

/// The binder channel and chain-slot mask for `expr`, if its untyped key matches a
/// [`BINDER_SPECS`] entry whose extractors read a binder out of the AST. `key` is `expr`'s
/// already-computed untyped key. Returns `None` for a non-binder shape, and for a declaration form
/// whose extractors install nothing (`VAL`).
pub(crate) fn binder_plan_for(
    expr: &KExpression<'_>,
    key: &UntypedKey,
) -> Option<(BinderKey, &'static [bool])> {
    let spec = BINDER_SPECS.iter().find(|spec| spec.matches_key(key))?;
    let binder_key = spec
        .extractors
        .iter()
        .find_map(|extract| extract.run(expr))?;
    Some((binder_key, spec.chain_slot_mask))
}

#[cfg(test)]
mod tests;
