//! Structural value equality — what `==` means over data values.
//!
//! Containers compare their contents only when their memoized types are related, one satisfied by
//! the other in either direction; an unrelated pair is unequal without descending. That makes `==`
//! intransitive across ascriptions by design.
//!
//! A value sealed behind an opaque view compares as its payload wherever the seal's bound reveals
//! the payload's kind ([`unsealed`]), so a sealed number behind a `Number`-bounded member equals
//! the number; any other seal compares by identity, as every tagged value does.
//!
//! A function has no structural equality: a comparison that reaches one on either side is
//! [`Incomparable`], which the `==` builtin reports, never `false`.
//!
//! Two data nodes of knots compare as a bisimulation: a node pair is recorded before its cells are
//! compared, and a recorded pair met again counts as equal. See
//! [README.md § Equality and rendering](README.md#equality-and-rendering) for why that is sound.

use crate::memory::{BumpAllocator, BumpBackedSet, bump_set};
use crate::parse::{ExpressionPart, KExpression, KLiteral};
use crate::type_lattice::{KType, TypeRegistry, satisfied_by};

use super::circular::{Cells, Composite};
use super::{Knotted, Value, unsealed};

/// A comparison reached a function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Incomparable;

impl<X: Knotted> Value<'_, '_, X> {
    /// Whether two values are equal. Numbers follow IEEE (`NaN != NaN`, `-0 == 0`); a tagged value
    /// compares its identity first, so it never equals its bare payload — save a sealed value whose
    /// seal [`unsealed`] reads through, which compares as its payload; two types are equal when
    /// they are the same handle; two quoted expressions compare as syntax, part by part with spans
    /// ignored; a knot's data node compares as the plain value of its kind would, its cells read
    /// through it. The two sides may live at unrelated lifetimes. A function on either side is
    /// [`Incomparable`], and so is a pair of containers whose contents reach one; a container pair
    /// with unrelated types is unequal without descending, whatever it holds. The node pairs a
    /// comparison has entered are staged over `scratch`.
    pub fn equals<Y: Knotted>(
        &self,
        other: &Value<'_, '_, Y>,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> Result<bool, Incomparable> {
        let mut seen = bump_set(scratch);
        self.equals_within(other, types, scratch, &mut seen)
    }

    /// [`equals`](Self::equals) under the node pairs already entered.
    fn equals_within<Y: Knotted>(
        &self,
        other: &Value<'_, '_, Y>,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        seen: &mut BumpBackedSet<'_, (X, Y)>,
    ) -> Result<bool, Incomparable> {
        // A seal whose bound reveals its payload's kind is read through, on either side.
        let this = unsealed(*self, types, scratch);
        let that = unsealed(*other, types, scratch);
        if this.as_opaque().is_some() || that.as_opaque().is_some() {
            return Err(Incomparable);
        }
        Ok(match (this.composite(), that.composite()) {
            (Some((left_node, left)), Some((right_node, right))) => {
                if let (Some(left_node), Some(right_node)) = (left_node, right_node)
                    && !seen.insert((left_node, right_node))
                {
                    return Ok(true);
                }
                composite_equal(left, right, types, scratch, seen)?
            }
            (Some(_), None) | (None, Some(_)) => false,
            (None, None) => match (&this, &that) {
                (Value::Number(left), Value::Number(right)) => left == right,
                (Value::Bool(left), Value::Bool(right)) => left == right,
                (Value::Null, Value::Null) => true,
                (Value::Str(left), Value::Str(right)) => left == right,
                (Value::Expression(left), Value::Expression(right)) => {
                    expression_equal(left, right)
                }
                (Value::Type(left), Value::Type(right)) => left.handle() == right.handle(),
                _ => false,
            },
        })
    }
}

/// Two composites of one kind: related types, then keys, names or identity, then the cells.
fn composite_equal<X: Knotted, Y: Knotted>(
    left: Composite<'_, X>,
    right: Composite<'_, Y>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    seen: &mut BumpBackedSet<'_, (X, Y)>,
) -> Result<bool, Incomparable> {
    let related = |left: KType, right: KType| {
        satisfied_by(types, scratch, left, right) || satisfied_by(types, scratch, right, left)
    };
    Ok(match (left, right) {
        (
            Composite::List { ktype, cells },
            Composite::List {
                ktype: other,
                cells: others,
            },
        ) => related(ktype, other) && cells_equal(cells, others, types, scratch, seen)?,
        (
            Composite::Dict { ktype, keys, cells },
            Composite::Dict {
                ktype: other,
                keys: other_keys,
                cells: others,
            },
        ) => {
            related(ktype, other)
                && keys == other_keys
                && cells_equal(cells, others, types, scratch, seen)?
        }
        (
            Composite::Record {
                ktype,
                names,
                cells,
            },
            Composite::Record {
                ktype: other,
                names: other_names,
                cells: others,
            },
        ) => {
            related(ktype, other)
                && names == other_names
                && cells_equal(cells, others, types, scratch, seen)?
        }
        (
            Composite::Tagged { ktype, payload },
            Composite::Tagged {
                ktype: other,
                payload: other_payload,
            },
        ) => ktype == other && payload.equals_within(&other_payload, types, scratch, seen)?,
        _ => false,
    })
}

/// Two aligned runs of cells, equal in length and pairwise. The first incomparable pair decides,
/// even past an unequal one, so whether a comparison reaches a function does not depend on order.
fn cells_equal<X: Knotted, Y: Knotted>(
    left: Cells<'_, X>,
    right: Cells<'_, Y>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    seen: &mut BumpBackedSet<'_, (X, Y)>,
) -> Result<bool, Incomparable> {
    if left.len() != right.len() {
        return Ok(false);
    }
    let mut equal = true;
    for (left, right) in left.iter().zip(right.iter()) {
        equal &= left.equals_within(&right, types, scratch, seen)?;
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
