//! The syntax nesting limit and the depth every [`KExpression`](super::KExpression) stores
//! against it.
//!
//! Every walk over parsed syntax — lowering, the operator-run rewrite, the shape builder, a quote's
//! comparison — recurses once per nested part, so the depth of the syntax a program writes is the
//! stack those walks need. [`MAX_SYNTAX_DEPTH`] bounds it: `sexlex` refuses groups nested past it
//! while reading, and [`parse_with_source`](super::parse_with_source) refuses a top-level
//! expression whose stored depth passes it, which catches the nesting no group spells — a dotted
//! chain, and the nodes an operator run folds into.
//!
//! A node's depth is computed once, at construction, from its parts' stored depths ([`node_depth`]),
//! so no check walks a tree. An operator run counts as the nesting the rewrite in
//! [scope/shape/build/rewrite.rs](../scope/shape/build/rewrite.rs) will fold it into, so a node the
//! rewrite builds is never deeper than the node it replaces. See [README.md](README.md).

use crate::parse::ast::{DispatchShape, ExpressionPart, NodeCache};
use crate::parse::builtin_shapes::KEYWORDS;
use crate::source::Spanned;

/// The deepest syntax a program may nest: groups as `sexlex` reads them, and the nesting of the
/// lowered syntax, operator runs counted as the nesting their rewrite builds.
pub const MAX_SYNTAX_DEPTH: usize = sexlex::MAX_DEPTH;

/// The depth of a node over `parts`: the levels the node itself adds, over its deepest part.
///
/// A node adds one level, except the two shapes the operator-run rewrite replaces with deeper
/// nesting. A run of `k` operators folds into at most `k + 3` levels above its deepest operand — a
/// pairwise run's block, `k - 1` combiners, a pair, the `NOT` of a `!=` pair and a statement's
/// wrapper around the block — and a lone `a != b` becomes `NOT (a == b)`, two levels.
pub(super) fn node_depth(parts: &[Spanned<ExpressionPart<'_>>], cache: &NodeCache<'_>) -> u32 {
    let levels = if cache.shape() == DispatchShape::OperatorChain {
        (parts.len() - 1) / 2 + 3
    } else if let [_, middle, _] = parts
        && let ExpressionPart::Keyword(symbol) = middle.value
        && symbol == KEYWORDS.unequal.symbol()
    {
        2
    } else {
        1
    };
    let deepest = parts.iter().map(|part| part_depth(&part.value)).max();
    levels as u32 + deepest.unwrap_or(0)
}

/// A part's depth: a nested node's stored depth, one more than a literal's deepest element, and
/// zero for a leaf. Literal parts nest only where `sexlex` read a group, so this recursion is
/// bounded by the reader's limit and never descends into a node.
fn part_depth(part: &ExpressionPart<'_>) -> u32 {
    match part {
        ExpressionPart::Expression(node)
        | ExpressionPart::QuotedExpression(node)
        | ExpressionPart::SigiledTypeExpr(node)
        | ExpressionPart::RecordType(node)
        | ExpressionPart::MarkedUse(_, node) => node.reference().depth() as u32,
        ExpressionPart::ListLiteral(items) => 1 + items.iter().map(part_depth).max().unwrap_or(0),
        ExpressionPart::DictLiteral(pairs) => {
            let deepest = pairs
                .iter()
                .map(|(key, value)| part_depth(key).max(part_depth(value)));
            1 + deepest.max().unwrap_or(0)
        }
        ExpressionPart::RecordLiteral(fields) => {
            1 + fields
                .iter()
                .map(|(_, value)| part_depth(value))
                .max()
                .unwrap_or(0)
        }
        ExpressionPart::Keyword(_)
        | ExpressionPart::Identifier(_)
        | ExpressionPart::Type(_)
        | ExpressionPart::Literal(_)
        | ExpressionPart::MarkedName(..) => 0,
    }
}
