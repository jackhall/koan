use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher as _};

use super::*;

fn hash_of(k: &KKey) -> u64 {
    let mut h = DefaultHasher::new();
    k.hash(&mut h);
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
    assert_eq!(KKey::String("a".into()), KKey::String("a".into()));
}

#[test]
fn number_and_string_with_same_text_differ() {
    assert_ne!(
        hash_of(&KKey::Number(1.0)),
        hash_of(&KKey::String("1".into()))
    );
    assert_ne!(KKey::Number(1.0), KKey::String("1".into()));
}

#[test]
fn bool_and_number_zero_differ() {
    assert_ne!(hash_of(&KKey::Bool(false)), hash_of(&KKey::Number(0.0)));
    assert_ne!(KKey::Bool(false), KKey::Number(0.0));
}

#[test]
fn try_from_kobject_accepts_scalars() {
    let types = TypeRegistry::new();
    assert!(matches!(
        KKey::try_from_kobject(&KObject::KString("a".into()), &types),
        Ok(KKey::String(s)) if s == "a"
    ));
    assert!(matches!(
        KKey::try_from_kobject(&KObject::Number(3.5), &types),
        Ok(KKey::Number(n)) if n == 3.5
    ));
    assert!(matches!(
        KKey::try_from_kobject(&KObject::Bool(true), &types),
        Ok(KKey::Bool(true))
    ));
}

#[test]
fn try_from_kobject_rejects_null() {
    let types = TypeRegistry::new();
    let err = KKey::try_from_kobject(&KObject::Null, &types).unwrap_err();
    assert!(err.contains("dict key must be String, Number, or Bool"));
}

#[test]
fn try_from_kobject_rejects_nan() {
    let types = TypeRegistry::new();
    let err = KKey::try_from_kobject(&KObject::Number(f64::NAN), &types).unwrap_err();
    assert!(err.contains("NaN"));
}

#[test]
fn negative_zero_normalizes_and_matches_positive_zero() {
    let types = TypeRegistry::new();
    let neg = KKey::try_from_kobject(&KObject::Number(-0.0), &types).unwrap();
    let pos = KKey::try_from_kobject(&KObject::Number(0.0), &types).unwrap();
    // Normalization erases the sign bit, so the two zeros are one key by equality and hash.
    assert_eq!(neg, pos);
    assert_eq!(hash_of(&neg), hash_of(&pos));
}

#[test]
fn summarize_quotes_strings_only() {
    assert_eq!(KKey::String("hi".into()).summarize(), "\"hi\"");
    assert_eq!(KKey::Number(3.0).summarize(), "3");
    assert_eq!(KKey::Bool(true).summarize(), "true");
}

#[test]
fn ktype_reports_variant() {
    assert_eq!(KKey::String("a".into()).ktype(), KType::STR);
    assert_eq!(KKey::Number(1.0).ktype(), KType::NUMBER);
    assert_eq!(KKey::Bool(false).ktype(), KType::BOOL);
}
