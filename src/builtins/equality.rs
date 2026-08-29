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

use crate::machine::WriteGate;
use crate::machine::model::{Held, KType, ValueEqualityError};
use crate::machine::{Action, BodyCtx};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};
use crate::machine::model::RunRegistries;
use crate::machine::model::Scalar;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { left, right } }

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
    registries: &RunRegistries,
) -> Result<bool, KError> {
    match (left, right) {
        (Held::Object(a), Held::Object(b)) => {
            a.value_equal(b, registries).map_err(|e| ban_error(op, e))
        }
        (Held::Type(a), Held::Type(b)) => Ok(a == b),
        _ => Ok(false),
    }
}

/// Read both operands and compare, or the canonical missing-arg diagnostic.
fn compare(ctx: &BodyCtx<'_, '_, '_>, op: &str) -> Result<bool, KError> {
    let left = ctx
        .args
        .held(&SLOTS.left)
        .ok_or_else(|| KError::new(KErrorKind::MissingArg("left".to_string())))?;
    let right = ctx
        .args
        .held(&SLOTS.right)
        .ok_or_else(|| KError::new(KErrorKind::MissingArg("right".to_string())))?;
    cells_equal(left, right, op, ctx.registries)
}

pub fn body_eq<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let equal = crate::try_action!(compare(ctx, "=="));
    Action::done(Ok(ctx
        .scope
        .brand()
        .alloc_scalar_witnessed(Scalar::Bool(equal))))
}

pub fn body_ne<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let equal = crate::try_action!(compare(ctx, "!="));
    Action::done(Ok(ctx
        .scope
        .brand()
        .alloc_scalar_witnessed(Scalar::Bool(!equal))))
}

/// Register `==` / `!=` as binary-only builtins. Deliberately **not** seeded into any operator
/// group (see [`super::arithmetic::register_builtin_operator_groups`]) — equality does not chain.
pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let eq_sig = |op: &'static str| {
        sig(
            KType::BOOL,
            vec![
                arg(registries, &SLOTS.left, KType::ANY),
                kw(registries, op),
                arg(registries, &SLOTS.right, KType::ANY),
            ],
        )
    };
    crate::builtins::register_builtin(scope, eq_sig("=="), body_eq, registries, gate);
    crate::builtins::register_builtin(scope, eq_sig("!="), body_ne, registries, gate);
}

#[cfg(test)]
mod tests;
