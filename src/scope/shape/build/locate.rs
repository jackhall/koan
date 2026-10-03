//! Where an error found while a statement is walked points: at the part it is about when that part
//! carries a span, else at the nearest spanned part or node around it. A container's items carry
//! no span of their own, so an error about one points at the container.
//!
//! The walk records a part by its [`Site`], its address, so the part is found again by searching
//! the statement being walked for that address. Only the error path searches: a shape that builds
//! locates nothing.

use crate::parse::{ExpressionPart, KExpression};
use crate::source::SourceRef;

use super::super::Site;
use super::Builder;

impl Builder<'_, '_, '_> {
    /// Where an error about the part at `site`, found in `statement` of the draft at `level`,
    /// points. Valid while the draft at `level` is on the chain, since the part is searched for in
    /// the statement that draft holds.
    pub(super) fn part_source(&self, level: usize, statement: u32, site: Site) -> SourceRef {
        let node = &self.chain[level].nodes[statement as usize];
        let found = source_within(node, site);
        debug_assert!(found.is_some(), "a part the walk met lies in its statement");
        found.unwrap_or(node.source)
    }
}

/// The source of the part at `site` within `node`: its own span in `node`'s file, else the nearest
/// spanned part's or node's around it. `None` when `node` does not hold the part.
pub(in crate::scope) fn source_within(node: &KExpression<'_>, site: Site) -> Option<SourceRef> {
    node.parts.iter().find_map(|part| {
        let here = part.span.map_or(node.source, |span| SourceRef {
            span,
            file: node.source.file,
        });
        within_part(&part.value, here, site)
    })
}

/// [`source_within`] for one part located at `here`. A nested node's own source is nearer than
/// `here`; a container's items carry none, so they are located at `here`.
fn within_part(part: &ExpressionPart<'_>, here: SourceRef, site: Site) -> Option<SourceRef> {
    if Site::of(part) == site {
        return Some(here);
    }
    match part {
        ExpressionPart::Expression(node)
        | ExpressionPart::SigiledTypeExpr(node)
        | ExpressionPart::RecordType(node)
        | ExpressionPart::QuotedExpression(node)
        | ExpressionPart::MarkedUse(_, node) => source_within(node.reference(), site),
        ExpressionPart::ListLiteral(items) => {
            items.iter().find_map(|item| within_part(item, here, site))
        }
        ExpressionPart::DictLiteral(pairs) => pairs.iter().find_map(|(key, value)| {
            within_part(key, here, site).or_else(|| within_part(value, here, site))
        }),
        ExpressionPart::RecordLiteral(fields) => fields
            .iter()
            .find_map(|(_, value)| within_part(value, here, site)),
        ExpressionPart::Keyword(_)
        | ExpressionPart::Identifier(_)
        | ExpressionPart::Type(_)
        | ExpressionPart::MarkedName(..)
        | ExpressionPart::Literal(_) => None,
    }
}
