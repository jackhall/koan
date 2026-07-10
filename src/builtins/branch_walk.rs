//! Branch walkers for `MATCH` and `TRY-WITH`, plus the shared arm-tail machinery.
//!
//! `TRY` selects an arm by **string tag** — [`find_branch_body_by_tag`] matches a
//! dispatched value's error/success tag and opts into wildcard `_` matching for
//! dispatcher-internal error kinds. `MATCH` selects an arm by **type** —
//! [`find_branch_body_by_type`] resolves each arm head to a `KType`, admits the arms
//! whose type matches the scrutinee value, and runs the most-specific-wins tournament
//! (ruling F1). [`resolve_arm_contract`] builds the `-> :T` return contract both arms
//! enforce on their result.

use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::core::LexicalFrame;
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral, TypeIdentifier};
use crate::machine::model::types::{RecursiveSet, TypeResolution};
use crate::machine::model::{KObject, KType};
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
        ret: ctx.scope.brand().alloc_ktype_pure(ret_kt)?,
        kind,
        scope: ctx.scope,
    })
}

/// How the matched scrutinee reaches the arm's `it` binding.
pub(crate) enum ItSource<'a> {
    /// An owned value plus the delivery envelope it was read out of — `MATCH`'s resolved argument
    /// (the envelope supplies the copy's stored reach and its producer pin) and `TRY`'s error
    /// payload (`None`: the payload is region-pure, reaching nothing).
    Value {
        value: crate::machine::model::KObject<'a>,
        delivered: Option<crate::machine::DeliveredCarried>,
    },
    /// The watched producer's delivery envelope — `TRY`'s success arm. Cloned once, directly into
    /// the arm frame at bind time; the envelope's retained host pins the producer until then and
    /// supplies the binding's stored reach.
    Carrier(crate::machine::DeliveredCarried),
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
    // re-homes it at the frame region, so no pre-shortening is needed. Either source is a deep copy
    // living in the arm frame, so the stored reach is the copy's (`adopted_reach_of` — a
    // residence-only host is not carried; a tail loop's retiring frame must not ride the arm's
    // binding), and a later read of `it` rebuilds its carrier from it.
    let seed: BlockSeed<'a> = Box::new(move |child| {
        let (it_object, reach) = match it_source {
            // `delivered` supplies this value's reach evidence; `None` is `TRY`'s
            // region-pure error payload, whose purity is an audit now instead of a comment.
            ItSource::Value {
                value,
                delivered: Some(d),
            } => {
                let reach = child.adopted_reach_of(&d);
                let object = child
                    .alloc_object_delivered(value, std::slice::from_ref(&reach))
                    .expect("ItSource::Value's delivered carrier must cover its own reach");
                (object, reach)
            }
            ItSource::Value {
                value,
                delivered: None,
            } => {
                let object = child
                    .brand()
                    .alloc_object_checked(value)
                    .expect("ItSource::Value with no delivered carrier must be region-pure");
                (object, Default::default())
            }
            ItSource::Carrier(carrier) => {
                // Adopt at the bind brand: one structural copy, made directly into the arm frame's
                // region inside the envelope's pinned open; the binding stores the copy's reach,
                // minted first so the copy's own residence audit can see it.
                let reach = child.adopted_reach_of(&carrier);
                let object = carrier.open(|live| {
                    child
                        .alloc_object_delivered(
                            live.object().deep_clone(),
                            std::slice::from_ref(&reach),
                        )
                        .expect("ItSource::Carrier's own reach must cover its deep copy")
                });
                (object, reach)
            }
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

/// `TRY`'s arm selector: returns the body for the first triple whose tag matches
/// `target_tag`, or — when `allow_wildcard` is true and no exact match was found — the
/// first `_` body. Exact-tag matches always win over `_`, regardless of source order.
pub(crate) fn find_branch_body_by_tag<'a>(
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

/// A `<head> -> <body>` arm the by-type walker selected for `MATCH`: the body to run and
/// the value bound to `it` under ruling F3 — the wrapped payload for a variant arm, the
/// scrutinee unchanged for a general type arm, `Null` for a boolean arm.
pub(crate) struct SelectedArm<'a> {
    pub body: KExpression<'a>,
    pub it_value: KObject<'a>,
}

/// A resolved, admitting arm head classified for the F1 specificity tournament.
enum ArmType<'a> {
    /// An exact value/tag match (a `true` / `false` literal head, or a tag head over a
    /// `TypeConstructor` value) — no subtype relation to refine.
    Exact,
    /// A type head that admits the scrutinee, carrying its resolved `KType`.
    Typed(KType<'a>),
}

/// Strict specificity between two admitting arm heads. An exact value/tag match outranks any
/// type head; two type heads compare via [`KType::is_more_specific_than`]. Reflexive / equal
/// pairs return `false`, so a duplicate head yields no strict winner (surfaced as ambiguity by
/// the caller).
fn arm_more_specific<'a>(a: &ArmType<'a>, b: &ArmType<'a>) -> bool {
    match (a, b) {
        (ArmType::Exact, ArmType::Exact) => false,
        (ArmType::Exact, ArmType::Typed(_)) => true,
        (ArmType::Typed(_), ArmType::Exact) => false,
        (ArmType::Typed(x), ArmType::Typed(y)) => x.is_more_specific_than(y),
    }
}

/// How a `MATCH` scrutinee resolves its type-name arm heads.
enum HeadMode<'a> {
    /// A union-variant value (`KObject::Wrapped` over a member `SetRef`): a head naming one of
    /// the scrutinee's own set members admits by member `SetRef` identity — the value is a
    /// specific variant, so only its own member name matches — and `it` binds the wrapped
    /// payload (`inner`, F3). A head that is not a member name falls back to scope resolution.
    WrappedMember {
        set: Rc<RecursiveSet<'a>>,
        index: usize,
    },
    /// A `TypeConstructor` value (`Result`): a head admits by tag-name equality against the
    /// value's own tag, and `it` binds the wrapped payload (F3).
    TaggedByTag { value_tag: String },
    /// Any other value: a head resolves through the scope and admits via
    /// [`KType::matches_value`]; `it` binds the scrutinee unchanged (F3).
    Scope,
}

/// Resolve a bare arm-head type token against the call-site scope — the same
/// [`Scope::resolve_type_identifier`] call [`resolve_arm_contract`] makes. A non-`Done`
/// resolution (parked or unbound) is not a synchronously-known type.
fn resolve_head_type<'a>(
    scope: &Scope<'a>,
    token: &TypeIdentifier,
    chain: Option<Rc<LexicalFrame>>,
) -> Result<KType<'a>, String> {
    match scope.resolve_type_identifier(token, chain) {
        TypeResolution::Done(hit) => Ok(hit.kt.clone()),
        _ => Err(format!(
            "match arm type `{}` is not a known type",
            token.render()
        )),
    }
}

/// `MATCH`'s arm selector (ruling F1 + F3). Classifies each `<head> -> <body>` triple, admits
/// the arms that match `scrutinee`, and returns the strictly most-specific admitting arm.
///
/// Head classification depends on the scrutinee ([`HeadMode`]):
/// - `true` / `false` literal heads admit a `Bool` scrutinee of that value.
/// - `Type(token)` heads over a union-variant value (`KObject::Wrapped` over a member `SetRef`)
///   naming one of the scrutinee's own set members admit by member `SetRef` identity (only the
///   value's own variant matches) and bind the payload; a non-member head resolves through `scope`.
/// - `Type(token)` heads over a `TypeConstructor` value (`Result`) admit by tag-name equality.
/// - `Type(token)` heads over any other value resolve through `scope` and admit via
///   [`KType::matches_value`].
///
/// `Ok(Some(arm))` selects an arm; `Ok(None)` means no arm admits (the caller raises the
/// inexhaustive error naming the runtime type); `Err` covers a malformed shape, an
/// unresolved head, or an F1 ambiguity (two admitting arms with no strict winner).
pub(crate) fn find_branch_body_by_type<'a>(
    branches: &KExpression<'a>,
    scrutinee: &KObject<'a>,
    scope: &Scope<'a>,
    chain: Option<Rc<LexicalFrame>>,
) -> Result<Option<SelectedArm<'a>>, String> {
    let parts = &branches.parts;
    if !parts.len().is_multiple_of(3) {
        return Err(format!(
            "branches must be `<head> -> <body>` triples; got {} parts (not a multiple of 3)",
            parts.len()
        ));
    }
    // A union-variant value (`Wrapped` over a member `SetRef`) resolves member-name heads against
    // its own set; a `TypeConstructor` value (`Result`) resolves them by tag string; any other
    // value resolves heads against the scope.
    let mode = match scrutinee {
        KObject::Wrapped {
            type_id: KType::SetRef { set, index },
            ..
        } => HeadMode::WrappedMember {
            set: Rc::clone(set),
            index: *index,
        },
        KObject::Tagged { tag, .. } => HeadMode::TaggedByTag {
            value_tag: tag.clone(),
        },
        _ => HeadMode::Scope,
    };

    struct Candidate<'a> {
        head_label: String,
        arm_type: ArmType<'a>,
        body: KExpression<'a>,
        /// A variant head binds the wrapped payload to `it` (F3); every other admitting head
        /// binds the scrutinee (or `Null`, for a boolean head).
        binds_payload: bool,
    }
    let mut candidates: Vec<Candidate<'a>> = Vec::new();

    let mut i = 0;
    while i < parts.len() {
        let head_part = &parts[i];
        let arrow_part = &parts[i + 1];
        let body_part = &parts[i + 2];

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

        match &head_part.value {
            // Booleans parse as `KLiteral::Boolean`; a head admits a `Bool` scrutinee of the
            // same value, binding `Null` to `it` (a boolean carries no payload).
            ExpressionPart::Literal(KLiteral::Boolean(b)) => {
                if matches!(scrutinee, KObject::Bool(sb) if sb == b) {
                    candidates.push(Candidate {
                        head_label: if *b { "true" } else { "false" }.to_string(),
                        arm_type: ArmType::Exact,
                        body: body_expr,
                        binds_payload: false,
                    });
                }
            }
            // A capitalized type name: a variant/tag match for a union-variant or tagged
            // scrutinee, else scope resolution.
            ExpressionPart::Type(token) => {
                let label = token.render();
                let admitting = match &mode {
                    // A head naming one of the scrutinee's own set members admits by member
                    // `SetRef` identity (only the value's own variant matches); `it` binds the
                    // payload. A non-member head resolves through the scope like any type arm.
                    HeadMode::WrappedMember { set, index } => match set.index_of(&label) {
                        Some(member_index) => {
                            // The value is a specific variant, so only its own member admits.
                            (member_index == *index).then(|| {
                                let member = KType::SetRef {
                                    set: Rc::clone(set),
                                    index: member_index,
                                };
                                (ArmType::Typed(member), true)
                            })
                        }
                        None => {
                            let kt = resolve_head_type(scope, token, chain.clone())?;
                            kt.matches_value(scrutinee)
                                .then_some((ArmType::Typed(kt), false))
                        }
                    },
                    HeadMode::TaggedByTag { value_tag } => {
                        (&label == value_tag).then_some((ArmType::Exact, true))
                    }
                    HeadMode::Scope => {
                        let kt = resolve_head_type(scope, token, chain.clone())?;
                        kt.matches_value(scrutinee)
                            .then_some((ArmType::Typed(kt), false))
                    }
                };
                if let Some((arm_type, binds_payload)) = admitting {
                    candidates.push(Candidate {
                        head_label: label,
                        arm_type,
                        body: body_expr,
                        binds_payload,
                    });
                }
            }
            other => {
                return Err(format!(
                    "branch head must be a capitalized type name or boolean literal, got {}",
                    other.summarize()
                ));
            }
        }
        i += 3;
    }

    if candidates.is_empty() {
        return Ok(None);
    }

    // F1 tournament: the winner is strictly more specific than every peer. `is_more_specific_than`
    // is a strict order, so at most one arm dominates all others; none dominating → ambiguity.
    let winner = candidates
        .iter()
        .enumerate()
        .find(|(i, cand)| {
            candidates
                .iter()
                .enumerate()
                .all(|(j, peer)| *i == j || arm_more_specific(&cand.arm_type, &peer.arm_type))
        })
        .map(|(i, _)| i);

    let Some(winner) = winner else {
        let heads: Vec<String> = candidates
            .iter()
            .map(|c| format!("`{}`", c.head_label))
            .collect();
        return Err(format!(
            "ambiguous match: value of type `{}` admits arms {} with no most-specific arm",
            scrutinee.ktype().name(),
            heads.join(", ")
        ));
    };

    let chosen = candidates
        .into_iter()
        .nth(winner)
        .expect("winner index valid");
    let it_value = if chosen.binds_payload {
        match scrutinee {
            KObject::Tagged { value, .. } => (**value).deep_clone(),
            KObject::Wrapped { inner, .. } => inner.get().deep_clone(),
            _ => scrutinee.deep_clone(),
        }
    } else {
        match &chosen.arm_type {
            ArmType::Exact => KObject::Null,
            ArmType::Typed(_) => scrutinee.deep_clone(),
        }
    };
    Ok(Some(SelectedArm {
        body: chosen.body,
        it_value,
    }))
}
