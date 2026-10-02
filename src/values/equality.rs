//! Structural value equality — what `==` means over data values.
//!
//! A value compares at the type it is seen at, through [the door](super::surface): a record by the
//! fields that type names, each part at its type there. Containers compare their contents only
//! when their seen types are related, one satisfied by the other in either direction; an unrelated
//! pair is unequal without descending. That makes `==` intransitive across ascriptions by design.
//!
//! A value sealed behind an opaque view compares as its payload wherever the seal's bound reveals
//! the payload's kind ([`unsealed`]), so a sealed number behind a `Number`-bounded member equals
//! the number; any other seal compares by identity, as every tagged value does.
//!
//! A module or a barrier has no structural equality: a comparison that reaches one on either side
//! is [`Incomparable`], which the `==` builtin reports, never `false`. A function compares by its
//! identity — one per `FN`, `EXPR` or `OP` written — and its closure bindings; a quote's code by its
//! syntax, marks included, and the values its `$` names and its supplied holes bind.
//!
//! Two knot members compare as a bisimulation: a member pair, each beside the type it is seen at,
//! is recorded before what it holds is compared, and a recorded pair met again counts as equal. See
//! [README.md § Equality and rendering](README.md#equality-and-rendering) for why that is sound.
//!
//! The comparison runs over an explicit stack of pending pairs, so the stack it uses does not grow
//! with the values' depth. Each pair is a gate — related types, keys, names, lengths, identity —
//! or a leaf, and a passed gate pushes its children. Every answer is a conjunction and any
//! incomparable pair decides, so the result is `Incomparable` if any reached pair is, and otherwise
//! the AND of every gate and leaf reached: neither depends on visit order, and `seen` only grows.
//! A quote's syntax is compared recursively ([`expression_equal`]): parsed syntax nests no deeper
//! than [`MAX_SYNTAX_DEPTH`](crate::parse::MAX_SYNTAX_DEPTH).

use crate::memory::{BumpAllocator, BumpBackedSet, BumpVec, bump_set};
use crate::parse::{ExpressionPart, KExpression, KLiteral};
use crate::type_lattice::{DeclaredType, KType, TypeRegistry, satisfied_by};

use super::circular::{CodeView, Resolved};
use super::surface::Parts;
use super::{Knotted, Seen, Surface, Value, unsealed};

/// A comparison reached a module or a barrier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Incomparable;

/// The pairs a comparison has yet to compare, each value beside the type it is seen at.
type Pending<'x, 'left, 'right, X, Y> = BumpVec<'x, (Seen<'left, X>, Seen<'right, Y>)>;

/// The member pairs a comparison has entered, each member beside the type it is seen at: one node
/// seen at two types shows two surfaces.
type Entered<'x, X, Y> = BumpBackedSet<'x, ((X, DeclaredType<KType>), (Y, DeclaredType<KType>))>;

impl<'left, X: Knotted> Value<'left, X> {
    /// Whether two values are equal. Numbers follow IEEE (`NaN != NaN`, `-0 == 0`); a tagged value
    /// compares its identity first, so it never equals its bare payload — save a sealed value whose
    /// seal [`unsealed`] reads through, which compares as its payload; two types are equal when
    /// they are the same handle; a knot's data node compares as the plain value of its kind would,
    /// its cells read through it; two functions by identity, then their closure bindings; two
    /// quotes' code as syntax, part by part with spans ignored, then the values their names bind.
    /// Each side compares at the type it is seen at: a record by the fields that type names, a
    /// tagged value's payload at its representation. The two sides may live at unrelated
    /// lifetimes. A module or a barrier on either side is [`Incomparable`], and so is a pair whose
    /// contents reach one; a container pair with unrelated types is unequal without descending,
    /// whatever it holds. The pending pairs and the member pairs a comparison has entered are
    /// staged over `scratch`.
    pub fn equals<'right, Y: Knotted>(
        &self,
        other: &Value<'right, Y>,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> Result<bool, Incomparable> {
        let mut seen = bump_set(scratch);
        let mut pending: Pending<'_, 'left, 'right, X, Y> = BumpVec::new_in(scratch);
        pending.push((Seen::of(*self), Seen::of(*other)));
        let mut equal = true;
        while let Some((left, right)) = pending.pop() {
            equal &= pair_equal(left, right, types, scratch, &mut seen, &mut pending)?;
        }
        Ok(equal)
    }
}

/// `seen` read through its seal where [`unsealed`] reads through it: the payload, at its own memo.
fn unsealed_seen<'cell, X: Knotted>(
    seen: Seen<'cell, X>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Seen<'cell, X> {
    match (seen.value(), unsealed(seen.value(), types, scratch)) {
        (Value::Tagged(sealed), Value::Tagged(read)) if std::ptr::eq(sealed, read) => seen,
        (Value::Tagged(_), payload) => Seen::of(payload),
        _ => seen,
    }
}

/// One pair's own answer — its gate or its leaf — with the children a passed gate pushes onto
/// `pending`. A pair of knot members already entered at the same seen types answers `true` and
/// pushes nothing.
fn pair_equal<'left, 'right, X: Knotted, Y: Knotted>(
    left: Seen<'left, X>,
    right: Seen<'right, Y>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    seen: &mut Entered<'_, X, Y>,
    pending: &mut Pending<'_, 'left, 'right, X, Y>,
) -> Result<bool, Incomparable> {
    // A seal whose bound reveals its payload's kind is read through, on either side.
    let this = unsealed_seen(left, types, scratch);
    let that = unsealed_seen(right, types, scratch);
    if this.value().as_opaque().is_some() || that.value().as_opaque().is_some() {
        return Err(Incomparable);
    }
    if let (Value::Knotted(left), Value::Knotted(right)) = (this.value(), that.value()) {
        let entered = ((left, this.ktype()), (right, that.ktype()));
        match (left.resolve(), right.resolve()) {
            (
                Resolved::Function {
                    identity,
                    instance,
                    closure,
                },
                Resolved::Function {
                    identity: other,
                    instance: solved,
                    closure: others,
                },
            ) => {
                if !seen.insert(entered) {
                    return Ok(true);
                }
                if identity != other || instance != solved || closure.len() != others.len() {
                    return Ok(false);
                }
                pending.extend(closure.iter().zip(others).map(|(this, that)| {
                    (Seen::of(this.resolve(left)), Seen::of(that.resolve(right)))
                }));
                return Ok(true);
            }
            (Resolved::Code(code), Resolved::Code(other)) => {
                if !seen.insert(entered) {
                    return Ok(true);
                }
                return Ok(code_gate((code, left), (other, right), pending));
            }
            _ => {}
        }
    }
    Ok(
        match (this.surface(types, scratch), that.surface(types, scratch)) {
            (Some(left), Some(right)) => {
                if let (Some(left_node), Some(right_node)) = (left.node(), right.node())
                    && !seen.insert((
                        (left_node, left.ktype().into()),
                        (right_node, right.ktype().into()),
                    ))
                {
                    return Ok(true);
                }
                if !surface_gate(&left, &right, types, scratch) {
                    return Ok(false);
                }
                pending.extend((0..left.len()).map(|at| {
                    (
                        left.child(at, types, scratch),
                        right.child(at, types, scratch),
                    )
                }));
                true
            }
            (Some(_), None) | (None, Some(_)) => false,
            (None, None) => match (this.value(), that.value()) {
                (Value::Number(left), Value::Number(right)) => left == right,
                (Value::Bool(left), Value::Bool(right)) => left == right,
                (Value::Null, Value::Null) => true,
                (Value::Str(left), Value::Str(right)) => left == right,
                (Value::Type(left), Value::Type(right)) => left.handle() == right.handle(),
                _ => false,
            },
        },
    )
}

/// Whether two surfaces may be equal before their parts are compared: one kind, related seen
/// types, then the same keys, visible names or identity, and as many parts.
fn surface_gate<X: Knotted, Y: Knotted>(
    left: &Surface<'_, '_, X>,
    right: &Surface<'_, '_, Y>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> bool {
    let related = |left: KType, right: KType| {
        satisfied_by(types, scratch, left, right) || satisfied_by(types, scratch, right, left)
    };
    let (ktype, other) = (left.ktype(), right.ktype());
    let length = left.len() == right.len();
    match (left.parts(), right.parts()) {
        (Parts::List { .. }, Parts::List { .. }) => related(ktype, other) && length,
        (Parts::Dict { keys, .. }, Parts::Dict { keys: others, .. }) => {
            related(ktype, other) && keys == others && length
        }
        (Parts::Record { .. }, Parts::Record { .. }) => {
            related(ktype, other)
                && length
                && (0..left.len()).all(|at| left.name(at) == right.name(at))
        }
        (Parts::Tagged { .. }, Parts::Tagged { .. }) => ktype == other,
        _ => false,
    }
}

/// Two quotes' code: the same syntax, and the same names bound by their `$` names and supplied
/// holes. When they are, each name's two values are pushed onto `pending`, bound then supplied.
fn code_gate<'left, 'right, X: Knotted, Y: Knotted>(
    (left, holder): (CodeView<'left, X>, X),
    (right, other): (CodeView<'right, Y>, Y),
    pending: &mut Pending<'_, 'left, 'right, X, Y>,
) -> bool {
    let names = |left: &[(_, _)], right: &[(_, _)]| {
        left.len() == right.len()
            && left
                .iter()
                .zip(right)
                .all(|(left, right)| left.0 == right.0)
    };
    if !expression_equal(left.body, right.body)
        || !names(left.bound, right.bound)
        || !names(left.supplied, right.supplied)
    {
        return false;
    }
    for (left, right) in [(left.bound, right.bound), (left.supplied, right.supplied)] {
        pending.extend(left.iter().zip(right).map(|(left, right)| {
            (
                Seen::of(left.1.resolve(holder)),
                Seen::of(right.1.resolve(other)),
            )
        }));
    }
    true
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
        (Part::MarkedName(mark, left), Part::MarkedName(other, right)) => {
            mark == other && left == right
        }
        (Part::MarkedUse(mark, left), Part::MarkedUse(other, right)) => {
            mark == other && expression_equal(left, right)
        }
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
