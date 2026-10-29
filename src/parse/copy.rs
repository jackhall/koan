//! The syntax copier: a whole expression laid again in another program storage, every part and
//! every nested quote with it, its strings through the destination's brand. Code composed at run
//! time owns its syntax this way — its composition copies the code value's syntax into its own
//! storage and builds its shapes over the copy, so nothing it holds borrows the value's storage.
//! Modeled on the shape builder's rewrite walk (`scope/shape/build/rewrite.rs`), which copies only
//! the path to a change and never enters a quote; this copies everything.

use crate::memory::{Bump, BumpVec, ProgramBrand, collect};
use crate::source::Spanned;

use super::{ExpressionPart, KExpression, KLiteral, ListItem, ProgramNode};

/// `syntax` laid whole through `brand`: equal to it as code, borrowing nothing of its storage.
pub fn copied<'a>(brand: ProgramBrand<'a>, syntax: &KExpression<'_>) -> ProgramNode<'a> {
    let scratch = Bump::new();
    Copier {
        brand,
        scratch: &scratch,
    }
    .node(syntax)
}

/// One copy's destination brand and the scratch its runs gather in.
struct Copier<'a, 'x> {
    brand: ProgramBrand<'a>,
    scratch: &'x Bump,
}

impl<'a> Copier<'a, '_> {
    fn node(&self, node: &KExpression<'_>) -> ProgramNode<'a> {
        let mut run = BumpVec::with_capacity_in(node.parts.len(), self.scratch);
        run.extend(node.parts.iter().map(|part| Spanned {
            value: self.part(&part.value),
            span: part.span,
        }));
        self.brand.nested_node(&run, node.source)
    }

    fn part(&self, part: &ExpressionPart<'_>) -> ExpressionPart<'a> {
        let writer = self.brand.writer();
        match part {
            ExpressionPart::Keyword(keyword) => ExpressionPart::Keyword(*keyword),
            ExpressionPart::Identifier(name) => ExpressionPart::Identifier(*name),
            ExpressionPart::Type(name) => ExpressionPart::Type(*name),
            ExpressionPart::Expression(node) => ExpressionPart::Expression(self.node(node)),
            ExpressionPart::SigiledTypeExpr(node) => {
                ExpressionPart::SigiledTypeExpr(self.node(node))
            }
            ExpressionPart::RecordType(node) => ExpressionPart::RecordType(self.node(node)),
            ExpressionPart::QuotedExpression(node) => {
                ExpressionPart::QuotedExpression(self.node(node))
            }
            ExpressionPart::MarkedName(mark, name) => ExpressionPart::MarkedName(*mark, *name),
            ExpressionPart::MarkedUse(mark, node) => {
                ExpressionPart::MarkedUse(*mark, self.node(node))
            }
            ExpressionPart::ListLiteral(items) => {
                let mut run = BumpVec::with_capacity_in(items.len(), self.scratch);
                run.extend(items.iter().map(|item| match item {
                    ListItem::Part(part) => ListItem::Part(self.part(part)),
                    ListItem::Spread(part) => ListItem::Spread(self.part(part)),
                }));
                ExpressionPart::ListLiteral(collect(writer, run.iter().copied()))
            }
            ExpressionPart::DictLiteral(pairs) => {
                let mut run = BumpVec::with_capacity_in(pairs.len(), self.scratch);
                run.extend((pairs.iter()).map(|(key, value)| (self.part(key), self.part(value))));
                ExpressionPart::DictLiteral(collect(writer, run.iter().copied()))
            }
            ExpressionPart::RecordLiteral(pairs) => {
                let mut run = BumpVec::with_capacity_in(pairs.len(), self.scratch);
                run.extend(pairs.iter().map(|(name, value)| (*name, self.part(value))));
                ExpressionPart::RecordLiteral(collect(writer, run.iter().copied()))
            }
            ExpressionPart::Literal(literal) => ExpressionPart::Literal(match literal {
                KLiteral::Number(number) => KLiteral::Number(*number),
                KLiteral::String(text) => KLiteral::String(writer.text(text)),
                KLiteral::Boolean(boolean) => KLiteral::Boolean(*boolean),
                KLiteral::Null => KLiteral::Null,
            }),
        }
    }
}
