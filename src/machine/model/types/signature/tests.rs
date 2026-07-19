use super::*;
use crate::source::Spanned;

fn one_slot<'a>(kt: KType) -> ExpressionSignature<'a> {
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
    let types = TypeRegistry::new();
    let any = one_slot(KType::Any);
    let num = one_slot(KType::Number);
    let cands: Vec<&ExpressionSignature<'_>> = vec![&any, &num];
    assert_eq!(ExpressionSignature::most_specific(&cands, &types), Some(1));
}

#[test]
fn most_specific_returns_none_for_empty() {
    let types = TypeRegistry::new();
    let cands: Vec<&ExpressionSignature<'_>> = Vec::new();
    assert_eq!(ExpressionSignature::most_specific(&cands, &types), None);
}

#[test]
fn most_specific_returns_none_when_tied() {
    let types = TypeRegistry::new();
    // Ambiguity must surface, not a winner.
    let a = one_slot(KType::Number);
    let b = one_slot(KType::Number);
    let cands: Vec<&ExpressionSignature<'_>> = vec![&a, &b];
    assert_eq!(ExpressionSignature::most_specific(&cands, &types), None);
}

#[test]
fn return_type_clone_round_trips_all_arms() {
    let r = ReturnType::Resolved(KType::Number);
    assert_eq!(r.name(), r.clone().name());
    let d = ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("er".into())));
    assert_eq!(d.name(), d.clone().name());
    let e = ReturnType::Deferred(DeferredReturn::Expression(expr_with_keyword("FOO")));
    assert_eq!(e.name(), e.clone().name());
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
    let types = TypeRegistry::new();
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![SignatureElement::Keyword("FOO".into())],
    };
    let empty: KExpression<'_> = KExpression::new(vec![]);
    assert!(!sig.matches(&empty, &types));

    let mismatched = KExpression::new(vec![Spanned::bare(ExpressionPart::Literal(
        crate::machine::model::ast::KLiteral::Number(1.0),
    ))]);
    assert!(!sig.matches(&mismatched, &types));

    let matching = KExpression::new(vec![Spanned::bare(ExpressionPart::Keyword("FOO".into()))]);
    assert!(sig.matches(&matching, &types));
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

fn sig_with<'a>(ret: ReturnType<'a>, slot: KType) -> ExpressionSignature<'a> {
    ExpressionSignature {
        return_type: ret,
        elements: vec![SignatureElement::Argument(Argument {
            name: "v".into(),
            ktype: slot,
        })],
    }
}

/// Return types never distinguish overloads: dispatch selects on argument slots alone, so
/// two same-shape signatures differing only in their return — deferred or resolved — are
/// indistinguishable and collide at definition.
#[test]
fn indistinguishable_ignores_return_type() {
    let er = sig_with(
        ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("er".into()))),
        KType::Number,
    );
    let ar = sig_with(
        ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("Ar".into()))),
        KType::Number,
    );
    assert!(er.indistinguishable_from(&ar));

    let num = sig_with(ReturnType::Resolved(KType::Number), KType::Number);
    let text = sig_with(ReturnType::Resolved(KType::Str), KType::Number);
    assert!(num.indistinguishable_from(&text));
}

#[test]
fn indistinguishable_splits_on_argument_type_and_keywords() {
    let num = sig_with(ReturnType::Resolved(KType::Any), KType::Number);
    let text = sig_with(ReturnType::Resolved(KType::Any), KType::Str);
    assert!(!num.indistinguishable_from(&text));

    let kw = |token: &str| ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![SignatureElement::Keyword(token.into())],
    };
    assert!(kw("FOO").indistinguishable_from(&kw("FOO")));
    assert!(!kw("FOO").indistinguishable_from(&kw("BAR")));
    assert!(!kw("FOO").indistinguishable_from(&num));
    assert!(!kw("FOO").indistinguishable_from(&ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![],
    }));
}

#[test]
fn return_type_matches_value_deferred_always_true_resolved_delegates() {
    let types = TypeRegistry::new();
    use crate::machine::model::values::KObject;
    let obj = KObject::Number(42.0);
    // Deferred always matches — per-call check runs elsewhere.
    let d = ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("er".into())));
    assert!(d.matches_value(&obj, &types));
    assert!(!d.is_resolved());
    let r_num = ReturnType::Resolved(KType::Number);
    assert!(r_num.matches_value(&obj, &types));
    assert!(r_num.is_resolved());
    let r_bool = ReturnType::Resolved(KType::Bool);
    assert!(!r_bool.matches_value(&obj, &types));
}
