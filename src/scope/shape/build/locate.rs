//! Where an error found while a statement is walked points: at the part it is about when that part
//! carries a span, else at the nearest spanned part or node around it. A container's items carry
//! no span of their own, so an error about one points at the container.
//!
//! The walk records a part by its [`Site`], which its shape's table resolves to an address, so the
//! part is found again by searching the statement being walked for that address. Only the error
//! path searches: a shape that builds locates nothing.

use crate::parse::{ExpressionPart, KExpression};
use crate::source::SourceRef;

use super::super::Site;
use super::Builder;

impl Builder<'_, '_, '_> {
    /// Where an error about the part at `site`, found in `statement` of the draft at `level`,
    /// points. Valid while the draft at `level` is on the chain, since the part is searched for in
    /// the statement that draft holds.
    pub(super) fn part_source(&self, level: usize, statement: u32, site: Site) -> SourceRef {
        let draft = &self.chain[level];
        let node = &draft.nodes[statement as usize];
        let found = source_within(node, draft.sites.address(site));
        debug_assert!(found.is_some(), "a part the walk met lies in its statement");
        found.unwrap_or(node.source)
    }
}

/// The source of the part at `address` within `node`: its own span in `node`'s file, else the
/// nearest spanned part's or node's around it. `None` when `node` does not hold the part.
pub(in crate::scope) fn source_within(node: &KExpression<'_>, address: usize) -> Option<SourceRef> {
    node.parts.iter().find_map(|part| {
        let here = part.span.map_or(node.source, |span| SourceRef {
            span,
            file: node.source.file,
        });
        within_part(&part.value, here, address)
    })
}

/// [`source_within`] for one part located at `here`. A nested node's own source is nearer than
/// `here`; a container's items carry none, so they are located at `here`.
fn within_part(part: &ExpressionPart<'_>, here: SourceRef, address: usize) -> Option<SourceRef> {
    if std::ptr::from_ref(part).addr() == address {
        return Some(here);
    }
    match part {
        ExpressionPart::Expression(node)
        | ExpressionPart::SigiledTypeExpr(node)
        | ExpressionPart::RecordType(node)
        | ExpressionPart::QuotedExpression(node)
        | ExpressionPart::MarkedUse(_, node)
        | ExpressionPart::FilledUse(_, _, node) => source_within(node, address),
        ExpressionPart::ListLiteral(items) => items
            .iter()
            .find_map(|item| within_part(item.part(), here, address)),
        ExpressionPart::DictLiteral(pairs) => pairs.iter().find_map(|(key, value)| {
            within_part(key, here, address).or_else(|| within_part(value, here, address))
        }),
        ExpressionPart::RecordLiteral(fields) => fields
            .iter()
            .find_map(|(_, value)| within_part(value, here, address)),
        ExpressionPart::Keyword(_)
        | ExpressionPart::Identifier(_)
        | ExpressionPart::Type(_)
        | ExpressionPart::MarkedName(..)
        | ExpressionPart::Splice(_)
        | ExpressionPart::Literal(_) => None,
    }
}

/// The part at `address` within `node`, at any depth. `None` when `node` does not hold
/// it.
pub(in crate::scope) fn part_within<'a>(
    node: &'a KExpression<'a>,
    address: usize,
) -> Option<&'a ExpressionPart<'a>> {
    node.parts
        .iter()
        .find_map(|part| part_in(&part.value, address))
}

/// [`part_within`] for one part.
fn part_in<'a>(part: &'a ExpressionPart<'a>, address: usize) -> Option<&'a ExpressionPart<'a>> {
    if std::ptr::from_ref(part).addr() == address {
        return Some(part);
    }
    match part {
        ExpressionPart::Expression(node)
        | ExpressionPart::SigiledTypeExpr(node)
        | ExpressionPart::RecordType(node)
        | ExpressionPart::QuotedExpression(node)
        | ExpressionPart::MarkedUse(_, node)
        | ExpressionPart::FilledUse(_, _, node) => part_within(node, address),
        ExpressionPart::ListLiteral(items) => {
            (items.iter()).find_map(|item| part_in(item.part(), address))
        }
        ExpressionPart::DictLiteral(pairs) => pairs
            .iter()
            .find_map(|(key, value)| part_in(key, address).or_else(|| part_in(value, address))),
        ExpressionPart::RecordLiteral(fields) => {
            fields.iter().find_map(|(_, value)| part_in(value, address))
        }
        ExpressionPart::Keyword(_)
        | ExpressionPart::Identifier(_)
        | ExpressionPart::Type(_)
        | ExpressionPart::MarkedName(..)
        | ExpressionPart::Splice(_)
        | ExpressionPart::Literal(_) => None,
    }
}

/// Call `visit` on every part within `node`, at any depth, in written order: the parts
/// [`part_within`] searches.
pub(in crate::scope) fn each_part_within<'a>(
    node: &'a KExpression<'a>,
    visit: &mut impl FnMut(&'a ExpressionPart<'a>),
) {
    for part in node.parts.iter() {
        each_part_in(&part.value, visit);
    }
}

/// [`each_part_within`] for one part.
fn each_part_in<'a>(part: &'a ExpressionPart<'a>, visit: &mut impl FnMut(&'a ExpressionPart<'a>)) {
    visit(part);
    match part {
        ExpressionPart::Expression(node)
        | ExpressionPart::SigiledTypeExpr(node)
        | ExpressionPart::RecordType(node)
        | ExpressionPart::QuotedExpression(node)
        | ExpressionPart::MarkedUse(_, node)
        | ExpressionPart::FilledUse(_, _, node) => each_part_within(node, visit),
        ExpressionPart::ListLiteral(items) => {
            for item in items.iter() {
                each_part_in(item.part(), visit);
            }
        }
        ExpressionPart::DictLiteral(pairs) => {
            for (key, value) in pairs.iter() {
                each_part_in(key, visit);
                each_part_in(value, visit);
            }
        }
        ExpressionPart::RecordLiteral(fields) => {
            for (_, value) in fields.iter() {
                each_part_in(value, visit);
            }
        }
        ExpressionPart::Keyword(_)
        | ExpressionPart::Identifier(_)
        | ExpressionPart::Type(_)
        | ExpressionPart::MarkedName(..)
        | ExpressionPart::Splice(_)
        | ExpressionPart::Literal(_) => {}
    }
}
