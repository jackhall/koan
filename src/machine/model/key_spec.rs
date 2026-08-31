//! Bucket-key specs: the shared vocabulary the static builtin-form tables are written in.
//!
//! Both spec tables — [`BINDER_SPECS`](crate::machine::model::binder::BINDER_SPECS) and
//! [`LAZY_SLOT_SPECS`](crate::machine::model::lazy_slots::LAZY_SLOT_SPECS) — recognize a form by
//! its **full untyped bucket key**, every keyword pinned in position. That recognition is sound
//! because builtin buckets are unshadowable: a node whose key matches a table entry can only ever
//! resolve to that builtin's overloads. This module holds the one keyword group both tables spell
//! their keys with and the one matcher both probe through, so a surface token's spelling and the
//! meaning of "this key matches" are each written once.

use crate::machine::model::ast::{Part, PartClass};
use crate::machine::model::labels::{KeywordSymbol, StaticName};
#[cfg(test)]
use crate::machine::model::{KeyElement, UntypedKey};
use crate::source::Spanned;

/// The fixed tokens the builtin forms are spelled with, each declared once and minted once. Every
/// spec-table entry names its keywords out of this group, and the binder module's reserved-symbol
/// list and its `FN` / `UNARY` position reads compare against the same memoized symbols, so the
/// spelling of a surface token is written in exactly one place.
pub(crate) struct SurfaceKeywords {
    pub(crate) let_: StaticName<KeywordSymbol>,
    pub(crate) type_: StaticName<KeywordSymbol>,
    pub(crate) module: StaticName<KeywordSymbol>,
    pub(crate) group: StaticName<KeywordSymbol>,
    pub(crate) fold: StaticName<KeywordSymbol>,
    pub(crate) left: StaticName<KeywordSymbol>,
    pub(crate) right: StaticName<KeywordSymbol>,
    pub(crate) pairwise: StaticName<KeywordSymbol>,
    pub(crate) equals: StaticName<KeywordSymbol>,
    pub(crate) sig: StaticName<KeywordSymbol>,
    pub(crate) union: StaticName<KeywordSymbol>,
    pub(crate) newtype: StaticName<KeywordSymbol>,
    pub(crate) fn_: StaticName<KeywordSymbol>,
    pub(crate) arrow: StaticName<KeywordSymbol>,
    pub(crate) op: StaticName<KeywordSymbol>,
    pub(crate) over: StaticName<KeywordSymbol>,
    pub(crate) unary: StaticName<KeywordSymbol>,
    pub(crate) val: StaticName<KeywordSymbol>,
    pub(crate) match_: StaticName<KeywordSymbol>,
    pub(crate) with: StaticName<KeywordSymbol>,
    pub(crate) try_: StaticName<KeywordSymbol>,
    pub(crate) catch: StaticName<KeywordSymbol>,
    pub(crate) using: StaticName<KeywordSymbol>,
    pub(crate) scope: StaticName<KeywordSymbol>,
    pub(crate) close: StaticName<KeywordSymbol>,
    pub(crate) from: StaticName<KeywordSymbol>,
    /// The pattern-guard sigil `:|`.
    pub(crate) guard: StaticName<KeywordSymbol>,
    /// The otherwise-guard sigil `:!`.
    pub(crate) otherwise: StaticName<KeywordSymbol>,
}

pub(crate) static KEYWORDS: SurfaceKeywords = SurfaceKeywords {
    let_: crate::static_name!(KeywordSymbol, "LET"),
    type_: crate::static_name!(KeywordSymbol, "TYPE"),
    module: crate::static_name!(KeywordSymbol, "MODULE"),
    group: crate::static_name!(KeywordSymbol, "GROUP"),
    fold: crate::static_name!(KeywordSymbol, "FOLD"),
    left: crate::static_name!(KeywordSymbol, "LEFT"),
    right: crate::static_name!(KeywordSymbol, "RIGHT"),
    pairwise: crate::static_name!(KeywordSymbol, "PAIRWISE"),
    equals: crate::static_name!(KeywordSymbol, "="),
    sig: crate::static_name!(KeywordSymbol, "SIG"),
    union: crate::static_name!(KeywordSymbol, "UNION"),
    newtype: crate::static_name!(KeywordSymbol, "NEWTYPE"),
    fn_: crate::static_name!(KeywordSymbol, "FN"),
    arrow: crate::static_name!(KeywordSymbol, "->"),
    op: crate::static_name!(KeywordSymbol, "OP"),
    over: crate::static_name!(KeywordSymbol, "OVER"),
    unary: crate::static_name!(KeywordSymbol, "UNARY"),
    val: crate::static_name!(KeywordSymbol, "VAL"),
    match_: crate::static_name!(KeywordSymbol, "MATCH"),
    with: crate::static_name!(KeywordSymbol, "WITH"),
    try_: crate::static_name!(KeywordSymbol, "TRY"),
    catch: crate::static_name!(KeywordSymbol, "CATCH"),
    using: crate::static_name!(KeywordSymbol, "USING"),
    scope: crate::static_name!(KeywordSymbol, "SCOPE"),
    close: crate::static_name!(KeywordSymbol, "CLOSE"),
    from: crate::static_name!(KeywordSymbol, "FROM"),
    guard: crate::static_name!(KeywordSymbol, ":|"),
    otherwise: crate::static_name!(KeywordSymbol, ":!"),
};

/// One element of a static bucket key: a fixed keyword token or a slot. The spec tables are
/// `static`, so a keyword rests as one of the [`KEYWORDS`] names and matching compares its memoized
/// symbol against the symbol a part carries — a spec probe is a table walk over short runs that
/// hashes nothing past each name's first touch.
pub enum KeyElementSpec {
    Keyword(&'static StaticName<KeywordSymbol>),
    Slot,
}

impl KeyElementSpec {
    /// True iff `part` fills this position: a spec keyword against the part's own symbol, a
    /// spec slot against any non-keyword part — the same classification
    /// [`stored_untyped_key`](crate::machine::model::ast::stored_untyped_key) reads.
    fn matches_part<'a, P: Part<'a>>(&self, part: &P) -> bool {
        match (self, part.class()) {
            (KeyElementSpec::Keyword(name), PartClass::Keyword(symbol)) => name.symbol() == symbol,
            (KeyElementSpec::Keyword(_), _) | (_, PartClass::Keyword(_)) => false,
            (KeyElementSpec::Slot, _) => true,
        }
    }

    /// Owned-key peer of [`Self::matches_part`], for the consistency tests that pin the spec tables
    /// against the live registration keys.
    #[cfg(test)]
    fn matches(&self, element: &KeyElement) -> bool {
        match (self, element) {
            (KeyElementSpec::Keyword(name), KeyElement::Keyword(symbol)) => {
                name.symbol() == *symbol
            }
            (KeyElementSpec::Slot, KeyElement::Slot) => true,
            _ => false,
        }
    }
}

/// True iff `key` matches `parts` element-for-element, materializing no key at all — the parts
/// already carry every token this compares.
pub fn key_matches_parts<'a, P: Part<'a>>(key: &[KeyElementSpec], parts: &[Spanned<P>]) -> bool {
    key.len() == parts.len()
        && key
            .iter()
            .zip(parts.iter())
            .all(|(spec, part)| spec.matches_part(&part.value))
}

/// [`key_matches_parts`]'s owned-key peer, for the spec⟺registration consistency tests.
#[cfg(test)]
pub fn key_matches_untyped(key: &[KeyElementSpec], live: &UntypedKey) -> bool {
    key.len() == live.len()
        && key
            .iter()
            .zip(live.iter())
            .all(|(spec, element)| spec.matches(element))
}

/// A spec key rendered for a failure message: keywords verbatim, slots as `_`.
#[cfg(test)]
pub fn render_key(key: &[KeyElementSpec]) -> Vec<String> {
    key.iter()
        .map(|element| match element {
            KeyElementSpec::Keyword(name) => name.text().to_string(),
            KeyElementSpec::Slot => "_".to_string(),
        })
        .collect()
}
