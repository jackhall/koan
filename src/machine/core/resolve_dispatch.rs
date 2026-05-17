//! Overload resolution for a [`KExpression`] against the lexical scope chain.
//!
//! Read‑only consumer of the dispatch table: walks `Scope::ancestors()` and, at each
//! scope, looks up `bindings().functions()` keyed by the expression's untyped key.
//! Never touches `data`, `placeholders`, `types`, `pending`, `out`, or `kind` —
//! that read‑only dependency is the boundary that justifies the module split.
//!
//! ## Invariant pinned here
//!
//! **Strict‑then‑tentative per scope.** Within a single scope's bucket the strict
//! pass runs first; a strict tie surfaces [`ResolveOutcome::Ambiguous`] immediately
//! (no outer fall‑through — silently shadowing an inner conflict would hide a real
//! author error). The tentative (auto‑wrap) pass then runs in the same scope; a
//! tentative tie *does* fall through to `outer`, because the wrap pass is already
//! a relaxation and any outer strict pick is a stronger signal.
//!
//! ## Post‑walk split
//!
//! If the chain produces nothing, [`expr_has_eager_part`] decides between
//! [`ResolveOutcome::Deferred`] (a nested call needs to evaluate first, and the
//! scheduler's eager‑sub loop should rebuild with `Future(_)` parts and re‑dispatch)
//! and [`ResolveOutcome::Unmatched`] (a flat dispatch failure the caller surfaces).

use crate::machine::core::kfunction::{ClassifiedSlots, KFunction};
use crate::machine::model::ast::KExpression;

use super::scope::Scope;

/// A successful resolution: which function was picked, what placeholder name (if any) to
/// install at dispatch time, and the per-slot classification a downstream scheduler driver
/// needs for auto-wrap, replay-park, and eager-sub scheduling. `slots` is held by value —
/// [`build_resolved`] is the sole producer, so this is the single carrier for the disjoint
/// `(eager_indices | wrap_indices | ref_name_indices)` invariant documented on
/// [`crate::machine::core::kfunction::ClassifiedSlots`].
pub struct Resolved<'a> {
    pub function: &'a KFunction<'a>,
    pub placeholder_name: Option<String>,
    pub slots: ClassifiedSlots,
}

/// Outcome of [`Scope::resolve_dispatch`]. The `Resolved | Ambiguous | Deferred |
/// Unmatched` split is the load-bearing typing — the scheduler's dispatch driver matches
/// on it directly to choose between immediate bind, ambiguity error, eager-sub
/// scheduling, and dispatch-failed error.
pub enum ResolveOutcome<'a> {
    Resolved(Resolved<'a>),
    Ambiguous(usize),
    Deferred,
    Unmatched,
}

impl<'a> Scope<'a> {
    /// Single-pass overload resolution: walks [`Scope::ancestors`] performing
    /// **strict-then-tentative per scope** (both passes consult the same scope's bucket
    /// before descending), so an inner-scope tentative match shadows an outer-scope strict
    /// one, mirroring lexical scoping. Ambiguity surfaces at the first scope where the
    /// strict pass ties — it does NOT fall through to `outer` (silently shadowing an inner
    /// conflict would hide a real author error).
    ///
    /// Outcomes:
    /// - [`ResolveOutcome::Resolved`]: a unique overload was picked. The carried
    ///   [`Resolved`] bundles the function plus the per-slot classification
    ///   ([`KFunction::classify_for_pick`]) plus an optional `placeholder_name` extracted
    ///   from the picked function's `pre_run` (the binder-side name to install at dispatch
    ///   time).
    /// - [`ResolveOutcome::Ambiguous(n)`]: the strict pass at some scope produced `n ≥ 2`
    ///   equally-specific candidates. No further scopes consulted.
    /// - [`ResolveOutcome::Deferred`]: nothing matched anywhere on the chain, but `expr`
    ///   contains at least one nested `Expression` / `ListLiteral` / `DictLiteral` part —
    ///   eagerly evaluating those subs may produce a `Future(_)` that matches a typed slot
    ///   the bare expression couldn't. The scheduler falls through to its eager-sub loop
    ///   on this variant. Covers shapes like `((deep_call) + 1)` where a typed `+`
    ///   overload only matches after `deep_call` resolves.
    /// - [`ResolveOutcome::Unmatched`]: no match anywhere, and no eager parts to wait on
    ///   either — a real dispatch failure the caller surfaces as an error.
    pub fn resolve_dispatch<'e>(&'a self, expr: &KExpression<'e>) -> ResolveOutcome<'a> {
        let key = expr.untyped_key();
        let picked = self.ancestors().find_map(|scope| -> Option<ResolveOutcome<'a>> {
            // Per-step `functions` guard drops at the closure boundary, so the chain
            // walk never accumulates live read borrows.
            let functions_guard = scope.bindings().functions();
            let bucket = OverloadBucket { candidates: functions_guard.get(&key)?.as_slice() };
            match bucket.pick_strict(expr) {
                PickPass::Picked(f) => Some(ResolveOutcome::Resolved(build_resolved(f, expr))),
                // Strict tie inside this scope — surface ambiguity rather than fall through.
                PickPass::Tie(n) => Some(ResolveOutcome::Ambiguous(n)),
                // No strict match: try the tentative (auto-wrap) pass. A tentative-pass tie
                // falls through to `outer` rather than surfacing `Ambiguous`: the wrap pass
                // is already a relaxation, and an outer scope's strict pick is stronger.
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
        // Nothing matched on the chain. Distinguish a flat-unbound shape from one whose
        // dispatch can't pick *yet* because nested subs need to evaluate first — the
        // scheduler's eager-sub loop will rebuild with `Future(_)` parts and re-dispatch.
        if expr_has_eager_part(expr) {
            ResolveOutcome::Deferred
        } else {
            ResolveOutcome::Unmatched
        }
    }
}

/// View over a single scope's overload bucket (the `Vec<&KFunction>` keyed by an
/// expression's untyped signature in `Bindings::functions`). Encapsulates the
/// **filter‑then‑[`ExpressionSignature::most_specific`]** dance that runs twice
/// per scope (once for the strict pass, once for the tentative auto‑wrap pass).
///
/// Held by value as a thin wrapper over a borrowed slice — no allocation, lives
/// only as long as the `Ref<HashMap>` the slice came from.
struct OverloadBucket<'a, 's> {
    candidates: &'s [&'a KFunction<'a>],
}

impl<'a> OverloadBucket<'a, '_> {
    /// Strict pass: only overloads whose `signature.matches(expr)` returns true
    /// (current binding+typing rules, no auto‑wrap).
    fn pick_strict(&self, expr: &KExpression<'_>) -> PickPass<'a> {
        self.pick(expr, |f, e| f.signature.matches(e))
    }

    /// Tentative (auto‑wrap) pass: overloads whose `accepts_for_wrap(expr)` returns
    /// true — typed slots reachable by speculatively wrapping the corresponding
    /// argument.
    fn pick_tentative(&self, expr: &KExpression<'_>) -> PickPass<'a> {
        self.pick(expr, |f, e| f.accepts_for_wrap(e))
    }

    /// Shared body: filter the bucket by `admit`, then run
    /// [`ExpressionSignature::most_specific`] on the surviving signatures.
    /// `Some(i)` → unique pick, `None` + non‑empty → `Tie`, empty → `Empty`.
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

/// Outcome of one filter→`most_specific` pass over an [`OverloadBucket`]. The
/// strict/tentative policy difference lives at the call site (strict `Tie` →
/// `ResolveOutcome::Ambiguous`, tentative `Tie` → fall through to `outer`) —
/// `PickPass` itself is policy‑free.
enum PickPass<'a> {
    Picked(&'a KFunction<'a>),
    Tie(usize),
    Empty,
}

/// True iff `expr` carries any `Expression` / `ListLiteral` / `DictLiteral` part — the
/// shapes the scheduler's eager loop would schedule as sub-Dispatches. Drives the
/// [`ResolveOutcome::Deferred`] vs [`ResolveOutcome::Unmatched`] split: a nested-call shape
/// like `((deep_call) + 1)` defers (today's behavior); a flat unbound name `nope` is
/// unmatched.
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

/// Pack a picked function + classification + `pre_run`-extracted placeholder name into a
/// [`Resolved`]. The sole producer of the embedded `slots` — disjointness lives in
/// [`KFunction::classify_for_pick`].
fn build_resolved<'a>(picked: &'a KFunction<'a>, expr: &KExpression<'_>) -> Resolved<'a> {
    Resolved {
        function: picked,
        placeholder_name: picked.pre_run.and_then(|extractor| extractor(expr)),
        slots: picked.classify_for_pick(expr),
    }
}
