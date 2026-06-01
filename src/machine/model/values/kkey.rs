use std::hash::Hasher;

use super::kobject::KObject;
use crate::machine::model::types::{KType, Parseable, Serializable};

/// Concrete dict-key implementor for the `Box<dyn Serializable>` slot on
/// `KObject::Dict`. Restricted to Python's hashable scalars; non-scalar keys
/// are rejected at construction via `try_from_kobject`.
///
/// `Number` hashes via `f64::to_bits()`, so NaN equals only an identical NaN
/// bit pattern — matching Python's object-identity behavior for NaN keys.
#[derive(Clone, Debug)]
pub enum KKey {
    String(String),
    Number(f64),
    Bool(bool),
}

impl KKey {
    /// Returns the rejection reason as a plain `String` so this value-type
    /// conversion stays free of the runtime `KError` type; the caller wraps
    /// it into a structured error.
    pub fn try_from_kobject(obj: &KObject<'_>) -> Result<KKey, String> {
        match obj {
            KObject::KString(s) => Ok(KKey::String(s.clone())),
            KObject::Number(n) => Ok(KKey::Number(*n)),
            KObject::Bool(b) => Ok(KKey::Bool(*b)),
            other => Err(format!(
                "dict key must be String, Number, or Bool; got {}",
                other.ktype().name()
            )),
        }
    }
}

impl<'a> Parseable<'a> for KKey {
    fn equal(&self, other: &dyn Parseable<'a>) -> bool {
        self.summarize() == other.summarize()
    }

    fn ktype(&self) -> KType<'a> {
        match self {
            KKey::String(_) => KType::Str,
            KKey::Number(_) => KType::Number,
            KKey::Bool(_) => KType::Bool,
        }
    }

    /// String keys are quoted so `{"1": x}` and `{1: x}` render distinctly.
    fn summarize(&self) -> String {
        match self {
            KKey::String(s) => format!("\"{}\"", s),
            KKey::Number(n) => n.to_string(),
            KKey::Bool(b) => b.to_string(),
        }
    }
}

impl<'a> Serializable<'a> for KKey {
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

    fn clone_box(&self) -> Box<dyn Serializable<'a>> {
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
        assert_ne!(
            hash_of(&KKey::String("a".into())),
            hash_of(&KKey::String("b".into()))
        );
    }

    #[test]
    fn equal_strings_hash_equal() {
        assert_eq!(
            hash_of(&KKey::String("a".into())),
            hash_of(&KKey::String("a".into()))
        );
    }

    #[test]
    fn number_and_string_with_same_text_differ() {
        assert_ne!(
            hash_of(&KKey::Number(1.0)),
            hash_of(&KKey::String("1".into()))
        );
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
        assert!(err.contains("dict key must be String, Number, or Bool"));
    }

    #[test]
    fn summarize_quotes_strings_only() {
        assert_eq!(KKey::String("hi".into()).summarize(), "\"hi\"");
        assert_eq!(KKey::Number(3.0).summarize(), "3");
        assert_eq!(KKey::Bool(true).summarize(), "true");
    }

    #[test]
    fn ktype_reports_variant() {
        assert_eq!(KKey::String("a".into()).ktype(), KType::Str);
        assert_eq!(KKey::Number(1.0).ktype(), KType::Number);
        assert_eq!(KKey::Bool(false).ktype(), KType::Bool);
    }

    #[test]
    fn encode_decode_roundtrip_each_variant() {
        for original in [
            KKey::String("hello".into()),
            KKey::Number(3.5),
            KKey::Bool(true),
            KKey::Bool(false),
        ] {
            let bytes = original.encode();
            let decoded = KKey::decode(&bytes);
            assert_eq!(hash_of(&original), hash_of(&decoded));
            assert_eq!(original.summarize(), decoded.summarize());
        }
    }

    #[test]
    fn nan_number_keys_with_same_bits_hash_equal() {
        let nan = KKey::Number(f64::NAN);
        let same = KKey::Number(f64::from_bits(f64::NAN.to_bits()));
        assert_eq!(hash_of(&nan), hash_of(&same));
        let other_nan = KKey::Number(f64::from_bits(f64::NAN.to_bits() ^ 1));
        assert_ne!(hash_of(&nan), hash_of(&other_nan));
    }
}
