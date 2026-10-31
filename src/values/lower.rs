//! Lowering a region-pure AST part straight to a value, with no dispatch and no scope: each string
//! copied into the region, or shared with a part in storage that outlives it.

use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::parse::{ExpressionPart, KLiteral};

use super::{Chain, Dict, Key, Knotted, List, Record, Value};

impl<'cell, X: Knotted> Value<'cell, X> {
    /// The value a region-pure part denotes: a scalar or string literal, or a container literal
    /// whose every element lowers and whose every dict key is a scalar literal. `None` for a part
    /// that needs dispatch or a scope — a name, a parenthesized expression, a sigiled type body, a
    /// quote, whose code binds its `$` names where it is written — anywhere inside it; the part is
    /// checked whole before anything is written, so a refusal leaves the region untouched.
    pub fn lower_part(
        writer: Writer<'cell, '_>,
        part: &ExpressionPart<'_>,
        types: &Chain<'_, 'cell, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Option<Value<'cell, X>> {
        lowers(part).then(|| lower(writer, part, &|text| writer.text(text), types, scratch))
    }

    /// Whether [`lower_part`](Self::lower_part) takes `part`.
    pub fn lowers(part: &ExpressionPart<'_>) -> bool {
        lowers(part)
    }

    /// [`lower_part`](Self::lower_part) of a part in storage that outlives the region, such as
    /// the program's: each string shares its text there rather than copying it in.
    pub fn lower_shared<'p: 'cell>(
        writer: Writer<'cell, '_>,
        part: &'p ExpressionPart<'p>,
        types: &Chain<'_, 'cell, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Option<Value<'cell, X>> {
        lowers(part).then(|| lower(writer, part, &|text| text, types, scratch))
    }
}

/// Whether [`lower`] takes `part` whole.
fn lowers(part: &ExpressionPart<'_>) -> bool {
    match part {
        ExpressionPart::Literal(_) => true,
        // A spread needs its list's value.
        ExpressionPart::ListLiteral(items) => {
            items.iter().all(|item| item.as_part().is_some_and(lowers))
        }
        ExpressionPart::DictLiteral(pairs) => pairs.iter().all(|(key, value)| {
            let scalar = match key {
                ExpressionPart::Literal(KLiteral::Number(number)) => !number.is_nan(),
                ExpressionPart::Literal(KLiteral::String(_) | KLiteral::Boolean(_)) => true,
                _ => false,
            };
            scalar && lowers(value)
        }),
        ExpressionPart::RecordLiteral(fields) => fields.iter().all(|(_, value)| lowers(value)),
        // A template with no brace is its text.
        ExpressionPart::Template(template) => template.slots.is_empty(),
        ExpressionPart::Keyword(_)
        | ExpressionPart::Identifier(_)
        | ExpressionPart::Type(_)
        | ExpressionPart::Expression(_)
        | ExpressionPart::SigiledTypeExpr(_)
        | ExpressionPart::RecordType(_)
        | ExpressionPart::QuotedExpression(_)
        | ExpressionPart::MarkedName(..)
        | ExpressionPart::MarkedUse(..)
        | ExpressionPart::FilledUse(..)
        | ExpressionPart::Splice(_) => false,
    }
}

/// The value of a part [`lowers`] took, each string's text laid in the region by `laid`.
fn lower<'cell, 'p, X: Knotted>(
    writer: Writer<'cell, '_>,
    part: &'p ExpressionPart<'p>,
    laid: &dyn Fn(&'p str) -> &'cell str,
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Value<'cell, X> {
    match part {
        ExpressionPart::Literal(KLiteral::Number(number)) => Value::Number(*number),
        ExpressionPart::Literal(KLiteral::String(literal)) => Value::Str(laid(literal)),
        ExpressionPart::Template(template) => Value::Str(laid(template.pieces[0])),
        ExpressionPart::Literal(KLiteral::Boolean(flag)) => Value::Bool(*flag),
        ExpressionPart::Literal(KLiteral::Null) => Value::Null,
        ExpressionPart::ListLiteral(items) => {
            let mut cells = BumpVec::with_capacity_in(items.len(), scratch);
            cells.extend(
                items
                    .iter()
                    .map(|item| lower(writer, item.part(), laid, types, scratch)),
            );
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
                (key, lower(writer, value, laid, types, scratch))
            }));
            Value::Dict(Dict::new(writer, &entries, types, scratch))
        }
        ExpressionPart::RecordLiteral(fields) => {
            let mut cells = BumpVec::with_capacity_in(fields.len(), scratch);
            cells.extend(
                fields
                    .iter()
                    .map(|(name, value)| (*name, lower(writer, value, laid, types, scratch))),
            );
            Value::Record(Record::new(writer, &cells, types, scratch))
        }
        ExpressionPart::Keyword(_)
        | ExpressionPart::Identifier(_)
        | ExpressionPart::Type(_)
        | ExpressionPart::Expression(_)
        | ExpressionPart::SigiledTypeExpr(_)
        | ExpressionPart::RecordType(_)
        | ExpressionPart::QuotedExpression(_)
        | ExpressionPart::MarkedName(..)
        | ExpressionPart::MarkedUse(..)
        | ExpressionPart::FilledUse(..)
        | ExpressionPart::Splice(_) => unreachable!("a lowerable part needs no dispatch"),
    }
}
