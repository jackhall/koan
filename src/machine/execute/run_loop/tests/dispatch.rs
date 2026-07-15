//! Overload routing rules end-to-end through the scheduler. Each registered builtin
//! returns a distinct labeled marker so a test can identify which overload won.
//! Counterpart `resolve_dispatch`-only assertions live in `machine::core::tests::dispatch`.

use crate::builtins::test_support::{marker, one_slot_sig, run_root_bare};
use crate::builtins::{register_builtin, register_overload_at};
use crate::machine::core::{Action, BodyCtx};
use crate::machine::core::{BindingIndex, FrameStorageExt};
use crate::machine::execute::KoanRuntime;
use crate::machine::model::Carried;
use crate::machine::model::KObject;
use crate::machine::model::{Argument, ExpressionSignature, KType, ReturnType, SignatureElement};
use crate::machine::model::{ExpressionPart, KExpression, KLiteral};
use crate::machine::run_root_storage;
use crate::source::Spanned;

fn body_identifier<'run>(ctx: &BodyCtx<'run, '_>) -> Action<'run> {
    Action::done_resident(Carried::Object(marker(ctx.scope, "identifier")))
}
fn body_marker_any<'run>(ctx: &BodyCtx<'run, '_>) -> Action<'run> {
    Action::done_resident(Carried::Object(marker(ctx.scope, "any")))
}
fn body_inner_any<'run>(ctx: &BodyCtx<'run, '_>) -> Action<'run> {
    Action::done_resident(Carried::Object(marker(ctx.scope, "inner_any")))
}
fn body_outer_number<'run>(ctx: &BodyCtx<'run, '_>) -> Action<'run> {
    Action::done_resident(Carried::Object(marker(ctx.scope, "outer_number")))
}
fn body_lowercase<'run>(ctx: &BodyCtx<'run, '_>) -> Action<'run> {
    Action::done_resident(Carried::Object(marker(ctx.scope, "lowercase")))
}

fn summarize_marker(obj: &KObject<'_>) -> String {
    match obj {
        KObject::KString(s) => s.clone(),
        KObject::Null => "null".into(),
        _ => "<other>".into(),
    }
}

/// Inner scope's `Any` overload shadows the outer scope's more-specific `Number`
/// overload — pure lexical shadowing, innermost match wins regardless of specificity
/// at outer levels. Triggered via a keyworded shape so the routing reaches bucket
/// dispatch; bare-literal shapes fast-lane via `LiteralPassThrough` and never
/// consult overload buckets.
#[test]
fn dispatch_inner_scope_shadows_outer_more_specific() {
    let region = run_root_storage();
    let outer = run_root_bare(&region);
    let outer_sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("MARK".into()),
            SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::Number,
            }),
        ],
    };
    // User-position so the builtin root-first short-circuit doesn't claim it; the inner
    // looser overload must shadow this outer more-specific one on the ordinary walk.
    register_overload_at(
        outer,
        "outer_specific",
        outer_sig,
        body_outer_number,
        BindingIndex::value(1),
    );

    let inner = region.brand().alloc_scope(outer.child_for_call());
    let inner_sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("MARK".into()),
            SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::Any,
            }),
        ],
    };
    register_builtin(inner, "inner_loose", inner_sig, body_inner_any);

    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("MARK".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(7.0))),
    ]);
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(expr, inner);
    runtime.execute().unwrap();
    let (matched, summary) = runtime
        .read_result_with(id, |v| {
            let obj = v.object();
            (
                matches!(obj, KObject::KString(s) if s == "inner_any"),
                summarize_marker(obj),
            )
        })
        .expect("value");
    assert!(
        matched,
        "inner Any must shadow outer Number (lexical shadowing > specificity), got {:?}",
        summary,
    );
}

/// Bare-name dispatch is name-resolution-only: an unbound identifier surfaces
/// `UnboundName(name)` directly rather than falling through to a `(v :Identifier)`
/// overload bucket.
#[test]
fn stateful_bare_identifier_surfaces_unbound_name_directly() {
    use crate::machine::KErrorKind;
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    register_builtin(
        scope,
        "any_first",
        one_slot_sig("v", KType::Any),
        body_marker_any,
    );
    register_builtin(
        scope,
        "ident_second",
        one_slot_sig("v", KType::Identifier),
        body_identifier,
    );

    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier(
        "foo".into(),
    ))]);
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(expr, scope);
    runtime.execute().unwrap();
    let err = match runtime.read_result_with(id, |v| v.summarize()) {
        Err(e) => e.clone(),
        Ok(summary) => panic!(
            "stateful BareIdentifier must surface UnboundName for an unbound name; \
             got value {}",
            summary,
        ),
    };
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(name) if name == "foo"),
        "expected UnboundName(\"foo\"), got {err}",
    );
}

/// A lowercase fixed token in a registered signature is coerced to uppercase, so
/// dispatching the uppercase form from a source program still hits the registered
/// function. (Once monadic effects exist, this should also produce a warning effect.)
#[test]
fn registration_coerces_lowercase_fixed_tokens_to_uppercase() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("foo".into()),
            SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::Number,
            }),
        ],
    };
    register_builtin(scope, "FOO", sig, body_lowercase);

    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("FOO".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(1.0))),
    ]);
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(expr, scope);
    runtime.execute().unwrap();
    assert!(runtime
        .read_result_with(
            id,
            |v| matches!(v.object(), KObject::KString(s) if s == "lowercase")
        )
        .expect("value"));
}
