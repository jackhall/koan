//! Shared extraction helpers for `dispatch/builtins/*` bodies.
//!
//! Collapses the `Rc::try_unwrap` + variant-match dance used to pull `KExpression`,
//! `TypeExpr`, and bare type names out of an `ArgumentBundle` slot.

use std::rc::Rc;

use crate::dispatch::kfunction::{ArgumentBundle, NodeId, SchedulerHandle};
use crate::dispatch::runtime::{KError, KErrorKind, Scope};
use crate::dispatch::values::KObject;
use crate::parse::kexpression::{ExpressionPart, KExpression, TypeExpr, TypeParams};

/// Take ownership of a `KType::KExpression`-typed argument out of `bundle.args`, cloning
/// only if the bundle is not the sole `Rc` holder. Returns `None` if the slot is missing
/// or holds a non-`KExpression` variant.
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
/// Resolve preserves the parser's `TypeExpr` as `KObject::TypeExprValue` so parameterized
/// types (`List<Number>`, `Function<(N) -> S>`) survive into the builtin's body intact.
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

/// Resolve a `KType::TypeExprRef` slot to its bare type name, rejecting parameterized
/// forms (`Foo<X>`). `surface` is the surface-form keyword (`"STRUCT"`, `"UNION"`, ...)
/// embedded in the `ShapeError` message.
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

/// Schedule each top-level statement in `body_expr` against `scope` on the OUTER scheduler
/// and return their `NodeId`s. Caller (MODULE / SIG body) wraps these in a `Combine` whose
/// finish closure builds the binder value once all statements terminalize.
///
/// A body counts as multi-statement only when *every* part is `ExpressionPart::Expression(_)`;
/// otherwise the whole body is dispatched as a single statement. The stricter all-Expression
/// rule prevents `LET x = (FN ...)` from being mis-split (its inner `Expression` part would
/// otherwise look like a second statement).
pub(crate) fn plan_body_statements<'a>(
    sched: &mut dyn SchedulerHandle<'a>,
    child_scope: &'a Scope<'a>,
    body_expr: KExpression<'a>,
) -> Vec<NodeId> {
    let is_multi_statement = !body_expr.parts.is_empty()
        && body_expr
            .parts
            .iter()
            .all(|p| matches!(p, ExpressionPart::Expression(_)));

    if is_multi_statement {
        body_expr
            .parts
            .into_iter()
            .filter_map(|p| match p {
                ExpressionPart::Expression(e) => Some(sched.add_dispatch(*e, child_scope)),
                _ => None,
            })
            .collect()
    } else {
        vec![sched.add_dispatch(body_expr, child_scope)]
    }
}

/// `pre_run` placeholder extractor for builtins whose `parts[1]` is a single `Type(t)`
/// token. Returns `None` on shape mismatch; the builtin body is still responsible for
/// surfacing the structured error (see [`crate::dispatch::kfunction::PreRunFn`]).
pub(crate) fn binder_name_from_type_part(expr: &KExpression<'_>) -> Option<String> {
    match expr.parts.get(1)? {
        ExpressionPart::Type(t) => Some(t.name.clone()),
        _ => None,
    }
}

/// Build a `KError::TypeMismatch` from the three usual fields.
pub(crate) fn type_mismatch(arg: &str, expected: &str, got: impl Into<String>) -> KError {
    KError::new(KErrorKind::TypeMismatch {
        arg: arg.to_string(),
        expected: expected.to_string(),
        got: got.into(),
    })
}
