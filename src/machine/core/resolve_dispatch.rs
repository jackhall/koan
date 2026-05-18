//! Overload resolution for a [`KExpression`] against the lexical scope chain.
//!
//! Read‑only consumer of the dispatch table: walks `Scope::ancestors()` and, at each
//! scope, looks up `bindings().functions()` keyed by the expression's untyped key.
//!
//! ## Invariant: strict‑then‑tentative per scope
//!
//! Within a scope's bucket the strict pass runs first; a strict tie surfaces
//! [`ResolveOutcome::Ambiguous`] immediately (no outer fall‑through — silently
//! shadowing an inner conflict would hide a real author error). The tentative
//! (auto‑wrap) pass then runs in the same scope; a tentative tie falls through to
//! `outer`, because the wrap pass is already a relaxation and an outer strict pick
//! is a stronger signal.

use crate::machine::core::kfunction::{ClassifiedSlots, KFunction};
use crate::machine::model::ast::KExpression;

use super::scope::Scope;

/// Picked function plus the per-slot classification the dispatch driver needs for
/// auto-wrap, replay-park, and eager-sub scheduling. Sole carrier of the disjoint
/// `(eager_indices | wrap_indices | ref_name_indices)` invariant documented on
/// [`crate::machine::core::kfunction::ClassifiedSlots`].
pub struct Resolved<'a> {
    pub function: &'a KFunction<'a>,
    pub placeholder_name: Option<String>,
    pub slots: ClassifiedSlots,
}

pub enum ResolveOutcome<'a> {
    Resolved(Resolved<'a>),
    Ambiguous(usize),
    Deferred,
    Unmatched,
}

impl<'a> Scope<'a> {
    /// Single-pass overload resolution: walks [`Scope::ancestors`] performing
    /// strict-then-tentative per scope (see module-level invariant). An inner-scope
    /// tentative match shadows an outer-scope strict one, mirroring lexical scoping.
    ///
    /// - [`ResolveOutcome::Deferred`] covers shapes like `((deep_call) + 1)` where a
    ///   typed `+` overload only matches after `deep_call` resolves; the scheduler's
    ///   eager-sub loop rebuilds with `Future(_)` parts and re-dispatches.
    pub fn resolve_dispatch<'e>(&'a self, expr: &KExpression<'e>) -> ResolveOutcome<'a> {
        let key = expr.untyped_key();
        let picked = self.ancestors().find_map(|scope| -> Option<ResolveOutcome<'a>> {
            // Per-step guard drops at the closure boundary so the chain walk never
            // accumulates live read borrows on `bindings().functions()`.
            let functions_guard = scope.bindings().functions();
            let bucket = OverloadBucket { candidates: functions_guard.get(&key)?.as_slice() };
            match bucket.pick_strict(expr) {
                PickPass::Picked(f) => Some(ResolveOutcome::Resolved(build_resolved(f, expr))),
                PickPass::Tie(n) => Some(ResolveOutcome::Ambiguous(n)),
                PickPass::Empty => match bucket.pick_tentative(expr) {
                    PickPass::Picked(f) => {
                        Some(ResolveOutcome::Resolved(build_resolved(f, expr)))
                    }
                    PickPass::Tie(_) | PickPass::Empty => None,
                },
            }
        });
        if let Some(outcome) = picked {
            return outcome;
        }
        if expr_has_eager_part(expr) {
            ResolveOutcome::Deferred
        } else {
            ResolveOutcome::Unmatched
        }
    }
}

/// View over a single scope's overload bucket. Encapsulates the
/// filter‑then‑[`ExpressionSignature::most_specific`] dance that runs twice per
/// scope (strict pass, then tentative auto‑wrap pass).
struct OverloadBucket<'a, 's> {
    candidates: &'s [&'a KFunction<'a>],
}

impl<'a> OverloadBucket<'a, '_> {
    fn pick_strict(&self, expr: &KExpression<'_>) -> PickPass<'a> {
        self.pick(expr, |f, e| f.signature.matches(e))
    }

    fn pick_tentative(&self, expr: &KExpression<'_>) -> PickPass<'a> {
        self.pick(expr, |f, e| f.accepts_for_wrap(e))
    }

    fn pick(
        &self,
        expr: &KExpression<'_>,
        admit: impl Fn(&'a KFunction<'a>, &KExpression<'_>) -> bool,
    ) -> PickPass<'a> {
        use crate::machine::model::types::ExpressionSignature;
        let survivors: Vec<&'a KFunction<'a>> =
            self.candidates.iter().copied().filter(|f| admit(f, expr)).collect();
        let sigs: Vec<&ExpressionSignature> = survivors.iter().map(|f| &f.signature).collect();
        match ExpressionSignature::most_specific(&sigs) {
            Some(i) => PickPass::Picked(survivors[i]),
            None if !survivors.is_empty() => PickPass::Tie(survivors.len()),
            None => PickPass::Empty,
        }
    }
}

/// Outcome of one filter→`most_specific` pass. Policy‑free: the strict/tentative
/// difference (strict `Tie` → `Ambiguous`, tentative `Tie` → fall through to `outer`)
/// lives at the call site.
enum PickPass<'a> {
    Picked(&'a KFunction<'a>),
    Tie(usize),
    Empty,
}

/// True iff `expr` carries any `Expression` / `ListLiteral` / `DictLiteral` part —
/// the shapes the scheduler's eager loop would schedule as sub-Dispatches.
fn expr_has_eager_part(expr: &KExpression<'_>) -> bool {
    use crate::machine::model::ast::ExpressionPart;
    expr.parts.iter().any(|p| {
        matches!(
            p,
            ExpressionPart::Expression(_)
                | ExpressionPart::ListLiteral(_)
                | ExpressionPart::DictLiteral(_)
        )
    })
}

/// Sole producer of the embedded `slots` — disjointness lives in
/// [`KFunction::classify_for_pick`].
fn build_resolved<'a>(picked: &'a KFunction<'a>, expr: &KExpression<'_>) -> Resolved<'a> {
    Resolved {
        function: picked,
        placeholder_name: picked.pre_run.and_then(|extractor| extractor(expr)),
        slots: picked.classify_for_pick(expr),
    }
}
