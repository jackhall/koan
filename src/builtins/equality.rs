//! Structural equality operators `==` and `!=` over `:Any` operands, returning `:Bool`.
//!
//! Both are **binary-only**: unlike the arithmetic comparison operators they are *not* seeded
//! into any operator group, so a chain (`a == b == c`) draws a keyword subset no group covers and
//! surfaces a resolution error rather than reducing pairwise. This is deliberate — equality does
//! not associate.
//!
//! Each `:Any` slot admits either channel, so a body reads its operands as raw [`Held`] cells: two
//! objects compare by [`KObject::value_equal`], two types by digest ([`KType`]'s cross-lifetime
//! `PartialEq`), and a mixed object/type pair is unequal. A comparison touching a banned operand (a
//! function or module value, at any depth) is a [`ValueEqualityError`], which the body renders to a
//! structured [`KErrorKind::User`] error — the module arm points at `(TYPE OF m1) == (TYPE OF m2)`
//! for interface comparison. `!=` negates a successful comparison and propagates the error
//! unchanged (an error is never negated into a `false`).

use crate::machine::model::TypeRegistry;
use crate::machine::model::{Held, KObject, KType, ValueEqualityError};
use crate::machine::{arg_held, Action, BodyCtx};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};

/// Render a banned-operand error for operator `op`.
fn ban_error(op: &str, error: ValueEqualityError) -> KError {
    let message = match error {
        ValueEqualityError::Function => {
            format!("{op} : a function value has no structural equality")
        }
        ValueEqualityError::Module => format!(
            "{op} : a module value has no structural equality; \
             compare interfaces with (TYPE OF m1) == (TYPE OF m2)"
        ),
    };
    KError::new(KErrorKind::User(message))
}

/// Compare the `left` / `right` operands as raw cells: objects structurally, types by digest, a
/// mixed channel unequal. `op` labels a banned-operand error.
fn cells_equal(
    left: &Held<'_>,
    right: &Held<'_>,
    op: &str,
    types: &TypeRegistry,
) -> Result<bool, KError> {
    match (left, right) {
        (Held::Object(a), Held::Object(b)) => a.value_equal(b, types).map_err(|e| ban_error(op, e)),
        (Held::Type(a), Held::Type(b)) => Ok(a == b),
        _ => Ok(false),
    }
}

/// Read both operands and compare, or the canonical missing-arg diagnostic.
fn compare(ctx: &BodyCtx<'_, '_>, op: &str) -> Result<bool, KError> {
    let left = arg_held(ctx.args, "left")
        .ok_or_else(|| KError::new(KErrorKind::MissingArg("left".to_string())))?;
    let right = arg_held(ctx.args, "right")
        .ok_or_else(|| KError::new(KErrorKind::MissingArg("right".to_string())))?;
    cells_equal(left, right, op, ctx.types)
}

pub fn body_eq<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    let equal = crate::try_action!(compare(ctx, "=="));
    Action::Done(Ok(ctx
        .scope
        .brand()
        .alloc_object_witnessed(KObject::Bool(equal))))
}

pub fn body_ne<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    let equal = crate::try_action!(compare(ctx, "!="));
    Action::Done(Ok(ctx
        .scope
        .brand()
        .alloc_object_witnessed(KObject::Bool(!equal))))
}

/// Register `==` / `!=` as binary-only builtins. Deliberately **not** seeded into any operator
/// group (see [`super::arithmetic::register_builtin_operator_groups`]) — equality does not chain.
pub fn register<'a>(scope: &'a Scope<'a>) {
    let eq_sig = |op: &str| {
        sig(
            KType::Bool,
            vec![arg("left", KType::Any), kw(op), arg("right", KType::Any)],
        )
    };
    crate::builtins::register_builtin(scope, "==", eq_sig("=="), body_eq);
    crate::builtins::register_builtin(scope, "!=", eq_sig("!="), body_ne);
}

#[cfg(test)]
mod tests;
