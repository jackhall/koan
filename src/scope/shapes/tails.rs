//! The tail-call facts of the text: which callables **escape**, and where each is called
//! ([where each body interns](../../../design/type-tiers.md#where-each-body-interns)).
//!
//! A body's tail statements are its last statement and, where that is a `MATCH`, each arm's last
//! statement, through every `MATCH` a tail arm ends in: a tail arm's block runs owing its frame's
//! contract ([`tail_owner`]). A tail arm's call reads the callable's parameters a block level out.
//!
//! A callable escapes where a frame of it may be born from anywhere: it is written in place, born
//! in a `MODULE` or `GROUP` body, or named as a value anywhere but a call's head. It reads the text
//! alone, so the load's folds and every later walk read one answer.
//!
//! The same pass lists every **call site** of each callable a binder births: a keyworded use's
//! candidate, by the use's node site; a call by name, by its head's site; and an `EVAL`'s offer of
//! it to the code it runs, which may call it with anything. It marks each body that is a quote's
//! code or nested in one. The [binding graph's fold](../../build/README.md#the-binding-graphs-text)
//! reads both.

use crate::memory::{BumpAllocator, BumpVec, Writer, collect};
use crate::parse::{BuiltinShapeId, ExpressionPart, KExpression, Role};

use super::super::shape::{
    BodyShape, Candidate, Coordinate, Lands, Offer, Position, Segment, ShapeKind, Site, Slot,
};
use super::Shapes;

/// What the tail-call facts give one body.
#[derive(Clone, Copy, Default)]
pub(super) struct Tails<'graph> {
    /// A callable's: whether a frame of it may be born from anywhere.
    pub(super) escapes: bool,
    /// A callable's: every site in the text that calls it or offers it to code, sorted.
    pub(super) calls: &'graph [CallSite],
    /// Whether the body is a quote's code or nested in one.
    pub(super) in_code: bool,
}

/// A site in the text that calls a callable a binder births, in the shape at `at`: a keyworded use
/// listing it, by the use's node site; a call by name of it, by its head's site; or, `offered`, an
/// `EVAL` offering it to the code it runs, by its operand's site.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct CallSite {
    pub at: Segment,
    pub site: Site,
    pub offered: bool,
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

    // Every call site, by callee.
    let mut calls: BumpVec<'_, (Segment, CallSite)> = BumpVec::new_in(scratch);
    for shape in bodies {
        let at = shape.segment();
        let mut call = |callee: Option<&BodyShape<'_>>, site, offered| {
            if let Some(callee) = callee {
                calls.push((callee.segment(), CallSite { at, site, offered }));
            }
        };
        for (site, list) in shape.candidate_lists().iter() {
            for candidate in list.candidates {
                if let Candidate::One(coordinate) = candidate {
                    call(candidate_callable(shapes, shape, *coordinate), *site, false);
                }
            }
        }
        for mention in shape.mentions().iter().filter(|mention| mention.head) {
            call(births_callable(shapes, mention.lands), mention.site, false);
        }
        for (site, offered) in shape.offer_table().iter() {
            for (_, offer) in offered.iter() {
                match offer {
                    Offer::Name(coordinate) => call(
                        (shapes.follow(at, *coordinate))
                            .and_then(|lands| births_callable(shapes, lands)),
                        *site,
                        true,
                    ),
                    Offer::Key(list) => {
                        for candidate in list.candidates {
                            if let Candidate::One(coordinate) = candidate {
                                call(candidate_callable(shapes, shape, *coordinate), *site, true);
                            }
                        }
                    }
                }
            }
        }
    }
    calls.sort_unstable();
    calls.dedup();
    // Whether each body is a quote's code or nested in one: an encloser's answer is its own.
    let mut in_code: BumpVec<'_, Option<bool>> = BumpVec::with_capacity_in(count, scratch);
    in_code.resize(count, None);
    let mut climbing = BumpVec::new_in(scratch);
    for shape in bodies {
        let mut at = shape.segment();
        let known = loop {
            if let Some(known) = in_code[at.index() as usize] {
                break known;
            }
            if shapes.shape_at(at).kind() == ShapeKind::Code {
                break true;
            }
            climbing.push(at);
            match shapes.parent(at) {
                Some((up, _)) => at = up,
                None => break false,
            }
        };
        for each in climbing.drain(..) {
            in_code[each.index() as usize] = Some(known);
        }
        if shapes.shape_at(at).kind() == ShapeKind::Code {
            in_code[at.index() as usize] = Some(true);
        }
    }

    let mut laid = BumpVec::with_capacity_in(count, scratch);
    for shape in bodies {
        let segment = shape.segment();
        let in_code = in_code[segment.index() as usize] == Some(true);
        if shape.kind() != ShapeKind::Callable {
            laid.push(Tails {
                in_code,
                ..Tails::default()
            });
            continue;
        }
        let from = calls.partition_point(|(each, _)| *each < segment);
        let to = calls.partition_point(|(each, _)| *each <= segment);
        laid.push(Tails {
            escapes: !invariant_body(segment),
            calls: collect(writer, calls[from..to].iter().map(|(_, call)| *call)),
            in_code,
        });
    }
    laid
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

/// The callable a binder births that the head `head` of a call by name in `shape` names bare.
pub(super) fn head_callable<'graph>(
    shapes: &Shapes<'graph>,
    shape: &BodyShape<'graph>,
    head: &ExpressionPart<'_>,
) -> Option<&'graph BodyShape<'graph>> {
    let mention = shape.mention(shape.site(name_of(shape, head)?))?;
    births_callable(shapes, mention.lands)
}

/// The callable a binder births that a keyworded use's candidate at `coordinate`, read in `shape`,
/// lands on.
pub(super) fn candidate_callable<'graph>(
    shapes: &Shapes<'graph>,
    shape: &BodyShape<'graph>,
    coordinate: Coordinate,
) -> Option<&'graph BodyShape<'graph>> {
    births_callable(shapes, shapes.follow(shape.segment(), coordinate)?)
}

/// The callable or quote's code whose tail statement `shape`'s last statement is: `shape` itself,
/// or for a tail arm's block, its `MATCH`'s body's owner. `None` for any other body.
pub(super) fn tail_owner<'graph>(
    shapes: &Shapes<'graph>,
    shape: &'graph BodyShape<'graph>,
) -> Option<&'graph BodyShape<'graph>> {
    match shape.kind() {
        ShapeKind::Callable | ShapeKind::Code => Some(shape),
        ShapeKind::Block if shape.arm().is_some_and(|arm| arm.tail) => {
            let (parent, _) = shapes.parent(shape.segment())?;
            tail_owner(shapes, shapes.shape_at(parent))
        }
        _ => None,
    }
}

/// The node of the tail call `shape`'s last statement makes, where `shape` is a callable, a
/// quote's code or a tail arm's block and the statement is a call by name or a keyworded use.
pub(super) fn tail_call<'graph>(
    shapes: &Shapes<'graph>,
    shape: &'graph BodyShape<'graph>,
) -> Option<&'graph KExpression<'graph>> {
    tail_owner(shapes, shape)?;
    match tail_of(shape, shape.body().last()?) {
        Tail::Apply(node) | Tail::Call(node) => Some(node),
        Tail::Other => None,
    }
}

/// What a node is to a tail call: a call by name, a keyworded use, or anything else.
enum Tail<'graph> {
    Apply(&'graph KExpression<'graph>),
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
    if by_name_parts(node).is_some() {
        return Tail::Apply(node);
    }
    match node.parts {
        [only] => match &only.value {
            ExpressionPart::Expression(inner)
                if shape.nested_in_place(shape.site(&only.value)).is_none() =>
            {
                tail_of(shape, inner)
            }
            ExpressionPart::MarkedUse(_, inner) => tail_of(shape, inner),
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
        ExpressionPart::Expression(node) if shape.nested_in_place(shape.site(part)).is_none() => {
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

/// The parameter of `caller` that `value` names bare, read in `shape`, `caller` itself or a tail
/// arm's block in it.
pub(super) fn bare_parameter<'graph>(
    shape: &BodyShape<'graph>,
    caller: &BodyShape<'graph>,
    value: &'graph ExpressionPart<'graph>,
) -> Option<Slot> {
    let mention = shape.mention(shape.site(name_of(shape, value)?))?;
    match mention.lands {
        Lands::Binder { at: segment, slot }
            if segment == caller.segment() && caller.position_of(slot) == Position::PARAMETER =>
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
