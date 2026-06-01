use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral, TypeExpr};
use crate::machine::model::types::KType;
use crate::machine::model::{KObject, Parseable};

fn kw(s: &str) -> ExpressionPart<'static> {
    ExpressionPart::Keyword(s.into())
}
fn ident(s: &str) -> ExpressionPart<'static> {
    ExpressionPart::Identifier(s.into())
}
fn expr(parts: Vec<ExpressionPart<'static>>) -> ExpressionPart<'static> {
    ExpressionPart::expression(parts)
}
fn sp(p: ExpressionPart<'static>) -> Spanned<ExpressionPart<'static>> {
    Spanned::bare(p)
}
fn parts_of(items: Vec<ExpressionPart<'static>>) -> Vec<Spanned<ExpressionPart<'static>>> {
    items.into_iter().map(Spanned::bare).collect()
}

#[test]
fn resolve_for_populates_builtin_cache() {
    let part: ExpressionPart<'static> = ExpressionPart::Type(TypeExpr::leaf("Number".into()));
    let slot = KType::TypeExprRef;
    let _ = part.resolve_for(&slot);
    if let ExpressionPart::Type(t) = &part {
        assert_eq!(t.builtin_cache.get(), Some(&KType::Number));
    } else {
        panic!("expected Type part");
    }
    let r2 = part.resolve_for(&slot);
    match r2 {
        KObject::KTypeValue(kt) => assert_eq!(kt, KType::Number),
        _ => panic!("expected KTypeValue"),
    }
}

#[test]
fn resolve_for_skips_cache_for_user_bound_leaf() {
    let part: ExpressionPart<'static> = ExpressionPart::Type(TypeExpr::leaf("MyType".into()));
    let slot = KType::TypeExprRef;
    let r = part.resolve_for(&slot);
    assert!(matches!(r, KObject::TypeNameRef(_)));
    if let ExpressionPart::Type(t) = &part {
        assert!(t.builtin_cache.get().is_none());
    } else {
        panic!("expected Type part");
    }
}

/// Miri coverage for the `TypeExpr::builtin_cache` lifetime-lift in
/// [`crate::machine::model::ast::ExpressionPart::resolve_for`]. The cache holds
/// `KType<'static>` and the cache-hit path clones-then-transmutes to `KType<'a>`.
/// SAFETY: the transmute is sound because only owned variants (`Number`,
/// `List<Any>`, `Function<...>`, wildcards) — never `Module` / `Signature`
/// arena references — reach the cache. Two pre-seeded runs against distinct
/// non-`'static` arenas exercise the cache-hit transmute under tree borrows.
#[test]
fn builtin_cache_lifetime_lift_does_not_dangle() {
    use crate::machine::core::RuntimeArena;

    let te = TypeExpr::leaf("Number".into());
    te.builtin_cache
        .set(KType::Number)
        .expect("OnceCell is empty");

    {
        let arena_a = RuntimeArena::new();
        let part_a: ExpressionPart<'_> = ExpressionPart::Type(te.clone());
        let slot_a: KType<'_> = KType::TypeExprRef;
        let r = part_a.resolve_for(&slot_a);
        match r {
            KObject::KTypeValue(kt) => assert_eq!(kt, KType::Number),
            _ => panic!("expected KTypeValue from cache-hit path"),
        }
        // Sibling alloc defeats any single-arena address-stability assumption
        // tree borrows might exploit.
        let _other = arena_a.alloc(KObject::Number(1.0));
        let _ = arena_a;
    }

    // Fresh arena with a different `'a`; the lift must be independent of
    // arena_a's now-dead lifetime.
    {
        let arena_b = RuntimeArena::new();
        let part_b: ExpressionPart<'_> = ExpressionPart::Type(te.clone());
        let slot_b: KType<'_> = KType::TypeExprRef;
        let r = part_b.resolve_for(&slot_b);
        match r {
            KObject::KTypeValue(kt) => assert_eq!(kt, KType::Number),
            _ => panic!("expected KTypeValue from cache-hit path"),
        }
        let _ = arena_b;
    }

    assert_eq!(te.builtin_cache.get(), Some(&KType::Number));
}

#[test]
fn summarize_atomic_variants() {
    assert_eq!(kw("LET").summarize(), "LET");
    assert_eq!(ident("x").summarize(), "x");
    assert_eq!(
        ExpressionPart::Type(TypeExpr::leaf("Number".into())).summarize(),
        "Number",
    );
}

#[test]
fn summarize_literal_variants() {
    assert_eq!(
        ExpressionPart::Literal(KLiteral::Number(1.5)).summarize(),
        "1.5"
    );
    assert_eq!(
        ExpressionPart::Literal(KLiteral::String("hi".into())).summarize(),
        "hi"
    );
    assert_eq!(
        ExpressionPart::Literal(KLiteral::Boolean(true)).summarize(),
        "true"
    );
    assert_eq!(ExpressionPart::Literal(KLiteral::Null).summarize(), "null");
}

#[test]
fn summarize_list_and_dict_literals() {
    let list = ExpressionPart::ListLiteral(vec![
        ExpressionPart::Literal(KLiteral::Number(1.0)),
        ExpressionPart::Literal(KLiteral::Number(2.0)),
    ]);
    assert_eq!(list.summarize(), "[1 2]");

    let dict = ExpressionPart::DictLiteral(vec![(
        ExpressionPart::Literal(KLiteral::String("k".into())),
        ExpressionPart::Literal(KLiteral::Number(7.0)),
    )]);
    assert_eq!(dict.summarize(), "{k: 7}");
}

#[test]
fn summarize_nested_expression_part_threads_through() {
    let inner = expr(vec![kw("ADD"), ident("a"), ident("b")]);
    assert_eq!(inner.summarize(), "ADD a b");
}

#[test]
fn kexpression_summarize_joins_parts_with_spaces() {
    let e = KExpression::new(parts_of(vec![kw("LET"), ident("x"), ident("=")]));
    assert_eq!(e.summarize(), "LET x =");
}

#[test]
fn parseable_equal_and_ktype_for_kexpression() {
    let a = KExpression::new(parts_of(vec![kw("LET"), ident("x")]));
    let b = KExpression::new(parts_of(vec![kw("LET"), ident("x")]));
    let c = KExpression::new(parts_of(vec![kw("LET"), ident("y")]));
    assert!(a.equal(&b));
    assert!(!a.equal(&c));
    assert!(matches!(
        a.ktype(),
        crate::machine::model::KType::KExpression
    ));
}

#[test]
fn binder_name_from_type_part_extracts_or_none() {
    let with_type = KExpression::new(parts_of(vec![
        kw("STRUCT"),
        ExpressionPart::Type(TypeExpr::leaf("Point".into())),
    ]));
    assert_eq!(with_type.binder_name_from_type_part(), Some("Point".into()));

    let with_ident = KExpression::new(parts_of(vec![kw("STRUCT"), ident("Point")]));
    assert_eq!(with_ident.binder_name_from_type_part(), None);

    let too_short = KExpression::new(parts_of(vec![kw("STRUCT")]));
    assert_eq!(too_short.binder_name_from_type_part(), None);
}

#[test]
fn borrow_inner_expressions_success_and_mismatch() {
    let all_exprs = KExpression::new(parts_of(vec![
        expr(vec![ident("a")]),
        expr(vec![ident("b")]),
    ]));
    let borrowed = all_exprs
        .borrow_inner_expressions()
        .expect("all parts are expressions");
    assert_eq!(borrowed.len(), 2);
    assert_eq!(borrowed[0].summarize(), "a");
    assert_eq!(borrowed[1].summarize(), "b");

    let mixed = KExpression::new(parts_of(vec![expr(vec![ident("a")]), ident("b")]));
    assert!(mixed.borrow_inner_expressions().is_none());
}

#[test]
fn try_take_inner_expressions_split_empty_returns_err() {
    let e: KExpression<'static> = KExpression::new(vec![]);
    let err = e
        .try_take_inner_expressions_split()
        .expect_err("empty must Err");
    assert!(err.parts.is_empty());
}

#[test]
fn try_take_inner_expressions_split_first_non_expression_returns_err() {
    let e = KExpression::new(parts_of(vec![ident("a"), expr(vec![ident("b")])]));
    let err = e
        .try_take_inner_expressions_split()
        .expect_err("non-expr head must Err");
    assert_eq!(err.summarize(), "a b");
}

#[test]
fn try_take_inner_expressions_split_middle_non_expression_returns_err() {
    let e = KExpression::new(parts_of(vec![
        expr(vec![ident("a")]),
        ident("b"),
        expr(vec![ident("c")]),
    ]));
    let err = e
        .try_take_inner_expressions_split()
        .expect_err("non-expr middle must Err");
    assert_eq!(err.summarize(), "a b c");
}

#[test]
fn try_take_inner_expressions_split_all_expressions_returns_ok() {
    let e = KExpression::new(parts_of(vec![
        expr(vec![ident("a")]),
        expr(vec![ident("b")]),
        expr(vec![ident("c")]),
    ]));
    let (preceding, last) = e
        .try_take_inner_expressions_split()
        .expect("all-expr is Ok");
    assert_eq!(preceding.len(), 2);
    assert_eq!(preceding[0].summarize(), "a");
    assert_eq!(preceding[1].summarize(), "b");
    assert_eq!(last.summarize(), "c");
}

#[test]
fn debug_for_expression_part_and_kexpression() {
    // Exact format isn't load-bearing; just assert non-empty / tagged output.
    let parts: Vec<ExpressionPart<'static>> = vec![
        kw("LET"),
        ident("x"),
        ExpressionPart::Type(TypeExpr::leaf("Number".into())),
        ExpressionPart::Literal(KLiteral::Number(1.0)),
        ExpressionPart::ListLiteral(vec![ident("a")]),
        ExpressionPart::DictLiteral(vec![(ident("k"), ident("v"))]),
        expr(vec![ident("z")]),
    ];
    for p in &parts {
        let s = format!("{:?}", p);
        assert!(!s.is_empty());
    }
    let e = KExpression::new(parts.into_iter().map(Spanned::bare).collect());
    assert!(format!("{:?}", e).starts_with("KExpression"));
}
