//! Shared extraction helpers for `dispatch/builtins/*` bodies.
//!
//! The builtin bodies that take parens-wrapped sub-expressions (FN, STRUCT, UNION, MODULE,
//! SIG, QUOTE, MATCH, type-call) all need the same `Rc::try_unwrap` + variant-match dance to
//! pull a `KExpression` out of an `ArgumentBundle` slot. Pre-consolidation that dance was
//! verbatim-copied across eight files; this module collapses the duplication. Same idea for
//! the `TypeExpr` extraction (currently only `fn_def` extracts a `TypeExpr` from a
//! `TypeExprRef` slot) and for the "bare type name from a `TypeExprRef` slot" idiom that
//! appears in five constructors that take a leading type token (STRUCT, UNION, MODULE, SIG,
//! type-call).
//!
//! All helpers are `pub(crate)`: the consumers all live under `dispatch::builtins`, so the
//! tighter visibility keeps them private to the family without imposing per-file imports.

use std::rc::Rc;

use crate::dispatch::kfunction::{ArgumentBundle, NodeId};
use crate::dispatch::runtime::{KError, KErrorKind, Scope};
use crate::dispatch::values::KObject;
use crate::execute::scheduler::Scheduler;
use crate::parse::kexpression::{ExpressionPart, KExpression, TypeExpr, TypeParams};

/// Take ownership of a `KType::KExpression`-typed argument out of `bundle.args` and return
/// the inner `KExpression`. Mirrors the `Rc::try_unwrap` shape consumers used to copy
/// in-place: when the bundle holds the only `Rc` reference, ownership transfers without
/// cloning; otherwise the inner `KExpression` is cloned. Returns `None` if the slot is
/// missing or holds a non-`KExpression` variant — the caller is expected to surface a
/// `ShapeError` describing what their slot expected.
pub(crate) fn extract_kexpression<'a>(
    bundle: &mut ArgumentBundle<'a>,
    name: &str,
) -> Option<KExpression<'a>> {
    let rc = bundle.args.remove(name)?;
    match Rc::try_unwrap(rc) {
        Ok(KObject::KExpression(e)) => Some(e),
        Ok(_) => None,
        Err(rc) => match &*rc {
            KObject::KExpression(e) => Some(e.clone()),
            _ => None,
        },
    }
}

/// Take ownership of the structured `TypeExpr` carried by a `KType::TypeExprRef` slot.
/// The resolve path preserves the parser's `TypeExpr` as `KObject::TypeExprValue`, so
/// parameterized types (`List<Number>`, `Function<(N) -> S>`) survive into the
/// builtin's body intact. Returns `None` if the slot is missing or holds a non-
/// `TypeExprValue` variant.
pub(crate) fn extract_type_expr<'a>(
    bundle: &mut ArgumentBundle<'a>,
    name: &str,
) -> Option<TypeExpr> {
    let rc = bundle.args.remove(name)?;
    match Rc::try_unwrap(rc) {
        Ok(KObject::TypeExprValue(t)) => Some(t),
        Ok(_) => None,
        Err(rc) => match &*rc {
            KObject::TypeExprValue(t) => Some(t.clone()),
            _ => None,
        },
    }
}

/// Look up a `KType::TypeExprRef` slot and return its bare type name, asserting along the
/// way that the slot resolves to a `TypeExprValue` whose `params` is `TypeParams::None`
/// (i.e., the user wrote `Foo` rather than `Foo<X>`). Builds the structured `KError` for
/// every failure path so the call site stays a one-liner.
///
/// `surface` is the surface-form keyword for the offending construct (`"STRUCT"`,
/// `"UNION"`, `"MODULE"`, `"SIG"`, `"type-call"`) and feeds the `ShapeError` message
/// when the slot resolves to a parameterized form.
pub(crate) fn extract_bare_type_name<'a>(
    bundle: &ArgumentBundle<'a>,
    name: &str,
    surface: &str,
) -> Result<String, KError> {
    match bundle.get(name) {
        Some(KObject::TypeExprValue(t)) => match &t.params {
            TypeParams::None => Ok(t.name.clone()),
            _ => Err(KError::new(KErrorKind::ShapeError(format!(
                "{surface} {name} must be a bare type name, got `{}`",
                t.render(),
            )))),
        },
        Some(other) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: name.to_string(),
            expected: "TypeExprRef".to_string(),
            got: other.ktype().name(),
        })),
        None => Err(KError::new(KErrorKind::MissingArg(name.to_string()))),
    }
}

/// Run each top-level statement in `body_expr` against `scope` using a fresh inner
/// scheduler, and return the first error (or `Ok(())` if all complete cleanly).
///
/// Body-shape detection: a multi-statement body consists *entirely* of
/// `ExpressionPart::Expression(_)` parts (each statement is its own parens-wrapped sub-
/// expression). Anything else — including a single `LET` or `FN` whose body contains a
/// nested `Expression` part — is dispatched as one statement against the body as a whole.
/// The stricter "all-Expression" rule avoids the false positive where `LET x = (FN ...)`
/// would otherwise look like a one-statement body but get partially dispatched.
///
/// Used by both `MODULE` and `SIG` body construction; the inner scheduler is private to
/// the call so the surrounding caller's scheduler doesn't get tangled with the body's
/// statements.
pub(crate) fn run_body_statements<'a>(
    scope: &'a Scope<'a>,
    body_expr: KExpression<'a>,
) -> Result<(), KError> {
    let is_multi_statement = !body_expr.parts.is_empty()
        && body_expr
            .parts
            .iter()
            .all(|p| matches!(p, ExpressionPart::Expression(_)));

    let mut sched = Scheduler::new();
    let ids: Vec<NodeId> = if is_multi_statement {
        body_expr
            .parts
            .into_iter()
            .filter_map(|p| match p {
                ExpressionPart::Expression(e) => Some(sched.add_dispatch(*e, scope)),
                _ => None,
            })
            .collect()
    } else {
        vec![sched.add_dispatch(body_expr, scope)]
    };
    sched.execute()?;
    for id in ids {
        if let Err(e) = sched.read_result(id) {
            return Err(e.clone());
        }
    }
    Ok(())
}

/// `pre_run` helper for the family of name-binding builtins whose `parts[1]` is a single
/// `Type(t)` token: STRUCT, UNION (named form), SIG, MODULE. Pulls the bare type-name
/// string out and returns `None` on shape mismatch. The body still does the full
/// shape-check and surfaces the structured error; this is only the dispatch-time
/// placeholder extractor (see [`crate::dispatch::kfunction::PreRunFn`]).
///
/// LET is intentionally excluded — its `parts[1]` accepts either `Identifier` or `Type`
/// (lowercase or uppercase binder), so it keeps its own slightly wider matcher. FN is
/// excluded too — its name lives inside the signature sub-expression, not at `parts[1]`.
pub(crate) fn binder_name_from_type_part(expr: &KExpression<'_>) -> Option<String> {
    match expr.parts.get(1)? {
        ExpressionPart::Type(t) => Some(t.name.clone()),
        _ => None,
    }
}

/// Build a `KError::TypeMismatch` from the three usual fields. Convenience wrapper used at
/// the noisiest builtin call sites — most of the in-tree `KError::new(KErrorKind::TypeMismatch
/// { ... })` calls predate this helper and are intentionally left untouched (changing them
/// would churn diff without payoff). Reach for this helper when a new TypeMismatch is being
/// added and the surrounding code is otherwise compact.
pub(crate) fn type_mismatch(arg: &str, expected: &str, got: impl Into<String>) -> KError {
    KError::new(KErrorKind::TypeMismatch {
        arg: arg.to_string(),
        expected: expected.to_string(),
        got: got.into(),
    })
}
