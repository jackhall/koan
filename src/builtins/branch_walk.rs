//! Branch walkers for `MATCH` and `TRY-WITH`, plus the shared arm-tail machinery.
//!
//! `TRY` selects an arm by **string tag** — [`find_branch_body_by_tag`] matches a
//! dispatched value's error/success tag and opts into wildcard `_` matching for
//! dispatcher-internal error kinds. `MATCH` selects an arm by **type** —
//! [`find_branch_body_by_type`] resolves each arm head to a `KType`, admits the arms
//! whose type matches the scrutinee value, and runs the most-specific-wins tournament
//! (ruling F1). [`resolve_arm_contract`] builds the `-> :T` return contract both arms
//! enforce on their result.

use super::{arg, sig};
use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::core::LexicalFrame;
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral, TypeIdentifier};
use crate::machine::model::types::{ExpressionSignature, RecursiveSet, TypeResolution};
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
        // A region-free return type takes the compile-enforced `'static` tier; one embedding a
        // scope borrow (a `Signature`, a module-sourced abstract) cannot rebuild at `'static` and
        // takes the runtime-checked seal.
        ret: match ret_kt.to_static() {
            Some(owned) => ctx.scope.brand().alloc_ktype(owned),
            None => ctx.scope.brand().alloc_ktype_checked(ret_kt)?,
        },
        kind,
        scope: ctx.scope,
    })
}

/// Which part of a carrier's carried value the arm's `it` binds.
pub(crate) enum ItProjection {
    /// `it` binds the carried value itself — `TRY`'s success arm, and a general-type `MATCH` arm.
    Scrutinee,
    /// `it` binds the carried value's wrapped payload — a variant/tag `MATCH` arm (ruling F3).
    Payload,
}

/// How the matched scrutinee reaches the arm's `it` binding.
pub(crate) enum ItSource<'a> {
    /// A region-pure owned value — `TRY`'s error payload, `MATCH`'s region-pure scrutinee (or its
    /// payload), and a boolean arm's `Null`. No carrier, no foreign reach: the copy's purity is an
    /// audit at bind time.
    Pure(crate::machine::model::KObject<'a>),
    /// The delivery envelope plus which part of its carried value `it` binds. Cloned once, directly
    /// into the arm frame at bind time; the envelope's retained host pins the producer until then
    /// and supplies the binding's stored reach.
    Carrier(crate::machine::DeliveredCarried, ItProjection),
}

/// Build the matched-arm tail shared by the `Action`-harness `MATCH` and `TRY` bodies: the
/// [`block_tail`](super::block_tail::block_tail) configuration for an arm — a fresh per-call frame
/// (`root`-rooted, chained onto `outer_frame`) whose own scope is the block, seeded with `it` bound
/// at idx 0 from `it_source`, running the arm body split into leading statements + a tail under
/// `contract`.
pub(crate) fn arm_tail<'a>(
    root: &'a Scope<'a>,
    it_source: ItSource<'a>,
    body_expr: KExpression<'a>,
    contract: ReturnContract<'a>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use super::block_tail::{block_tail, BlockBody, BlockScope, BlockSeed};
    use crate::machine::core::kfunction::action::FramePlacement;
    use crate::machine::{BindingIndex, CallFrame};
    let frame: Rc<CallFrame> = CallFrame::new(root);
    // Bind `it` into the frame's own scope: `alloc_object` erases the caller-`'a` input and
    // re-homes it at the frame region, so no pre-shortening is needed. Either source is a deep copy
    // living in the arm frame, so the stored reach is the copy's (`adopted_reach_of` — a
    // residence-only host is not carried; a tail loop's retiring frame must not ride the arm's
    // binding), and a later read of `it` rebuilds its carrier from it.
    let seed: BlockSeed<'a> = Box::new(move |child| {
        let (it_object, reach) = match it_source {
            // A region-pure value reaches nothing; its purity is an audit at the bind brand.
            ItSource::Pure(value) => {
                let object = child
                    .brand()
                    .alloc_object_checked(value)
                    .expect("ItSource::Pure must be region-pure");
                (object, Default::default())
            }
            ItSource::Carrier(carrier, projection) => {
                // Adopt at the bind brand: one structural copy, made directly into the arm frame's
                // region inside the envelope's pinned open; the binding stores the copy's reach,
                // minted first so the copy's own residence audit can see it. The projection selects
                // which sub-object of the carried value feeds that copy — the payload lives inside
                // the carried value, so its reach is a subset of the envelope's, still covered by
                // `adopted_reach_of`.
                let reach = child.adopted_reach_of(&carrier);
                let object = carrier.open(|live| {
                    let source = match projection {
                        ItProjection::Scrutinee => live.object(),
                        ItProjection::Payload => match live.object() {
                            KObject::Wrapped { inner, .. } => inner.get(),
                            KObject::Tagged { value, .. } => &**value,
                            other => other,
                        },
                    };
                    child
                        .alloc_object_delivered(source.deep_clone(), std::slice::from_ref(&reach))
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
/// whether `it` binds the scrutinee's wrapped payload (ruling F3) rather than the scrutinee
/// itself. A variant/tag arm sets `binds_payload`; a general type arm and a boolean arm clear it.
pub(crate) struct SelectedArm<'a> {
    pub body: KExpression<'a>,
    pub binds_payload: bool,
}

/// How a `MATCH` scrutinee resolves its type-name arm heads.
enum HeadMode<'a> {
    /// A union-variant value (`KObject::Wrapped` over a member `SetRef`): a head naming one of
    /// the scrutinee's own set members resolves to that member `SetRef`, which admits only a
    /// `Wrapped` of that exact identity — the value is a specific variant, so only its own member
    /// name matches — and `it` binds the wrapped payload (`inner`, F3). A head that is not a member
    /// name falls back to scope resolution.
    WrappedMember { set: Rc<RecursiveSet<'a>> },
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
            type_id: KType::SetRef { set, .. },
            ..
        } => HeadMode::WrappedMember {
            set: Rc::clone(set),
        },
        KObject::Tagged { tag, .. } => HeadMode::TaggedByTag {
            value_tag: tag.clone(),
        },
        _ => HeadMode::Scope,
    };

    // An exact arm is a boolean-literal head admitting a `Bool` scrutinee of that value, or a
    // tag head equal to a `Tagged` scrutinee's own tag. An exact arm ranks strictly above every
    // typed arm, so the pre-pass below settles it without entering the tournament.
    struct ExactArm<'a> {
        head_label: String,
        body: KExpression<'a>,
        binds_payload: bool,
    }
    // A typed arm carries the `KType` its head resolved to; the tournament admits it by
    // `matches_value` and ranks admitted arms by `most_specific`.
    struct TypedArm<'a> {
        head_label: String,
        ktype: KType<'a>,
        body: KExpression<'a>,
        binds_payload: bool,
    }
    let mut exact_arms: Vec<ExactArm<'a>> = Vec::new();
    let mut typed_arms: Vec<TypedArm<'a>> = Vec::new();

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
            // Booleans parse as `KLiteral::Boolean`; a head is an exact arm admitting a `Bool`
            // scrutinee of the same value, binding `Null` to `it` (a boolean carries no payload).
            ExpressionPart::Literal(KLiteral::Boolean(b)) => {
                if matches!(scrutinee, KObject::Bool(sb) if sb == b) {
                    exact_arms.push(ExactArm {
                        head_label: if *b { "true" } else { "false" }.to_string(),
                        body: body_expr,
                        binds_payload: false,
                    });
                }
            }
            // A capitalized type name: a variant/tag match for a union-variant or tagged
            // scrutinee, else scope resolution.
            ExpressionPart::Type(token) => {
                let label = token.render();
                match &mode {
                    // A head naming one of the scrutinee's own set members resolves to that
                    // member `SetRef` and enters the tournament; the member `SetRef` admits only a
                    // `Wrapped` of that exact identity, so only the value's own variant matches
                    // (`matches_value`), and `it` binds the payload. A non-member head resolves
                    // through the scope like any type arm; failing that, the error names the
                    // scrutinee's variants.
                    HeadMode::WrappedMember { set } => match set.index_of(&label) {
                        Some(member_index) => {
                            typed_arms.push(TypedArm {
                                head_label: label,
                                ktype: KType::SetRef {
                                    set: Rc::clone(set),
                                    index: member_index,
                                },
                                body: body_expr,
                                binds_payload: true,
                            });
                        }
                        None => {
                            let kt = match resolve_head_type(scope, token, chain.clone()) {
                                Ok(kt) => kt,
                                Err(_) => {
                                    let variants: Vec<String> = set
                                        .members()
                                        .iter()
                                        .map(|m| format!("`{}`", m.name))
                                        .collect();
                                    return Err(format!(
                                        "match arm type `{}` is not a known type; the scrutinee's \
                                         union variants are {}",
                                        token.render(),
                                        variants.join(", ")
                                    ));
                                }
                            };
                            typed_arms.push(TypedArm {
                                head_label: label,
                                ktype: kt,
                                body: body_expr,
                                binds_payload: false,
                            });
                        }
                    },
                    // A tag head equal to the scrutinee's own tag is an exact arm binding the
                    // payload; a non-tag head is a silent non-match (no scope resolution for a
                    // `Tagged` scrutinee).
                    HeadMode::TaggedByTag { value_tag } => {
                        if &label == value_tag {
                            exact_arms.push(ExactArm {
                                head_label: label,
                                body: body_expr,
                                binds_payload: true,
                            });
                        }
                    }
                    HeadMode::Scope => {
                        let kt = resolve_head_type(scope, token, chain.clone())?;
                        typed_arms.push(TypedArm {
                            head_label: label,
                            ktype: kt,
                            body: body_expr,
                            binds_payload: false,
                        });
                    }
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

    // Exact pre-pass: an exact arm ranks strictly above every typed arm. Two admitting exact
    // heads have no strict winner → ambiguity; exactly one wins outright and skips the tournament.
    if exact_arms.len() >= 2 {
        let heads: Vec<String> = exact_arms
            .iter()
            .map(|a| format!("`{}`", a.head_label))
            .collect();
        return Err(format!(
            "ambiguous match: value of type `{}` admits arms {} with no most-specific arm",
            scrutinee.ktype().name(),
            heads.join(", ")
        ));
    }
    if let Some(arm) = exact_arms.into_iter().next() {
        return Ok(Some(SelectedArm {
            body: arm.body,
            binds_payload: arm.binds_payload,
        }));
    }

    // Typed tournament via the shared core: admit by `matches_value`, then let
    // `ExpressionSignature::most_specific` pick the strictly most-specific admitting arm — the
    // same filter→`most_specific` core ordinary overload buckets resolve through. Each arm lowers
    // to a one-slot signature whose only argument carries the head's `KType`, so specificity turns
    // entirely on that type.
    let admitted: Vec<TypedArm<'a>> = typed_arms
        .into_iter()
        .filter(|arm| arm.ktype.matches_value(scrutinee))
        .collect();
    if admitted.is_empty() {
        return Ok(None);
    }
    let sigs: Vec<ExpressionSignature<'a>> = admitted
        .iter()
        .map(|arm| sig(KType::Any, vec![arg("it", arm.ktype.clone())]))
        .collect();
    let refs: Vec<&ExpressionSignature<'a>> = sigs.iter().collect();
    match ExpressionSignature::most_specific(&refs) {
        Some(winner) => {
            let arm = admitted
                .into_iter()
                .nth(winner)
                .expect("winner index valid");
            Ok(Some(SelectedArm {
                body: arm.body,
                binds_payload: arm.binds_payload,
            }))
        }
        None => {
            let heads: Vec<String> = admitted
                .iter()
                .map(|arm| format!("`{}`", arm.head_label))
                .collect();
            Err(format!(
                "ambiguous match: value of type `{}` admits arms {} with no most-specific arm",
                scrutinee.ktype().name(),
                heads.join(", ")
            ))
        }
    }
}
