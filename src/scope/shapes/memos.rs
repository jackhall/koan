//! Where a content digest's memos sit, read off the text
//! ([where a digest is demanded](../../../design/modules.md#a-module-is-its-content)): the slots
//! that keep one, each `:|` that reads them, and each call that hands one on.
//!
//! A `:|` over a module binder read bare demands the module's content, and each capture that
//! content composes that the module read from a slot of the view's own activation chain is a value
//! that activation may digest again. So that slot is **digested**: it keeps a memo beside its
//! value, which the `:|` fills and reads ([`ViewSlots`]). A call that passes a digested slot bare —
//! a call by name's record field, or a keyworded use's slot — hands its memo ([`Handoff`]) to the
//! parameter it binds. Every hand-off is found, whatever order the calls and views are written in;
//! one to a callee that keeps no memo there is dropped where the call runs.

use crate::memory::{BumpAllocator, BumpVec, Writer, collect};
use crate::parse::{BuiltinShapeId, ExpressionPart, KExpression};

use super::super::keyed::Keyed;
use super::super::shape::{
    BodyShape, Candidate, CaptureSlot, CaptureSource, Coordinate, ShapeKind, Site, Slot, Target,
};
use super::super::typed::{HandedTo, Handoff, ViewSlots};
use super::Shapes;
use super::tails::{by_name_parts, evaluated, name_of};

/// What the memo facts give one body.
#[derive(Clone, Copy)]
pub(super) struct Memos<'graph> {
    /// The slots that keep a digest memo, sorted.
    pub(super) digested: &'graph [Slot],
    /// Each `:|` over a module binder read bare, by its node's site, beside where the captures of
    /// the module it views keep their memos.
    pub(super) views: Keyed<'graph, (Site, ViewSlots<'graph>)>,
    /// Each call passing a digested slot bare, by its argument's site for a call by name and its
    /// node's for a keyworded use, beside what it hands on.
    pub(super) handoffs: Keyed<'graph, (Site, &'graph [Handoff])>,
}

impl Default for Memos<'_> {
    fn default() -> Self {
        Memos {
            digested: &[],
            views: Keyed::from_sorted(&[]),
            handoffs: Keyed::from_sorted(&[]),
        }
    }
}

/// The memo facts of every body of `shapes`, by segment.
pub(super) fn memos<'graph, 'x>(
    writer: Writer<'graph, '_>,
    scratch: BumpAllocator<'x>,
    shapes: &Shapes<'graph>,
) -> BumpVec<'x, Memos<'graph>> {
    let bodies = shapes.bodies();
    let mut digested: BumpVec<'_, BumpVec<'_, Slot>> =
        BumpVec::with_capacity_in(bodies.len(), scratch);
    digested.resize_with(bodies.len(), || BumpVec::new_in(scratch));
    let mut views = BumpVec::with_capacity_in(bodies.len(), scratch);
    // Each body's own nodes, walked once: its hand-offs read every body's digests after.
    let mut nodes = BumpVec::with_capacity_in(bodies.len(), scratch);
    for shape in bodies {
        let mut viewed = BumpVec::new_in(scratch);
        let mut own = BumpVec::new_in(scratch);
        own_nodes(shape, &mut |node| {
            own.push(node);
            let opaque = node
                .cache()
                .builtin_shape()
                .is_some_and(|builtin| builtin.id == BuiltinShapeId::AscribeOpaque);
            if !opaque {
                return;
            }
            let [operand, _, _] = node.parts else {
                return;
            };
            if let Some(slots) = view_slots(shapes, shape, &operand.value, scratch, &mut digested) {
                viewed.push((shape.site_of_node(node), slots));
            }
        });
        views.push(viewed);
        nodes.push(own.leak());
    }
    for slots in digested.iter_mut() {
        slots.sort_unstable();
        slots.dedup();
    }
    let mut laid = BumpVec::with_capacity_in(bodies.len(), scratch);
    for ((shape, viewed), own) in bodies.iter().zip(views.iter_mut()).zip(nodes.iter()) {
        let mut handoffs = BumpVec::new_in(scratch);
        for node in own.iter() {
            handed(shapes, shape, node, &digested, scratch, &mut handoffs);
        }
        let mut laid_views = BumpVec::with_capacity_in(viewed.len(), scratch);
        laid_views.extend(viewed.iter().map(|(site, slots)| {
            let captures = collect(writer, slots.captures.iter().copied());
            (
                *site,
                ViewSlots {
                    body: slots.body,
                    captures,
                },
            )
        }));
        let mut laid_handoffs = BumpVec::with_capacity_in(handoffs.len(), scratch);
        laid_handoffs.extend(
            (handoffs.iter()).map(|(site, handed): &(Site, &[Handoff])| {
                (*site, collect(writer, handed.iter().copied()))
            }),
        );
        laid.push(Memos {
            digested: collect(
                writer,
                digested[shape.segment().index() as usize].iter().copied(),
            ),
            views: Keyed::sorted(writer, &mut laid_views),
            handoffs: Keyed::sorted(writer, &mut laid_handoffs),
        });
    }
    laid
}

/// Where the captures of the module the `:|` over `operand`, in `shape`, views keep their memos,
/// where `operand` is a module binder of the activation chain read bare; each capture's source slot
/// is marked in `digested`.
fn view_slots<'x>(
    shapes: &Shapes<'_>,
    shape: &BodyShape<'_>,
    operand: &ExpressionPart<'_>,
    scratch: BumpAllocator<'x>,
    digested: &mut BumpVec<'_, BumpVec<'_, Slot>>,
) -> Option<ViewSlots<'x>> {
    let (hops, slot) = local_read(shape, operand)?;
    let born = shapes.up(shape.segment(), hops)?;
    let body =
        (shapes.shape_at(born).births(slot)).filter(|body| body.kind() == ShapeKind::Module)?;
    let mut captures = BumpVec::new_in(scratch);
    for (index, capture) in body.captures().iter().enumerate() {
        let CaptureSource::Read(Coordinate::Activation {
            hops: out,
            target: Target::Local(source),
        }) = capture.source
        else {
            continue;
        };
        if !body.composes(CaptureSlot(index as u32)) {
            continue;
        }
        let Some(at) = shapes.up(born, out) else {
            continue;
        };
        captures.push((
            index as u32,
            Coordinate::Activation {
                hops: hops + out,
                target: Target::Local(source),
            },
        ));
        digested[at.index() as usize].push(source);
    }
    if captures.is_empty() {
        return None;
    }
    Some(ViewSlots {
        body: body.segment(),
        captures: scratch.alloc_slice_copy(&captures),
    })
}

/// Push each hand-off the call `node`, in `shape`, makes onto `handoffs`: each argument that is a
/// bare read of a digested slot of the activation chain.
fn handed<'x>(
    shapes: &Shapes<'_>,
    shape: &BodyShape<'_>,
    node: &KExpression<'_>,
    digested: &[BumpVec<'_, Slot>],
    scratch: BumpAllocator<'x>,
    handoffs: &mut BumpVec<'x, (Site, &'x [Handoff])>,
) {
    let read = |part: &ExpressionPart<'_>| {
        let (hops, slot) = local_read(shape, part)?;
        let at = shapes.up(shape.segment(), hops)?;
        (digested[at.index() as usize].binary_search(&slot).is_ok()).then_some(
            Coordinate::Activation {
                hops,
                target: Target::Local(slot),
            },
        )
    };
    let mut handed = BumpVec::new_in(scratch);
    let site = match shape.candidates(shape.site_of_node(node)) {
        // A builtin runs no frame to keep a memo in.
        Some(list)
            if (list.candidates.iter())
                .all(|candidate| matches!(candidate, Candidate::One(Coordinate::Builtin(_)))) =>
        {
            return;
        }
        Some(_) => {
            for (position, (part, label)) in evaluated(node, scratch).iter().enumerate() {
                if !label && let Some(from) = read(part) {
                    handed.push(Handoff {
                        to: HandedTo::Position(position as u32),
                        from,
                    });
                }
            }
            shape.site_of_node(node)
        }
        None => {
            let Some((_, argument)) = by_name_parts(node) else {
                return;
            };
            let ExpressionPart::RecordLiteral(fields) = argument else {
                return;
            };
            for (field, value) in fields.iter() {
                if let Some(from) = read(value) {
                    handed.push(Handoff {
                        to: HandedTo::Field(field.symbol()),
                        from,
                    });
                }
            }
            shape.site(argument)
        }
    };
    if !handed.is_empty() {
        handoffs.push((site, scratch.alloc_slice_copy(&handed)));
    }
}

/// Where `part`, read in `shape`, names a slot of its activation chain bare: the hops out to it
/// and the slot.
fn local_read<'p>(shape: &BodyShape<'_>, part: &'p ExpressionPart<'p>) -> Option<(u32, Slot)> {
    let name = name_of(shape, part)?;
    match shape.mention(shape.site(name))?.coordinate {
        Coordinate::Activation {
            hops,
            target: Target::Local(slot),
        } => Some((hops, slot)),
        _ => None,
    }
}

/// Call `visit` on every node `shape`'s own statements hold, in written order, short of the
/// bodies nested in it and of the types it spells.
pub(super) fn own_nodes<'graph>(
    shape: &BodyShape<'graph>,
    visit: &mut dyn FnMut(&'graph KExpression<'graph>),
) {
    fn walk_part<'graph>(
        shape: &BodyShape<'graph>,
        part: &'graph ExpressionPart<'graph>,
        visit: &mut dyn FnMut(&'graph KExpression<'graph>),
    ) {
        if shape.nested(shape.site(part)).is_some() {
            return;
        }
        match part {
            ExpressionPart::Expression(inner) | ExpressionPart::MarkedUse(_, inner) => {
                node(shape, inner, visit)
            }
            ExpressionPart::ListLiteral(items) => {
                for item in items.iter() {
                    walk_part(shape, item.part(), visit);
                }
            }
            ExpressionPart::DictLiteral(pairs) => {
                for (key, value) in pairs.iter() {
                    walk_part(shape, key, visit);
                    walk_part(shape, value, visit);
                }
            }
            ExpressionPart::RecordLiteral(fields) => {
                for (_, value) in fields.iter() {
                    walk_part(shape, value, visit);
                }
            }
            _ => {}
        }
    }
    fn node<'graph>(
        shape: &BodyShape<'graph>,
        node: &'graph KExpression<'graph>,
        visit: &mut dyn FnMut(&'graph KExpression<'graph>),
    ) {
        visit(node);
        for each in node.parts {
            walk_part(shape, &each.value, visit);
        }
    }
    for statement in shape.body() {
        node(shape, statement, visit);
    }
}
