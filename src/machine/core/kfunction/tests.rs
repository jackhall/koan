use super::*;
use crate::builtins::test_support::{marker, run_root_bare};
use crate::builtins::{default_scope, register_builtin};
use crate::machine::core::{RuntimeArena, Scope};
use crate::machine::model::ast::{KLiteral, TypeName};
use crate::machine::model::types::{Argument, ExpressionSignature, KType, ReturnType};
use crate::machine::model::{KKind, KObject};

fn body_any<'a>(ctx: &super::action::BodyCtx<'a, '_>) -> super::action::Action<'a> {
    super::action::Action::Done(Ok(crate::machine::model::Carried::Object(marker(
        ctx.scope, "any",
    ))))
}

/// Coarse bucket-key lookup over the scope chain. Returns the first strict-shape
/// match, falling back to any overload registered under the bucket so the
/// classification check still runs against a real `KFunction` shape.
fn find_match<'a>(scope: &'a Scope<'a>, expr: &KExpression<'a>) -> Option<&'a KFunction<'a>> {
    let key = expr.untyped_key();
    let mut current: Option<&Scope<'a>> = Some(scope);
    while let Some(s) = current {
        let functions = s.bindings().functions();
        if let Some(bucket) = functions.get(&key) {
            if let Some((f, _)) = bucket.iter().find(|(f, _)| f.signature.matches(expr)) {
                return Some(*f);
            }
            if let Some((f, _)) = bucket.iter().next() {
                return Some(*f);
            }
        }
        current = s.outer();
    }
    None
}

/// `OP <v:Number>` classified against `OP someName` (Identifier in Number slot)
/// returns `wrap_indices = [1]` — the dispatcher wraps `someName` as a sub-Dispatch
/// resolved through the `BareIdentifier` fast lane.
#[test]
fn classify_returns_wrap_indices_for_value_slot_identifiers() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("OP".into()),
            SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::Number,
            }),
        ],
    };
    register_builtin(scope, "OP", sig, body_any);
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("OP".into())),
        Spanned::bare(ExpressionPart::Identifier("someName".into())),
    ]);
    let f = find_match(scope, &expr).expect("OP <Number> should match");
    let pick = f.classify_for_pick(&expr);
    assert_eq!(pick.wrap_indices, vec![1]);
    assert!(pick.ref_name_indices.is_empty());
    assert!(!pick.picked_has_binder_name);
}

/// `<verb:Identifier> <args:KExpression>` picked against `myFn (x: 1)` returns
/// `ref_name_indices = [0]`: the Identifier slot is a literal-name reference and
/// the function has no `binder_name`, so replay-park checks whether `myFn`
/// resolves to a placeholder.
#[test]
fn classify_returns_ref_name_indices_for_non_binder_function() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Argument(Argument {
                name: "verb".into(),
                ktype: KType::Identifier,
            }),
            SignatureElement::Argument(Argument {
                name: "args".into(),
                ktype: KType::KExpression,
            }),
        ],
    };
    register_builtin(scope, "ident_call_probe", sig, body_any);
    let inner = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Identifier("x".into())),
        Spanned::bare(ExpressionPart::Keyword(":".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(1.0))),
    ]);
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Identifier("myFn".into())),
        Spanned::bare(ExpressionPart::Expression(Box::new(inner))),
    ]);
    let f = find_match(scope, &expr)
        .expect("test overload should match an Identifier-leading expression");
    let pick = f.classify_for_pick(&expr);
    assert!(pick.ref_name_indices.contains(&0));
    assert!(!pick.picked_has_binder_name);
}

/// LET has `binder_name = Some(_)`, so its Identifier name slot is a *declaration*,
/// not a reference, and `classify_for_pick` must exclude it from `ref_name_indices`.
#[test]
fn classify_skips_ref_name_indices_for_binder_function() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("LET".into())),
        Spanned::bare(ExpressionPart::Identifier("x".into())),
        Spanned::bare(ExpressionPart::Keyword("=".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(1.0))),
    ]);
    let f = find_match(scope, &expr).expect("LET should match");
    let pick = f.classify_for_pick(&expr);
    assert!(pick.picked_has_binder_name);
    assert!(
        pick.ref_name_indices.is_empty(),
        "LET's Identifier name slot is a declaration, not a reference; \
         should not be ref_name_index. Got {:?}",
        pick.ref_name_indices,
    );
}

/// A bare leaf Type-token in a `TypeExprRef` slot lands in `ref_name_indices` the
/// same way an Identifier in an Identifier slot does. Symmetry pinned by
/// [design/execution-model.md § Dispatch-time name placeholders](../../../../design/execution-model.md#dispatch-time-name-placeholders).
#[test]
fn classify_type_token_in_typeexprref_slot_returns_ref_name_indices() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("OP".into()),
            SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::OfKind(KKind::Proper),
            }),
        ],
    };
    register_builtin(scope, "OP", sig, body_any);
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("OP".into())),
        Spanned::bare(ExpressionPart::Type(TypeName::leaf("IntOrd".into()))),
    ]);
    let f = find_match(scope, &expr).expect("OP <TypeExprRef> should match");
    let pick = f.classify_for_pick(&expr);
    assert_eq!(pick.ref_name_indices, vec![1]);
    assert!(pick.wrap_indices.is_empty());
    assert!(!pick.picked_has_binder_name);
}

/// `is_functor`-flagged `KFunction` projects through `KObject::ktype()` as
/// `KType::KFunctor`; unflagged stays `KType::KFunction`.
#[test]
fn function_value_ktype_projects_kfunctor_when_flagged() {
    use crate::machine::model::types::{ExpressionSignature, ReturnType};
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let make_sig = || ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Number),
        elements: vec![
            SignatureElement::Keyword("CALL".into()),
            SignatureElement::Argument(crate::machine::model::types::Argument {
                name: "x".into(),
                ktype: KType::Number,
            }),
        ],
    };
    let plain = KFunction::with_binder_name(make_sig(), Body::Builtin(body_any), scope, None);
    let plain_obj = KObject::KFunction(arena.alloc_function(plain), None);
    assert!(matches!(plain_obj.ktype(), KType::KFunction { .. }));
    let functor = KFunction::with_binder_and_functor(
        make_sig(),
        Body::Builtin(body_any),
        scope,
        None,
        None,
        true,
    );
    let functor_obj = KObject::KFunction(arena.alloc_function(functor), None);
    match functor_obj.ktype() {
        KType::KFunctor { params, ret, body } => {
            assert_eq!(params.get("x"), Some(&KType::Number));
            assert_eq!(params.len(), 1);
            assert_eq!(*ret, KType::Number);
            // The projection carries the callable body so a type-bound functor
            // name can be applied through it.
            assert!(body.is_some(), "a functor value projects body: Some(f)");
        }
        other => panic!("expected KFunctor, got {:?}", other),
    }
}

/// `KType::KFunctor`'s `body` is identity-inert: two distinct functor values with
/// identical signatures project functor types that compare AND hash equal,
/// despite carrying different `body` pointers.
#[test]
fn functor_ktype_identity_ignores_body() {
    use crate::machine::model::types::{ExpressionSignature, ReturnType};
    use std::hash::{Hash, Hasher};
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let make_sig = || ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Number),
        elements: vec![
            SignatureElement::Keyword("CALL".into()),
            SignatureElement::Argument(crate::machine::model::types::Argument {
                name: "x".into(),
                ktype: KType::Number,
            }),
        ],
    };
    let mk_functor = || {
        let f = KFunction::with_binder_and_functor(
            make_sig(),
            Body::Builtin(body_any),
            scope,
            None,
            None,
            true,
        );
        KObject::KFunction(arena.alloc_function(f), None)
    };
    let a = mk_functor().ktype();
    let b = mk_functor().ktype();
    // Distinct `body` pointers (two independent allocations) but identical shape.
    assert!(matches!(
        (&a, &b),
        (
            KType::KFunctor { body: Some(_), .. },
            KType::KFunctor { body: Some(_), .. }
        )
    ));
    assert_eq!(
        a, b,
        "functor types with different bodies must compare equal"
    );
    let hash_of = |t: &KType<'_>| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        t.hash(&mut h);
        h.finish()
    };
    assert_eq!(
        hash_of(&a),
        hash_of(&b),
        "functor types with different bodies must hash equal",
    );
    // And both equal the body-less annotation form (the `:(FUNCTOR …)` surface).
    let annotation = KType::KFunctor {
        params: crate::machine::model::types::Record::from_pairs(vec![("x".into(), KType::Number)]),
        ret: Box::new(KType::Number),
        body: None,
    };
    assert_eq!(
        a, annotation,
        "a body-bearing functor equals its annotation"
    );
    assert_eq!(hash_of(&a), hash_of(&annotation));
}

/// A bare leaf Type-token in an `Any` slot lands in `wrap_indices` — the auto-wrap
/// pass rewrites it into a sub-Dispatch resolved through the `BareTypeLeaf` fast
/// lane.
#[test]
fn classify_type_token_in_any_slot_returns_wrap_indices() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("OP".into()),
            SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::Any,
            }),
        ],
    };
    register_builtin(scope, "OP", sig, body_any);
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("OP".into())),
        Spanned::bare(ExpressionPart::Type(TypeName::leaf("Number".into()))),
    ]);
    let f = find_match(scope, &expr).expect("OP <Any> should match");
    let pick = f.classify_for_pick(&expr);
    assert_eq!(pick.wrap_indices, vec![1]);
    assert!(pick.ref_name_indices.is_empty());
    assert!(!pick.picked_has_binder_name);
}
