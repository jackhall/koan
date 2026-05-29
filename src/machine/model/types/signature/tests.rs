use super::*;
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::TypeParams;
use std::cell::OnceCell;

fn one_slot<'a>(kt: KType<'a>) -> ExpressionSignature<'a> {
    ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![SignatureElement::Argument(Argument {
            name: "v".into(),
            ktype: kt,
        })],
    }
}

fn list_te(name: &str, items: Vec<TypeExpr>) -> TypeExpr {
    TypeExpr {
        name: name.into(),
        params: TypeParams::List(items),
        builtin_cache: OnceCell::new(),
    }
}

fn fn_te(args: Vec<TypeExpr>, ret: TypeExpr) -> TypeExpr {
    TypeExpr {
        name: "Function".into(),
        params: TypeParams::Function { args, ret: Box::new(ret) },
        builtin_cache: OnceCell::new(),
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
    let d = ReturnType::Deferred(DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into())));
    assert_eq!(d, d.clone());
    let e = ReturnType::Deferred(DeferredReturn::Expression(expr_with_keyword("FOO")));
    assert_eq!(e, e.clone());
}

#[test]
fn return_type_eq_deferred_match_and_variant_mismatch() {
    let r = ReturnType::Resolved(KType::Number);
    let d = ReturnType::Deferred(DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into())));
    assert_ne!(r, d);
    let d2 = ReturnType::Deferred(DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into())));
    assert_eq!(d, d2);
    let d3 = ReturnType::Deferred(DeferredReturn::TypeExpr(TypeExpr::leaf("Other".into())));
    assert_ne!(d, d3);
}

#[test]
fn deferred_return_eq_matches_per_carrier() {
    let t1 = DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into()));
    let t2 = DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into()));
    let t3 = DeferredReturn::TypeExpr(TypeExpr::leaf("Other".into()));
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
fn type_expr_eq_covers_all_param_arms() {
    let leaf_a = TypeExpr::leaf("A".into());
    let leaf_a2 = TypeExpr::leaf("A".into());
    let leaf_b = TypeExpr::leaf("B".into());
    assert!(type_expr_eq(&leaf_a, &leaf_a2));
    assert!(!type_expr_eq(&leaf_a, &leaf_b));

    let list_a = list_te("List", vec![TypeExpr::leaf("A".into())]);
    let list_a2 = list_te("List", vec![TypeExpr::leaf("A".into())]);
    let list_diff = list_te("List", vec![TypeExpr::leaf("X".into())]);
    let list_two = list_te(
        "List",
        vec![TypeExpr::leaf("A".into()), TypeExpr::leaf("B".into())],
    );
    assert!(type_expr_eq(&list_a, &list_a2));
    assert!(!type_expr_eq(&list_a, &list_diff));
    assert!(!type_expr_eq(&list_a, &list_two));

    let fn_a = fn_te(vec![TypeExpr::leaf("A".into())], TypeExpr::leaf("R".into()));
    let fn_a2 = fn_te(vec![TypeExpr::leaf("A".into())], TypeExpr::leaf("R".into()));
    let fn_arg_diff =
        fn_te(vec![TypeExpr::leaf("X".into())], TypeExpr::leaf("R".into()));
    let fn_ret_diff =
        fn_te(vec![TypeExpr::leaf("A".into())], TypeExpr::leaf("X".into()));
    let fn_arity = fn_te(
        vec![TypeExpr::leaf("A".into()), TypeExpr::leaf("B".into())],
        TypeExpr::leaf("R".into()),
    );
    assert!(type_expr_eq(&fn_a, &fn_a2));
    assert!(!type_expr_eq(&fn_a, &fn_arg_diff));
    assert!(!type_expr_eq(&fn_a, &fn_ret_diff));
    assert!(!type_expr_eq(&fn_a, &fn_arity));

    // Same name across both sides so the name short-circuit doesn't pre-empt
    // the params-shape fallthrough.
    let same_name_leaf = TypeExpr::leaf("Shape".into());
    let same_name_list = list_te("Shape", vec![TypeExpr::leaf("A".into())]);
    let same_name_fn =
        TypeExpr {
            name: "Shape".into(),
            params: TypeParams::Function {
                args: vec![TypeExpr::leaf("A".into())],
                ret: Box::new(TypeExpr::leaf("R".into())),
            },
            builtin_cache: OnceCell::new(),
        };
    assert!(!type_expr_eq(&same_name_leaf, &same_name_list));
    assert!(!type_expr_eq(&same_name_list, &same_name_fn));
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
    let d = ReturnType::Deferred(DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into())));
    assert!(format!("{:?}", d).contains("Deferred"));
}

#[test]
fn deferred_return_debug_renders_both_arms() {
    let t = DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into()));
    assert!(format!("{:?}", t).contains("TypeExpr"));
    let e = DeferredReturn::Expression(expr_with_keyword("FOO"));
    assert!(format!("{:?}", e).contains("Expression"));
}

#[test]
fn return_type_name_covers_all_arms() {
    let r = ReturnType::Resolved(KType::Number);
    assert_eq!(r.name(), KType::Number.name());
    let t = ReturnType::Deferred(DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into())));
    assert_eq!(t.name(), "Er");
    let e = ReturnType::Deferred(DeferredReturn::Expression(expr_with_keyword("FOO")));
    assert_eq!(e.name(), "FOO");
}

#[test]
fn return_type_matches_value_deferred_always_true_resolved_delegates() {
    use crate::machine::model::values::KObject;
    let obj = KObject::Number(42.0);
    // Deferred always matches — per-call check runs elsewhere.
    let d = ReturnType::Deferred(DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into())));
    assert!(d.matches_value(&obj));
    assert!(!d.is_resolved());
    let r_num = ReturnType::Resolved(KType::Number);
    assert!(r_num.matches_value(&obj));
    assert!(r_num.is_resolved());
    let r_bool = ReturnType::Resolved(KType::Bool);
    assert!(!r_bool.matches_value(&obj));
}
