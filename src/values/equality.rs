//! Structural value equality — what `==` means over data values.
//!
//! Containers compare their contents only when their memoized types are related, one satisfied by
//! the other in either direction; an unrelated pair is unequal without descending. That makes `==`
//! intransitive across ascriptions by design.

use crate::memory::BumpAllocator;
use crate::parse::{ExpressionPart, KExpression, KLiteral};
use crate::type_lattice::{KType, TypeRegistry, satisfied_by};

use super::Value;

impl Value<'_, '_> {
    /// Whether two values are equal. Numbers follow IEEE (`NaN != NaN`, `-0 == 0`); a tagged value
    /// compares its identity first, so it never equals its bare payload; two types are equal when
    /// they are the same handle; two quoted expressions compare as syntax, part by part with spans
    /// ignored. The two sides may live at unrelated lifetimes.
    pub fn equals(
        &self,
        other: &Value<'_, '_>,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> bool {
        let related = |left: KType, right: KType| {
            satisfied_by(types, scratch, left, right) || satisfied_by(types, scratch, right, left)
        };
        match (self, other) {
            (Value::Number(left), Value::Number(right)) => left == right,
            (Value::Bool(left), Value::Bool(right)) => left == right,
            (Value::Null, Value::Null) => true,
            (Value::Str(left), Value::Str(right)) => left == right,
            (Value::Expression(left), Value::Expression(right)) => expression_equal(left, right),
            (Value::Type(left), Value::Type(right)) => left.handle() == right.handle(),
            (Value::List(left), Value::List(right)) => {
                related(left.ktype(), right.ktype())
                    && cells_equal(left.cells(), right.cells(), types, scratch)
            }
            (Value::Dict(left), Value::Dict(right)) => {
                related(left.ktype(), right.ktype())
                    && left.keys() == right.keys()
                    && cells_equal(left.cells(), right.cells(), types, scratch)
            }
            (Value::Record(left), Value::Record(right)) => {
                related(left.ktype(), right.ktype())
                    && left.names() == right.names()
                    && cells_equal(left.cells(), right.cells(), types, scratch)
            }
            (Value::Tagged(left), Value::Tagged(right)) => {
                left.ktype() == right.ktype()
                    && left.payload().equals(right.payload(), types, scratch)
            }
            _ => false,
        }
    }
}

/// Two aligned runs of cells, equal in length and pairwise.
fn cells_equal(
    left: &[Value<'_, '_>],
    right: &[Value<'_, '_>],
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.equals(right, types, scratch))
}

/// Quoted code as syntax: the same parts in the same order. A literal compares by what was written
/// and a container literal order-sensitively, since it is syntax and not the value it would build.
fn expression_equal(left: &KExpression<'_>, right: &KExpression<'_>) -> bool {
    left.parts.len() == right.parts.len()
        && left
            .parts
            .iter()
            .zip(right.parts)
            .all(|(left, right)| part_equal(&left.value, &right.value))
}

fn part_equal(left: &ExpressionPart<'_>, right: &ExpressionPart<'_>) -> bool {
    use ExpressionPart as Part;
    match (left, right) {
        (Part::Keyword(left), Part::Keyword(right)) => left == right,
        (Part::Identifier(left), Part::Identifier(right)) => left == right,
        (Part::Type(left), Part::Type(right)) => left.symbol() == right.symbol(),
        (Part::Literal(left), Part::Literal(right)) => literal_equal(left, right),
        (Part::Expression(left), Part::Expression(right))
        | (Part::SigiledTypeExpr(left), Part::SigiledTypeExpr(right))
        | (Part::RecordType(left), Part::RecordType(right))
        | (Part::QuotedExpression(left), Part::QuotedExpression(right)) => {
            expression_equal(left, right)
        }
        (Part::ListLiteral(left), Part::ListLiteral(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(left, right)| part_equal(left, right))
        }
        (Part::DictLiteral(left), Part::DictLiteral(right)) => {
            left.len() == right.len()
                && left.iter().zip(right.iter()).all(|(left, right)| {
                    part_equal(&left.0, &right.0) && part_equal(&left.1, &right.1)
                })
        }
        (Part::RecordLiteral(left), Part::RecordLiteral(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(left, right)| left.0 == right.0 && part_equal(&left.1, &right.1))
        }
        _ => false,
    }
}

/// Literal equality, IEEE for numbers as the values it lowers to are.
fn literal_equal(left: &KLiteral<'_>, right: &KLiteral<'_>) -> bool {
    match (left, right) {
        (KLiteral::Number(left), KLiteral::Number(right)) => left == right,
        (KLiteral::String(left), KLiteral::String(right)) => left == right,
        (KLiteral::Boolean(left), KLiteral::Boolean(right)) => left == right,
        (KLiteral::Null, KLiteral::Null) => true,
        _ => false,
    }
}
