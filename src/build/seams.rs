//! The seams the load's frontiers decide ([frontiers and seams](README.md#frontiers-and-seams)):
//! for every fact a body's residual holds, the binding site at which the last unknown its
//! frontier waits on binds, listed on the body that site lies in.
//!
//! An unknown binds where its declaring body binds it: a callable's parameter, `FOR ALL` name or
//! `:Type` parameter and a quote's code's hole, mark or `$` name at that body's entry; a type
//! binder or a local at the unit that binds it; and a `USING … SCOPE` block's surfaced name at
//! the entry of the frame body whose parameter its operand names, or else at the unit holding the
//! `USING`. Binding order follows the lexical chain — an outer body before an inner one, and in a
//! body its entry before its units in order — so a frontier's seam is the latest of its members'
//! sites. Each body's **reach**, every unknown a fact of it or of a body nested in it waits on, is
//! laid beside its seams.

use crate::memory::{BumpAllocator, BumpVec, Writer, collect};
use crate::scope::{
    Address, BodyShape, Coordinate, Frontier, Position, Seam, SeamAt, ShapeKind, Slot, Target,
    UnitWork, Unknown, merge_into,
};

/// A seam found for one frontier: the chain depth of its body, the body, and where in it.
type Found<'graph> = (usize, &'graph BodyShape<'graph>, SeamAt);

/// List the seams of `root` and every body nested in it, and lay each body's reach down, once the
/// load's passes have quiesced.
pub fn list_seams<'graph>(
    root: &'graph BodyShape<'graph>,
    writer: Writer<'graph, '_>,
    scratch: BumpAllocator<'_>,
) {
    let mut chain = BumpVec::new_in(scratch);
    let mut found = BumpVec::new_in(scratch);
    let mut reaches = BumpVec::new_in(scratch);
    walk(root, &mut chain, &mut found, &mut reaches, scratch);
    found.sort_unstable_by_key(|(shape, at, _): &(&BodyShape<'_>, SeamAt, Unknown)| {
        (shape.segment(), *at)
    });
    for (shape, reach) in reaches.iter_mut() {
        let segment = shape.segment();
        let from = found.partition_point(|(held, _, _)| held.segment() < segment);
        let to = found.partition_point(|(held, _, _)| held.segment() <= segment);
        let mut seams = BumpVec::new_in(scratch);
        let mut at = from;
        while at < to {
            let seam = found[at].1;
            let mut waits = BumpVec::new_in(scratch);
            while at < to && found[at].1 == seam {
                merge_into(&mut waits, &[found[at].2]);
                at += 1;
            }
            seams.push(Seam {
                at: seam,
                waits_on: Frontier::laid(writer, &mut waits),
            });
        }
        let seams = collect(writer, seams.iter().copied());
        shape.fix_seams(seams, Frontier::laid(writer, reach));
    }
}

/// Find the seam of each frontier `shape` and every body nested in it hold, `chain` the bodies
/// enclosing `shape`, outermost first; and gather each body's reach. Returns `shape`'s reach.
fn walk<'graph, 'x>(
    shape: &'graph BodyShape<'graph>,
    chain: &mut BumpVec<'x, &'graph BodyShape<'graph>>,
    found: &mut BumpVec<'x, (&'graph BodyShape<'graph>, SeamAt, Unknown)>,
    reaches: &mut BumpVec<'x, (&'graph BodyShape<'graph>, BumpVec<'x, Unknown>)>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, Unknown> {
    chain.push(shape);
    let mut reach = BumpVec::new_in(scratch);
    let (channel, statics) = (shape.channel_residual(), shape.statics_residual());
    for (_, frontier) in channel.entries().iter().chain(statics.entries()) {
        merge_into(&mut reach, frontier.unknowns());
        let latest = (frontier.unknowns().iter())
            .filter_map(|unknown| site(*unknown, chain))
            .max_by_key(|(depth, _, at)| (*depth, *at));
        if let Some((_, body, at)) = latest {
            found.extend(
                frontier
                    .unknowns()
                    .iter()
                    .map(|unknown| (body, at, *unknown)),
            );
        }
    }
    for (_, nested) in shape.nested_shapes() {
        let inner = walk(nested, chain, found, reaches, scratch);
        merge_into(&mut reach, &inner);
    }
    chain.pop();
    let mut kept = BumpVec::with_capacity_in(reach.len(), scratch);
    kept.extend_from_slice(&reach);
    reaches.push((shape, kept));
    reach
}

/// Where `unknown` binds, read from the bodies `chain` holds, outermost first: `None` where no
/// body on it declares it.
fn site<'graph>(unknown: Unknown, chain: &[&'graph BodyShape<'graph>]) -> Option<Found<'graph>> {
    let depth = chain
        .iter()
        .position(|shape| shape.segment() == unknown.segment)?;
    let declaring = chain[depth];
    let slot = match unknown.at {
        // Only a quote's code holds a capture a name it is run with fills.
        Address::Capture(_) => return Some((depth, declaring, SeamAt::Entry)),
        Address::Slot(slot) => slot,
    };
    if declaring.position_of(slot) != Position::PARAMETER {
        return Some((depth, declaring, SeamAt::Unit(unit_of(declaring, slot)?)));
    }
    match declaring.kind() {
        ShapeKind::Block => surfaced(depth, declaring, chain),
        _ => Some((depth, declaring, SeamAt::Entry)),
    }
}

/// Where a name the block `block`, at `depth` on `chain`, binds as a parameter binds: at the
/// entry of the frame body whose parameter its `USING`'s operand names, and otherwise at the unit
/// of the body enclosing it that holds the expression it is.
fn surfaced<'graph>(
    depth: usize,
    block: &'graph BodyShape<'graph>,
    chain: &[&'graph BodyShape<'graph>],
) -> Option<Found<'graph>> {
    let outer = depth.checked_sub(1)?;
    let encloser = chain[outer];
    let (site, _) =
        (encloser.nested_shapes().iter()).find(|(_, nested)| std::ptr::eq(*nested, block))?;
    let (statement, operand) = encloser
        .using_at(*site)
        .unwrap_or((statement_of(encloser, *site)?, None));
    if let Some(Coordinate::Activation {
        hops,
        target: Target::Local(slot),
    }) = operand
    {
        let at = outer.checked_sub(hops as usize)?;
        let frame = chain[at];
        if matches!(frame.kind(), ShapeKind::Callable | ShapeKind::Code)
            && frame.position_of(slot) == Position::PARAMETER
        {
            return Some((at, frame, SeamAt::Entry));
        }
    }
    let unit = unit_at(encloser, statement)?;
    Some((outer, encloser, SeamAt::Unit(unit)))
}

/// The index of the statement of `shape` holding the part at `site`.
fn statement_of(shape: &BodyShape<'_>, site: crate::scope::Site) -> Option<usize> {
    (shape.body().iter()).position(|node| crate::scope::source_of(node, site).is_some())
}

/// The index of the unit of `shape` that binds `slot`.
fn unit_of(shape: &BodyShape<'_>, slot: Slot) -> Option<u32> {
    let component = shape.component_index(slot);
    let unit = (shape.units().iter()).position(|unit| unit.work == UnitWork::Component(component));
    match unit {
        Some(unit) => Some(unit as u32),
        None => unit_at(shape, shape.position_of(slot).statement_index()?),
    }
}

/// The index of the unit of `shape` that runs statement `statement`.
fn unit_at(shape: &BodyShape<'_>, statement: usize) -> Option<u32> {
    let unit = shape.units().iter().position(|unit| match unit.work {
        UnitWork::Statement(index) => index as usize == statement,
        UnitWork::Component(index) => (shape.components()[index.index()].members.iter())
            .any(|member| shape.position_of(*member).statement_index() == Some(statement)),
    })?;
    Some(unit as u32)
}
