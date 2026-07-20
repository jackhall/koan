use std::hash::{Hash, Hasher};

use super::kobject::KObject;
use crate::machine::model::types::{KType, Parseable, TypeRegistry};

/// Concrete dict-key value for the `KObject::Dict` map. Restricted to the hashable scalars;
/// non-scalar keys are rejected at construction via [`Self::try_from_kobject`].
///
/// The key domain is kept NaN-free and zero-normalized (see `try_from_kobject`), so `Number`
/// bit equality coincides with IEEE equality here and the [`PartialEq`] / [`Hash`] impls agree
/// by construction — the map contract holds.
#[derive(Clone, Debug)]
pub enum KKey {
    String(String),
    Number(f64),
    Bool(bool),
}

impl KKey {
    /// Returns the rejection reason as a plain `String` so this value-type conversion stays
    /// free of the runtime `KError` type; the caller wraps it into a structured error. NaN is
    /// rejected (it would be equal-to-nothing, breaking key lookup) and `-0.0` is normalized to
    /// `0.0` so the two zeros are one key.
    pub fn try_from_kobject(obj: &KObject<'_>, types: &TypeRegistry) -> Result<KKey, String> {
        match obj {
            KObject::KString(s) => Ok(KKey::String(s.clone())),
            KObject::Number(n) if n.is_nan() => Err("dict key must not be NaN".to_string()),
            KObject::Number(n) => Ok(KKey::Number(if *n == 0.0 { 0.0 } else { *n })),
            KObject::Bool(b) => Ok(KKey::Bool(*b)),
            other => Err(format!(
                "dict key must be String, Number, or Bool; got {}",
                other.ktype().name(types)
            )),
        }
    }
}

impl PartialEq for KKey {
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

impl Eq for KKey {}

impl Hash for KKey {
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

impl Parseable for KKey {
    fn ktype(&self) -> KType {
        match self {
            KKey::String(_) => KType::STR,
            KKey::Number(_) => KType::NUMBER,
            KKey::Bool(_) => KType::BOOL,
        }
    }
}

impl KKey {
    /// String keys are quoted so `{"1": x}` and `{1: x}` render distinctly. A key is a scalar,
    /// so its rendering carries no type and needs no registry.
    pub fn summarize(&self) -> String {
        match self {
            KKey::String(s) => format!("\"{}\"", s),
            KKey::Number(n) => n.to_string(),
            KKey::Bool(b) => b.to_string(),
        }
    }
}

#[cfg(test)]
mod tests;
