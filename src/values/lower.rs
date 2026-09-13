//! Lowering a region-pure AST part straight to a value, with no dispatch and no scope.

use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::parse::{ExpressionPart, KLiteral};
use crate::type_lattice::TypeRegistry;

use super::{Dict, Key, List, Record, Value, text};

impl<'graph, 'cell> Value<'graph, 'cell> {
    /// The value a region-pure part denotes: a scalar or string literal, a quote, or a container
    /// literal whose every element lowers and whose every dict key is a scalar literal. `None` for a
    /// part that needs dispatch or a scope — a name, a parenthesized expression, a sigiled type
    /// body — anywhere inside it; the part is checked whole before anything is written, so a refusal
    /// leaves the region untouched. A quote lowers to its program node, borrowed at `'graph`.
    pub fn lower_part(
        writer: Writer<'cell>,
        part: &ExpressionPart<'graph>,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> Option<Value<'graph, 'cell>> {
        lowers(part).then(|| lower(writer, part, types, scratch))
    }
}

/// Whether [`lower`] takes `part` whole.
fn lowers(part: &ExpressionPart<'_>) -> bool {
    match part {
        ExpressionPart::Literal(_) | ExpressionPart::QuotedExpression(_) => true,
        ExpressionPart::ListLiteral(items) => items.iter().all(lowers),
        ExpressionPart::DictLiteral(pairs) => pairs.iter().all(|(key, value)| {
            let scalar = match key {
                ExpressionPart::Literal(KLiteral::Number(number)) => !number.is_nan(),
                ExpressionPart::Literal(KLiteral::String(_) | KLiteral::Boolean(_)) => true,
                _ => false,
            };
            scalar && lowers(value)
        }),
        ExpressionPart::RecordLiteral(fields) => fields.iter().all(|(_, value)| lowers(value)),
        ExpressionPart::Keyword(_)
        | ExpressionPart::Identifier(_)
        | ExpressionPart::Type(_)
        | ExpressionPart::Expression(_)
        | ExpressionPart::SigiledTypeExpr(_)
        | ExpressionPart::RecordType(_) => false,
    }
}

/// The value of a part [`lowers`] took.
fn lower<'graph, 'cell>(
    writer: Writer<'cell>,
    part: &ExpressionPart<'graph>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Value<'graph, 'cell> {
    match part {
        ExpressionPart::Literal(KLiteral::Number(number)) => Value::Number(*number),
        ExpressionPart::Literal(KLiteral::String(literal)) => text(writer, literal),
        ExpressionPart::Literal(KLiteral::Boolean(flag)) => Value::Bool(*flag),
        ExpressionPart::Literal(KLiteral::Null) => Value::Null,
        ExpressionPart::QuotedExpression(node) => Value::Expression(*node),
        ExpressionPart::ListLiteral(items) => {
            let mut cells = BumpVec::with_capacity_in(items.len(), scratch);
            cells.extend(items.iter().map(|item| lower(writer, item, types, scratch)));
            Value::List(List::new(writer, cells.iter().copied(), types, scratch))
        }
        ExpressionPart::DictLiteral(pairs) => {
            let mut entries = BumpVec::with_capacity_in(pairs.len(), scratch);
            entries.extend(pairs.iter().map(|(key, value)| {
                // A string key borrows the literal where the parse left it; the dict door writes
                // every key into the region once.
                let key = match key {
                    ExpressionPart::Literal(KLiteral::String(literal)) => Key::str(literal),
                    ExpressionPart::Literal(KLiteral::Number(number)) => {
                        Key::number(*number).expect("a lowerable key is not NaN")
                    }
                    ExpressionPart::Literal(KLiteral::Boolean(flag)) => Key::bool(*flag),
                    _ => unreachable!("a lowerable dict key is a scalar literal"),
                };
                (key, lower(writer, value, types, scratch))
            }));
            Value::Dict(Dict::new(writer, &entries, types, scratch))
        }
        ExpressionPart::RecordLiteral(fields) => {
            let mut cells = BumpVec::with_capacity_in(fields.len(), scratch);
            cells.extend(
                fields
                    .iter()
                    .map(|(name, value)| (*name, lower(writer, value, types, scratch))),
            );
            Value::Record(Record::new(writer, &cells, types, scratch))
        }
        ExpressionPart::Keyword(_)
        | ExpressionPart::Identifier(_)
        | ExpressionPart::Type(_)
        | ExpressionPart::Expression(_)
        | ExpressionPart::SigiledTypeExpr(_)
        | ExpressionPart::RecordType(_) => unreachable!("a lowerable part needs no dispatch"),
    }
}
