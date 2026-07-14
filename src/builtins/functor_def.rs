//! `FUNCTOR <signature:KExpression> -> <return_type:Type> = <body:KExpression>` —
//! the user-defined functor constructor. The body shares
//! [`crate::builtins::fn_def::build_fn_like`] with FN, selecting
//! `FnKind::Functor`; the divergences from FN are:
//!
//! 1. The constructed `KFunction` carries `is_functor: true`, so its
//!    `function_value_ktype` projects to `KType::KFunctor`.
//! 2. The return-type slot is validated at the FUNCTOR site against the
//!    admissible-carrier list from
//!    [design/typing/functors.md](../../design/typing/functors.md). Other carriers
//!    error here, before the body has a chance to surface a frames-removed
//!    `TypeMismatch`.
//!
//! This module owns only the two surface-form overload registrations.

use crate::machine::model::types::KKind;
use crate::machine::model::KType;
use crate::machine::Scope;

use super::fn_def::finalize::FnKind;
use super::{arg, kw, sig};

pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    super::fn_def::build_fn_like(ctx, "FUNCTOR", FnKind::Functor)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Two overloads: `ProperType` for a bare `-> Number`, `SigiledTypeExpr` for a
    // `:(…)` / dotted carrier like `-> er.Type`. `binder_bucket` lets a sibling
    // bare-arg call park on a still-finalizing overload — siblings sharing a
    // bucket key all install for it and only the first finalize wins.
    let typeexpr_sig = || {
        sig(
            KType::Any,
            vec![
                kw("FUNCTOR"),
                arg("signature", KType::KExpression),
                kw("->"),
                arg("return_type", KType::OfKind(KKind::ProperType)),
                kw("="),
                arg("body", KType::KExpression),
            ],
        )
    };
    // Lazy `:(...)` return carrier: a dotted `-> er.Type` defers per-call rather
    // than eager-sub-dispatching to an unbound parameter.
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
    use crate::builtins::register_builtin_full;
    let bucket = super::fn_def::binder_bucket;
    register_builtin_full(
        scope,
        "FUNCTOR",
        typeexpr_sig(),
        body,
        None,
        Some(bucket),
        false,
    );
    register_builtin_full(
        scope,
        "FUNCTOR",
        sigil_sig(),
        body,
        None,
        Some(bucket),
        false,
    );
}

#[cfg(test)]
mod tests;
