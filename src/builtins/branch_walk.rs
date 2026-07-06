//! Shared `<tag> -> <body>` branch walker for `MATCH` and `TRY-WITH`. Shape-only: picks the
//! body whose tag matches a dispatched value's tag without knowing what tags mean.
//!
//! `TRY` opts into wildcard `_` matching for dispatcher-internal error kinds; `MATCH`'s
//! exhaustiveness check is enforced by the caller. [`resolve_arm_contract`] builds
//! the `-> :T` return contract both arms enforce on their result.

use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::machine::model::types::TypeResolution;
use crate::machine::model::KType;
use crate::machine::{KError, KErrorKind, Scope};
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
        Some(KType::Unresolved(te)) => {
            match ctx.scope.resolve_type_identifier(te, ctx.chain.clone()) {
                TypeResolution::Done(resolved) => resolved.kt.clone(),
                // The builtin fallback is already tried inside `resolve_type_identifier`; a
                // non-`Done` arm here (parked or unbound) is not a synchronously-known type.
                _ => {
                    return Err(KError::new(KErrorKind::ShapeError(format!(
                        "{kind} return type `{}` is not a known type",
                        te.render()
                    ))))
                }
            }
        }
        Some(other) => other.clone(),
        None => {
            return Err(KError::new(KErrorKind::MissingArg(
                "return_type".to_string(),
            )))
        }
    };
    Ok(ReturnContract::Arm {
        ret: ctx.scope.brand().alloc_ktype(ret_kt),
        kind,
        region: ctx.scope.brand(),
    })
}

/// How the matched scrutinee reaches the arm's `it` binding.
pub(crate) enum ItSource<'a> {
    /// An owned value plus its reach — `MATCH`'s resolved argument and `TRY`'s error payload.
    Value {
        value: crate::machine::model::KObject<'a>,
        reach: crate::machine::CarrierWitness,
    },
    /// The watched producer's sealed carrier — `TRY`'s success arm. Cloned once, directly into
    /// the arm frame at bind time; the carrier's witness pins the producer until then and
    /// supplies the binding's stored reach.
    Carrier(
        crate::witnessed::Sealed<
            crate::machine::model::values::CarriedFamily,
            crate::machine::CarrierWitness,
        >,
    ),
}

/// Build the matched-arm tail shared by the `Action`-harness `MATCH` and `TRY` bodies: the
/// [`block_tail`](super::block_tail::block_tail) configuration for an arm — a fresh per-call frame
/// (`root`-rooted, chained onto `outer_frame`) whose own scope is the block, seeded with `it` bound
/// at idx 0 from `it_source`, running the arm body split into leading statements + a tail under
/// `contract`.
pub(crate) fn arm_tail<'a>(
    root: &'a Scope<'a>,
    outer_frame: Option<Rc<crate::machine::core::FrameStorage>>,
    it_source: ItSource<'a>,
    body_expr: KExpression<'a>,
    contract: ReturnContract<'a>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use super::block_tail::{block_tail, BlockBody, BlockScope, BlockSeed};
    use crate::machine::core::kfunction::action::FramePlacement;
    use crate::machine::{BindingIndex, CallFrame};
    let frame: Rc<CallFrame> = CallFrame::new(root, outer_frame);
    // Bind `it` into the frame's own scope: `alloc_object` erases the caller-`'a` input and
    // re-homes it at the frame region, so no pre-shortening is needed. Either source ends up
    // stored with the reach it arrived with, so a later read of `it` rebuilds its carrier from it.
    let seed: BlockSeed<'a> = Box::new(move |child| {
        let (it_object, reach) = match it_source {
            ItSource::Value { value, reach } => (
                child.brand().alloc_object(value),
                child.host_reach_of(&reach),
            ),
            ItSource::Carrier(carrier) => (
                // Adopt at the bind brand: one structural copy, made directly into the arm frame's
                // region inside the carrier's open; the binding stores the carrier's reach.
                carrier.open(|live| child.brand().alloc_object(live.object().deep_clone())),
                child.host_reach_of(carrier.witness()),
            ),
        };
        let _ = child.bind_value("it".to_string(), it_object, BindingIndex::value(0), reach);
    });
    block_tail(
        FramePlacement::FreshChild { frame },
        BlockScope::FrameOwn,
        Some(seed),
        BlockBody::Block(body_expr),
        Some(contract),
    )
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
