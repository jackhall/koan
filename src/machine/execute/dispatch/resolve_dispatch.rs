//! Overload resolution for a [`KExpression`] against the lexical scope chain.
//!
//! Read-only consumer of the dispatch table. Strict admission is the only
//! admission rule: the caller builds a `bare_outcomes` cache (one
//! [`NameOutcome`] per bare-name part) which [`signature_admits_strict`]
//! consults instead of re-resolving each part per scope. A strict tie surfaces
//! [`ResolveOutcome::Ambiguous`] (or [`ResolveOutcome::Deferred`] when the
//! deciding arg is still an unevaluated eager part). Strict-Empty at every
//! scope falls through to the post-walk cache-driven fallback.

use crate::machine::core::kfunction::{ClassifiedSlots, KFunction};
use crate::machine::core::{FunctionLookup, KError, LexicalFrame, Scope};
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{ExpressionSignature, KType, SignatureElement};
use crate::machine::model::values::KObject;
use crate::machine::NodeId;

/// Cached outcome of resolving a bare-name part (`Identifier` or leaf `Type`).
/// Built once per dispatch into a slice paralleling `expr.parts` (`None` for
/// non-bare-name parts) and consumed by strict admission and the post-walk
/// fallback. `Cycle` and `ProducerErrored` are short-circuited upfront and
/// treated as defensive rejects here.
pub enum NameOutcome<'a> {
    Resolved(&'a KObject<'a>),
    Parked(NodeId),
    ProducerErrored(KError),
    Unbound(String),
    /// Parking would close a wake cycle (trivial `LET Ty = Ty` etc.).
    Cycle(String),
}

// Test-only entry counter: fast-lane dispatch shapes must route around the
// candidate machinery, so the counter must not advance for them.
#[cfg(test)]
thread_local! {
    static RESOLVE_DISPATCH_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub fn resolve_dispatch_entry_count() -> usize {
    RESOLVE_DISPATCH_ENTRIES.with(|c| c.get())
}

#[cfg(test)]
pub fn reset_resolve_dispatch_entry_count() {
    RESOLVE_DISPATCH_ENTRIES.with(|c| c.set(0));
}

/// Picked function plus the per-slot classification the dispatch driver needs
/// for auto-wrap, replay-park, and eager-sub scheduling. Sole carrier of the
/// disjoint `(eager_indices | wrap_indices | ref_name_indices)` invariant from
/// [`crate::machine::core::kfunction::ClassifiedSlots`].
pub struct Resolved<'a> {
    pub function: &'a KFunction<'a>,
    pub placeholder_name: Option<String>,
    /// `Some(_)` only for binder builtins whose body registers a callable
    /// function (FN, FUNCTOR): holds the inner-call bucket key so a sibling
    /// bare-arg call to the to-be-registered overload parks on this slot.
    pub pending_overload_bucket: Option<crate::machine::model::types::UntypedKey>,
    pub slots: ClassifiedSlots,
}

pub enum ResolveOutcome<'a> {
    Resolved(Resolved<'a>),
    Ambiguous(usize),
    Deferred,
    /// Park on forward-reference placeholders (or an in-flight sibling
    /// FN/FUNCTOR `pending_overloads[key]`) and re-dispatch once they bind.
    /// Distinct from `Deferred`: waits on existing producers without
    /// scheduling new work.
    ParkOnProducers(Vec<NodeId>),
    /// A bare-name arg resolves to nothing — no binding and no placeholder.
    /// The unbound name is the precise cause, so it surfaces here rather than
    /// as a dispatch miss.
    UnboundName(String),
    Unmatched,
}

impl<'a> Scope<'a> {
    /// Chain-gated, cache-driven dispatch resolution.
    ///
    /// Each candidate is filtered against the visibility predicate before
    /// admission — per-overload tagging matters because overloads in a bucket
    /// may sit at different lexical positions. `chain = None` is reserved for
    /// test-only callers; production paths always supply the slot's chain.
    /// An empty `bare_outcomes` reverts admission to shape-only
    /// `arg.matches(part)`.
    pub fn resolve_dispatch(
        &'a self,
        expr: &KExpression<'a>,
        chain: Option<&LexicalFrame>,
        bare_outcomes: &[Option<NameOutcome<'a>>],
    ) -> ResolveOutcome<'a> {
        #[cfg(test)]
        RESOLVE_DISPATCH_ENTRIES.with(|c| c.set(c.get() + 1));
        let key = expr.untyped_key();
        // Innermost pending-overload producer, surfaced post-walk only if no
        // bucket admits anywhere.
        let mut pending_producer: Option<NodeId> = None;
        let picked = self
            .ancestors()
            .find_map(|scope| -> Option<ResolveOutcome<'a>> {
                let cutoff = chain.and_then(|c| c.index_for(scope.id));
                match scope.bindings().lookup_function(&key, cutoff) {
                    FunctionLookup::None => None,
                    FunctionLookup::Pending(producer) => {
                        if pending_producer.is_none() {
                            pending_producer = Some(producer);
                        }
                        None
                    }
                    FunctionLookup::Bucket(candidates) => {
                        let bucket = OverloadBucket {
                            candidates: &candidates,
                        };
                        match bucket.pick_strict(expr, bare_outcomes) {
                            PickPass::Picked(f) => {
                                Some(ResolveOutcome::Resolved(build_resolved(f, expr)))
                            }
                            // Tie with an unevaluated eager part may break once it
                            // evaluates: a typed `Future(List …)` re-dispatch is
                            // element-aware where the bare literal is shape-only.
                            // Defer; a genuine tie resurfaces as `Ambiguous` on the
                            // post-eager-subs pass.
                            PickPass::Tie(n) if expr_has_eager_part(expr) => {
                                let _ = n;
                                Some(ResolveOutcome::Deferred)
                            }
                            PickPass::Tie(n) => Some(ResolveOutcome::Ambiguous(n)),
                            PickPass::Empty => None,
                        }
                    }
                }
            });
        if let Some(outcome) = picked {
            return outcome;
        }
        // Post-walk strict-Empty fallback derived from the cache. See
        // [design/typing/scheduler.md § Post-walk dispatch fallback precedence](../../../../design/typing/scheduler.md#post-walk-dispatch-fallback-precedence).
        let mut placeholders: Vec<NodeId> = Vec::new();
        for outcome in bare_outcomes.iter().flatten() {
            if let NameOutcome::Parked(p) = outcome {
                if !placeholders.contains(p) {
                    placeholders.push(*p);
                }
            }
        }
        if !placeholders.is_empty() {
            return ResolveOutcome::ParkOnProducers(placeholders);
        }
        if expr_has_eager_part(expr) {
            return ResolveOutcome::Deferred;
        }
        if let Some(name) = bare_outcomes.iter().find_map(|o| match o {
            Some(NameOutcome::Unbound(n)) => Some(n.clone()),
            _ => None,
        }) {
            return ResolveOutcome::UnboundName(name);
        }
        if let Some(producer) = pending_producer {
            return ResolveOutcome::ParkOnProducers(vec![producer]);
        }
        ResolveOutcome::Unmatched
    }
}

/// View over a single scope's visibility-pre-filtered overload bucket.
/// Encapsulates the filter-then-[`ExpressionSignature::most_specific`] dance.
struct OverloadBucket<'a, 's> {
    candidates: &'s [&'a KFunction<'a>],
}

impl<'a> OverloadBucket<'a, '_> {
    fn pick_strict(
        &self,
        expr: &KExpression<'a>,
        bare_outcomes: &[Option<NameOutcome<'a>>],
    ) -> PickPass<'a> {
        let survivors: Vec<&'a KFunction<'a>> = self
            .candidates
            .iter()
            .copied()
            .filter(|f| signature_admits_strict(&f.signature, expr, bare_outcomes))
            .collect();
        let sigs: Vec<&ExpressionSignature> = survivors.iter().map(|f| &f.signature).collect();
        match ExpressionSignature::most_specific(&sigs) {
            Some(i) => PickPass::Picked(survivors[i]),
            None if !survivors.is_empty() => PickPass::Tie(survivors.len()),
            None => PickPass::Empty,
        }
    }
}

/// Policy-free outcome of one filter→`most_specific` pass; the `Tie` →
/// `Ambiguous` / `Deferred` translation lives at the call site.
enum PickPass<'a> {
    Picked(&'a KFunction<'a>),
    Tie(usize),
    Empty,
}

/// Strict admission against the `bare_outcomes` cache. Rule table at
/// [design/typing/elaboration.md § Strict admission rules](../../../../design/typing/elaboration.md#strict-admission-rules).
fn signature_admits_strict<'a>(
    sig: &ExpressionSignature<'a>,
    expr: &KExpression<'a>,
    bare_outcomes: &[Option<NameOutcome<'a>>],
) -> bool {
    if sig.elements.len() != expr.parts.len() {
        return false;
    }
    // Lazy-candidate gate: a `KType::KExpression` slot bound by an
    // `ExpressionPart::Expression` relaxes other non-`KExpression` slots to
    // admit `Expression` / `SigiledTypeExpr` parts speculatively (they route
    // through `eager_indices` post-pick). Required by FN / FUNCTOR overloads.
    let has_lazy_kexpr_slot =
        sig.elements
            .iter()
            .zip(&expr.parts)
            .any(|(el, part)| match (el, &part.value) {
                (SignatureElement::Argument(arg), ExpressionPart::Expression(_)) => {
                    matches!(arg.ktype, KType::KExpression)
                }
                _ => false,
            });
    sig.elements
        .iter()
        .zip(&expr.parts)
        .enumerate()
        .all(|(i, (el, part))| {
            match (el, &part.value) {
                (SignatureElement::Keyword(s), ExpressionPart::Keyword(t)) => s == t,
                (SignatureElement::Keyword(_), _) => false,
                (SignatureElement::Argument(arg), part_value) => {
                    // Binder declaration slot: the slot owns the name, so admission
                    // is shape-only. SigiledTypeExpr still admits speculatively (it
                    // sub-dispatches to a type-side carrier).
                    if matches!(arg.ktype, KType::Identifier | KType::TypeExprRef) {
                        if matches!(part_value, ExpressionPart::SigiledTypeExpr(_)) {
                            return true;
                        }
                        return arg.matches(part_value);
                    }
                    // SigiledTypeExpr in a non-KExpression slot sub-dispatches to a
                    // type-side carrier.
                    if matches!(part_value, ExpressionPart::SigiledTypeExpr(_))
                        && !matches!(arg.ktype, KType::KExpression)
                    {
                        return true;
                    }
                    // Lazy-candidate relaxation (see `has_lazy_kexpr_slot`).
                    if has_lazy_kexpr_slot
                        && matches!(part_value, ExpressionPart::Expression(_))
                        && !matches!(arg.ktype, KType::KExpression)
                    {
                        return true;
                    }
                    match bare_outcomes.get(i).and_then(|o| o.as_ref()) {
                        Some(NameOutcome::Resolved(obj)) => {
                            arg.ktype.accepts_part(&ExpressionPart::Future(obj))
                        }
                        // Speculative admit so the splice/park walk can surface the
                        // precise per-slot diagnostic.
                        Some(NameOutcome::Parked(_)) | Some(NameOutcome::Unbound(_)) => {
                            arg.matches(part_value)
                        }
                        Some(NameOutcome::Cycle(_)) | Some(NameOutcome::ProducerErrored(_)) => {
                            false
                        }
                        None => arg.matches(part_value),
                    }
                }
            }
        })
}

/// True iff `expr` carries any part shape the scheduler's eager loop would
/// schedule as a sub-Dispatch.
fn expr_has_eager_part(expr: &KExpression<'_>) -> bool {
    use crate::machine::model::ast::ExpressionPart;
    expr.parts.iter().any(|p| {
        matches!(
            &p.value,
            ExpressionPart::Expression(_)
                | ExpressionPart::SigiledTypeExpr(_)
                | ExpressionPart::ListLiteral(_)
                | ExpressionPart::DictLiteral(_)
                | ExpressionPart::RecordLiteral(_)
        )
    })
}

/// Sole producer of the embedded `slots`; disjointness lives in
/// [`KFunction::classify_for_pick`].
fn build_resolved<'a>(picked: &'a KFunction<'a>, expr: &KExpression<'a>) -> Resolved<'a> {
    Resolved {
        function: picked,
        placeholder_name: picked.binder_name.and_then(|extractor| extractor(expr)),
        pending_overload_bucket: picked.binder_bucket.and_then(|extractor| extractor(expr)),
        slots: picked.classify_for_pick(expr),
    }
}
