//! Structural value equality — what `==` means over data values.
//!
//! Containers compare their contents only when their memoized types are related, one satisfied by
//! the other in either direction; an unrelated pair is unequal without descending. That makes `==`
//! intransitive across ascriptions by design.
//!
//! A callable has no structural equality: a comparison that reaches one on either side is
//! [`Incomparable`], which the `==` builtin reports, never `false`.

use crate::memory::BumpAllocator;
use crate::parse::{ExpressionPart, KExpression, KLiteral};
use crate::type_lattice::{KType, TypeRegistry, satisfied_by};

use super::{Callable, Value};

/// A comparison reached a callable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Incomparable;

impl<X: Callable> Value<'_, '_, X> {
    /// Whether two values are equal. Numbers follow IEEE (`NaN != NaN`, `-0 == 0`); a tagged value
    /// compares its identity first, so it never equals its bare payload; two types are equal when
    /// they are the same handle; two quoted expressions compare as syntax, part by part with spans
    /// ignored. The two sides may live at unrelated lifetimes. A callable on either side is
    /// [`Incomparable`], and so is a pair of containers whose contents reach one; a container pair
    /// with unrelated types is unequal without descending, whatever it holds.
    pub fn equals<Y: Callable>(
        &self,
        other: &Value<'_, '_, Y>,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> Result<bool, Incomparable> {
        let related = |left: KType, right: KType| {
            satisfied_by(types, scratch, left, right) || satisfied_by(types, scratch, right, left)
        };
        Ok(match (self, other) {
            (Value::Callable(_), _) | (_, Value::Callable(_)) => return Err(Incomparable),
            (Value::Number(left), Value::Number(right)) => left == right,
            (Value::Bool(left), Value::Bool(right)) => left == right,
            (Value::Null, Value::Null) => true,
            (Value::Str(left), Value::Str(right)) => left == right,
            (Value::Expression(left), Value::Expression(right)) => expression_equal(left, right),
            (Value::Type(left), Value::Type(right)) => left.handle() == right.handle(),
            (Value::List(left), Value::List(right)) => {
                related(left.ktype(), right.ktype())
                    && cells_equal(left.cells(), right.cells(), types, scratch)?
            }
            (Value::Dict(left), Value::Dict(right)) => {
                related(left.ktype(), right.ktype())
                    && left.keys() == right.keys()
                    && cells_equal(left.cells(), right.cells(), types, scratch)?
            }
            (Value::Record(left), Value::Record(right)) => {
                related(left.ktype(), right.ktype())
                    && left.names() == right.names()
                    && cells_equal(left.cells(), right.cells(), types, scratch)?
            }
            (Value::Tagged(left), Value::Tagged(right)) => {
                left.ktype() == right.ktype()
                    && left.payload().equals(right.payload(), types, scratch)?
            }
            _ => false,
        })
    }
}

/// Two aligned runs of cells, equal in length and pairwise. The first incomparable pair decides,
/// even past an unequal one, so whether a comparison reaches a callable does not depend on order.
fn cells_equal<X: Callable, Y: Callable>(
    left: &[Value<'_, '_, X>],
    right: &[Value<'_, '_, Y>],
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<bool, Incomparable> {
    if left.len() != right.len() {
        return Ok(false);
    }
    let mut equal = true;
    for (left, right) in left.iter().zip(right) {
        equal &= left.equals(right, types, scratch)?;
    }
    Ok(equal)
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
