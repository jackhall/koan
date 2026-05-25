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
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{ExpressionSignature, KType, SignatureElement};
use crate::machine::NodeId;

use super::scope::{Resolution, Scope};

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
    /// A tentative tie hinged on a bare name still pending as a forward-reference
    /// `Placeholder`: park on the carried producer `NodeId`s and re-dispatch once they bind,
    /// where the strict-pass peek can read the now-bound type. Distinct from `Deferred`,
    /// which schedules eager parts; this waits on existing producers without scheduling work.
    ParkOnProducers(Vec<NodeId>),
    /// A tentative tie hinged on a bare name that resolves to nothing — no binding and no
    /// forward-reference placeholder. The call can never resolve, and the precise cause is
    /// the unbound name, so it surfaces as `UnboundName` rather than a dispatch miss.
    UnboundName(String),
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
    pub fn resolve_dispatch(&'a self, expr: &KExpression<'a>) -> ResolveOutcome<'a> {
        let key = expr.untyped_key();
        // Bare-name peeks resolve in the *dispatch* scope, not the ancestor whose bucket is
        // under test — a name bound in an inner scope must still be found when the matching
        // function lives in an outer one.
        let dispatch_scope: &'a Scope<'a> = self;
        // A tentative tie's bare-name status, recorded inner-most-first but acted on only
        // after the full walk: an outer strict/tentative pick still wins. Producers to park
        // on (forward refs), plus the first genuinely-unbound tie name.
        let mut park_producers: Vec<NodeId> = Vec::new();
        let mut unbound_name: Option<String> = None;
        let picked = self.ancestors().find_map(|scope| -> Option<ResolveOutcome<'a>> {
            // Per-step guard drops at the closure boundary so the chain walk never
            // accumulates live read borrows on `bindings().functions()`.
            let functions_guard = scope.bindings().functions();
            let bucket = OverloadBucket { candidates: functions_guard.get(&key)?.as_slice() };
            match bucket.pick_strict(expr, dispatch_scope) {
                PickPass::Picked(f) => Some(ResolveOutcome::Resolved(build_resolved(f, expr))),
                // A strict tie whose deciding argument is still an unevaluated literal /
                // sub-expression may resolve once evaluated: a typed `Future(List …)`
                // re-dispatch is element-aware (`accepts_part`) where the bare literal is
                // shape-only and ties. Defer rather than hard-erroring — `run_bind`
                // re-dispatches the rewritten `Future(_)` expression, which carries no eager
                // parts, so a genuine tie surfaces as `Ambiguous` on that second pass.
                PickPass::Tie(n) if expr_has_eager_part(expr) => {
                    let _ = n;
                    Some(ResolveOutcome::Deferred)
                }
                PickPass::Tie(n) => Some(ResolveOutcome::Ambiguous(n)),
                PickPass::Empty => match bucket.pick_tentative(expr) {
                    PickPass::Picked(f) => {
                        Some(ResolveOutcome::Resolved(build_resolved(f, expr)))
                    }
                    // A tentative tie admits ≥2 overloads only via the blind bare-name
                    // relaxation. If a tying bare name is a still-pending forward reference,
                    // remember its producer and keep walking; once nothing matches anywhere,
                    // parking on it and re-dispatching lets the strict-pass peek disambiguate
                    // off the bound type.
                    PickPass::Tie(_) => {
                        if park_producers.is_empty() && unbound_name.is_none() {
                            let (placeholders, unbound) =
                                classify_tie_bare_names(expr, dispatch_scope);
                            park_producers = placeholders;
                            unbound_name = unbound;
                        }
                        None
                    }
                    PickPass::Empty => None,
                },
            }
        });
        if let Some(outcome) = picked {
            return outcome;
        }
        // Park only on a real forward reference; an unbound tie name names nothing, so the
        // precise error is the unbound name rather than a dispatch miss.
        if !park_producers.is_empty() {
            return ResolveOutcome::ParkOnProducers(park_producers);
        }
        if let Some(name) = unbound_name {
            return ResolveOutcome::UnboundName(name);
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
    fn pick_strict(&self, expr: &KExpression<'a>, dispatch_scope: &'a Scope<'a>) -> PickPass<'a> {
        self.pick(expr, move |f, e| signature_admits_strict(&f.signature, e, dispatch_scope))
    }

    fn pick_tentative(&self, expr: &KExpression<'a>) -> PickPass<'a> {
        self.pick(expr, |f, e| f.accepts_for_wrap(e))
    }

    fn pick(
        &self,
        expr: &KExpression<'a>,
        admit: impl Fn(&'a KFunction<'a>, &KExpression<'a>) -> bool,
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

/// Strict-pass admission with a bare-name type peek. Mirrors
/// [`ExpressionSignature::matches`], but for a *container* slot (`List` / `Dict`) tested
/// against a bare `Identifier`, it resolves the name in `dispatch_scope` and admits on the
/// bound value's carried type — the same element-aware check `accepts_part` runs for an
/// evaluated `Future` (constructed here for free, since `Future` holds a reference). This
/// lets `DESCRIBE xs` route by `xs`'s element type in one pass rather than tying.
///
/// A name that resolves to a placeholder or is unbound yields `false` for that slot, so the
/// signature falls out of the strict pass and into the tentative auto-wrap / forward-reference
/// park path unchanged — which is what keeps dispatch order-independent: a not-yet-bound name
/// parks and re-dispatches onto the same pick once its binding exists, rather than peeking a
/// transiently-absent type. Non-container slots are left to the pure `Argument::matches`.
fn signature_admits_strict<'a>(
    sig: &ExpressionSignature<'a>,
    expr: &KExpression<'a>,
    dispatch_scope: &Scope<'a>,
) -> bool {
    if sig.elements.len() != expr.parts.len() {
        return false;
    }
    sig.elements.iter().zip(&expr.parts).all(|(el, part)| match (el, &part.value) {
        (SignatureElement::Keyword(s), ExpressionPart::Keyword(t)) => s == t,
        (SignatureElement::Keyword(_), _) => false,
        (SignatureElement::Argument(arg), ExpressionPart::Identifier(name))
            if matches!(arg.ktype, KType::List(_) | KType::Dict(_, _)) =>
        {
            match dispatch_scope.resolve(name) {
                Resolution::Value(obj) => arg.ktype.accepts_part(&ExpressionPart::Future(obj)),
                Resolution::Placeholder(_) | Resolution::Unbound => false,
            }
        }
        (SignatureElement::Argument(arg), part_value) => arg.matches(part_value),
    })
}

/// Classify the bare `Identifier` parts of a tentatively-tied `expr`: producers to park on
/// (forward-reference `Placeholder`s) and the first genuinely-`Unbound` name. A tentative
/// tie hinges on bare names admitted blindly; a `Placeholder` will bind (park and retry),
/// while an `Unbound` name names nothing and is a definitive [`ResolveOutcome::UnboundName`],
/// not a dispatch miss. Only `Identifier` parts are considered: the blind relaxation never
/// admits a bare name into a binder `Identifier` / `TypeExprRef` slot, so a tying name fills
/// a value slot (type-token forward refs park through the resolved-pick replay-park instead).
fn classify_tie_bare_names(
    expr: &KExpression<'_>,
    dispatch_scope: &Scope<'_>,
) -> (Vec<NodeId>, Option<String>) {
    let mut placeholders = Vec::new();
    let mut unbound = None;
    for part in &expr.parts {
        if let ExpressionPart::Identifier(name) = &part.value {
            match dispatch_scope.resolve(name) {
                Resolution::Placeholder(producer) => placeholders.push(producer),
                Resolution::Unbound if unbound.is_none() => unbound = Some(name.clone()),
                Resolution::Unbound | Resolution::Value(_) => {}
            }
        }
    }
    (placeholders, unbound)
}

/// True iff `expr` carries any `Expression` / `ListLiteral` / `DictLiteral` part —
/// the shapes the scheduler's eager loop would schedule as sub-Dispatches.
fn expr_has_eager_part(expr: &KExpression<'_>) -> bool {
    use crate::machine::model::ast::ExpressionPart;
    expr.parts.iter().any(|p| {
        matches!(
            &p.value,
            ExpressionPart::Expression(_)
                | ExpressionPart::ListLiteral(_)
                | ExpressionPart::DictLiteral(_)
        )
    })
}

/// Sole producer of the embedded `slots` — disjointness lives in
/// [`KFunction::classify_for_pick`].
fn build_resolved<'a>(picked: &'a KFunction<'a>, expr: &KExpression<'a>) -> Resolved<'a> {
    Resolved {
        function: picked,
        placeholder_name: picked.pre_run.and_then(|extractor| extractor(expr)),
        slots: picked.classify_for_pick(expr),
    }
}
