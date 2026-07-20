//! Binary arithmetic (`+ - * /`) and comparison (`< <= > >=`) builtins over `:Number`
//! operands, plus `AND` over `:Bool`. Each of these keywords is a fixed-syntax operator
//! token dispatched through the ordinary keyworded bucket, exactly like any other binary
//! keyworded builtin — not a callable function name. `AND` is registered as a plain
//! keyworded builtin here (it is the pairwise-mode combiner keyword the operator-group
//! reducer folds pair results through; it is not itself a group member).
//!
//! Each body is an action builtin: it reads its two typed operands, computes the owned
//! scalar, and returns it the way [`super::print::body`] returns its rendered string — a
//! fresh `KObject::Bool`/`KObject::Number` born witnessed at the empty (region-pure)
//! reach, no folded placement. The `:Number`/`:Bool` parameter types are dispatch's own
//! admission gate: a non-matching operand is a bucket miss before any body runs, so no
//! body re-checks operand types beyond the pattern match that reads the scalar out.
//!
//! [`register_builtin_operator_groups`] seeds the three builtin operator groups these
//! bodies serve: comparison (pairwise, combined by `AND`), additive, and multiplicative
//! (both fold-left). The registry record is member set + mode only — see
//! [`crate::machine::model::operators`] — so seeding is a separate step from registering
//! the per-operator bodies above.

use std::collections::HashSet;

use crate::machine::model::{FoldDirection, OperatorGroup, ReductionMode};
use crate::machine::model::{KObject, KType, TypeRegistry};
use crate::machine::BindingIndex;
use crate::machine::{arg_object, Action, BodyCtx};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};

/// Read a `:Number` operand named `name`, or the canonical missing/mismatch diagnostic.
fn number_arg(args: &KObject<'_>, name: &str, types: &TypeRegistry) -> Result<f64, KError> {
    match arg_object(args, name) {
        Some(KObject::Number(n)) => Ok(*n),
        Some(other) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: name.to_string(),
            expected: "Number".to_string(),
            got: other.ktype().name(types),
        })),
        None => Err(KError::new(KErrorKind::MissingArg(name.to_string()))),
    }
}

/// Read the `left` / `right` `:Number` operands.
fn number_operands(args: &KObject<'_>, types: &TypeRegistry) -> Result<(f64, f64), KError> {
    Ok((
        number_arg(args, "left", types)?,
        number_arg(args, "right", types)?,
    ))
}

/// Read a `:Bool` operand named `name`, or the canonical missing/mismatch diagnostic.
fn bool_arg(args: &KObject<'_>, name: &str, types: &TypeRegistry) -> Result<bool, KError> {
    match arg_object(args, name) {
        Some(KObject::Bool(b)) => Ok(*b),
        Some(other) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: name.to_string(),
            expected: "Bool".to_string(),
            got: other.ktype().name(types),
        })),
        None => Err(KError::new(KErrorKind::MissingArg(name.to_string()))),
    }
}

/// Read the `left` / `right` `:Bool` operands.
fn bool_operands(args: &KObject<'_>, types: &TypeRegistry) -> Result<(bool, bool), KError> {
    Ok((
        bool_arg(args, "left", types)?,
        bool_arg(args, "right", types)?,
    ))
}

pub fn body_add<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.types));
    Action::Done(Ok(ctx
        .scope
        .brand()
        .alloc_object_witnessed(KObject::Number(left + right))))
}

pub fn body_sub<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.types));
    Action::Done(Ok(ctx
        .scope
        .brand()
        .alloc_object_witnessed(KObject::Number(left - right))))
}

pub fn body_mul<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.types));
    Action::Done(Ok(ctx
        .scope
        .brand()
        .alloc_object_witnessed(KObject::Number(left * right))))
}

/// `Number` has one representation (`f64`; see `KObject::Number`) and the codebase has no
/// prior division operator to match, so a zero divisor raises a structured `KError`
/// (`KErrorKind::User`, the in-language-error landing pad) rather than following IEEE 754's
/// infinity/NaN convention — no NaN value is ever minted onto a koan `Number`.
pub fn body_div<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.types));
    if right == 0.0 {
        return Action::Done(Err(KError::new(KErrorKind::User(
            "/ : division by zero".to_string(),
        ))));
    }
    Action::Done(Ok(ctx
        .scope
        .brand()
        .alloc_object_witnessed(KObject::Number(left / right))))
}

pub fn body_lt<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.types));
    Action::Done(Ok(ctx
        .scope
        .brand()
        .alloc_object_witnessed(KObject::Bool(left < right))))
}

pub fn body_le<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.types));
    Action::Done(Ok(ctx
        .scope
        .brand()
        .alloc_object_witnessed(KObject::Bool(left <= right))))
}

pub fn body_gt<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.types));
    Action::Done(Ok(ctx
        .scope
        .brand()
        .alloc_object_witnessed(KObject::Bool(left > right))))
}

pub fn body_ge<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.types));
    Action::Done(Ok(ctx
        .scope
        .brand()
        .alloc_object_witnessed(KObject::Bool(left >= right))))
}

pub fn body_and<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(bool_operands(ctx.args, ctx.types));
    Action::Done(Ok(ctx
        .scope
        .brand()
        .alloc_object_witnessed(KObject::Bool(left && right))))
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry) {
    let number_sig = |op: &str| {
        sig(
            KType::Number,
            vec![
                arg("left", KType::Number),
                kw(op),
                arg("right", KType::Number),
            ],
        )
    };
    let comparison_sig = |op: &str| {
        sig(
            KType::Bool,
            vec![
                arg("left", KType::Number),
                kw(op),
                arg("right", KType::Number),
            ],
        )
    };

    crate::builtins::register_builtin(scope, "+", number_sig("+"), body_add, types);
    crate::builtins::register_builtin(scope, "-", number_sig("-"), body_sub, types);
    crate::builtins::register_builtin(scope, "*", number_sig("*"), body_mul, types);
    crate::builtins::register_builtin(scope, "/", number_sig("/"), body_div, types);

    crate::builtins::register_builtin(scope, "<", comparison_sig("<"), body_lt, types);
    crate::builtins::register_builtin(scope, "<=", comparison_sig("<="), body_le, types);
    crate::builtins::register_builtin(scope, ">", comparison_sig(">"), body_gt, types);
    crate::builtins::register_builtin(scope, ">=", comparison_sig(">="), body_ge, types);

    let and_sig = sig(
        KType::Bool,
        vec![
            arg("left", KType::Bool),
            kw("AND"),
            arg("right", KType::Bool),
        ],
    );
    crate::builtins::register_builtin(scope, "AND", and_sig, body_and, types);
}

/// Seeds the three builtin operator groups: comparison (`< <= > >=`, pairwise, combined by
/// `AND`), additive (`+ -`, fold-left), and multiplicative (`* /`, fold-left). Each group is
/// allocated once and registered — through [`Scope::register_group_under_all_subsets`] — under
/// every nonempty subset of its member set, so any chain probe drawn from that set resolves to
/// the same shared record.
///
/// These seeds land in the run-global root, which the innermost-wins registry walk reaches last:
/// they are the defaults a declaring scope may override, not unshadowable claims on the symbols.
///
/// A comparison chain (`1 < 2 < 3`, `1 <= x < 10`) resolves to this group and reduces through the
/// pairwise reducer (`operator_chain::reduce_pairwise`): each adjacent pair dispatches through its
/// own operator's body above, and the pair results fold left through the `AND` keyword combiner.
pub fn register_builtin_operator_groups<'a>(scope: &'a Scope<'a>, _types: &TypeRegistry) {
    let region = scope.brand();

    let comparison_operators = ["<", "<=", ">", ">="];
    let comparison_members: HashSet<String> =
        comparison_operators.iter().map(|s| s.to_string()).collect();
    let comparison_group = region.alloc_operator_group(OperatorGroup::new(
        comparison_members,
        ReductionMode::Pairwise {
            combiner: "AND".to_string(),
            direction: FoldDirection::Left,
        },
    ));
    seed(scope, &comparison_operators, comparison_group);

    let additive_operators = ["+", "-"];
    let additive_members: HashSet<String> =
        additive_operators.iter().map(|s| s.to_string()).collect();
    let additive_group = region.alloc_operator_group(OperatorGroup::new(
        additive_members,
        ReductionMode::FoldLeft,
    ));
    seed(scope, &additive_operators, additive_group);

    let multiplicative_operators = ["*", "/"];
    let multiplicative_members: HashSet<String> = multiplicative_operators
        .iter()
        .map(|s| s.to_string())
        .collect();
    let multiplicative_group = region.alloc_operator_group(OperatorGroup::new(
        multiplicative_members,
        ReductionMode::FoldLeft,
    ));
    seed(scope, &multiplicative_operators, multiplicative_group);
}

/// One builtin seed: the group's powerset keys, at [`BindingIndex::BUILTIN`].
fn seed<'a>(scope: &'a Scope<'a>, members: &[&str], group: &'a OperatorGroup) {
    scope
        .register_group_under_all_subsets(members, group, BindingIndex::BUILTIN)
        .expect("builtin operator-group seeding must not collide");
}

#[cfg(test)]
mod tests;
