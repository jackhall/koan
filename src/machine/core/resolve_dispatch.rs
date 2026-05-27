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
use crate::machine::core::lexical_frame::LexicalFrame;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{ExpressionSignature, KType, SignatureElement};
use crate::machine::NodeId;

use super::bindings::BindingIndex;
use super::scope::{Resolution, Scope};

/// Picked function plus the per-slot classification the dispatch driver needs for
/// auto-wrap, replay-park, and eager-sub scheduling. Sole carrier of the disjoint
/// `(eager_indices | wrap_indices | ref_name_indices)` invariant documented on
/// [`crate::machine::core::kfunction::ClassifiedSlots`].
pub struct Resolved<'a> {
    pub function: &'a KFunction<'a>,
    pub placeholder_name: Option<String>,
    /// `Some(_)` only for binder builtins whose body registers a callable function
    /// (FN, FUNCTOR). Holds the *inner-call* bucket key the dispatch driver will
    /// install in `bindings.pending_overloads` so a sibling bare-arg call to the
    /// to-be-registered overload parks on this slot. See
    /// [`crate::machine::core::kfunction::BinderBucketFn`].
    pub pending_overload_bucket: Option<crate::machine::model::types::UntypedKey>,
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
        self.resolve_dispatch_with_chain(expr, None)
    }

    /// Chain-gated dispatch resolution. Each candidate is checked against the
    /// visibility predicate before the admit pass; per-overload tagging matters
    /// because overloads in the same bucket may sit at different lexical positions.
    /// `chain = None` disables the gate (test fixtures, builtin registration paths).
    pub fn resolve_dispatch_with_chain(
        &'a self,
        expr: &KExpression<'a>,
        chain: Option<&LexicalFrame>,
    ) -> ResolveOutcome<'a> {
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
            let raw = functions_guard.get(&key)?.as_slice();
            // Pre-filter overloads by per-overload visibility. A consumer between two
            // overloads in the same bucket sees only the earlier; a later-sibling
            // overload is hidden from this consumer's reference. The `nominal_binder`
            // carve-out doesn't apply here — FN bodies are value-style gated.
            let visible_candidates: Vec<(&'a KFunction<'a>, BindingIndex)> = raw
                .iter()
                .filter(|(_, idx)| crate::machine::core::scope::visible(scope.id, *idx, chain))
                .copied()
                .collect();
            if visible_candidates.is_empty() {
                return None;
            }
            let bucket = OverloadBucket { candidates: &visible_candidates };
            match bucket.pick_strict(expr, dispatch_scope, chain) {
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
                                classify_tie_bare_names(expr, dispatch_scope, chain);
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
            return ResolveOutcome::Deferred;
        }
        // Keyword-headed dispatch with no matching bucket and no eager parts to
        // re-dispatch through: if some sibling FN / FUNCTOR binder has installed
        // a `pending_overloads[bucket_key]` entry for the *exact bucket* this call
        // would dispatch into, park on that producer so the re-dispatch on wake
        // picks up the now-registered overload. Without this, a bare-arg form
        // like `(MAKESET IntOrd)` whose FUNCTOR sibling is still parked on a SIG
        // body's Combine would race the FIFO submission order and surface
        // `DispatchFailed` even though every ingredient is in flight.
        //
        // Keying by the full `UntypedKey` (not just the lead keyword) keeps
        // overloads that share a head keyword but differ in later keywords
        // (`(MAKESET _)` vs `(MAKESET _ USING _)`) from colliding: the bare-arg
        // call's `untyped_key()` matches exactly one in-flight binder's
        // inner-call bucket.
        if let Some(producer) = pending_overload_producer(&key, dispatch_scope, chain) {
            return ResolveOutcome::ParkOnProducers(vec![producer]);
        }
        ResolveOutcome::Unmatched
    }
}

/// Walk `scope.ancestors()` looking for a `pending_overloads[key]` entry — installed
/// by an FN / FUNCTOR binder's `binder_bucket` hook for the inner-call bucket key
/// of the to-be-registered overload. Returns the producer `NodeId` of the first
/// matching binder slot (lexically inner-most wins, mirroring `Scope::resolve`).
///
/// Filters per-entry visibility against `chain` so a later-sibling pending overload
/// is hidden from this consumer: the bare-arg call would have to wait on a producer
/// that is, by the visibility rule, not yet in scope. `chain = None` disables the
/// gate, matching the value-side resolver convention (test fixtures / builtin paths).
fn pending_overload_producer<'a>(
    key: &crate::machine::model::types::UntypedKey,
    scope: &Scope<'a>,
    chain: Option<&LexicalFrame>,
) -> Option<NodeId> {
    for scope in scope.ancestors() {
        if let Some((producer, idx)) = scope.bindings().pending_overloads().get(key).copied() {
            if crate::machine::core::scope::visible(scope.id, idx, chain) {
                return Some(producer);
            }
        }
    }
    None
}

/// View over a single scope's overload bucket. Encapsulates the
/// filter‑then‑[`ExpressionSignature::most_specific`] dance that runs twice per
/// scope (strict pass, then tentative auto‑wrap pass).
///
/// Candidates carry their per-overload [`BindingIndex`] — Phase 4 will pre-filter
/// by visibility before the admit predicate runs.
struct OverloadBucket<'a, 's> {
    candidates: &'s [(&'a KFunction<'a>, crate::machine::core::BindingIndex)],
}

impl<'a> OverloadBucket<'a, '_> {
    /// `chain` is the consumer's lexical chain — threaded through to
    /// `signature_admits_strict`'s bare-name peek so the gated `resolve` agrees
    /// with the gate the bucket itself just applied (D4). `None` disables the gate.
    fn pick_strict(
        &self,
        expr: &KExpression<'a>,
        dispatch_scope: &'a Scope<'a>,
        chain: Option<&LexicalFrame>,
    ) -> PickPass<'a> {
        self.pick(expr, move |f, e| {
            signature_admits_strict(&f.signature, e, dispatch_scope, chain)
        })
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
        let survivors: Vec<&'a KFunction<'a>> = self
            .candidates
            .iter()
            .map(|(f, _)| *f)
            .filter(|f| admit(f, expr))
            .collect();
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
    chain: Option<&LexicalFrame>,
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
            // Consult the gated resolver so a not-yet-visible later-sibling
            // binding is invisible to the strict-pass peek (it would park /
            // tentative-tie instead).
            match dispatch_scope.resolve_with_chain(name, chain) {
                Resolution::Value(obj) => arg.ktype.accepts_part(&ExpressionPart::Future(obj)),
                Resolution::Placeholder(_) | Resolution::UnboundName => false,
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
    chain: Option<&LexicalFrame>,
) -> (Vec<NodeId>, Option<String>) {
    let mut placeholders = Vec::new();
    let mut unbound = None;
    for part in &expr.parts {
        if let ExpressionPart::Identifier(name) = &part.value {
            match dispatch_scope.resolve_with_chain(name, chain) {
                Resolution::Placeholder(producer) => placeholders.push(producer),
                Resolution::UnboundName if unbound.is_none() => unbound = Some(name.clone()),
                Resolution::UnboundName | Resolution::Value(_) => {}
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
        placeholder_name: picked.binder_name.and_then(|extractor| extractor(expr)),
        pending_overload_bucket: picked.binder_bucket.and_then(|extractor| extractor(expr)),
        slots: picked.classify_for_pick(expr),
    }
}
