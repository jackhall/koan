use super::*;
use crate::source::Spanned;

fn one_slot<'a>(kt: KType<'a>) -> ExpressionSignature<'a> {
    ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![SignatureElement::Argument(Argument {
            name: "v".into(),
            ktype: kt,
        })],
    }
}

fn expr_with_keyword<'a>(kw: &str) -> KExpression<'a> {
    KExpression::new(vec![Spanned::bare(ExpressionPart::Keyword(kw.into()))])
}

#[test]
fn most_specific_picks_number_over_any() {
    let any = one_slot(KType::Any);
    let num = one_slot(KType::Number);
    let cands: Vec<&ExpressionSignature<'_>> = vec![&any, &num];
    assert_eq!(ExpressionSignature::most_specific(&cands), Some(1));
}

#[test]
fn most_specific_returns_none_for_empty() {
    let cands: Vec<&ExpressionSignature<'_>> = Vec::new();
    assert_eq!(ExpressionSignature::most_specific(&cands), None);
}

#[test]
fn most_specific_returns_none_when_tied() {
    // Ambiguity must surface, not a winner.
    let a = one_slot(KType::Number);
    let b = one_slot(KType::Number);
    let cands: Vec<&ExpressionSignature<'_>> = vec![&a, &b];
    assert_eq!(ExpressionSignature::most_specific(&cands), None);
}

#[test]
fn return_type_clone_round_trips_all_arms() {
    let r = ReturnType::Resolved(KType::Number);
    assert_eq!(r, r.clone());
    let d = ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("er".into())));
    assert_eq!(d, d.clone());
    let e = ReturnType::Deferred(DeferredReturn::Expression(expr_with_keyword("FOO")));
    assert_eq!(e, e.clone());
}

#[test]
fn return_type_eq_deferred_match_and_variant_mismatch() {
    let r = ReturnType::Resolved(KType::Number);
    let d = ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("er".into())));
    assert_ne!(r, d);
    let d2 = ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("er".into())));
    assert_eq!(d, d2);
    let d3 = ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("Other".into())));
    assert_ne!(d, d3);
}

#[test]
fn deferred_return_eq_matches_per_carrier() {
    let t1 = DeferredReturn::Type(TypeIdentifier::leaf("er".into()));
    let t2 = DeferredReturn::Type(TypeIdentifier::leaf("er".into()));
    let t3 = DeferredReturn::Type(TypeIdentifier::leaf("Other".into()));
    assert_eq!(t1, t2);
    assert_ne!(t1, t3);

    let e1 = DeferredReturn::Expression(expr_with_keyword("FOO"));
    let e2 = DeferredReturn::Expression(expr_with_keyword("FOO"));
    let e3 = DeferredReturn::Expression(expr_with_keyword("BAR"));
    assert_eq!(e1, e2);
    assert_ne!(e1, e3);

    assert_ne!(t1, e1);
}

#[test]
fn type_name_eq_compares_leaf_names() {
    let leaf_a = TypeIdentifier::leaf("A".into());
    let leaf_a2 = TypeIdentifier::leaf("A".into());
    let leaf_b = TypeIdentifier::leaf("B".into());
    assert_eq!(leaf_a, leaf_a2);
    assert_ne!(leaf_a, leaf_b);
}

#[test]
fn expression_signature_matches_rejects_length_and_keyword_part_mismatches() {
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![SignatureElement::Keyword("FOO".into())],
    };
    let empty: KExpression<'_> = KExpression::new(vec![]);
    assert!(!sig.matches(&empty));

    let mismatched = KExpression::new(vec![Spanned::bare(ExpressionPart::Literal(
        crate::machine::model::ast::KLiteral::Number(1.0),
    ))]);
    assert!(!sig.matches(&mismatched));

    let matching = KExpression::new(vec![Spanned::bare(ExpressionPart::Keyword("FOO".into()))]);
    assert!(sig.matches(&matching));
}

#[test]
fn return_type_debug_renders_both_arms() {
    let r = ReturnType::Resolved(KType::Number);
    assert!(format!("{:?}", r).contains("Resolved"));
    let d = ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("er".into())));
    assert!(format!("{:?}", d).contains("Deferred"));
}

#[test]
fn deferred_return_debug_renders_both_arms() {
    let t = DeferredReturn::Type(TypeIdentifier::leaf("er".into()));
    assert!(format!("{:?}", t).contains("Type"));
    let e = DeferredReturn::Expression(expr_with_keyword("FOO"));
    assert!(format!("{:?}", e).contains("Expression"));
}

#[test]
fn return_type_name_covers_all_arms() {
    let r = ReturnType::Resolved(KType::Number);
    assert_eq!(r.name(), KType::Number.name());
    let t = ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("er".into())));
    assert_eq!(t.name(), "er");
    let e = ReturnType::Deferred(DeferredReturn::Expression(expr_with_keyword("FOO")));
    assert_eq!(e.name(), "FOO");
}

/// `exact_equal` (the duplicate-overload gate) keeps reading `ReturnType`'s structure-aware
/// `PartialEq`, so synthesizing a precision-aware `KType::DeferredReturn` elsewhere does not
/// alter routing: two same-shape signatures differing only in their deferred return are
/// unequal, and identical deferred returns are equal.
#[test]
fn exact_equal_unaffected_by_deferred_return_synthesis() {
    fn sig_with<'a>(ret: ReturnType<'a>) -> ExpressionSignature<'a> {
        ExpressionSignature {
            return_type: ret,
            elements: vec![SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::Number,
            })],
        }
    }
    let er = sig_with(ReturnType::Deferred(DeferredReturn::Type(
        TypeIdentifier::leaf("er".into()),
    )));
    let er2 = sig_with(ReturnType::Deferred(DeferredReturn::Type(
        TypeIdentifier::leaf("er".into()),
    )));
    let ar = sig_with(ReturnType::Deferred(DeferredReturn::Type(
        TypeIdentifier::leaf("Ar".into()),
    )));
    assert!(er.exact_equal(&er2));
    assert!(!er.exact_equal(&ar));
}

#[test]
fn return_type_matches_value_deferred_always_true_resolved_delegates() {
    use crate::machine::model::values::KObject;
    let obj = KObject::Number(42.0);
    // Deferred always matches — per-call check runs elsewhere.
    let d = ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("er".into())));
    assert!(d.matches_value(&obj));
    assert!(!d.is_resolved());
    let r_num = ReturnType::Resolved(KType::Number);
    assert!(r_num.matches_value(&obj));
    assert!(r_num.is_resolved());
    let r_bool = ReturnType::Resolved(KType::Bool);
    assert!(!r_bool.matches_value(&obj));
}
