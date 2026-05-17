use super::*;
use crate::machine::model::ast::{ExpressionPart, KExpression};

fn one_slot_bundle<'a>(name: &str, obj: KObject<'a>) -> ArgumentBundle<'a> {
    let mut args = HashMap::new();
    args.insert(name.to_string(), Rc::new(obj));
    ArgumentBundle { args }
}

fn type_name_ref<'a>(name: &str, params: TypeParams) -> KObject<'a> {
    KObject::TypeNameRef(TypeExpr {
        name: name.into(),
        params,
        builtin_cache: std::cell::OnceCell::new(),
    })
}

// ---------- shared-Rc clone paths on the extract_* helpers ----------

/// `extract_kexpression`'s `Err(rc) => KExpression` arm: when the bundle's slot is
/// shared with an outside holder, `Rc::try_unwrap` fails and the helper falls back
/// to cloning the inner `KExpression`.
#[test]
fn extract_kexpression_clones_when_rc_is_shared() {
    let expr = KExpression {
        parts: vec![ExpressionPart::Identifier("k".into())],
    };
    let shared = Rc::new(KObject::KExpression(expr));
    let _outside = Rc::clone(&shared);
    let mut bundle = ArgumentBundle { args: HashMap::new() };
    bundle.args.insert("e".into(), shared);
    let got = extract_kexpression(&mut bundle, "e").expect("clone path should return Some");
    assert!(matches!(got.parts.as_slice(), [ExpressionPart::Identifier(n)] if n == "k"));
}

/// `extract_kexpression`'s `Err(rc) => _` arm: shared `Rc` holding a non-`KExpression`
/// variant yields `None`.
#[test]
fn extract_kexpression_shared_non_matching_variant_returns_none() {
    let shared = Rc::new(KObject::Number(1.0));
    let _outside = Rc::clone(&shared);
    let mut bundle = ArgumentBundle { args: HashMap::new() };
    bundle.args.insert("e".into(), shared);
    assert!(extract_kexpression(&mut bundle, "e").is_none());
}

/// `extract_ktype`'s `Err(rc) => KTypeValue` arm: shared `Rc` clones the inner
/// `KType`.
#[test]
fn extract_ktype_clones_when_rc_is_shared() {
    let shared = Rc::new(KObject::KTypeValue(KType::Number));
    let _outside = Rc::clone(&shared);
    let mut bundle = ArgumentBundle { args: HashMap::new() };
    bundle.args.insert("t".into(), shared);
    assert_eq!(extract_ktype(&mut bundle, "t"), Some(KType::Number));
}

/// `extract_ktype`'s `Err(rc) => _` arm.
#[test]
fn extract_ktype_shared_non_matching_variant_returns_none() {
    let shared = Rc::new(KObject::Number(2.0));
    let _outside = Rc::clone(&shared);
    let mut bundle = ArgumentBundle { args: HashMap::new() };
    bundle.args.insert("t".into(), shared);
    assert!(extract_ktype(&mut bundle, "t").is_none());
}

/// `extract_type_name_ref`'s `Err(rc) => TypeNameRef` arm clones the carried
/// `TypeExpr` when the slot's `Rc` is shared.
#[test]
fn extract_type_name_ref_clones_when_rc_is_shared() {
    let shared = Rc::new(type_name_ref("Foo", TypeParams::None));
    let _outside = Rc::clone(&shared);
    let mut bundle = ArgumentBundle { args: HashMap::new() };
    bundle.args.insert("t".into(), shared);
    let got = extract_type_name_ref(&mut bundle, "t").expect("clone path should return Some");
    assert_eq!(got.name, "Foo");
}

/// `extract_type_name_ref`'s `Err(rc) => _` arm.
#[test]
fn extract_type_name_ref_shared_non_matching_variant_returns_none() {
    let shared = Rc::new(KObject::KTypeValue(KType::Number));
    let _outside = Rc::clone(&shared);
    let mut bundle = ArgumentBundle { args: HashMap::new() };
    bundle.args.insert("t".into(), shared);
    assert!(extract_type_name_ref(&mut bundle, "t").is_none());
}

// ---------- extract_bare_type_name arms ----------

/// `TypeNameRef` carrier with `TypeParams::List(_)` → `ShapeError` with the
/// rendered surface form.
#[test]
fn extract_bare_type_name_rejects_parameterized_type_name_ref_list() {
    let bundle = one_slot_bundle(
        "T",
        type_name_ref("Foo", TypeParams::List(vec![TypeExpr::leaf("Bar".into())])),
    );
    let err = extract_bare_type_name(&bundle, "T", "STRUCT").expect_err("should reject");
    match err.kind {
        KErrorKind::ShapeError(msg) => {
            assert!(msg.contains("STRUCT T must be a bare type name"));
            assert!(msg.contains(":(Foo Bar)"));
        }
        other => panic!("expected ShapeError, got {:?}", std::mem::discriminant(&other)),
    }
}

/// `TypeNameRef` carrier with `TypeParams::Function { .. }` → `ShapeError`.
#[test]
fn extract_bare_type_name_rejects_parameterized_type_name_ref_function() {
    let bundle = one_slot_bundle(
        "T",
        type_name_ref(
            "Foo",
            TypeParams::Function {
                args: vec![TypeExpr::leaf("A".into())],
                ret: Box::new(TypeExpr::leaf("R".into())),
            },
        ),
    );
    let err = extract_bare_type_name(&bundle, "T", "UNION").expect_err("should reject");
    match err.kind {
        KErrorKind::ShapeError(msg) => {
            assert!(msg.contains("UNION T must be a bare type name"));
            assert!(msg.contains("Foo"));
        }
        other => panic!("expected ShapeError, got {:?}", std::mem::discriminant(&other)),
    }
}

/// `KTypeValue` leaf-variant arm: surface name is the `KType::name()` rendering.
/// Picks `KType::Number` as a representative leaf — the arm shares one body across
/// every leaf variant in the match.
#[test]
fn extract_bare_type_name_accepts_ktypevalue_leaf() {
    let bundle = one_slot_bundle("T", KObject::KTypeValue(KType::Number));
    let name = extract_bare_type_name(&bundle, "T", "STRUCT").expect("leaf should be accepted");
    assert_eq!(name, "Number");
}

/// `KTypeValue` structural arm: `List<Number>` is parameterized and rejected with
/// the rendered `:(List Number)` surface form embedded in the message.
#[test]
fn extract_bare_type_name_rejects_ktypevalue_structural() {
    let list = KType::List(Box::new(KType::Number));
    let bundle = one_slot_bundle("T", KObject::KTypeValue(list));
    let err = extract_bare_type_name(&bundle, "T", "STRUCT").expect_err("should reject");
    match err.kind {
        KErrorKind::ShapeError(msg) => {
            assert!(msg.contains("STRUCT T must be a bare type name"));
            assert!(msg.contains(":(List Number)"));
        }
        other => panic!("expected ShapeError, got {:?}", std::mem::discriminant(&other)),
    }
}

/// `Some(other)` arm: a slot holding a value-typed `KObject` (not a `TypeNameRef`
/// or `KTypeValue` carrier) returns `TypeMismatch { expected: "TypeExprRef" }`.
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
        other => panic!("expected TypeMismatch, got {:?}", std::mem::discriminant(&other)),
    }
}

/// `None` arm: missing slot returns `MissingArg`.
#[test]
fn extract_bare_type_name_missing_slot_returns_missing_arg() {
    let bundle = ArgumentBundle { args: HashMap::new() };
    let err = extract_bare_type_name(&bundle, "T", "STRUCT").expect_err("should reject");
    match err.kind {
        KErrorKind::MissingArg(name) => assert_eq!(name, "T"),
        other => panic!("expected MissingArg, got {:?}", std::mem::discriminant(&other)),
    }
}

// ---------- require_* mismatch + missing closures ----------

fn unwrap_err<T>(r: Result<T, KError>) -> KError {
    match r {
        Ok(_) => panic!("expected Err"),
        Err(e) => e,
    }
}

/// `require_kexpression` mismatch arm: a non-`KExpression` slot routes through the
/// shared `mismatch` helper.
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
        other => panic!("expected TypeMismatch, got {:?}", std::mem::discriminant(&other)),
    }
}

/// `require_ktype` mismatch arm.
#[test]
fn require_ktype_mismatch_routes_through_mismatch_helper() {
    let bundle = one_slot_bundle("t", KObject::Number(1.0));
    let err = unwrap_err(bundle.require_ktype("t"));
    assert!(matches!(err.kind, KErrorKind::TypeMismatch { .. }));
}

/// `require_module` mismatch arm.
#[test]
fn require_module_mismatch_routes_through_mismatch_helper() {
    let bundle = one_slot_bundle("m", KObject::Number(1.0));
    let err = unwrap_err(bundle.require_module("m"));
    assert!(matches!(err.kind, KErrorKind::TypeMismatch { .. }));
}

/// `require_signature` mismatch arm.
#[test]
fn require_signature_mismatch_routes_through_mismatch_helper() {
    let bundle = one_slot_bundle("s", KObject::Number(1.0));
    let err = unwrap_err(bundle.require_signature("s"));
    assert!(matches!(err.kind, KErrorKind::TypeMismatch { .. }));
}

/// `require` (the no-narrow variant) routes a missing slot through `get_or_missing`'s
/// `MissingArg` closure — exercises the second arm of `ok_or_else`.
#[test]
fn require_missing_slot_returns_missing_arg() {
    let bundle = ArgumentBundle { args: HashMap::new() };
    let err = unwrap_err(bundle.require("x"));
    match err.kind {
        KErrorKind::MissingArg(name) => assert_eq!(name, "x"),
        other => panic!("expected MissingArg, got {:?}", std::mem::discriminant(&other)),
    }
}

// ---------- unique-Rc Ok(_) => None arms on the extract_* helpers ----------

/// `extract_kexpression`'s `Ok(_) => None` arm: the bundle owns the only `Rc`
/// (`try_unwrap` succeeds) but the inner variant isn't `KExpression`, so the helper
/// returns `None`. Distinct from the shared-`Rc` mismatch arm covered above.
#[test]
fn extract_kexpression_unique_non_matching_variant_returns_none() {
    let mut bundle = one_slot_bundle("e", KObject::Number(1.0));
    assert!(extract_kexpression(&mut bundle, "e").is_none());
}

/// `extract_ktype`'s `Ok(_) => None` arm.
#[test]
fn extract_ktype_unique_non_matching_variant_returns_none() {
    let mut bundle = one_slot_bundle("t", KObject::Number(1.0));
    assert!(extract_ktype(&mut bundle, "t").is_none());
}

/// `extract_type_name_ref`'s `Ok(_) => None` arm.
#[test]
fn extract_type_name_ref_unique_non_matching_variant_returns_none() {
    let mut bundle = one_slot_bundle("t", KObject::KTypeValue(KType::Number));
    assert!(extract_type_name_ref(&mut bundle, "t").is_none());
}
