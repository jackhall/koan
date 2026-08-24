use super::*;
use crate::builtins::register_builtin;
use crate::builtins::test_support::kw_part;
use crate::builtins::test_support::{TestRun, marker, run_root_bare};
use crate::machine::core::{FrameStorageExt, Scope, program_storage, run_root_storage};
use crate::machine::model::RunRegistries;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{Argument, KExpression, KType, ReturnType, SignatureDraft};
use crate::machine::model::{KKind, KObject};
use crate::machine::model::{KLiteral, TypeIdentifier};

fn body_any<'a>(ctx: &super::action::BodyCtx<'_, 'a, '_>) -> super::action::Action<'a> {
    super::action::Action::done_resident(
        ctx.scope,
        crate::machine::model::Carried::Object(marker(ctx.scope, "any")),
    )
}

/// Coarse bucket-key lookup over the scope chain. Returns the first strict-shape
/// match, falling back to any overload registered under the bucket so the
/// classification check still runs against a real `KFunction` shape.
fn find_match<'a>(
    scope: &'a Scope<'a>,
    expr: &KExpression<'a>,
    types: &TypeRegistry,
) -> Option<&'a KFunction<'a>> {
    let key = expr.untyped_key();
    let mut current: Option<&Scope<'a>> = Some(scope);
    while let Some(s) = current {
        let bucket: Vec<_> = match s.bindings().functions().get(key.as_slice()) {
            Some(bucket) => bucket
                .iter()
                .map(|entry| entry.sealed.duplicate())
                .collect(),
            None => {
                current = s.outer();
                continue;
            }
        };
        let matched = bucket
            .iter()
            .find(|sealed| s.read_function(sealed, |f| f.signature.matches(expr, types)))
            .or_else(|| bucket.first());
        if let Some(sealed) = matched {
            return Some(scope.open_function(sealed).value());
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
    let registries = RunRegistries::new();
    let types = &registries.types;
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let sig = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::keyword("OP"),
            SignatureElement::Argument(Argument {
                name: crate::machine::model::BinderSymbol::of("v")
                    .expect("a test fixture parameter is a value token"),
                ktype: KType::NUMBER,
            }),
        ],
    };
    register_builtin(
        scope,
        "OP",
        sig,
        body_any,
        &registries,
        &mut crate::machine::WriteGate::for_test(),
    );
    let brand = region.brand();
    let expr = KExpression::new(
        brand,
        &[
            Spanned::bare(kw_part("OP")),
            Spanned::bare(ExpressionPart::Identifier("someName")),
        ],
    );
    let f = find_match(scope, &expr, types).expect("OP <Number> should match");
    let pick = f.classify_for_pick(&WorkingExpression::from_ast(brand, expr), &registries);
    assert_eq!(pick.wrap_indices, vec![1]);
}

/// `<verb:Identifier> <args:KExpression>` picked against `myFn (x: 1)`: the Identifier slot is a
/// literal-name slot — it owns its token — so `classify_for_pick` excludes it from `wrap_indices`
/// and the token rides to the bind unresolved.
#[test]
fn classify_excludes_literal_name_slots_from_wrap() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let sig = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::Argument(Argument {
                name: crate::machine::model::BinderSymbol::of("verb")
                    .expect("a test fixture parameter is a value token"),
                ktype: KType::IDENTIFIER,
            }),
            SignatureElement::Argument(Argument {
                name: crate::machine::model::BinderSymbol::of("args")
                    .expect("a test fixture parameter is a value token"),
                ktype: KType::KEXPRESSION,
            }),
        ],
    };
    register_builtin(
        scope,
        "ident_call_probe",
        sig,
        body_any,
        &registries,
        &mut crate::machine::WriteGate::for_test(),
    );
    let brand = region.brand();
    let program = program_storage();
    let inner = ExpressionPart::expression(
        program.brand(),
        &[
            Spanned::bare(ExpressionPart::Identifier("x")),
            Spanned::bare(kw_part(":")),
            Spanned::bare(ExpressionPart::Literal(KLiteral::Number(1.0))),
        ],
    );
    let expr = KExpression::new(
        brand,
        &[
            Spanned::bare(ExpressionPart::Identifier("myFn")),
            Spanned::bare(inner),
        ],
    );
    let f = find_match(scope, &expr, types)
        .expect("test overload should match an Identifier-leading expression");
    let pick = f.classify_for_pick(&WorkingExpression::from_ast(brand, expr), &registries);
    assert!(pick.wrap_indices.is_empty());
}

/// LET's Identifier name slot is a literal-name slot (a *declaration* — the slot owns the name),
/// so `classify_for_pick` must exclude it from `wrap_indices`.
#[test]
fn classify_excludes_binder_name_slot_from_wrap() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = test_run.registry_handle();
    let brand = region.brand();
    let expr = KExpression::new(
        brand,
        &[
            Spanned::bare(kw_part("LET")),
            Spanned::bare(ExpressionPart::Identifier("x")),
            Spanned::bare(kw_part("=")),
            Spanned::bare(ExpressionPart::Literal(KLiteral::Number(1.0))),
        ],
    );
    let f = find_match(scope, &expr, &types).expect("LET should match");
    let pick = f.classify_for_pick(
        &WorkingExpression::from_ast(brand, expr),
        types.registries(),
    );
    assert!(
        pick.wrap_indices.is_empty(),
        "LET's Identifier name slot is a declaration, not a reference; \
         should not be a wrap index. Got {:?}",
        pick.wrap_indices,
    );
}

/// A bare leaf Type-token in a `ProperType` slot is a literal-name slot the same way an
/// Identifier in an Identifier slot is: excluded from `wrap_indices`, the token riding to the
/// bind. Symmetry pinned by
/// [design/execution/name-placeholders.md § Dispatch-time name placeholders](../../../../design/execution/name-placeholders.md#dispatch-time-name-placeholders).
#[test]
fn classify_excludes_type_token_in_propertype_slot_from_wrap() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let sig = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::keyword("OP"),
            SignatureElement::Argument(Argument {
                name: crate::machine::model::BinderSymbol::of("v")
                    .expect("a test fixture parameter is a value token"),
                ktype: KType::of_kind(KKind::ProperType),
            }),
        ],
    };
    register_builtin(
        scope,
        "OP",
        sig,
        body_any,
        &registries,
        &mut crate::machine::WriteGate::for_test(),
    );
    let brand = region.brand();
    let expr = KExpression::new(
        brand,
        &[
            Spanned::bare(kw_part("OP")),
            Spanned::bare(ExpressionPart::Type(TypeIdentifier::leaf("IntOrd"))),
        ],
    );
    let f = find_match(scope, &expr, types).expect("OP <ProperType> should match");
    let pick = f.classify_for_pick(&WorkingExpression::from_ast(brand, expr), &registries);
    assert!(pick.wrap_indices.is_empty());
}

/// Every `KFunction` value projects through `KObject::ktype()` as `KType::KFunction`,
/// carrying its parameter record and return slot.
#[test]
fn function_value_ktype_projects_kfunction() {
    use crate::machine::model::{ReturnType, SignatureDraft, TypeNode};
    let registries = RunRegistries::new();
    let types = &registries.types;
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let sig = SignatureDraft {
        return_type: ReturnType::Resolved(KType::NUMBER),
        elements: vec![
            SignatureElement::keyword("CALL"),
            SignatureElement::Argument(crate::machine::model::Argument {
                name: crate::machine::model::BinderSymbol::of("x")
                    .expect("a test fixture parameter is a value token"),
                ktype: KType::NUMBER,
            }),
        ],
    };
    let f = KFunction::alloc_captured_for_test(scope, sig, Body::Builtin(body_any), &registries);
    let obj = KObject::KFunction(f);
    match types.node(obj.ktype()) {
        TypeNode::KFunction { params, ret } => {
            assert_eq!(params.get(Symbol::of("x")), Some(&KType::NUMBER));
            assert_eq!(params.len(), 1);
            assert_eq!(ret, KType::NUMBER);
        }
        _ => panic!("expected KFunction, got {}", obj.ktype().name(&registries)),
    }
}

/// A bare leaf Type-token in an `Any` slot lands in `wrap_indices` — the auto-wrap
/// pass rewrites it into a sub-Dispatch resolved through the `BareTypeLeaf` fast
/// lane.
#[test]
fn classify_type_token_in_any_slot_returns_wrap_indices() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let sig = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::keyword("OP"),
            SignatureElement::Argument(Argument {
                name: crate::machine::model::BinderSymbol::of("v")
                    .expect("a test fixture parameter is a value token"),
                ktype: KType::ANY,
            }),
        ],
    };
    register_builtin(
        scope,
        "OP",
        sig,
        body_any,
        &registries,
        &mut crate::machine::WriteGate::for_test(),
    );
    let brand = region.brand();
    let expr = KExpression::new(
        brand,
        &[
            Spanned::bare(kw_part("OP")),
            Spanned::bare(ExpressionPart::Type(TypeIdentifier::leaf("Number"))),
        ],
    );
    let f = find_match(scope, &expr, types).expect("OP <Any> should match");
    let pick = f.classify_for_pick(&WorkingExpression::from_ast(brand, expr), &registries);
    assert_eq!(pick.wrap_indices, vec![1]);
}
