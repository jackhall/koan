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
//! (both fold-left). A registry record is member set + mode only — see
//! [`crate::machine::model::operators`] — so seeding is a separate step from registering
//! the per-operator bodies above.

use crate::machine::WriteGate;

use crate::machine::BindingIndex;
use crate::machine::GroupSeal;
use crate::machine::model::{FoldDirection, ReductionMode};
use crate::machine::model::{KObject, KType};
use crate::machine::{Action, BodyCtx};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};
use crate::machine::BoundArgs;
use crate::machine::model::RunRegistries;
use crate::machine::model::Scalar;
use crate::machine::model::{KeywordSymbol, StaticName, ValueSymbol};

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { left, right } }

/// Read a `:Number` operand named `name`, or the canonical missing/mismatch diagnostic.
fn number_arg(
    args: BoundArgs<'_, '_>,
    name: &StaticName<ValueSymbol>,
    registries: &RunRegistries,
) -> Result<f64, KError> {
    match args.object(name) {
        Some(KObject::Number(n)) => Ok(*n),
        Some(other) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: name.text().to_string(),
            expected: "Number".to_string(),
            got: other.ktype().name(registries),
        })),
        None => Err(KError::new(KErrorKind::MissingArg(name.text().to_string()))),
    }
}

/// Read the `left` / `right` `:Number` operands.
fn number_operands(
    args: BoundArgs<'_, '_>,
    registries: &RunRegistries,
) -> Result<(f64, f64), KError> {
    Ok((
        number_arg(args, &SLOTS.left, registries)?,
        number_arg(args, &SLOTS.right, registries)?,
    ))
}

/// Read a `:Bool` operand named `name`, or the canonical missing/mismatch diagnostic.
fn bool_arg(
    args: BoundArgs<'_, '_>,
    name: &StaticName<ValueSymbol>,
    registries: &RunRegistries,
) -> Result<bool, KError> {
    match args.object(name) {
        Some(KObject::Bool(b)) => Ok(*b),
        Some(other) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: name.text().to_string(),
            expected: "Bool".to_string(),
            got: other.ktype().name(registries),
        })),
        None => Err(KError::new(KErrorKind::MissingArg(name.text().to_string()))),
    }
}

/// Read the `left` / `right` `:Bool` operands.
fn bool_operands(
    args: BoundArgs<'_, '_>,
    registries: &RunRegistries,
) -> Result<(bool, bool), KError> {
    Ok((
        bool_arg(args, &SLOTS.left, registries)?,
        bool_arg(args, &SLOTS.right, registries)?,
    ))
}

pub fn body_add<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.registries));
    Action::done(Ok(ctx
        .scope
        .brand()
        .alloc_scalar_witnessed(Scalar::Number(left + right))))
}

pub fn body_sub<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.registries));
    Action::done(Ok(ctx
        .scope
        .brand()
        .alloc_scalar_witnessed(Scalar::Number(left - right))))
}

pub fn body_mul<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.registries));
    Action::done(Ok(ctx
        .scope
        .brand()
        .alloc_scalar_witnessed(Scalar::Number(left * right))))
}

/// `Number` has one representation (`f64`; see `KObject::Number`) and the codebase has no
/// prior division operator to match, so a zero divisor raises a structured `KError`
/// (`KErrorKind::User`, the in-language-error landing pad) rather than following IEEE 754's
/// infinity/NaN convention — no NaN value is ever minted onto a koan `Number`.
pub fn body_div<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.registries));
    if right == 0.0 {
        return Action::done(Err(KError::new(KErrorKind::User(
            "/ : division by zero".to_string(),
        ))));
    }
    Action::done(Ok(ctx
        .scope
        .brand()
        .alloc_scalar_witnessed(Scalar::Number(left / right))))
}

pub fn body_lt<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.registries));
    Action::done(Ok(ctx
        .scope
        .brand()
        .alloc_scalar_witnessed(Scalar::Bool(left < right))))
}

pub fn body_le<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.registries));
    Action::done(Ok(ctx
        .scope
        .brand()
        .alloc_scalar_witnessed(Scalar::Bool(left <= right))))
}

pub fn body_gt<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.registries));
    Action::done(Ok(ctx
        .scope
        .brand()
        .alloc_scalar_witnessed(Scalar::Bool(left > right))))
}

pub fn body_ge<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(number_operands(ctx.args, ctx.registries));
    Action::done(Ok(ctx
        .scope
        .brand()
        .alloc_scalar_witnessed(Scalar::Bool(left >= right))))
}

pub fn body_and<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let (left, right) = crate::try_action!(bool_operands(ctx.args, ctx.registries));
    Action::done(Ok(ctx
        .scope
        .brand()
        .alloc_scalar_witnessed(Scalar::Bool(left && right))))
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let number_sig = |op: &'static str| {
        sig(
            KType::NUMBER,
            vec![
                arg(registries, &SLOTS.left, KType::NUMBER),
                kw(registries, op),
                arg(registries, &SLOTS.right, KType::NUMBER),
            ],
        )
    };
    let comparison_sig = |op: &'static str| {
        sig(
            KType::BOOL,
            vec![
                arg(registries, &SLOTS.left, KType::NUMBER),
                kw(registries, op),
                arg(registries, &SLOTS.right, KType::NUMBER),
            ],
        )
    };

    crate::builtins::register_builtin(scope, number_sig("+"), body_add, registries, gate);
    crate::builtins::register_builtin(scope, number_sig("-"), body_sub, registries, gate);
    crate::builtins::register_builtin(scope, number_sig("*"), body_mul, registries, gate);
    crate::builtins::register_builtin(scope, number_sig("/"), body_div, registries, gate);

    crate::builtins::register_builtin(scope, comparison_sig("<"), body_lt, registries, gate);
    crate::builtins::register_builtin(scope, comparison_sig("<="), body_le, registries, gate);
    crate::builtins::register_builtin(scope, comparison_sig(">"), body_gt, registries, gate);
    crate::builtins::register_builtin(scope, comparison_sig(">="), body_ge, registries, gate);

    let and_sig = sig(
        KType::BOOL,
        vec![
            arg(registries, &SLOTS.left, KType::BOOL),
            kw(registries, "AND"),
            arg(registries, &SLOTS.right, KType::BOOL),
        ],
    );
    crate::builtins::register_builtin(scope, and_sig, body_and, registries, gate);
}

/// Seeds the three builtin operator groups: comparison (`< <= > >=`, pairwise, combined by
/// `AND`), additive (`+ -`, fold-left), and multiplicative (`* /`, fold-left). Each group's record
/// is allocated once and registered — through [`Scope::register_group_under_all_subsets_direct`] —
/// under every nonempty subset of its member set, so any chain probe drawn from that set resolves
/// to the same record by address.
///
/// These seeds land in the run-global root, which the innermost-wins registry walk reaches last:
/// they are the defaults a declaring scope may override, not unshadowable claims on the symbols.
///
/// A comparison chain (`1 < 2 < 3`, `1 <= x < 10`) resolves to this group and reduces through the
/// pairwise reducer (`operator_chain::reduce_pairwise`): each adjacent pair dispatches through its
/// own operator's body above, and the pair results fold left through the `AND` keyword combiner.
pub fn register_builtin_operator_groups<'a>(
    scope: &'a Scope<'a>,
    registries: &RunRegistries,
    gate: &mut WriteGate,
) {
    seed(
        scope,
        &["<", "<=", ">", ">="],
        ReductionMode::Pairwise {
            combiner: registries.labels.record(&AND_COMBINER),
            direction: FoldDirection::Left,
        },
        registries,
        gate,
    );
    seed(
        scope,
        &["+", "-"],
        ReductionMode::FoldLeft,
        registries,
        gate,
    );
    seed(
        scope,
        &["*", "/"],
        ReductionMode::FoldLeft,
        registries,
        gate,
    );
}

/// The comparison group's pair-result combiner, spelled in Rust source and so declared once.
static AND_COMBINER: StaticName<KeywordSymbol> = crate::static_name!(KeywordSymbol, "AND");

/// One builtin seed: the group record in the root's own region, then its powerset keys at
/// [`BindingIndex::BUILTIN`]. The root's region is the eternal tier, so a builtin group outlives
/// every per-call region and an inner scope's resolved carrier names an ordinary foreign member.
///
/// The glyphs are classified and interned here, at the seam where their spellings are still in
/// hand — which is what lets a later conflict diagnostic render the probe keys this seeding
/// installs.
fn seed<'a>(
    scope: &'a Scope<'a>,
    glyphs: &[&str],
    mode: ReductionMode,
    registries: &RunRegistries,
    gate: &mut WriteGate,
) {
    let members: Vec<KeywordSymbol> = glyphs
        .iter()
        .map(|glyph| {
            KeywordSymbol::declared(glyph, &registries.labels)
                .expect("a builtin operator glyph is keyword-class")
        })
        .collect();
    let cell = scope.birth_operator_group(&members, mode);
    let seal = GroupSeal::of_delivered(scope, &cell);
    scope
        .register_group_under_all_subsets_direct(
            &members,
            seal,
            BindingIndex::BUILTIN,
            registries,
            gate,
        )
        .expect("builtin operator-group seeding must not collide");
}

#[cfg(test)]
mod tests;
