use super::*;
use crate::builtins::test_support::{marker, run_root_bare};
use crate::builtins::{default_scope, register_builtin};
use crate::machine::core::{run_root_storage, FrameStorageExt, Scope};
use crate::machine::model::{Argument, ExpressionSignature, KType, ReturnType};
use crate::machine::model::{KKind, KObject};
use crate::machine::model::{KLiteral, TypeIdentifier};

fn body_any<'a>(ctx: &super::action::BodyCtx<'a, '_>) -> super::action::Action<'a> {
    super::action::Action::done_resident(crate::machine::model::Carried::Object(marker(
        ctx.scope, "any",
    )))
}

/// Coarse bucket-key lookup over the scope chain. Returns the first strict-shape
/// match, falling back to any overload registered under the bucket so the
/// classification check still runs against a real `KFunction` shape.
fn find_match<'a>(scope: &'a Scope<'a>, expr: &KExpression<'a>) -> Option<&'a KFunction<'a>> {
    let types = TypeRegistry::new();
    let key = expr.untyped_key();
    let mut current: Option<&Scope<'a>> = Some(scope);
    while let Some(s) = current {
        let functions = s.bindings().functions();
        if let Some(bucket) = functions.get(&key) {
            if let Some((f, _)) = bucket
                .iter()
                .find(|(f, _)| f.signature.matches(expr, &types))
            {
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
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
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
    let pick = f.classify_for_pick(&expr, &types);
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
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
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
    let pick = f.classify_for_pick(&expr, &types);
    assert!(pick.ref_name_indices.contains(&0));
    assert!(!pick.picked_has_binder_name);
}

/// LET has `binder_name = Some(_)`, so its Identifier name slot is a *declaration*,
/// not a reference, and `classify_for_pick` must exclude it from `ref_name_indices`.
#[test]
fn classify_skips_ref_name_indices_for_binder_function() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("LET".into())),
        Spanned::bare(ExpressionPart::Identifier("x".into())),
        Spanned::bare(ExpressionPart::Keyword("=".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(1.0))),
    ]);
    let f = find_match(scope, &expr).expect("LET should match");
    let pick = f.classify_for_pick(&expr, &types);
    assert!(pick.picked_has_binder_name);
    assert!(
        pick.ref_name_indices.is_empty(),
        "LET's Identifier name slot is a declaration, not a reference; \
         should not be ref_name_index. Got {:?}",
        pick.ref_name_indices,
    );
}

/// A bare leaf Type-token in a `ProperType` slot lands in `ref_name_indices` the
/// same way an Identifier in an Identifier slot does. Symmetry pinned by
/// [design/execution/name-placeholders.md § Dispatch-time name placeholders](../../../../design/execution/name-placeholders.md#dispatch-time-name-placeholders).
#[test]
fn classify_type_token_in_typeexprref_slot_returns_ref_name_indices() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("OP".into()),
            SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::OfKind(KKind::ProperType),
            }),
        ],
    };
    register_builtin(scope, "OP", sig, body_any);
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("OP".into())),
        Spanned::bare(ExpressionPart::Type(TypeIdentifier::leaf("IntOrd".into()))),
    ]);
    let f = find_match(scope, &expr).expect("OP <ProperType> should match");
    let pick = f.classify_for_pick(&expr, &types);
    assert_eq!(pick.ref_name_indices, vec![1]);
    assert!(pick.wrap_indices.is_empty());
    assert!(!pick.picked_has_binder_name);
}

/// Every `KFunction` value projects through `KObject::ktype()` as `KType::KFunction`,
/// carrying its parameter record and return slot.
#[test]
fn function_value_ktype_projects_kfunction() {
    use crate::machine::model::{ExpressionSignature, ReturnType};
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Number),
        elements: vec![
            SignatureElement::Keyword("CALL".into()),
            SignatureElement::Argument(crate::machine::model::Argument {
                name: "x".into(),
                ktype: KType::Number,
            }),
        ],
    };
    let f = KFunction::new(sig, Body::Builtin(body_any), scope, None, None);
    let obj = KObject::KFunction(region.brand().alloc_function(f));
    match obj.ktype() {
        KType::KFunction { params, ret, .. } => {
            assert_eq!(params.get("x"), Some(&KType::Number));
            assert_eq!(params.len(), 1);
            assert_eq!(*ret, KType::Number);
        }
        other => panic!("expected KFunction, got {other:?}"),
    }
}

/// A bare leaf Type-token in an `Any` slot lands in `wrap_indices` — the auto-wrap
/// pass rewrites it into a sub-Dispatch resolved through the `BareTypeLeaf` fast
/// lane.
#[test]
fn classify_type_token_in_any_slot_returns_wrap_indices() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
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
        Spanned::bare(ExpressionPart::Type(TypeIdentifier::leaf("Number".into()))),
    ]);
    let f = find_match(scope, &expr).expect("OP <Any> should match");
    let pick = f.classify_for_pick(&expr, &types);
    assert_eq!(pick.wrap_indices, vec![1]);
    assert!(pick.ref_name_indices.is_empty());
    assert!(!pick.picked_has_binder_name);
}
