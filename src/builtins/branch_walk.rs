//! Shared `<tag> -> <body>` branch walker for `MATCH` and `TRY-WITH`. Shape-only: picks the
//! body whose tag matches a dispatched value's tag without knowing what tags mean.
//!
//! `TRY` opts into wildcard `_` matching for dispatcher-internal error kinds; `MATCH`'s
//! exhaustiveness check is enforced by the caller. [`resolve_arm_contract`] builds
//! the `-> :T` return contract both arms enforce on their result.

use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::machine::model::KType;
use crate::machine::{KError, KErrorKind, TypeIdentifierResolution, Scope};
use std::rc::Rc;

/// Read the MATCH / TRY `-> :T` slot from `ctx.args` (resolving a forward-referenced bare name
/// against the call-site scope/chain) into the [`ReturnContract::Arm`] both `MATCH` and `TRY`
/// arms are checked against.
pub(crate) fn resolve_arm_contract<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
    kind: &'static str,
) -> Result<ReturnContract<'a>, KError> {
    use crate::machine::core::kfunction::action::arg_type;
    let ret_kt = match arg_type(ctx.args, "return_type") {
        Some(KType::Unresolved(te)) => match ctx.scope.resolve_type_identifier(te, ctx.chain.clone()) {
            TypeIdentifierResolution::Done(kt) => kt.clone(),
            _ => KType::from_name(&te.render()).ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{kind} return type `{}` is not a known type",
                    te.render()
                )))
            })?,
        },
        Some(other) => other.clone(),
        None => {
            return Err(KError::new(KErrorKind::MissingArg(
                "return_type".to_string(),
            )))
        }
    };
    Ok(ReturnContract::Arm {
        ret: ctx.scope.arena.alloc_ktype(ret_kt),
        kind,
        arena: ctx.scope.arena,
    })
}

/// Build the matched-arm tail shared by the `Action`-harness `MATCH` and `TRY` bodies: a fresh
/// per-call frame (`root`-rooted, chained onto `outer_frame`) with `it` bound at idx 0,
/// tail-replacing into the arm body's last statement (the harness parks on the leading statements
/// as owned deps, running them before the tail continues) carrying `contract`.
pub(crate) fn arm_tail<'a>(
    root: &Scope<'a>,
    outer_frame: Option<Rc<crate::machine::core::FrameStorage>>,
    it_value: crate::machine::model::KObject<'a>,
    body_expr: KExpression<'a>,
    contract: ReturnContract<'a>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{Action, FramePlacement};
    use crate::machine::core::kfunction::body::split_body_statements;
    use crate::machine::{BindingIndex, CallFrame};
    let frame: Rc<CallFrame> = CallFrame::new(root, outer_frame);
    frame.with_frame_interior(|arena, child| {
        let it_obj = arena.alloc_object(it_value);
        let _ = child.bind_value("it".to_string(), it_obj, BindingIndex::value(0));
    });
    let arm_scope_id = frame.scope_for_bind().id;
    let mut statements = split_body_statements(body_expr);
    let tail = statements
        .pop()
        .expect("split_body_statements always yields at least one");
    Action::Tail {
        leading: statements,
        tail,
        contract: Some(contract),
        frame_placement: FramePlacement::FreshChild { frame },
        block_entry: Some(arm_scope_id),
    }
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
