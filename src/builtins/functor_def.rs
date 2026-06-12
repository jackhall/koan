//! `FUNCTOR <signature:KExpression> -> <return_type:Type> = <body:KExpression>` —
//! the user-defined functor constructor. The body shares
//! [`crate::builtins::fn_def::build_fn_like`] with FN, selecting `FnKind::Functor`;
//! the divergences from FN are:
//!
//! 1. The constructed `KFunction` carries `is_functor: true`, so its
//!    `function_value_ktype` projects to `KType::KFunctor`.
//! 2. The return-type slot is validated at the FUNCTOR site against the
//!    admissible-carrier list from
//!    [design/typing/functors.md](../../design/typing/functors.md). Other carriers
//!    error here, before the body has a chance to surface a frames-removed
//!    `TypeMismatch`.
//!
//! Both divergences key on `FnKind::Functor`: `build_fn_like` passes
//! `Some(&param_type_map)` to the shared `classify_return_type`, which emits a
//! `Rejected`/`Admissible`/`DeferredToCombine` verdict alongside classification so
//! the carrier is walked once; the deferred arm rides Combine-finish gated by the
//! same kind, with no separate predicate closure threaded through the schedule.
//!
//! This module owns only the two surface-form overload registrations.

use crate::machine::model::types::KKind;
use crate::machine::model::KType;
use crate::machine::{ArgumentBundle, BodyResult, SchedulerHandle, Scope};

use super::fn_def::build_fn_like;
use super::fn_def::finalize::FnKind;
use super::{arg, kw, sig};
#[cfg(not(feature = "action-harness"))]
use super::register_builtin_full;

/// FUNCTOR binder body. Shares [`crate::builtins::fn_def::build_fn_like`] with FN;
/// `FnKind::Functor` selects the return-admissibility verdict and the
/// `is_functor: true` flag downstream.
pub fn body<'a, 's>(
    sched: &mut dyn SchedulerHandle<'a, 's>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    build_fn_like(sched, bundle, "FUNCTOR", FnKind::Functor)
}

/// `Action`-harness twin of [`body`]: shares [`crate::builtins::fn_def::build_fn_like_action`]
/// with FN, selecting `FnKind::Functor`.
#[cfg(feature = "action-harness")]
pub fn body_action<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    super::fn_def::build_fn_like_action(ctx, "FUNCTOR", FnKind::Functor)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Two overloads mirror FN: `TypeExprRef` for a bare `-> Number` / `-> Er`, and
    // `SigiledTypeExpr` for a `:(…)` / dotted carrier like `-> Er.Type` /
    // `-> :(Set WITH {…})`. `binder_bucket` lets a sibling bare-arg call park on
    // a still-finalizing overload; sibling overloads sharing a bucket key all
    // install for it and only the first finalize wins. No `binder_name` —
    // FUNCTOR registers under `functions[bucket]`, not a value-side carrier.
    let typeexpr_sig = || {
        sig(
            KType::Any,
            vec![
                kw("FUNCTOR"),
                arg("signature", KType::KExpression),
                kw("->"),
                arg("return_type", KType::OfKind(KKind::Proper)),
                kw("="),
                arg("body", KType::KExpression),
            ],
        )
    };
    // Lazy `:(...)` return carrier — the FN counterpart's rationale applies: a dotted
    // `-> Er.Type` defers per-call rather than eager-sub-dispatching to an unbound parameter.
    let sigil_sig = || {
        sig(
            KType::Any,
            vec![
                kw("FUNCTOR"),
                arg("signature", KType::KExpression),
                kw("->"),
                arg("return_type", KType::SigiledTypeExpr),
                kw("="),
                arg("body", KType::KExpression),
            ],
        )
    };
    #[cfg(feature = "action-harness")]
    {
        use crate::builtins::register_action_builtin_full;
        let bucket = super::fn_def::binder_bucket;
        register_action_builtin_full(scope, "FUNCTOR", typeexpr_sig(), body_action, None, Some(bucket), false);
        register_action_builtin_full(scope, "FUNCTOR", sigil_sig(), body_action, None, Some(bucket), false);
    }
    #[cfg(not(feature = "action-harness"))]
    {
        let bucket = super::fn_def::binder_bucket;
        register_builtin_full(scope, "FUNCTOR", typeexpr_sig(), body, None, Some(bucket), false);
        register_builtin_full(scope, "FUNCTOR", sigil_sig(), body, None, Some(bucket), false);
    }
}

#[cfg(test)]
mod tests;
