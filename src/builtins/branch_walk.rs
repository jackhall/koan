//! Shared `<tag> -> <body>` branch walker for `MATCH` and `TRY-WITH`. Shape-only: picks the
//! body whose tag matches a dispatched value's tag without knowing what tags mean.
//!
//! `TRY` opts into wildcard `_` matching for dispatcher-internal error kinds; `MATCH`'s
//! exhaustiveness check is enforced by the caller. [`resolve_arm_return_contract`] builds
//! the `-> :T` return contract both arms enforce on their result.

use crate::builtins::fn_def::return_type::{extract_return_type_raw, ReturnTypeRaw};
use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::core::LexicalFrame;
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::machine::model::KType;
use crate::machine::{ArgumentBundle, KError, KErrorKind, ResolveTypeExprOutcome, Scope};
use std::rc::Rc;

/// Resolve a MATCH / TRY `-> :T` annotation slot into the [`ReturnContract::Arm`] its
/// arms are checked against. Reuses the FN return-type extraction, then resolves the
/// type-expression in `scope` (MATCH / TRY take no parameters, so there is no deferred or
/// parameter-referencing case). Only a fully-resolved type is supported; a
/// forward-referenced or non-type slot raises `ShapeError`. `kind` (`"MATCH"` / `"TRY"`)
/// labels both the diagnostic and the error-frame appended on a return mismatch.
pub(crate) fn resolve_arm_return_contract<'a>(
    scope: &Scope<'a>,
    bundle: &mut ArgumentBundle<'a>,
    kind: &'static str,
    chain: Option<Rc<LexicalFrame>>,
) -> Result<ReturnContract<'a>, KError> {
    let kt = match extract_return_type_raw(bundle)? {
        ReturnTypeRaw::Resolved(kt) => kt,
        // Gated to the MATCH / TRY position — a forward type reference is a position error.
        ReturnTypeRaw::TypeExprCarrier(te) => match scope.resolve_type_expr(&te, chain) {
            ResolveTypeExprOutcome::Done(kt) => kt.clone(),
            _ => KType::from_name(&te.render()).ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{kind} return type `{}` is not a known type",
                    te.render()
                )))
            })?,
        },
        ReturnTypeRaw::ExprCarrier(_) => {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "{kind} return type must be a type expression, not a parenthesized expression"
            ))))
        }
    };
    Ok(ReturnContract::Arm {
        ret: scope.arena.alloc_ktype(kt),
        kind,
    })
}

/// Returns the body for the first triple whose tag matches `target_tag`, or — when
/// `allow_wildcard` is true and no exact match was found — the first `_` body. Exact-tag
/// matches always win over `_`, regardless of source order.
pub(crate) fn find_branch_body<'a>(
    branches: &KExpression<'a>,
    target_tag: &str,
    allow_wildcard: bool,
) -> Result<Option<KExpression<'a>>, String> {
    let parts = &branches.parts;
    if !parts.len().is_multiple_of(3) {
        return Err(format!(
            "branches must be `<tag> -> <body>` triples; got {} parts (not a multiple of 3)",
            parts.len()
        ));
    }
    let mut wildcard_body: Option<KExpression<'a>> = None;
    let mut i = 0;
    while i < parts.len() {
        let tag_part = &parts[i];
        let arrow_part = &parts[i + 1];
        let body_part = &parts[i + 2];
        let tag_name = match &tag_part.value {
            // Variant tags are capitalized type names (`Some`, `Ok`, `TypeMismatch`).
            ExpressionPart::Type(t) => t.render(),
            // Booleans parse as `KLiteral::Boolean`, not type tokens; accept them so
            // `MATCH` on a `Bool` can spell its arms `true ->` / `false ->`.
            ExpressionPart::Literal(KLiteral::Boolean(b)) => {
                if *b {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            // `_` is a pure-symbol token classified as `Keyword`, not a type name.
            ExpressionPart::Keyword(s) if allow_wildcard && s == "_" => s.clone(),
            other => {
                return Err(format!(
                    "branch tag must be a capitalized variant tag or boolean literal, got {}",
                    other.summarize()
                ));
            }
        };
        match &arrow_part.value {
            ExpressionPart::Keyword(k) if k == "->" => {}
            other => {
                return Err(format!(
                    "branch separator must be `->`, got {}",
                    other.summarize()
                ));
            }
        }
        let body_expr = match &body_part.value {
            ExpressionPart::Expression(e) => (**e).clone(),
            other => {
                return Err(format!(
                    "branch body must be a parenthesized expression, got {}",
                    other.summarize()
                ));
            }
        };
        if tag_name == target_tag {
            return Ok(Some(body_expr));
        }
        if allow_wildcard && tag_name == "_" && wildcard_body.is_none() {
            wildcard_body = Some(body_expr);
        }
        i += 3;
    }
    Ok(wildcard_body)
}
