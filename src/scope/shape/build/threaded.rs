//! The threaded set of a callable's, a code body's or a tail arm's last statement: what a tail call
//! it makes hands on that its frame built itself, so the frame builds it where it outlives the hop
//! rather than in a region the next hop reclaims
//! ([`BodyShape::threaded`](super::super::BodyShape::threaded)). Read off a draft at its seal, beside
//! `last_reads`.
//!
//! A value the frame received — a parameter, a capture, a binder bound bare to one, or a member read
//! of one through the builtin `ATTR` alone — is no member: it rests where it was handed from, and a
//! hop hands it on from there. A tail arm's `it` is received where its `MATCH`'s scrutinee is; one
//! that is not is built by the `MATCH`, so an arm handing it on threads the scrutinee in the body
//! holding the `MATCH`. An arm reads any other name of the bodies around it as received: the body
//! it lands in reads it in its own last statement, so its own set holds it where it is built.

use super::{
    BumpAllocator, BumpVec, Candidate, Coordinate, Draft, ExpressionPart, KExpression, Lands,
    Position, Role, Site, Slot,
};
use crate::parse::BuiltinShapeId;

/// The threaded set of `draft`, the body at level `this` of the chain its mentions land on: the
/// binders in the backward closure over its read edges from its last statement's reads, and its
/// last statement's argument parts that are no name and no literal — and, where `threads_it` says
/// a tail arm of it hands on an `it` it built, its `MATCH`'s scrutinee — each less what the frame
/// received, both sorted. Empty for any body but a callable's, a code body or a tail arm.
pub(super) fn threaded<'x>(
    draft: &Draft<'_, 'x>,
    this: usize,
    threads_it: bool,
    scratch: BumpAllocator<'x>,
) -> (BumpVec<'x, Slot>, BumpVec<'x, Site>) {
    let mut slots = BumpVec::new_in(scratch);
    let mut parts = BumpVec::new_in(scratch);
    let Some(last) = draft.statements.checked_sub(1) else {
        return (slots, parts);
    };
    if !draft.tail {
        return (slots, parts);
    }
    // A type slot and a parameter are never built by the frame, save an arm's `it` it built.
    let built = |slot: Slot| {
        !draft.is_type_slot(slot)
            && (draft.channels().get(slot.index()) != Position::PARAMETER
                || !received_binder(draft, this, slot))
    };
    let mut pending = BumpVec::new_in(scratch);
    pending.extend(
        (draft.reads.iter())
            .filter(|(statement, slot)| *statement == last && built(*slot))
            .map(|(_, slot)| *slot),
    );
    while let Some(slot) = pending.pop() {
        if slots.contains(&slot) || received_binder(draft, this, slot) {
            continue;
        }
        slots.push(slot);
        pending.extend(
            (draft.edges.iter())
                .filter(|(binder, bound, _)| *binder == slot && built(*bound))
                .map(|(_, bound, _)| *bound),
        );
    }
    slots.sort_unstable();
    let node = &draft.body[last as usize];
    let matched = (node.cache().builtin_shape()).is_some_and(|shape| {
        matches!(shape.id, BuiltinShapeId::Match | BuiltinShapeId::MatchUnder)
    });
    let scrutinee = (threads_it && matched).then(|| &node.parts[1].value);
    for part in handed_on(draft, node, scratch).into_iter().chain(scrutinee) {
        let named = part.is_name() || matches!(part, ExpressionPart::Literal(_));
        if !named && !received(draft, this, part) {
            parts.push(draft.sites.site(part));
        }
    }
    parts.sort_unstable();
    (slots, parts)
}

/// The parts of `node`, a statement, a tail call of it hands on: a keyworded use's slots, a label
/// written bare excluded, where some candidate is no builtin; an application's argument, or each
/// field of a record literal written there. None for anything else.
fn handed_on<'graph, 'x>(
    draft: &Draft<'graph, 'x>,
    node: &'graph KExpression<'graph>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, &'graph ExpressionPart<'graph>> {
    let mut parts = BumpVec::new_in(scratch);
    let site = draft.sites.site_of_node(node);
    if let Some((_, list)) = draft.candidates.iter().find(|(at, _)| *at == site) {
        let builtin =
            |candidate: &Candidate| matches!(candidate, Candidate::One(Coordinate::Builtin(_)));
        if list.candidates.iter().all(builtin) {
            return parts;
        }
        match node.cache().builtin_shape() {
            Some(shape) => parts.extend(
                (shape.roles().zip(node.parts))
                    .filter(|(role, part)| {
                        *role != Role::Keyword && role.label_reads(&part.value).is_none()
                    })
                    .map(|(_, part)| &part.value),
            ),
            None => parts.extend(
                (node.parts.iter())
                    .filter(|part| !matches!(part.value, ExpressionPart::Keyword(_)))
                    .map(|part| &part.value),
            ),
        }
        return parts;
    }
    let keyword = |part: &ExpressionPart<'_>| matches!(part, ExpressionPart::Keyword(_));
    if let [head, argument] = node.parts
        && !keyword(&head.value)
        && !keyword(&argument.value)
    {
        match &argument.value {
            ExpressionPart::RecordLiteral(fields) => {
                parts.extend(fields.iter().map(|(_, value)| value));
            }
            argument => parts.push(argument),
        }
    }
    parts
}

/// Whether `part` is a value the frame body at level `this` received: a name landing on one of its
/// parameters, a capture, a builtin or a binder of a body around it, or on a binder of its own
/// bound bare to such a value; or a member read of such a value through the builtin `ATTR` alone.
pub(super) fn received(draft: &Draft<'_, '_>, this: usize, part: &ExpressionPart<'_>) -> bool {
    match part {
        ExpressionPart::Expression(node) | ExpressionPart::MarkedUse(_, node) => {
            let Some(attribute) = node.cache().builtin_shape() else {
                return false;
            };
            if attribute.id != BuiltinShapeId::Attribute {
                return false;
            }
            let [_, operand, _] = node.parts else {
                return false;
            };
            let bare = (attribute.roles().zip(node.parts))
                .any(|(role, written)| role.label_reads(&written.value).is_some());
            let site = draft.sites.site_of_node(node);
            let builtins = (draft.candidates.iter())
                .find(|(at, _)| *at == site)
                .is_some_and(|(_, list)| {
                    (list.candidates.iter()).all(|candidate| {
                        matches!(candidate, Candidate::One(Coordinate::Builtin(_)))
                    })
                });
            bare && builtins && received(draft, this, &operand.value)
        }
        part if part.is_name() => {
            let site = draft.sites.site(part);
            let Some(mention) = (draft.mentions.iter()).find(|mention| mention.site == site) else {
                return false;
            };
            match mention.landing {
                Lands::Binder { at, slot } if at == this => {
                    // A fellow read is the cyclic rule's, and is no value received.
                    let fellow = mention.read.is_some_and(|(statement, across)| {
                        !across
                            && (draft.statement_binders[statement as usize].first()).is_some_and(
                                |binder| {
                                    draft.component_of[binder.index()]
                                        == draft.component_of[slot.index()]
                                },
                            )
                    });
                    !fellow && received_binder(draft, this, slot)
                }
                _ => true,
            }
        }
        _ => false,
    }
}

/// Whether the binder at `slot` of the body at level `this` holds a value it received: a parameter
/// — an arm's `it` where its scrutinee is received — or a binder bound bare to a received value.
fn received_binder(draft: &Draft<'_, '_>, this: usize, slot: Slot) -> bool {
    if draft.channels().get(slot.index()) == Position::PARAMETER {
        return draft.arm.is_none() || draft.it_received;
    }
    (draft.binders.get(slot.index()))
        .and_then(|binder| binder.rhs)
        .is_some_and(|rhs| rhs.is_name() && received(draft, this, rhs))
}
