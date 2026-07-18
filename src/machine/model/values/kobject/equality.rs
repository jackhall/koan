//! Structural value equality — the `==` / `!=` semantics over runtime values.
//!
//! [`KObject::value_equal`] walks two values per variant, returning `Ok(true)`/`Ok(false)` for a
//! comparable pair and `Err` when a banned operand (a `KFunction` or a `Module`) participates at any
//! depth. Values are acyclic by construction (see
//! [circular-value-construction.md](../../../../../roadmap/type_language/circular-value-construction.md)),
//! so the walk carries no cycle guard.
//!
//! The comparison is cross-lifetime (`&KObject<'a>` vs `&KObject<'b>`): a spliced expression part
//! opens its delivery envelope at a fresh brand per side, so the two carried values never share a
//! lifetime. The whole suite therefore threads independent slot (`'a`) and value (`'b`) lifetimes,
//! resting on the heterogeneous `KType` predicate suite ([`KType::satisfied_by`], `KType`'s
//! cross-lifetime `PartialEq`).
//!
//! Container variants (`List`/`Dict`/`Record`/`Tagged`) gate on a *comparability* relation: contents
//! are compared only when the memoized/ascribed type parameters are **related** (one `satisfied_by`
//! the other, in either direction); unrelated parameters yield `Ok(false)` without descending. This
//! makes `==` intentionally intransitive across ascriptions and is documented in the value-equality
//! design note.

use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::machine::model::types::{same_nominal, KType, TypeRegistry};
use crate::machine::model::values::{Carried, Held};

use super::KObject;

#[cfg(test)]
mod tests;

/// A comparison touched a banned operand — a value whose identity is generative, not structural, so
/// `==` is meaningless on it. The `==` / `!=` builtin renders each arm to a structured error; the
/// `Module` arm points the user at `(TYPE OF m1) == (TYPE OF m2)` for interface comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueEqualityError {
    Function,
    Module,
}

/// Comparability gate for container type parameters: the two are compared iff one `satisfied_by` the
/// other in either direction (an unrelated pair short-circuits the container to `Ok(false)`).
/// Cross-lifetime — the slot and value type parameters carry independent lifetimes.
fn types_related<'a, 'b>(a: &KType<'a>, b: &KType<'b>, types: &TypeRegistry) -> bool {
    a.satisfied_by(b, types) || b.satisfied_by(a, types)
}

impl<'a> KObject<'a> {
    /// Structural equality against `other`, the engine behind `==` / `!=`.
    ///
    /// Numbers follow IEEE (`NaN != NaN`, `-0.0 == 0.0`); nominal carriers (`Tagged`, `Wrapped`)
    /// compare identity first, so a `Wrapped` value is never equal to its bare representation.
    /// Containers gate on [`types_related`] before descending. Any comparison in which either side —
    /// at any depth — is a `KFunction` or `Module` is [`Err`], not `false`: these values are
    /// generative, and the builtin turns the error into a diagnostic. A shape short-circuit that
    /// never reaches a banned cell (e.g. a length mismatch) may return `Ok(false)` first; that
    /// asymmetry is intended — the error fires when a banned value actually participates.
    pub fn value_equal<'b>(
        &self,
        other: &KObject<'b>,
        types: &TypeRegistry,
    ) -> Result<bool, ValueEqualityError> {
        match (self, other) {
            // Banned operands first, so the error fires even against a mismatched-variant partner.
            (KObject::KFunction(_), _) | (_, KObject::KFunction(_)) => {
                Err(ValueEqualityError::Function)
            }
            (KObject::Module(_), _) | (_, KObject::Module(_)) => Err(ValueEqualityError::Module),

            (KObject::Number(a), KObject::Number(b)) => Ok(a == b),
            (KObject::KString(a), KObject::KString(b)) => Ok(a == b),
            (KObject::Bool(a), KObject::Bool(b)) => Ok(a == b),
            (KObject::Null, KObject::Null) => Ok(true),

            (KObject::List(items_a, elem_a), KObject::List(items_b, elem_b)) => {
                if !types_related(elem_a, elem_b, types) || items_a.len() != items_b.len() {
                    return Ok(false);
                }
                for (a, b) in items_a.iter().zip(items_b.iter()) {
                    if !held_equal(a, b, types)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }

            (KObject::Dict(map_a, k_a, v_a), KObject::Dict(map_b, k_b, v_b)) => {
                if !types_related(k_a, k_b, types)
                    || !types_related(v_a, v_b, types)
                    || map_a.len() != map_b.len()
                {
                    return Ok(false);
                }
                for (key, held_a) in map_a.iter() {
                    match map_b.get(key) {
                        Some(held_b) if held_equal(held_a, held_b, types)? => {}
                        _ => return Ok(false),
                    }
                }
                Ok(true)
            }

            (KObject::Record(fields_a, types_a), KObject::Record(fields_b, types_b)) => {
                let ra = KType::record(types_a.clone());
                let rb = KType::record(types_b.clone());
                if !(ra.satisfied_by(&rb, types) || rb.satisfied_by(&ra, types))
                    || fields_a.len() != fields_b.len()
                {
                    return Ok(false);
                }
                // Order-blind: same name set, per-name held equality.
                for (name, held_a) in fields_a.iter() {
                    match fields_b.get(name) {
                        Some(held_b) if held_equal(held_a, held_b, types)? => {}
                        _ => return Ok(false),
                    }
                }
                Ok(true)
            }

            (
                KObject::Tagged {
                    value: value_a,
                    set: set_a,
                    index: index_a,
                    type_args: args_a,
                    ..
                },
                KObject::Tagged {
                    value: value_b,
                    set: set_b,
                    index: index_b,
                    type_args: args_b,
                    ..
                },
            ) => {
                if !same_nominal(set_a, *index_a, set_b, *index_b) {
                    return Ok(false);
                }
                // Empty args on either side = erased = comparable; both populated must agree on
                // arity and be pairwise related.
                if !args_a.is_empty() && !args_b.is_empty() {
                    if args_a.len() != args_b.len() {
                        return Ok(false);
                    }
                    if !args_a
                        .iter()
                        .zip(args_b.iter())
                        .all(|(x, y)| types_related(x, y, types))
                    {
                        return Ok(false);
                    }
                }
                value_a.value_equal(value_b, types)
            }

            (
                KObject::Wrapped {
                    inner: inner_a,
                    type_id: type_id_a,
                },
                KObject::Wrapped {
                    inner: inner_b,
                    type_id: type_id_b,
                },
            ) => {
                // Nominal identity via the digest-based `KType` equality; a `Wrapped` is never equal
                // to its bare representation because that pair falls to the `Ok(false)` catch-all.
                if **type_id_a != **type_id_b {
                    return Ok(false);
                }
                inner_a.get().value_equal(inner_b.get(), types)
            }

            (KObject::KExpression(a), KObject::KExpression(b)) => expression_equal(a, b, types),

            // Every remaining cross-variant pair (including `Wrapped` vs a bare value) is unequal.
            _ => Ok(false),
        }
    }
}

/// Cell-wise equality: two objects walk structurally, two types compare by digest (the cross-lifetime
/// `KType` `PartialEq`), a mixed object/type pair is unequal.
fn held_equal<'a, 'b>(
    a: &Held<'a>,
    b: &Held<'b>,
    types: &TypeRegistry,
) -> Result<bool, ValueEqualityError> {
    match (a, b) {
        (Held::Object(oa), Held::Object(ob)) => oa.value_equal(ob, types),
        (Held::Type(ta), Held::Type(tb)) => Ok(ta == tb),
        _ => Ok(false),
    }
}

/// Structural equality of quoted code: same part count, pairwise [`part_equal`]. Syntax equality, not
/// value equality — literal parts compare by their written form, and list/dict/record *literals*
/// compare order-sensitively (they are syntax, not the values they would evaluate to).
pub(crate) fn expression_equal<'a, 'b>(
    a: &KExpression<'a>,
    b: &KExpression<'b>,
    types: &TypeRegistry,
) -> Result<bool, ValueEqualityError> {
    if a.parts.len() != b.parts.len() {
        return Ok(false);
    }
    for (pa, pb) in a.parts.iter().zip(b.parts.iter()) {
        if !part_equal(&pa.value, &pb.value, types)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn part_equal<'a, 'b>(
    a: &ExpressionPart<'a>,
    b: &ExpressionPart<'b>,
    types: &TypeRegistry,
) -> Result<bool, ValueEqualityError> {
    use ExpressionPart::*;
    match (a, b) {
        (Keyword(x), Keyword(y)) => Ok(x == y),
        (Identifier(x), Identifier(y)) => Ok(x == y),
        (Type(x), Type(y)) => Ok(x.render() == y.render()),
        (Literal(x), Literal(y)) => Ok(literal_equal(x, y)),
        (Expression(x), Expression(y))
        | (SigiledTypeExpr(x), SigiledTypeExpr(y))
        | (RecordType(x), RecordType(y))
        | (QuotedExpression(x), QuotedExpression(y)) => expression_equal(x, y, types),
        (ListLiteral(xs), ListLiteral(ys)) => {
            if xs.len() != ys.len() {
                return Ok(false);
            }
            for (x, y) in xs.iter().zip(ys.iter()) {
                if !part_equal(x, y, types)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (DictLiteral(xs), DictLiteral(ys)) => {
            if xs.len() != ys.len() {
                return Ok(false);
            }
            for ((kx, vx), (ky, vy)) in xs.iter().zip(ys.iter()) {
                if !part_equal(kx, ky, types)? || !part_equal(vx, vy, types)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (RecordLiteral(xs), RecordLiteral(ys)) => {
            if xs.len() != ys.len() {
                return Ok(false);
            }
            for ((nx, vx), (ny, vy)) in xs.iter().zip(ys.iter()) {
                if nx != ny || !part_equal(vx, vy, types)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        // A spliced result compares by the value walk: open both envelopes at their own brand (hence
        // the cross-lifetime comparison) and compare the carried values.
        (Spliced { cell: cell_a }, Spliced { cell: cell_b }) => cell_a
            .open(|carried_a| cell_b.open(|carried_b| carried_equal(carried_a, carried_b, types))),
        _ => Ok(false),
    }
}

/// Literal-part equality. Number literals follow IEEE, matching the value semantics.
fn literal_equal(a: &KLiteral, b: &KLiteral) -> bool {
    match (a, b) {
        (KLiteral::Number(x), KLiteral::Number(y)) => x == y,
        (KLiteral::String(x), KLiteral::String(y)) => x == y,
        (KLiteral::Boolean(x), KLiteral::Boolean(y)) => x == y,
        (KLiteral::Null, KLiteral::Null) => true,
        _ => false,
    }
}

/// Two spliced carried values: objects walk structurally, types compare by digest, a mixed pair is
/// unequal — the [`Held`] semantics over the borrowed [`Carried`] currency.
fn carried_equal<'a, 'b>(
    a: Carried<'a>,
    b: Carried<'b>,
    types: &TypeRegistry,
) -> Result<bool, ValueEqualityError> {
    match (a, b) {
        (Carried::Object(oa), Carried::Object(ob)) => oa.value_equal(ob, types),
        (Carried::Type(ta), Carried::Type(tb)) => Ok(ta == tb),
        _ => Ok(false),
    }
}
