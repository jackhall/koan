//! The tail-call facts of the text: which callables **escape**, every **tail edge** a callable's or
//! a quote's code's last statement makes, and the **invariance** fixpoint over those edges — the
//! parameters every tail edge into a body passes on bare
//! ([where each body interns](../../../design/type-tiers.md#where-each-body-interns)).
//!
//! A callable escapes where a frame of it may be born from anywhere: it is written in place, born
//! in a `MODULE` or `GROUP` body, or named as a value anywhere but a call's head. A tail edge is a
//! call by name of a callable a binder births, its record literal's fields paired with the callee's
//! parameters by name, or a keyworded use's every candidate a binder births, its slots paired with
//! the registration's written slot names. Each reads the text alone, so the load's folds and every
//! later walk read one answer.

use crate::memory::{BumpAllocator, BumpVec, Writer, collect};
use crate::parse::{BuiltinShapeId, ExpressionPart, KExpression, Role};
use crate::symbols::BinderSymbol;

use super::super::shape::{BodyShape, Candidate, Lands, Position, Segment, ShapeKind, Slot, Which};
use super::super::signature::declare_parameters;
use super::Shapes;

/// What the tail-call facts give one body.
#[derive(Clone, Copy, Default)]
pub(super) struct Tails<'graph> {
    /// A callable's: whether a frame of it may be born from anywhere.
    pub(super) escapes: bool,
    /// The parameters a tail edge from the body into itself passes anything but that same
    /// parameter at, sorted.
    pub(super) rebound: &'graph [Slot],
    /// The parameters every tail edge into the body passes on bare, sorted: empty for a body that
    /// escapes.
    pub(super) invariant: &'graph [Slot],
}

/// One tail call: the caller, the callee, and per parameter of the callee the caller's own
/// parameter passed bare at it, where one is.
struct Edge<'x> {
    caller: Segment,
    callee: Segment,
    arguments: &'x [(Slot, Option<Slot>)],
}

/// The tail-call facts of every body of `shapes`, by segment.
pub(super) fn tails<'graph, 'x>(
    writer: Writer<'graph, '_>,
    scratch: BumpAllocator<'x>,
    shapes: &Shapes<'graph>,
) -> BumpVec<'x, Tails<'graph>> {
    let bodies = shapes.bodies();
    let count = bodies.len();
    // Per body: whether a binder births it, and where it escapes.
    let mut at_binder = BumpVec::with_capacity_in(count, scratch);
    at_binder.resize(count, false);
    let mut escaping = BumpVec::with_capacity_in(count, scratch);
    escaping.resize(count, false);
    for shape in bodies {
        for slot in (0..shape.slots() as u32).map(Slot) {
            let Some(body) = shape.births(slot) else {
                continue;
            };
            if body.kind() != ShapeKind::Callable {
                continue;
            }
            at_binder[body.segment().index() as usize] = true;
            if shape.kind() == ShapeKind::Module {
                escaping[body.segment().index() as usize] = true;
            }
        }
        // A callable named as a value may be called from anywhere it reaches.
        for mention in shape.mentions() {
            if mention.head {
                continue;
            }
            if let Some(body) = births_callable(shapes, mention.lands) {
                escaping[body.segment().index() as usize] = true;
            }
        }
    }
    let invariant_body = |segment: Segment| {
        at_binder[segment.index() as usize] && !escaping[segment.index() as usize]
    };

    let mut edges = BumpVec::new_in(scratch);
    for shape in bodies {
        tail_edges(scratch, shapes, shape, &mut edges);
    }

    // Each body's first slot in one flat set of every body's slots, by segment.
    let mut offsets = BumpVec::with_capacity_in(count, scratch);
    let mut total = 0;
    for shape in bodies {
        offsets.push(total);
        total += shape.slots();
    }
    let bit = |segment: Segment, slot: Slot| offsets[segment.index() as usize] + slot.index();
    // Every parameter starts invariant and falls where a tail edge into it passes anything but a
    // bare invariant parameter of the caller there.
    let mut varying = BumpVec::with_capacity_in(total, scratch);
    varying.resize(total, false);
    loop {
        let mut changed = false;
        for edge in edges.iter() {
            for (slot, passed) in edge.arguments {
                let key = bit(edge.callee, *slot);
                if varying[key] {
                    continue;
                }
                let falls = match passed {
                    None => true,
                    // A hop of a body into itself keeps a parameter only where it passes that same
                    // one on.
                    Some(from) if edge.caller == edge.callee && from != slot => true,
                    Some(from) => !invariant_body(edge.caller) || varying[bit(edge.caller, *from)],
                };
                if falls {
                    varying[key] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Each parameter a hop of a body into itself rebinds, by segment, sorted.
    let mut rebinds: BumpVec<'_, (Segment, Slot)> = BumpVec::new_in(scratch);
    for edge in edges.iter().filter(|edge| edge.caller == edge.callee) {
        for (slot, passed) in edge.arguments {
            if *passed != Some(*slot) {
                rebinds.push((edge.callee, *slot));
            }
        }
    }
    rebinds.sort_unstable();
    rebinds.dedup();

    let mut laid = BumpVec::with_capacity_in(count, scratch);
    for shape in bodies {
        let segment = shape.segment();
        if shape.kind() != ShapeKind::Callable {
            laid.push(Tails::default());
            continue;
        }
        let start = rebinds.partition_point(|(each, _)| *each < segment);
        let end = rebinds.partition_point(|(each, _)| *each <= segment);
        let escapes = !invariant_body(segment);
        let mut invariant = BumpVec::new_in(scratch);
        if !escapes {
            invariant.extend(parameters(shape).filter(|slot| !varying[bit(segment, *slot)]));
        }
        laid.push(Tails {
            escapes,
            rebound: collect(writer, rebinds[start..end].iter().map(|(_, slot)| *slot)),
            invariant: collect(writer, invariant.iter().copied()),
        });
    }
    laid
}

/// The parameter slots of `shape`, in slot order.
fn parameters<'a>(shape: &'a BodyShape<'_>) -> impl Iterator<Item = Slot> + 'a {
    (0..shape.slots() as u32)
        .map(Slot)
        .filter(|slot| shape.position_of(*slot) == Position::PARAMETER)
}

/// The callable a binder births where `lands` reaches that binder.
fn births_callable<'graph>(
    shapes: &Shapes<'graph>,
    lands: Lands,
) -> Option<&'graph BodyShape<'graph>> {
    let Lands::Binder { at: segment, slot } = lands else {
        return None;
    };
    (shapes.shape_at(segment).births(slot)).filter(|body| body.kind() == ShapeKind::Callable)
}

/// What a node is to a tail call: a call by name over its head and argument, a keyworded use, or
/// anything else.
enum Tail<'graph> {
    Apply(
        &'graph ExpressionPart<'graph>,
        &'graph ExpressionPart<'graph>,
    ),
    Call(&'graph KExpression<'graph>),
    Other,
}

/// What `node`, read off `shape`, is as a tail call: the evaluator's reading of a node, short of
/// every form a tail edge ignores.
fn tail_of<'graph>(shape: &BodyShape<'graph>, node: &'graph KExpression<'graph>) -> Tail<'graph> {
    let builtin = node.cache().builtin_shape();
    if let Some(builtin) = builtin
        && matches!(
            builtin.id,
            BuiltinShapeId::Lambda
                | BuiltinShapeId::QuantifiedLambda
                | BuiltinShapeId::AscribeTransparent
                | BuiltinShapeId::AscribeOpaque
                | BuiltinShapeId::Eval
                | BuiltinShapeId::UsingScope
                | BuiltinShapeId::BucketDeclaration
        )
    {
        return Tail::Other;
    }
    if shape.candidates(shape.site_of_node(node)).is_some() {
        // A member read whose label is written bare is no call of a registration.
        let member = builtin.is_some_and(|builtin| {
            builtin.id == BuiltinShapeId::Attribute
                && (builtin.roles().zip(node.parts))
                    .any(|(role, part)| role.label_reads(&part.value).is_some())
        });
        return match member {
            true => Tail::Other,
            false => Tail::Call(node),
        };
    }
    if builtin.is_some() {
        return Tail::Other;
    }
    if let Some((head, argument)) = by_name_parts(node) {
        return Tail::Apply(head, argument);
    }
    match node.parts {
        [only] => match &only.value {
            ExpressionPart::Expression(inner)
                if shape.nested(shape.site(&only.value)).is_none() =>
            {
                tail_of(shape, inner.reference())
            }
            ExpressionPart::MarkedUse(_, inner) => tail_of(shape, inner.reference()),
            _ => Tail::Other,
        },
        _ => Tail::Other,
    }
}

/// The name `part` reads as, through any one-part wrappers: `None` for any other part.
pub(super) fn name_of<'p>(
    shape: &BodyShape<'_>,
    part: &'p ExpressionPart<'p>,
) -> Option<&'p ExpressionPart<'p>> {
    match part {
        part if part.is_name() => Some(part),
        ExpressionPart::Expression(node) if shape.nested(shape.site(part)).is_none() => {
            let node = node.reference();
            match (node.cache().builtin_shape(), node.parts) {
                (None, [only]) if shape.candidates(shape.site_of_node(node)).is_none() => {
                    name_of(shape, &only.value)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The caller's own parameter `value` names bare, read in `shape`.
fn bare_parameter<'graph>(
    shape: &BodyShape<'graph>,
    value: &'graph ExpressionPart<'graph>,
) -> Option<Slot> {
    let mention = shape.mention(shape.site(name_of(shape, value)?))?;
    match mention.lands {
        Lands::Binder { at: segment, slot }
            if segment == shape.segment() && shape.position_of(slot) == Position::PARAMETER =>
        {
            Some(slot)
        }
        _ => None,
    }
}

/// The head and argument of a call by name: a builtin-less node of two parts, neither a keyword.
pub(super) fn by_name_parts<'graph>(
    node: &'graph KExpression<'graph>,
) -> Option<(
    &'graph ExpressionPart<'graph>,
    &'graph ExpressionPart<'graph>,
)> {
    let keyword = |part: &ExpressionPart<'_>| matches!(part, ExpressionPart::Keyword(_));
    let (None, [head, argument]) = (node.cache().builtin_shape(), node.parts) else {
        return None;
    };
    (!keyword(&head.value) && !keyword(&argument.value)).then_some((&head.value, &argument.value))
}

/// The parts of a keyworded node its call evaluates, in slot order, each beside whether it is an
/// `ATTR` label written bare, which is read as written.
pub(super) fn evaluated<'x, 'graph>(
    node: &'graph KExpression<'graph>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, (&'graph ExpressionPart<'graph>, bool)> {
    let mut wanted = BumpVec::with_capacity_in(node.parts.len(), scratch);
    match node.cache().builtin_shape() {
        Some(shape) => {
            for (role, part) in shape.roles().zip(node.parts) {
                if role != Role::Keyword {
                    wanted.push((&part.value, role.label_reads(&part.value).is_some()));
                }
            }
        }
        None => wanted.extend(
            (node.parts.iter())
                .filter(|part| !matches!(part.value, ExpressionPart::Keyword(_)))
                .map(|part| (&part.value, false)),
        ),
    }
    wanted
}

/// The parameter each slot of the registration at `slot` of `shape` binds, in slot order, as its
/// head is written: `None` for a `UNARY OP` at its binary key, which packs its slots into one list.
fn written_slots<'x>(
    shape: &BodyShape<'_>,
    slot: Slot,
    callee: &BodyShape<'_>,
    scratch: BumpAllocator<'x>,
) -> Option<BumpVec<'x, BinderSymbol>> {
    let registration = shape.registration(slot)?;
    let mut names = BumpVec::new_in(scratch);
    let form = callee.form()?;
    let head = (form.cache().builtin_shape()).and_then(|builtin| {
        builtin
            .roles()
            .zip(form.parts)
            .find(|(role, _)| *role == Role::Head)
    });
    match (head, registration.which) {
        (Some((_, head)), _) => declare_parameters(&head.value, &mut names),
        (None, Which::Binary) => return None,
        // An operator's operands, as its body binds them.
        (None, _) => names.extend(parameters(callee).map(|slot| callee.slot_name(slot))),
    }
    Some(names)
}

/// Push the tail edges of `shape`'s last statement onto `edges`, where `shape` is a callable or a
/// quote's code: a call by name of a callable a binder births, and one edge per candidate a
/// keyworded use there may select, each beside the caller's own parameters it passes bare.
fn tail_edges<'x, 'graph>(
    scratch: BumpAllocator<'x>,
    shapes: &Shapes<'graph>,
    shape: &'graph BodyShape<'graph>,
    edges: &mut BumpVec<'x, Edge<'x>>,
) {
    if !matches!(shape.kind(), ShapeKind::Callable | ShapeKind::Code) {
        return;
    }
    let Some(last) = shape.body().last() else {
        return;
    };
    let mut edge = |callee: &BodyShape<'_>, passed: &dyn Fn(Slot) -> Option<Slot>| {
        let mut arguments = BumpVec::new_in(scratch);
        arguments.extend(parameters(callee).map(|slot| (slot, passed(slot))));
        edges.push(Edge {
            caller: shape.segment(),
            callee: callee.segment(),
            arguments: scratch.alloc_slice_copy(&arguments),
        });
    };
    match tail_of(shape, last) {
        Tail::Apply(head, argument) => {
            let callee = name_of(shape, head)
                .and_then(|head| shape.mention(shape.site(head)))
                .and_then(|mention| births_callable(shapes, mention.lands));
            let Some(callee) = callee else {
                return;
            };
            let mut bare = BumpVec::new_in(scratch);
            if let ExpressionPart::RecordLiteral(fields) = argument {
                for (field, value) in fields.iter() {
                    if let Some(from) = bare_parameter(shape, value) {
                        bare.push((field.symbol(), from));
                    }
                }
            }
            edge(callee, &|slot| {
                let name = callee.slot_name(slot).symbol();
                (bare.iter().find(|(field, _)| *field == name)).map(|(_, from)| *from)
            });
        }
        Tail::Call(node) => {
            let list = shape
                .candidates(shape.site_of_node(node))
                .expect("a call has its candidates");
            let wanted = evaluated(node, scratch);
            for candidate in list.candidates {
                let Candidate::One(coordinate) = candidate else {
                    continue;
                };
                let Some(Lands::Binder { at: segment, slot }) =
                    shapes.follow(shape.segment(), *coordinate)
                else {
                    continue;
                };
                let holder = shapes.shape_at(segment);
                let Some(callee) = holder
                    .births(slot)
                    .filter(|body| body.kind() == ShapeKind::Callable)
                else {
                    continue;
                };
                let names = written_slots(holder, slot, callee, scratch)
                    .unwrap_or_else(|| BumpVec::new_in(scratch));
                let mut bare = BumpVec::new_in(scratch);
                for (name, (part, label)) in names.iter().zip(wanted.iter()) {
                    if !label && let Some(from) = bare_parameter(shape, part) {
                        bare.push((*name, from));
                    }
                }
                edge(callee, &|slot| {
                    let name = callee.slot_name(slot);
                    (bare.iter().find(|(each, _)| *each == name)).map(|(_, from)| *from)
                });
            }
        }
        Tail::Other => {}
    }
}
