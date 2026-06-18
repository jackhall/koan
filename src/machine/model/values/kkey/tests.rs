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
