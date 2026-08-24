//! Branch walkers for `MATCH` and `TRY-WITH`, plus the shared arm-tail machinery.
//!
//! `TRY` selects an arm by **string tag** — [`find_branch_body_by_tag`] matches a
//! dispatched value's error/success tag and opts into wildcard `_` matching for
//! dispatcher-internal error kinds. `MATCH` selects an arm by **type** —
//! [`find_branch_body_by_type`] resolves each arm head to a `KType`, admits the arms
//! whose type matches the scrutinee value, and runs the most-specific-wins tournament
//! (ruling F1). [`resolve_arm_contract`] builds the `-> :T` return contract both arms
//! enforce on their result.

use crate::machine::model::labels::LabelInterner;
use crate::machine::model::{ExpressionPart, KExpression, KLiteral, TypeIdentifier};
use crate::machine::model::{Symbol, TypeSymbol, ValueSymbol};
use crate::machine::model::{TypeResolution, most_specific_ktype};

use crate::machine::DeliveredCarried;
use crate::machine::LexicalFrame;
use crate::machine::ReturnContract;
use crate::machine::model::RunRegistries;
use crate::machine::model::{Carried, CarriedFamily, KObject, KType};
use crate::machine::{KError, KErrorKind, Scope};
use std::rc::Rc;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { return_type } }

/// Read the MATCH / TRY `-> :T` slot from `ctx.args` (resolving a forward-referenced bare name
/// against the call-site scope/chain) into the [`ReturnContract::Arm`] both `MATCH` and `TRY`
/// arms are checked against.
pub(crate) fn resolve_arm_contract<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
    kind: &'static str,
) -> Result<ReturnContract<'a>, KError> {
    let ret_kt = if let Some(te) = ctx.args.unresolved_type(&SLOTS.return_type) {
        match ctx
            .scope
            .resolve_type_identifier(te, ctx.chain.clone(), ctx.registries)
        {
            TypeResolution::Done(kt) => kt,
            // The builtin fallback is already tried inside `resolve_type_identifier`; a
            // non-`Done` arm here (parked or unbound) is not a synchronously-known type.
            _ => {
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "{kind} return type `{}` is not a known type",
                    te.render()
                ))));
            }
        }
    } else {
        match ctx.args.ktype(&SLOTS.return_type) {
            Some(other) => other,
            None => {
                return Err(KError::new(KErrorKind::MissingArg(
                    "return_type".to_string(),
                )));
            }
        }
    };
    Ok(ReturnContract::Arm { ret: ret_kt, kind })
}

/// Narrow `carrier` onto the payload of a `Tagged` / `Wrapped` value (ruling F3's variant/tag arm),
/// by **parting** the payload cell from its container: the cell comes out bundled with exactly its
/// own run's stored reach — read off the run, never a subset walk over the container — and
/// [`Opened::lift_out`](crate::witnessed::Opened::lift_out), the relocation seam, turns that run
/// into owned coverage: its members plus the region the payload lives in, and nothing else. The
/// arm's `it` binding therefore names what the payload reaches instead of everything the scrutinee
/// did. A value with no payload keeps its own envelope.
pub(crate) fn payload_envelope(carrier: &DeliveredCarried) -> DeliveredCarried {
    // The open borrows the envelope's own pins, which cover both the read and the lift's upgrade.
    let opened = carrier.open_at();
    let parted = match opened.value().object() {
        KObject::Wrapped { inner, .. } => inner.project(0),
        KObject::Tagged { value, .. } => value.project(0),
        _ => None,
    };
    match parted {
        Some(cell) => cell.lift_out().project::<CarriedFamily>(|payload, _token| {
            Carried::Object(
                payload
                    .as_object()
                    .expect("a payload substrate's single cell is always an object"),
            )
        }),
        None => carrier.duplicate(),
    }
}

/// Build the matched-arm tail shared by the `Action`-harness `MATCH` and `TRY` bodies: the
/// [`block_tail`](crate::machine::block_tail) configuration for an arm — a fresh per-call frame
/// (`root`-rooted, chained onto `outer_frame`) whose own scope is the block, seeded with `it` bound
/// at idx 0 from `it_carrier`, running the arm body split into leading statements + a tail under
/// `contract`.
///
/// `it_carrier` is the delivery envelope for exactly what `it` binds — the scrutinee itself, or its
/// payload already narrowed by [`payload_envelope`]. An envelope is what the seed's `for<'b>` brand
/// admits: a bare caller-`'a` value names a lifetime the opened arm scope has no relation to, while
/// an envelope crosses as a witnessed shortening. A region-pure scrutinee (no carrier of its own)
/// is enveloped at the read site through
/// [`Scope::deliver_pure_value`](crate::machine::core::Scope::deliver_pure_value) before it gets
/// here, so there is one `it` tier rather than two.
pub(crate) fn arm_tail<'a>(
    root: &'a Scope<'a>,
    it_carrier: crate::machine::DeliveredCarried,
    body_expr: KExpression<'a>,
    contract: ReturnContract<'a>,
    registries: &RunRegistries,
) -> crate::machine::Action<'a> {
    use crate::machine::FramePlacement;
    use crate::machine::{BindingIndex, CallFrame};
    use crate::machine::{BlockBody, BlockScope, BlockSeed, block_tail};
    let frame: Rc<CallFrame> = CallFrame::new(root);
    let seed: BlockSeed<'a> = Box::new(move |child, registries, gate| {
        // Fused copy + bind of `it` at idx 0 in the fresh arm frame: one structural copy made
        // directly into the arm frame's region inside the envelope's pinned open, the binding
        // storing the copy's derived reach (a residence-only host is dropped, so a tail loop's
        // retiring frame does not ride the arm's binding). The projection is identity — the
        // envelope already names exactly what `it` binds — and a later read of `it` rebuilds its
        // carrier from the stored reach.
        let it = ValueSymbol::declared("it", &registries.labels).expect("`it` is a value token");
        let _ = child.bind_delivered_direct(
            it,
            &it_carrier,
            BindingIndex::value(0),
            |carried| Ok(carried.object()),
            registries,
            gate,
        );
    });
    block_tail(
        root.brand(),
        FramePlacement::FreshChild { frame },
        BlockScope::FrameOwn,
        Some(seed),
        BlockBody::Block(body_expr),
        Some(contract),
        registries,
    )
}

/// `TRY`'s arm selector: returns the body for the first triple whose tag matches
/// `target_tag`, or — when `allow_wildcard` is true and no exact match was found — the
/// first `_` body. Exact-tag matches always win over `_`, regardless of source order.
///
/// `target_tag` is the scrutinee's classified tag; an arm head is a reference, so it probes by
/// bare symbol bits — the recovery door. A boolean-literal head classifies the same way and simply
/// never carries a Type token's bits, so it can only match a boolean target.
pub(crate) fn find_branch_body_by_tag<'a>(
    branches: &KExpression<'a>,
    target_tag: Symbol,
    allow_wildcard: bool,
    labels: &LabelInterner,
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
        // `None` is the wildcard arm, which matches nothing by name and is remembered instead.
        let arm_tag: Option<Symbol> = match tag_part.value {
            // Variant tags are capitalized type names (`Some`, `Ok`, `TypeMismatch`).
            ExpressionPart::Type(t) => Some(Symbol::of(t.as_str())),
            // Booleans parse as `KLiteral::Boolean`, not type tokens; accept them so
            // `MATCH` on a `Bool` can spell its arms `true ->` / `false ->`.
            ExpressionPart::Literal(KLiteral::Boolean(b)) => {
                Some(Symbol::of(if b { "true" } else { "false" }))
            }
            // `_` is a pure-symbol token classified as `Keyword`, not a type name.
            ExpressionPart::Keyword(kw) if allow_wildcard && kw.text() == "_" => None,
            other => {
                return Err(format!(
                    "branch tag must be a capitalized variant tag or boolean literal, got {}",
                    other.summarize(labels)
                ));
            }
        };
        match arrow_part.value {
            ExpressionPart::Keyword(kw) if kw.text() == "->" => {}
            other => {
                return Err(format!(
                    "branch separator must be `->`, got {}",
                    other.summarize(labels)
                ));
            }
        }
        let body_expr = match body_part.value {
            ExpressionPart::Expression(e) => *e,
            other => {
                return Err(format!(
                    "branch body must be a parenthesized expression, got {}",
                    other.summarize(labels)
                ));
            }
        };
        match arm_tag {
            Some(tag) if tag == target_tag => return Ok(Some(body_expr)),
            Some(_) => {}
            None if wildcard_body.is_none() => wildcard_body = Some(body_expr),
            None => {}
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
enum HeadMode {
    /// A tagged value (a user-`UNION` variant or a builtin `Result`, both `KObject::Tagged`): a
    /// head admits by symbol equality against the value's own tag, and `it` binds the wrapped
    /// payload (F3). A union's sibling variants need no resolution — the value carries its own tag,
    /// so a non-matching head is a silent non-match, and the arm slate settles exhaustiveness.
    TaggedByTag { value_tag: TypeSymbol },
    /// Any other value: a head resolves through the scope and admits via
    /// [`KType::matches_value`]; `it` binds the scrutinee unchanged (F3).
    Scope,
}

/// Resolve a bare arm-head type token against the call-site scope — the same
/// [`Scope::resolve_type_identifier`] call [`resolve_arm_contract`] makes. A non-`Done`
/// resolution (parked or unbound) is not a synchronously-known type.
fn resolve_head_type<'a>(
    scope: &Scope<'a>,
    token: &TypeIdentifier<'a>,
    chain: Option<Rc<LexicalFrame>>,
    registries: &RunRegistries,
) -> Result<KType, String> {
    match scope.resolve_type_identifier(token, chain, registries) {
        TypeResolution::Done(kt) => Ok(kt),
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
/// - `Type(token)` heads over a tagged value (a user-`UNION` variant or a builtin `Result`, both
///   `KObject::Tagged`) admit by symbol equality against the value's own tag and bind the payload;
///   a non-matching head is a silent non-match (the value carries its own tag).
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
    registries: &RunRegistries,
) -> Result<Option<SelectedArm<'a>>, String> {
    let parts = &branches.parts;
    if !parts.len().is_multiple_of(3) {
        return Err(format!(
            "branches must be `<head> -> <body>` triples; got {} parts (not a multiple of 3)",
            parts.len()
        ));
    }
    // A tagged value (a user-`UNION` variant or a builtin `Result`, both `Tagged`) resolves
    // member-name heads by its own tag symbol; any other value — including a `NEWTYPE (T AS W)`
    // identity wrapper or a standalone newtype — resolves heads against the scope.
    let mode = match scrutinee {
        KObject::Tagged { tag, .. } => HeadMode::TaggedByTag { value_tag: *tag },
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
        ktype: KType,
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

        match arrow_part.value {
            ExpressionPart::Keyword(kw) if kw.text() == "->" => {}
            other => {
                return Err(format!(
                    "branch separator must be `->`, got {}",
                    other.summarize(&registries.labels)
                ));
            }
        }
        let body_expr = match body_part.value {
            ExpressionPart::Expression(e) => *e,
            other => {
                return Err(format!(
                    "branch body must be a parenthesized expression, got {}",
                    other.summarize(&registries.labels)
                ));
            }
        };

        match head_part.value {
            // Booleans parse as `KLiteral::Boolean`; a head is an exact arm admitting a `Bool`
            // scrutinee of the same value, binding `Null` to `it` (a boolean carries no payload).
            ExpressionPart::Literal(KLiteral::Boolean(b)) => {
                if matches!(scrutinee, KObject::Bool(sb) if *sb == b) {
                    exact_arms.push(ExactArm {
                        head_label: if b { "true" } else { "false" }.to_string(),
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
                    // A tag head equal to the scrutinee's own tag is an exact arm binding the
                    // payload; a non-tag head is a silent non-match (no scope resolution for a
                    // `Tagged` scrutinee).
                    HeadMode::TaggedByTag { value_tag } => {
                        // The head is a reference, not a declaration, so it probes the
                        // scrutinee's classified tag by bare symbol bits — the recovery door.
                        if Symbol::of(token.as_str()) == value_tag.symbol() {
                            exact_arms.push(ExactArm {
                                head_label: label,
                                body: body_expr,
                                binds_payload: true,
                            });
                        }
                    }
                    HeadMode::Scope => {
                        let kt = resolve_head_type(scope, &token, chain.clone(), registries)?;
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
                    other.summarize(&registries.labels)
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
            scrutinee.ktype().name(registries),
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
    // `most_specific_ktype` pick the strictly most-specific admitting arm — the one-slot case of
    // the same tournament ordinary overload buckets resolve through, where specificity turns
    // entirely on the head's own `KType`.
    let admitted: Vec<TypedArm<'a>> = typed_arms
        .into_iter()
        .filter(|arm| arm.ktype.matches_value(scrutinee, registries))
        .collect();
    if admitted.is_empty() {
        return Ok(None);
    }
    let heads: Vec<KType> = admitted.iter().map(|arm| arm.ktype).collect();
    match most_specific_ktype(&heads, registries) {
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
                scrutinee.ktype().name(registries),
                heads.join(", ")
            ))
        }
    }
}
