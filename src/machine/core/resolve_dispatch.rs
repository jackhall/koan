//! Overload resolution for a [`KExpression`] against the lexical scope chain.
//!
//! Read‑only consumer of the dispatch table: walks `Scope::ancestors()` and, at each
//! scope, looks up `bindings().functions()` keyed by the expression's untyped key.
//!
//! ## Invariant: strict-only admission via cached bare-name outcomes
//!
//! Strict admission is the only admission rule. Caller builds a `bare_outcomes`
//! cache (one [`NameOutcome`] entry per bare-name part) before the candidate walk
//! and passes it as a slice; [`signature_admits_strict`] reads the cache rather
//! than re-resolving each part per scope. A `Resolved` outcome admits via
//! `accepts_part` on the carried type; `Parked` / `Unbound` reject so the
//! candidate falls out of the strict pass. The post-walk fallback consults the
//! cache to surface the precise [`ResolveOutcome::ParkOnProducers`] /
//! [`ResolveOutcome::UnboundName`] (placeholders > unbound > eager > pending
//! overload > Unmatched). A strict tie at any scope surfaces
//! [`ResolveOutcome::Ambiguous`] (or [`ResolveOutcome::Deferred`] when the
//! deciding arg is an unevaluated eager part). Strict-Empty at every scope
//! reaches the post-walk fallback; the candidate walk is a single ancestor pass.

use crate::machine::core::kerror::KError;
use crate::machine::core::kfunction::{ClassifiedSlots, KFunction};
use crate::machine::core::lexical_frame::LexicalFrame;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{ExpressionSignature, KType, SignatureElement};
use crate::machine::model::values::KObject;
use crate::machine::NodeId;

use super::bindings::FunctionLookup;
use super::scope::Scope;


/// Outcome of resolving a bare-name part (`Identifier` or leaf `Type`) against the
/// dispatching scope. Built once per `Scheduler::run_dispatch` into a
/// `bare_outcomes: Vec<Option<NameOutcome<'a>>>` cache and consumed by:
///
/// - **Strict admission** ([`signature_admits_strict`]): `Resolved` admits on the
///   carried type via `accepts_part`; `Parked` / `Unbound` reject so the candidate
///   falls out of the strict pass and the post-walk fallback consults the cache for
///   the precise `ParkOnProducers` / `UnboundName` surface. `Cycle` and
///   `ProducerErrored` are short-circuited *upfront* in `run_dispatch` (they trump
///   any overload choice), so they're treated as defensive rejects here.
/// - **Splice/park walk** (the fused Phase 3 / Phase 4 in `run_dispatch`): `Resolved`
///   splices `Future(obj)` into wrap slots; `Parked` feeds the combined-park
///   producer list; `Unbound` surfaces `UnboundName` as a slot terminal;
///   `ProducerErrored` and `Cycle` are unreachable here (the upfront sweep fired).
///
/// Cached `None` entries correspond to non-bare-name parts (literals, parens,
/// Future, etc.); admission falls back to `arg.matches(part)` for them.
pub enum NameOutcome<'a> {
    /// Bare name resolved to a value-side binding. Strict admission tests the
    /// carrier's actual carried type via [`crate::machine::model::types::KType::accepts_part`];
    /// the splice walk rewrites the slot to `ExpressionPart::Future(obj)`.
    Resolved(&'a KObject<'a>),
    /// Bare name resolves to a still-pending placeholder. The post-walk fallback
    /// surfaces `ResolveOutcome::ParkOnProducers`, and the splice walk pushes the
    /// producer onto the shared `producers_to_wait` list.
    Parked(NodeId),
    /// The producer this name resolved to has already terminalized with `Err`.
    /// Caught by the upfront sweep in `run_dispatch` and propagated with a
    /// `<wrap-resolve>` frame; never reaches admission or the splice walk.
    ProducerErrored(KError),
    /// Bare name has no binding anywhere on the scope chain. The post-walk
    /// fallback surfaces `ResolveOutcome::UnboundName`; the splice walk surfaces
    /// the same `UnboundName` as a slot terminal at the wrap site.
    Unbound(String),
    /// Caller-side parking would close a wake cycle (trivial `LET Ty = Ty` etc.).
    /// Caught by the upfront sweep in `run_dispatch` and surfaced as
    /// `SchedulerDeadlock`; never reaches admission or the splice walk.
    Cycle(String),
}

// Test-only entry counter for `Scope::resolve_dispatch_with_chain`. Used by the
// fast-lane dispatch-shape tests (`scheduler::tests::dispatch_shapes`) to assert
// that no-keyword shapes (`BareTypeLeaf`, `TypeCall`, `FunctionValueCall`,
// `BareIdentifier`) route around the candidate machinery — the counter must not
// advance for a fast-lane shape. Lives on a thread-local because the test harness
// runs each test in a single thread and constructs a fresh scheduler per case.
#[cfg(test)]
thread_local! {
    static RESOLVE_DISPATCH_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Test-only: read the entry counter. Returns the number of times
/// `resolve_dispatch_with_chain` has been entered on this thread since the last reset.
#[cfg(test)]
pub fn resolve_dispatch_entry_count() -> usize {
    RESOLVE_DISPATCH_ENTRIES.with(|c| c.get())
}

/// Test-only: zero the entry counter so a subsequent call to
/// `resolve_dispatch_entry_count` measures only the operations that follow.
#[cfg(test)]
pub fn reset_resolve_dispatch_entry_count() {
    RESOLVE_DISPATCH_ENTRIES.with(|c| c.set(0));
}

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
    /// No bucket admitted anywhere, but ≥1 bare-name arg resolved to a still-pending
    /// forward-reference `Placeholder`. Park on the carried producer `NodeId`s and
    /// re-dispatch once they bind, where strict admission can read the now-bound type
    /// via the rebuilt cache. Distinct from `Deferred` (which schedules eager parts);
    /// this waits on existing producers without scheduling work. Also covers the
    /// "no bucket, but a sibling FN / FUNCTOR binder's `pending_overloads[key]`
    /// entry is in flight" case (innermost wins).
    ParkOnProducers(Vec<NodeId>),
    /// No bucket admitted anywhere and ≥1 bare-name arg resolves to nothing — no
    /// binding and no forward-reference placeholder. The call can never resolve, and
    /// the precise cause is the unbound name, so it surfaces as `UnboundName` rather
    /// than a dispatch miss.
    UnboundName(String),
    Unmatched,
}

impl<'a> Scope<'a> {
    /// Convenience entry point for test fixtures and builtin registration paths that
    /// construct `KExpression`s directly. Passes `chain = None` (visibility gate
    /// disabled) and `bare_outcomes = &[]` (admission falls back to
    /// `arg.matches(part)` for every slot — same shape as the legacy "no cache"
    /// behavior).
    pub fn resolve_dispatch(&'a self, expr: &KExpression<'a>) -> ResolveOutcome<'a> {
        self.resolve_dispatch_with_chain(expr, None, &[])
    }

    /// Chain-gated, cache-driven dispatch resolution. The caller (currently
    /// [`Scheduler::run_dispatch`](../execute/scheduler/dispatch.rs)) builds a
    /// `bare_outcomes` cache by calling `resolve_name_part` once per bare-name
    /// part of `expr` and short-circuits `NameOutcome::ProducerErrored` upfront;
    /// the resolver here consumes the `Resolved` / `Parked` / `Unbound` outcomes
    /// for strict admission and the post-walk fallback. Cycle detection is
    /// deferred to the post-pick splice/park walk in the caller (where it runs
    /// only on slots the picked function classifies as references, so a binder
    /// declaration slot's self-parked outcome doesn't false-positive).
    ///
    /// Each candidate is filtered against the visibility predicate before
    /// admission (per-overload tagging matters because overloads in a bucket may
    /// sit at different lexical positions). `chain = None` disables the gate
    /// (test fixtures, builtin registration paths); `bare_outcomes = &[]`
    /// reverts admission to `arg.matches(part)` for every slot.
    pub fn resolve_dispatch_with_chain(
        &'a self,
        expr: &KExpression<'a>,
        chain: Option<&LexicalFrame>,
        bare_outcomes: &[Option<NameOutcome<'a>>],
    ) -> ResolveOutcome<'a> {
        #[cfg(test)]
        RESOLVE_DISPATCH_ENTRIES.with(|c| c.set(c.get() + 1));
        let key = expr.untyped_key();
        // Innermost pending-overload producer recorded during the walk, surfaced
        // post-walk only if no bucket admits anywhere. Folding the previous
        // separate `pending_overload_producer` walk into the main ancestor pass
        // saves a second traversal.
        let mut pending_producer: Option<NodeId> = None;
        let picked = self.ancestors().find_map(|scope| -> Option<ResolveOutcome<'a>> {
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
                    let bucket = OverloadBucket { candidates: &candidates };
                    match bucket.pick_strict(expr, bare_outcomes) {
                        PickPass::Picked(f) => Some(ResolveOutcome::Resolved(build_resolved(f, expr))),
                        // A strict tie whose deciding argument is still an unevaluated literal /
                        // sub-expression may resolve once evaluated: a typed `Future(List …)`
                        // re-dispatch is element-aware (`accepts_part`) where the bare literal is
                        // shape-only and ties. Defer rather than hard-erroring — the eager-subs
                        // resume re-resolves the rewritten `Future(_)` expression, which carries no
                        // eager parts, so a genuine tie surfaces as `Ambiguous` on that second pass.
                        PickPass::Tie(n) if expr_has_eager_part(expr) => {
                            let _ = n;
                            Some(ResolveOutcome::Deferred)
                        }
                        PickPass::Tie(n) => Some(ResolveOutcome::Ambiguous(n)),
                        // Strict-Empty: nothing in this scope's bucket admits. Keep
                        // walking; the post-walk fallback reads `bare_outcomes` to
                        // surface the precise `ParkOnProducers` / `UnboundName` /
                        // `Deferred` / `Unmatched` shape if no outer scope picks.
                        PickPass::Empty => None,
                    }
                }
            }
        });
        if let Some(outcome) = picked {
            return outcome;
        }
        // Post-walk strict-Empty fallback derived from the cache. Precedence:
        // 1. Bare-name Placeholders ⇒ ParkOnProducers (re-dispatch on wake, when
        //    strict admission can read the now-bound type via the rebuilt cache).
        // 2. Eager parts present    ⇒ Deferred (the candidate may match after
        //    sub-evaluation yields a typed `Future(_)`). Higher precedence than
        //    Unbound because an eager part's resolution may itself surface the
        //    precise diagnostic; surfacing UnboundName here would pre-empt a
        //    `Expression-in-Type-slot`-style dispatch (`(maybe) some 42`) whose
        //    head `Expression([maybe])` evaluates to the schema after one sub-Dispatch.
        // 3. Bare-name Unbound      ⇒ UnboundName (precise error — name resolves
        //    to nothing and no eager part can salvage the dispatch).
        // 4. Pending-overload entry ⇒ ParkOnProducers (FN/FUNCTOR sibling parked on
        //    its own Combine has installed `pending_overloads[key]` for the exact
        //    inner-call bucket; wake re-dispatches against the now-registered overload).
        // 5. Otherwise              ⇒ Unmatched (DispatchFailed at the call site).
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

/// View over a single scope's pre-filtered overload bucket. Encapsulates the
/// filter‑then‑[`ExpressionSignature::most_specific`] dance.
///
/// The slice holds only candidates already pre-filtered for visibility by
/// [`crate::machine::core::Bindings::lookup_function`]; the per-overload
/// `BindingIndex` is consumed there and no longer needed here.
struct OverloadBucket<'a, 's> {
    candidates: &'s [&'a KFunction<'a>],
}

impl<'a> OverloadBucket<'a, '_> {
    /// `bare_outcomes` is the per-`run_dispatch` resolution cache — one
    /// [`NameOutcome`] entry per bare-name part of `expr`, `None` for non-bare-name
    /// parts. Strict admission reads the cache rather than re-resolving each part
    /// per scope.
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

/// Outcome of one filter→`most_specific` pass. Policy-free: the strict `Tie` →
/// `Ambiguous` / `Deferred` translation lives at the call site.
enum PickPass<'a> {
    Picked(&'a KFunction<'a>),
    Tie(usize),
    Empty,
}

/// Strict admission against the per-`run_dispatch` `bare_outcomes` cache.
///
/// Admission rules per cache entry on a bare-name part:
/// - **`Resolved(obj)`** — admit iff [`KType::accepts_part`] returns true for
///   `Future(obj)`. **This is the strict-only PR C admission**: a bare name
///   that resolves to a value with the wrong carried type strict-rejects
///   (rather than tentative-admitting into a TypeMismatch at bind time).
///   This unifies the pre-PR-C container-only peek and the tentative blind
///   admit into one cache-driven rule. Ties between an Identifier-typed slot
///   overload and a concrete-typed slot overload (`ATTR <s:Identifier>` vs
///   `ATTR <s:Struct>` for `ATTR p z`) resolve via [`KType::is_more_specific_than`],
///   which ranks concrete types above the unconstrained name slots.
/// - **`Parked(_)` / `Unbound(_)`** — admit via shape-only `arg.matches(part)`.
///   These cases preserve the pre-PR-C tentative-admit behavior: the candidate
///   admits speculatively, the dispatch driver picks the overload, and the
///   fused splice/park walk surfaces the precise [`ResolveOutcome`]
///   (`ParkOnProducers` for `Parked`, `UnboundName` as a slot terminal for
///   `Unbound`). This is the only path that produces precise per-slot
///   `UnboundName` / `ParkOnProducers` diagnostics, so admission must not
///   reject and lose them.
/// - **`ProducerErrored(_)`** — defensive reject (the upfront sweep in
///   `Scheduler::run_dispatch` already short-circuited).
/// - **`Cycle(_)`** — unreachable (cache is built with `consumer = None`).
///
/// `None` cache entries (non-bare-name parts: literals, parens, `Future`, …)
/// fall back to `arg.matches(part)` (the pure shape-and-literal check).
///
/// **Binder declaration slots** (`KType::Identifier`, `KType::TypeExprRef`)
/// skip the cache lookup entirely and fall back to `arg.matches(part)`: the
/// slot is a *declaration* whose name is owned by the binder (`LET x`,
/// `STRUCT Foo`, …), so admission must depend only on the part's shape, not on
/// whether `x` happens to be bound or parked.
fn signature_admits_strict<'a>(
    sig: &ExpressionSignature<'a>,
    expr: &KExpression<'a>,
    bare_outcomes: &[Option<NameOutcome<'a>>],
) -> bool {
    if sig.elements.len() != expr.parts.len() {
        return false;
    }
    // Lazy-candidate gate: does the signature have a `KType::KExpression` slot
    // bound by an `ExpressionPart::Expression`? If so, an `Expression` /
    // `SigiledTypeExpr` part in a *non-*`KExpression` slot admits speculatively
    // (it'll route through `eager_indices` for sub-Dispatch post-pick). Mirrors
    // the legacy `accepts_for_wrap` relaxation; required by FN / FUNCTOR
    // overloads whose `signature` + `body` slots are `KExpression` and whose
    // `return_type` slot is `TypeExprRef`.
    let has_lazy_kexpr_slot = sig.elements.iter().zip(&expr.parts).any(|(el, part)| match (
        el,
        &part.value,
    ) {
        (SignatureElement::Argument(arg), ExpressionPart::Expression(_)) => {
            matches!(arg.ktype, KType::KExpression)
        }
        _ => false,
    });
    sig.elements.iter().zip(&expr.parts).enumerate().all(|(i, (el, part))| {
        match (el, &part.value) {
            (SignatureElement::Keyword(s), ExpressionPart::Keyword(t)) => s == t,
            (SignatureElement::Keyword(_), _) => false,
            (SignatureElement::Argument(arg), part_value) => {
                // Binder declaration slots: the slot owns the name; admission is
                // shape-only (the binder's body will install / consume the name).
                if matches!(arg.ktype, KType::Identifier | KType::TypeExprRef) {
                    // Special case: SigiledTypeExpr in a TypeExprRef slot
                    // admits speculatively — the sigil sub-dispatches to a
                    // type-side carrier (`KTypeValue`) that the receiving slot
                    // accepts after splice. Symmetric with the
                    // SigiledTypeExpr-in-other-non-KExpression-slot relaxation
                    // below, but called out here so the binder-decl exemption
                    // doesn't pre-empt it.
                    if matches!(part_value, ExpressionPart::SigiledTypeExpr(_)) {
                        return true;
                    }
                    return arg.matches(part_value);
                }
                // SigiledTypeExpr in a non-KExpression slot: admit speculatively
                // (the sigil sub-dispatches to a type-side carrier).
                if matches!(part_value, ExpressionPart::SigiledTypeExpr(_))
                    && !matches!(arg.ktype, KType::KExpression)
                {
                    return true;
                }
                // Expression in a non-KExpression slot, *gated by* the
                // lazy-candidate shape: admit speculatively. The post-pick walk
                // routes this slot through `eager_indices`.
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
                    // Parked / Unbound: admit via shape-only check. The candidate
                    // admits speculatively; the fused splice/park walk surfaces
                    // the precise per-slot `ParkOnProducers` / `UnboundName`.
                    Some(NameOutcome::Parked(_)) | Some(NameOutcome::Unbound(_)) => {
                        arg.matches(part_value)
                    }
                    Some(NameOutcome::Cycle(_)) | Some(NameOutcome::ProducerErrored(_)) => false,
                    None => arg.matches(part_value),
                }
            }
        }
    })
}

/// True iff `expr` carries any `Expression` / `SigiledTypeExpr` / `ListLiteral` /
/// `DictLiteral` part — the shapes the scheduler's eager loop would schedule as
/// sub-Dispatches.
fn expr_has_eager_part(expr: &KExpression<'_>) -> bool {
    use crate::machine::model::ast::ExpressionPart;
    expr.parts.iter().any(|p| {
        matches!(
            &p.value,
            ExpressionPart::Expression(_)
                | ExpressionPart::SigiledTypeExpr(_)
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
