use std::hash::Hasher;

use crate::machine::core::{KError, KErrorKind};
use crate::machine::model::types::{KType, Parseable, Serializable};
use super::kobject::KObject;

/// Concrete dict-key type. The `KObject::Dict` runtime variant stores keys as
/// `Box<dyn Serializable>`; this enum is the implementor that fills that slot. Restricted to
/// Python's hashable scalars (string, number, bool) — non-scalar keys are rejected at dict
/// construction time via `try_from_kobject`.
///
/// `Number` keys hash and compare via `f64::to_bits()`. NaN therefore equals only an identical
/// NaN bit pattern and otherwise becomes effectively unreachable as a key, matching Python's
/// behavior for object identity on NaN.
#[derive(Clone, Debug)]
pub enum KKey {
    String(String),
    Number(f64),
    Bool(bool),
}

impl KKey {
    /// Try to convert a runtime `KObject` value into a dict key. The dict aggregator calls
    /// this when materializing a dict literal whose key positions were sub-expressions; a
    /// non-scalar result (e.g. a List) becomes a structured `ShapeError` instead of silently
    /// stringifying.
    pub fn try_from_kobject(obj: &KObject<'_>) -> Result<KKey, KError> {
        match obj {
            KObject::KString(s) => Ok(KKey::String(s.clone())),
            KObject::Number(n) => Ok(KKey::Number(*n)),
            KObject::Bool(b) => Ok(KKey::Bool(*b)),
            other => Err(KError::new(KErrorKind::ShapeError(format!(
                "dict key must be String, Number, or Bool; got {}",
                other.ktype().name()
            )))),
        }
    }
}

impl Parseable for KKey {
    fn equal(&self, other: &dyn Parseable) -> bool {
        self.summarize() == other.summarize()
    }

    fn ktype(&self) -> KType {
        match self {
            KKey::String(_) => KType::Str,
            KKey::Number(_) => KType::Number,
            KKey::Bool(_) => KType::Bool,
        }
    }

    /// String keys are quoted in the rendering so a `{"1": x}` and a `{1: x}` look distinct
    /// when a dict is summarized. Numbers and bools render unquoted.
    fn summarize(&self) -> String {
        match self {
            KKey::String(s) => format!("\"{}\"", s),
            KKey::Number(n) => n.to_string(),
            KKey::Bool(b) => b.to_string(),
        }
    }
}

impl Serializable for KKey {
    fn hash(&self, state: &mut dyn Hasher) {
        match self {
            KKey::String(s) => {
                state.write_u8(0);
                state.write(s.as_bytes());
            }
            KKey::Number(n) => {
                state.write_u8(1);
                state.write(&n.to_bits().to_ne_bytes());
            }
            KKey::Bool(b) => {
                state.write_u8(2);
                state.write_u8(*b as u8);
            }
        }
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            KKey::String(s) => {
                out.push(0);
                out.extend_from_slice(s.as_bytes());
            }
            KKey::Number(n) => {
                out.push(1);
                out.extend_from_slice(&n.to_bits().to_ne_bytes());
            }
            KKey::Bool(b) => {
                out.push(2);
                out.push(*b as u8);
            }
        }
        out
    }

    fn decode(bytes: &[u8]) -> Self
    where
        Self: Sized,
    {
        match bytes.first() {
            Some(&0) => KKey::String(String::from_utf8_lossy(&bytes[1..]).into_owned()),
            Some(&1) => {
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&bytes[1..9]);
                KKey::Number(f64::from_bits(u64::from_ne_bytes(buf)))
            }
            Some(&2) => KKey::Bool(bytes.get(1).copied().unwrap_or(0) != 0),
            _ => panic!("KKey::decode = unrecognized tag byte"),
        }
    }

    fn clone_box(&self) -> Box<dyn Serializable> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher as _;

    use super::*;

    fn hash_of(k: &KKey) -> u64 {
        let mut h = DefaultHasher::new();
        Serializable::hash(k, &mut h);
        h.finish()
    }

    #[test]
    fn distinct_strings_hash_differently() {
        assert_ne!(hash_of(&KKey::String("a".into())), hash_of(&KKey::String("b".into())));
    }

    #[test]
    fn equal_strings_hash_equal() {
        assert_eq!(hash_of(&KKey::String("a".into())), hash_of(&KKey::String("a".into())));
    }

    #[test]
    fn number_and_string_with_same_text_differ() {
        // {1: x} vs {"1": x} must be distinguishable.
        assert_ne!(hash_of(&KKey::Number(1.0)), hash_of(&KKey::String("1".into())));
    }

    #[test]
    fn bool_and_number_zero_differ() {
        assert_ne!(hash_of(&KKey::Bool(false)), hash_of(&KKey::Number(0.0)));
    }

    #[test]
    fn try_from_kobject_accepts_scalars() {
        assert!(matches!(
            KKey::try_from_kobject(&KObject::KString("a".into())),
            Ok(KKey::String(s)) if s == "a"
        ));
        assert!(matches!(
            KKey::try_from_kobject(&KObject::Number(3.5)),
            Ok(KKey::Number(n)) if n == 3.5
        ));
        assert!(matches!(
            KKey::try_from_kobject(&KObject::Bool(true)),
            Ok(KKey::Bool(true))
        ));
    }

    #[test]
    fn try_from_kobject_rejects_null() {
        let err = KKey::try_from_kobject(&KObject::Null).unwrap_err();
        assert!(matches!(err.kind, KErrorKind::ShapeError(_)));
    }

    #[test]
    fn summarize_quotes_strings_only() {
        assert_eq!(KKey::String("hi".into()).summarize(), "\"hi\"");
        assert_eq!(KKey::Number(3.0).summarize(), "3");
        assert_eq!(KKey::Bool(true).summarize(), "true");
    }
}
