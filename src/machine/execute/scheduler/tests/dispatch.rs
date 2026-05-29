//! Overload routing rules end-to-end through the scheduler. Each registered builtin
//! returns a distinct labeled marker so a test can identify which overload won.
//! Counterpart `resolve_dispatch`-only assertions live in `machine::core::tests::dispatch`.

use crate::builtins::register_builtin;
use crate::builtins::test_support::{marker, one_slot_sig, run_root_bare};
use crate::machine::model::{KObject, Parseable};
use crate::machine::model::types::{Argument, ExpressionSignature, KType, SignatureElement, ReturnType};
use crate::machine::{RuntimeArena, Scope};
use crate::machine::core::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use super::super::Scheduler;


fn body_identifier<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "identifier")) }
fn body_marker_any<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "any")) }
fn body_inner_any<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "inner_any")) }
fn body_outer_number<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "outer_number")) }
fn body_lowercase<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "lowercase")) }

fn summarize_marker(obj: &KObject<'_>) -> String {
    match obj {
        KObject::KString(s) => s.clone(),
        KObject::Null => "null".into(),
        _ => "<other>".into(),
    }
}


/// Inner scope's `Any` overload shadows the outer scope's more-specific `Number`
/// overload — pure lexical shadowing, innermost match wins regardless of specificity
/// at outer levels.
#[test]
fn dispatch_inner_scope_shadows_outer_more_specific() {
    let arena = RuntimeArena::new();
    let outer = run_root_bare(&arena);
    register_builtin(outer, "outer_specific", one_slot_sig("v", KType::Number), body_outer_number);

    let inner = arena.alloc_scope(outer.child_for_call());
    register_builtin(inner, "inner_loose", one_slot_sig("v", KType::Any), body_inner_any);

    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Literal(KLiteral::Number(7.0)))]);
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, inner);
    sched.execute().unwrap();
    let result = sched.read(id);
    assert!(
        matches!(result, KObject::KString(s) if s == "inner_any"),
        "inner Any must shadow outer Number (lexical shadowing > specificity), got {:?}",
        summarize_marker(result),
    );
}

/// Bare-name dispatch is name-resolution-only: an unbound identifier surfaces
/// `UnboundName(name)` directly rather than falling through to a `(v :Identifier)`
/// overload bucket.
#[test]
fn stateful_bare_identifier_surfaces_unbound_name_directly() {
    use crate::machine::KErrorKind;
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    register_builtin(scope, "any_first", one_slot_sig("v", KType::Any), body_marker_any);
    register_builtin(scope, "ident_second", one_slot_sig("v", KType::Identifier), body_identifier);

    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("foo".into()))]);
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, scope);
    sched.execute().unwrap();
    let err = match sched.read_result(id) {
        Err(e) => e.clone(),
        Ok(v) => panic!(
            "stateful BareIdentifier must surface UnboundName for an unbound name; \
             got value {}",
            v.summarize(),
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
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
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
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, scope);
    sched.execute().unwrap();
    let result = sched.read(id);
    assert!(matches!(result, KObject::KString(s) if s == "lowercase"));
}
