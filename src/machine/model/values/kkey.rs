use std::hash::{Hash, Hasher};

use super::kobject::KObject;
use crate::machine::core::SubstrateDoor;
use crate::machine::model::RunRegistries;
use crate::machine::model::types::{KType, Parseable};

/// Concrete dict-key value for the `KObject::Dict` map. Restricted to the hashable scalars;
/// non-scalar keys are rejected at construction via [`Self::try_from_kobject`].
///
/// The key domain is kept NaN-free and zero-normalized (see `try_from_kobject`), so `Number`
/// bit equality coincides with IEEE equality here and the [`PartialEq`] / [`Hash`] impls agree
/// by construction — the map contract holds.
///
/// A `String` key carries a region-hosted `&'a str`, the same representation
/// [`KObject::KString`] takes, so the whole enum is `Copy` and `Drop`-free — a dict's frozen
/// key→index table costs region teardown nothing per key. Equality and hashing stay `str`
/// compares (no interning table, ruling 1), so a key produced in one region matches a key
/// produced in another by content.
#[derive(Clone, Copy, Debug)]
pub enum KKey<'a> {
    String(&'a str),
    Number(f64),
    Bool(bool),
}

impl<'a> KKey<'a> {
    /// Returns the rejection reason as a plain `String` so this value-type conversion stays
    /// free of the runtime `KError` type; the caller wraps it into a structured error. NaN is
    /// rejected (it would be equal-to-nothing, breaking key lookup) and `-0.0` is normalized to
    /// `0.0` so the two zeros are one key.
    ///
    /// A string key rides `obj`'s own bytes verbatim — the key is minted where the object already
    /// lives, and the dict door re-bumps every string key into the dict's own region as it freezes
    /// the table, so residence is settled there rather than here.
    pub fn try_from_kobject(
        obj: &'a KObject<'a>,
        registries: &RunRegistries,
    ) -> Result<KKey<'a>, String> {
        match obj {
            KObject::KString(s) => Ok(KKey::String(s)),
            KObject::Number(n) if n.is_nan() => Err("dict key must not be NaN".to_string()),
            KObject::Number(n) => Ok(KKey::Number(if *n == 0.0 { 0.0 } else { *n })),
            KObject::Bool(b) => Ok(KKey::Bool(*b)),
            other => Err(format!(
                "dict key must be String, Number, or Bool; got {}",
                other.ktype().display_name(registries)
            )),
        }
    }

    /// Re-home this key's bytes into `door`'s region, so a key frozen into a dict's table is
    /// resident in the dict's own region rather than borrowing wherever it was produced. The
    /// scalar arms are owned data and ride verbatim. Called once per key by the dict door.
    pub(crate) fn rehomed<'d>(self, door: SubstrateDoor<'d, '_>) -> KKey<'d> {
        match self {
            KKey::String(s) => KKey::String(door.allocator().text(s)),
            KKey::Number(n) => KKey::Number(n),
            KKey::Bool(b) => KKey::Bool(b),
        }
    }
}

impl PartialEq for KKey<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (KKey::String(a), KKey::String(b)) => a == b,
            (KKey::Bool(a), KKey::Bool(b)) => a == b,
            // Bit equality over the NaN-free, zero-normalized domain — the same bits `Hash`
            // reads, and equal to IEEE `==` on this domain.
            (KKey::Number(a), KKey::Number(b)) => a.to_bits() == b.to_bits(),
            _ => false,
        }
    }
}

impl Eq for KKey<'_> {}

impl Hash for KKey<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            KKey::String(s) => {
                state.write_u8(0);
                s.hash(state);
            }
            KKey::Number(n) => {
                state.write_u8(1);
                state.write_u64(n.to_bits());
            }
            KKey::Bool(b) => {
                state.write_u8(2);
                b.hash(state);
            }
        }
    }
}

impl Parseable for KKey<'_> {
    fn ktype(&self) -> KType {
        match self {
            KKey::String(_) => KType::STR,
            KKey::Number(_) => KType::NUMBER,
            KKey::Bool(_) => KType::BOOL,
        }
    }
}

/// String keys are quoted so `{"1": x}` and `{1: x}` render distinctly. A key is a scalar, so its
/// rendering carries no type and needs no registry — which makes plain `Display` the whole view.
impl std::fmt::Display for KKey<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KKey::String(s) => write!(f, "\"{s}\""),
            KKey::Number(n) => write!(f, "{n}"),
            KKey::Bool(b) => write!(f, "{b}"),
        }
    }
}

impl KKey<'_> {
    /// The rendered key as an owned `String` — the `Display` view for a caller that keeps the text.
    pub fn summarize(&self) -> String {
        self.to_string()
    }
}

#[cfg(test)]
mod tests;
