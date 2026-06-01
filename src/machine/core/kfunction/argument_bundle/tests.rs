use super::*;
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};

fn one_slot_bundle<'a>(name: &str, obj: KObject<'a>) -> ArgumentBundle<'a> {
    let mut args = Record::new();
    args.insert(name.to_string(), Rc::new(obj));
    ArgumentBundle { args }
}

fn type_name_ref<'a>(name: &str) -> KObject<'a> {
    KObject::TypeNameRef(TypeName::leaf(name.into()))
}

// ---------- shared-Rc clone paths on the extract_* helpers ----------

#[test]
fn extract_kexpression_clones_when_rc_is_shared() {
    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("k".into()))]);
    let shared = Rc::new(KObject::KExpression(expr));
    let _outside = Rc::clone(&shared);
    let mut bundle = ArgumentBundle {
        args: Record::new(),
    };
    bundle.args.insert("e".into(), shared);
    let got = extract_kexpression(&mut bundle, "e").expect("clone path should return Some");
    assert!(
        matches!(got.parts.as_slice(), [Spanned { value: ExpressionPart::Identifier(n), .. }] if n == "k")
    );
}

#[test]
fn extract_kexpression_shared_non_matching_variant_returns_none() {
    let shared = Rc::new(KObject::Number(1.0));
    let _outside = Rc::clone(&shared);
    let mut bundle = ArgumentBundle {
        args: Record::new(),
    };
    bundle.args.insert("e".into(), shared);
    assert!(extract_kexpression(&mut bundle, "e").is_none());
}

#[test]
fn extract_ktype_clones_when_rc_is_shared() {
    let shared = Rc::new(KObject::KTypeValue(KType::Number));
    let _outside = Rc::clone(&shared);
    let mut bundle = ArgumentBundle {
        args: Record::new(),
    };
    bundle.args.insert("t".into(), shared);
    assert_eq!(extract_ktype(&mut bundle, "t"), Some(KType::Number));
}

#[test]
fn extract_ktype_shared_non_matching_variant_returns_none() {
    let shared = Rc::new(KObject::Number(2.0));
    let _outside = Rc::clone(&shared);
    let mut bundle = ArgumentBundle {
        args: Record::new(),
    };
    bundle.args.insert("t".into(), shared);
    assert!(extract_ktype(&mut bundle, "t").is_none());
}

#[test]
fn extract_type_name_ref_clones_when_rc_is_shared() {
    let shared = Rc::new(type_name_ref("Foo"));
    let _outside = Rc::clone(&shared);
    let mut bundle = ArgumentBundle {
        args: Record::new(),
    };
    bundle.args.insert("t".into(), shared);
    let got = extract_type_name_ref(&mut bundle, "t").expect("clone path should return Some");
    assert_eq!(got.as_str(), "Foo");
}

#[test]
fn extract_type_name_ref_shared_non_matching_variant_returns_none() {
    let shared = Rc::new(KObject::KTypeValue(KType::Number));
    let _outside = Rc::clone(&shared);
    let mut bundle = ArgumentBundle {
        args: Record::new(),
    };
    bundle.args.insert("t".into(), shared);
    assert!(extract_type_name_ref(&mut bundle, "t").is_none());
}

// ---------- extract_bare_type_name arms ----------

/// A bare-leaf `TypeNameRef` carrier resolves to its name.
#[test]
fn extract_bare_type_name_accepts_type_name_ref_leaf() {
    let bundle = one_slot_bundle("T", type_name_ref("Foo"));
    let name = extract_bare_type_name(&bundle, "T", "STRUCT").expect("leaf should be accepted");
    assert_eq!(name, "Foo");
}

/// `KType::Number` stands in for every leaf variant — the match arm shares one
/// body across all of them.
#[test]
fn extract_bare_type_name_accepts_ktypevalue_leaf() {
    let bundle = one_slot_bundle("T", KObject::KTypeValue(KType::Number));
    let name = extract_bare_type_name(&bundle, "T", "STRUCT").expect("leaf should be accepted");
    assert_eq!(name, "Number");
}

#[test]
fn extract_bare_type_name_rejects_ktypevalue_structural() {
    let list = KType::List(Box::new(KType::Number));
    let bundle = one_slot_bundle("T", KObject::KTypeValue(list));
    let err = extract_bare_type_name(&bundle, "T", "STRUCT").expect_err("should reject");
    match err.kind {
        KErrorKind::ShapeError(msg) => {
            assert!(msg.contains("STRUCT T must be a bare type name"));
            assert!(msg.contains(":(LIST OF Number)"));
        }
        other => panic!(
            "expected ShapeError, got {:?}",
            std::mem::discriminant(&other)
        ),
    }
}

#[test]
fn extract_bare_type_name_rejects_non_type_carrier() {
    let bundle = one_slot_bundle("T", KObject::Number(1.0));
    let err = extract_bare_type_name(&bundle, "T", "STRUCT").expect_err("should reject");
    match err.kind {
        KErrorKind::TypeMismatch { arg, expected, got } => {
            assert_eq!(arg, "T");
            assert_eq!(expected, "TypeExprRef");
            assert_eq!(got, "Number");
        }
        other => panic!(
            "expected TypeMismatch, got {:?}",
            std::mem::discriminant(&other)
        ),
    }
}

#[test]
fn extract_bare_type_name_missing_slot_returns_missing_arg() {
    let bundle = ArgumentBundle {
        args: Record::new(),
    };
    let err = extract_bare_type_name(&bundle, "T", "STRUCT").expect_err("should reject");
    match err.kind {
        KErrorKind::MissingArg(name) => assert_eq!(name, "T"),
        other => panic!(
            "expected MissingArg, got {:?}",
            std::mem::discriminant(&other)
        ),
    }
}

// ---------- require_* mismatch + missing closures ----------

fn unwrap_err<T>(r: Result<T, KError>) -> KError {
    match r {
        Ok(_) => panic!("expected Err"),
        Err(e) => e,
    }
}

#[test]
fn require_kexpression_mismatch_routes_through_mismatch_helper() {
    let bundle = one_slot_bundle("e", KObject::Number(1.0));
    let err = unwrap_err(bundle.require_kexpression("e"));
    match err.kind {
        KErrorKind::TypeMismatch { arg, expected, got } => {
            assert_eq!(arg, "e");
            assert_eq!(expected, "KExpression");
            assert_eq!(got, "Number");
        }
        other => panic!(
            "expected TypeMismatch, got {:?}",
            std::mem::discriminant(&other)
        ),
    }
}

#[test]
fn require_ktype_mismatch_routes_through_mismatch_helper() {
    let bundle = one_slot_bundle("t", KObject::Number(1.0));
    let err = unwrap_err(bundle.require_ktype("t"));
    assert!(matches!(err.kind, KErrorKind::TypeMismatch { .. }));
}

#[test]
fn require_module_mismatch_routes_through_mismatch_helper() {
    let bundle = one_slot_bundle("m", KObject::Number(1.0));
    let err = unwrap_err(bundle.require_module("m"));
    assert!(matches!(err.kind, KErrorKind::TypeMismatch { .. }));
}

#[test]
fn require_signature_mismatch_routes_through_mismatch_helper() {
    let bundle = one_slot_bundle("s", KObject::Number(1.0));
    let err = unwrap_err(bundle.require_signature("s"));
    assert!(matches!(err.kind, KErrorKind::TypeMismatch { .. }));
}

#[test]
fn require_missing_slot_returns_missing_arg() {
    let bundle = ArgumentBundle {
        args: Record::new(),
    };
    let err = unwrap_err(bundle.require("x"));
    match err.kind {
        KErrorKind::MissingArg(name) => assert_eq!(name, "x"),
        other => panic!(
            "expected MissingArg, got {:?}",
            std::mem::discriminant(&other)
        ),
    }
}

// ---------- unique-Rc Ok(_) => None arms on the extract_* helpers ----------

#[test]
fn extract_kexpression_unique_non_matching_variant_returns_none() {
    let mut bundle = one_slot_bundle("e", KObject::Number(1.0));
    assert!(extract_kexpression(&mut bundle, "e").is_none());
}

#[test]
fn extract_ktype_unique_non_matching_variant_returns_none() {
    let mut bundle = one_slot_bundle("t", KObject::Number(1.0));
    assert!(extract_ktype(&mut bundle, "t").is_none());
}

#[test]
fn extract_type_name_ref_unique_non_matching_variant_returns_none() {
    let mut bundle = one_slot_bundle("t", KObject::KTypeValue(KType::Number));
    assert!(extract_type_name_ref(&mut bundle, "t").is_none());
}
