//! Overload resolution for a [`KExpression`] against the lexical scope chain.
//!
//! Read-only consumer of the dispatch table. The caller builds a
//! `bare_outcomes` cache (one [`NameOutcome`] per bare-name part) consulted by
//! admission instead of re-resolving each part per scope. Each scope is decided
//! in walk order (innermost first): a visible in-flight pending overload parks
//! the scope (it would shadow once finalized); a strict [`OverloadBucket::pick_strict`]
//! Picks (tie ⇒ `Ambiguous`, or `Deferred` when an eager part may break it); a
//! strict-Empty bucket runs one relaxed-admission pass per candidate that may
//! park (forward-reference producers) or defer (eager parts). Only a *dead*
//! unbound bare-name lean and total non-admission are post-walk terminals — a
//! dead lean must not pre-empt an outer scope that could strict-Pick the bare
//! name as an `:Identifier` / `:Any` slot.

use crate::machine::core::kfunction::{ClassifiedSlots, KFunction};
use crate::machine::core::{BindKind, FunctionLookup, KError, LexicalFrame, Scope};
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::KKind;
use crate::machine::model::types::{ExpressionSignature, KType, SignatureElement};
use crate::machine::model::Carried;
use crate::machine::NodeId;

/// Cached outcome of resolving a bare-name part (`Identifier` or leaf `Type`).
/// Built once per dispatch into a slice paralleling `expr.parts` (`None` for
/// non-bare-name parts) and consumed by strict admission and the relaxed pass.
/// `Cycle` and `ProducerErrored` are short-circuited upfront and treated as
/// defensive rejects here.
pub enum NameOutcome<'step> {
    Resolved(Carried<'step>),
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
pub struct Resolved<'step> {
    pub function: &'step KFunction<'step>,
    /// The forward-reference name a binder declares, with the language it binds in, so the
    /// dispatch driver installs a kind-tagged placeholder. `None` for a non-binder pick.
    pub placeholder: Option<(String, BindKind)>,
    /// `Some(_)` only for binder builtins whose body registers a callable
    /// function (FN, FUNCTOR): holds the inner-call bucket key so a sibling
    /// bare-arg call to the to-be-registered overload parks on this slot.
    pub pending_overload_bucket: Option<crate::machine::model::types::UntypedKey>,
    pub slots: ClassifiedSlots,
}

pub enum DispatchOutcome<'step> {
    Resolved(Resolved<'step>),
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

impl<'step> Scope<'step> {
    /// Chain-gated, cache-driven dispatch resolution.
    ///
    /// Each candidate is filtered against the visibility predicate before
    /// admission — per-overload tagging matters because overloads in a bucket
    /// may sit at different lexical positions. `chain = None` is reserved for
    /// test-only callers; production paths always supply the slot's chain.
    /// An empty `bare_outcomes` reverts admission to shape-only
    /// `arg.matches(part)`.
    ///
    /// Innermost scope with a terminal decision wins (lexical shadowing). A
    /// dead unbound bare-name lean is the sole non-terminal — it is accumulated
    /// and surfaced post-walk only if no scope terminated, so an outer scope can
    /// still strict-Pick the bare name as an `:Identifier` / `:Any` slot.
    pub fn resolve_dispatch<'e>(
        &self,
        expr: &KExpression<'e>,
        chain: Option<&LexicalFrame>,
        bare_outcomes: &[Option<NameOutcome<'e>>],
    ) -> DispatchOutcome<'step> {
        #[cfg(test)]
        RESOLVE_DISPATCH_ENTRIES.with(|c| c.set(c.get() + 1));
        let key = expr.untyped_key();
        // Builtin dispatch buckets are unshadowable — no user overload may join them — so a
        // builtin bucket is authoritative. Consult the immutable root directly and return its
        // terminal decision, skipping the user-chain walk for the hottest names. Only a
        // `Terminal` decision short-circuits; a non-terminal root falls through to the full
        // walk below unchanged, so precedence is preserved. The `idx == 0` gate keeps a
        // synthetic root-position user bucket on the ordinary walk.
        let root = self.root_scope();
        if root.bindings().has_builtin_function(&key) {
            let cutoff = chain.and_then(|c| c.index_for(root.id));
            let lookup = root.bindings().lookup_function(&key, cutoff);
            if let ScopeDecision::Terminal(outcome) = decide_scope(&lookup, expr, bare_outcomes) {
                return outcome;
            }
        }
        // Innermost dead unbound bare-name lean, surfaced post-walk only if no
        // scope reached a terminal decision.
        let mut dead_lean: Option<String> = None;
        for scope in self.ancestors() {
            let cutoff = scope.binding_cutoff(chain);
            let lookup = scope.bindings().lookup_function(&key, cutoff);
            match decide_scope(&lookup, expr, bare_outcomes) {
                ScopeDecision::Terminal(outcome) => return outcome,
                ScopeDecision::DeadLean(name) => {
                    if dead_lean.is_none() {
                        dead_lean = Some(name);
                    }
                }
                ScopeDecision::Continue => {}
            }
        }
        match dead_lean {
            Some(name) => DispatchOutcome::UnboundName(name),
            None => DispatchOutcome::Unmatched,
        }
    }
}

/// Per-scope precedence: the innermost scope with a `Terminal` decision wins.
/// `DeadLean` records an unbound bare-name blocker without terminating (an
/// outer scope may strict-Pick the bare name); `Continue` means this scope
/// raised nothing.
enum ScopeDecision<'step> {
    Terminal(DispatchOutcome<'step>),
    DeadLean(String),
    Continue,
}

/// Decide one scope's contribution from its [`FunctionLookup`].
///
/// 1. A visible pending overload parks the scope — once finalized it would
///    shadow any finalized sibling here, so it takes precedence even over a
///    finalized strict-Pick at the same scope (the leaned-on parked producers
///    from the relaxed pass union in).
/// 2. Otherwise the strict gate Picks / ties / is empty over the finalized
///    overloads.
/// 3. A strict-Empty bucket runs the relaxed pass: leaned-parked ⇒ park,
///    else leaned-eager ⇒ defer, else leaned-dead ⇒ `DeadLean` (continue),
///    else `Continue`.
fn decide_scope<'step, 'e>(
    lookup: &FunctionLookup<'step>,
    expr: &KExpression<'e>,
    bare_outcomes: &[Option<NameOutcome<'e>>],
) -> ScopeDecision<'step> {
    let bucket = OverloadBucket {
        candidates: &lookup.overloads,
    };
    // Pending always parks at its scope, even over a finalized Pick: the
    // pending sibling would shadow once it finalizes, so resolve nothing until
    // it does (Decision 5). Union in any parked producers the relaxed pass
    // would have leaned on so a single wake re-runs the full resolution.
    if let Some(pending) = lookup.pending {
        let mut producers = vec![pending];
        for p in bucket.relaxed_parked_producers(expr, bare_outcomes) {
            if !producers.contains(&p) {
                producers.push(p);
            }
        }
        return ScopeDecision::Terminal(DispatchOutcome::ParkOnProducers(producers));
    }
    match bucket.pick_strict(expr, bare_outcomes) {
        PickPass::Picked(f) => {
            ScopeDecision::Terminal(DispatchOutcome::Resolved(build_resolved(f, expr)))
        }
        // Tie with an unevaluated eager part may break once it evaluates: a
        // typed `Spliced(List …)` re-dispatch is element-aware where the bare
        // literal is shape-only. Defer; a genuine tie resurfaces as `Ambiguous`
        // on the post-eager-subs pass.
        PickPass::Tie(n) if expr_has_eager_part(expr) => {
            let _ = n;
            ScopeDecision::Terminal(DispatchOutcome::Deferred)
        }
        PickPass::Tie(n) => ScopeDecision::Terminal(DispatchOutcome::Ambiguous(n)),
        PickPass::Empty => decide_relaxed(&bucket, expr, bare_outcomes),
    }
}

/// Strict-Empty relaxed pass: one assume-every-unresolved-slot-satisfiable pass
/// per candidate, classifying which unresolved-slot kinds each leaned on.
/// Parked beats eager; a dead unbound lean only records a `DeadLean` blocker —
/// it never parks, since an unbound name never arrives. A candidate that rejects
/// on a hard already-resolved /
/// literal / keyword slot does not admit even relaxed and contributes nothing.
fn decide_relaxed<'step, 'e>(
    bucket: &OverloadBucket<'step, '_>,
    expr: &KExpression<'e>,
    bare_outcomes: &[Option<NameOutcome<'e>>],
) -> ScopeDecision<'step> {
    let mut parked: Vec<NodeId> = Vec::new();
    let mut any_eager_lean = false;
    let mut dead_name: Option<String> = None;
    for f in bucket.candidates.iter() {
        let Some(leans) = relaxed_admits(&f.signature, expr, bare_outcomes) else {
            continue;
        };
        for lean in leans {
            match lean {
                Lean::Parked(p) => {
                    if !parked.contains(&p) {
                        parked.push(p);
                    }
                }
                Lean::Eager => any_eager_lean = true,
                Lean::Dead(name) => {
                    if dead_name.is_none() {
                        dead_name = Some(name);
                    }
                }
            }
        }
    }
    if !parked.is_empty() {
        return ScopeDecision::Terminal(DispatchOutcome::ParkOnProducers(parked));
    }
    if any_eager_lean {
        return ScopeDecision::Terminal(DispatchOutcome::Deferred);
    }
    match dead_name {
        Some(name) => ScopeDecision::DeadLean(name),
        None => ScopeDecision::Continue,
    }
}

/// View over a single scope's visibility-pre-filtered overload bucket.
/// Encapsulates the filter-then-[`ExpressionSignature::most_specific`] dance.
struct OverloadBucket<'step, 'b> {
    candidates: &'b [&'step KFunction<'step>],
}

impl<'step> OverloadBucket<'step, '_> {
    fn pick_strict<'e>(
        &self,
        expr: &KExpression<'e>,
        bare_outcomes: &[Option<NameOutcome<'e>>],
    ) -> PickPass<'step> {
        let survivors: Vec<&'step KFunction<'step>> = self
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

    /// Deduped `Parked` producers any candidate leans on under the relaxed pass.
    /// The caller unions these into a pending park so a single wake re-runs the
    /// full resolution.
    fn relaxed_parked_producers<'e>(
        &self,
        expr: &KExpression<'e>,
        bare_outcomes: &[Option<NameOutcome<'e>>],
    ) -> Vec<NodeId> {
        let mut producers: Vec<NodeId> = Vec::new();
        for f in self.candidates.iter() {
            let Some(leans) = relaxed_admits(&f.signature, expr, bare_outcomes) else {
                continue;
            };
            for lean in leans {
                if let Lean::Parked(p) = lean {
                    if !producers.contains(&p) {
                        producers.push(p);
                    }
                }
            }
        }
        producers
    }
}

/// Policy-free outcome of one filter→`most_specific` pass; the `Tie` →
/// `Ambiguous` / `Deferred` translation lives at the call site.
enum PickPass<'step> {
    Picked(&'step KFunction<'step>),
    Tie(usize),
    Empty,
}

/// Which unresolved-slot kind the relaxed pass leaned on at a rejecting slot.
/// `Parked` carries the forward-reference producer to park on; `Eager` a
/// not-yet-evaluated eager part; `Dead` an unbound bare name (no producer will
/// ever bind it — only labels the `UnboundName` terminal, never waits).
enum Lean {
    Parked(NodeId),
    Eager,
    Dead(String),
}

/// Strict admission against the `bare_outcomes` cache. Rule table at
/// [design/typing/elaboration.md § Strict admission rules](../../../../design/typing/elaboration.md#strict-admission-rules).
fn signature_admits_strict<'e>(
    sig: &ExpressionSignature<'_>,
    expr: &KExpression<'e>,
    bare_outcomes: &[Option<NameOutcome<'e>>],
) -> bool {
    if sig.elements.len() != expr.parts.len() {
        return false;
    }
    let has_lazy_kexpr_slot = has_lazy_kexpr_slot(sig, expr);
    sig.elements
        .iter()
        .zip(&expr.parts)
        .enumerate()
        .all(|(i, (el, part))| {
            slot_admits_strict(el, &part.value, i, has_lazy_kexpr_slot, bare_outcomes)
        })
}

/// Relaxed admission: assume every *unresolved* slot satisfiable and report
/// which kinds were leaned on. `None` ⇒ the candidate rejects even relaxed (a
/// hard already-resolved / literal / keyword slot rejects, which no arriving
/// input or binding can flip). `Some(leans)` ⇒ admits relaxed, leaning on the
/// returned unresolved slots.
///
/// "Leaned on" = strict rejects at the slot but the assume-satisfiable
/// relaxation passes it: an unevaluated eager part (`Eager`), or a bare-name
/// `Parked` / `Unbound` whose declared type rejects the bare shape (`Parked` /
/// `Dead`). A `Parked` / `Unbound` slot that strict-admits shape-only via an
/// `:Identifier` / `:Any` declaration is *not* leaned on — it just Picks.
/// One per-candidate pass names every leaned-on kind — which arriving (`Eager` /
/// `Parked`) slots, and any `Dead` blocker — so the caller decides park / defer /
/// unbound at the scope rather than re-deriving it.
fn relaxed_admits<'e>(
    sig: &ExpressionSignature<'_>,
    expr: &KExpression<'e>,
    bare_outcomes: &[Option<NameOutcome<'e>>],
) -> Option<Vec<Lean>> {
    if sig.elements.len() != expr.parts.len() {
        return None;
    }
    let has_lazy_kexpr_slot = has_lazy_kexpr_slot(sig, expr);
    let mut leans: Vec<Lean> = Vec::new();
    for (i, (el, part)) in sig.elements.iter().zip(&expr.parts).enumerate() {
        if slot_admits_strict(el, &part.value, i, has_lazy_kexpr_slot, bare_outcomes) {
            continue;
        }
        // Strict rejected — admit relaxed only if this is an unresolved slot the
        // relaxation assumes satisfiable. An unevaluated eager part in an
        // *argument* slot routes through `eager_indices` post-pick; a keyword
        // element can't be satisfied by an eager part.
        if is_eager_part(&part.value) && matches!(el, SignatureElement::Argument(_)) {
            leans.push(Lean::Eager);
            continue;
        }
        match bare_outcomes.get(i).and_then(|o| o.as_ref()) {
            Some(NameOutcome::Parked(p)) => leans.push(Lean::Parked(*p)),
            Some(NameOutcome::Unbound(name)) => leans.push(Lean::Dead(name.clone())),
            // Resolved / Cycle / ProducerErrored / keyword / literal mismatch:
            // a hard reject no arriving input or binding can flip.
            _ => return None,
        }
    }
    Some(leans)
}

/// Lazy-candidate gate: a `KType::KExpression` slot bound by an
/// `ExpressionPart::Expression` relaxes other non-`KExpression` slots to admit
/// `Expression` / `SigiledTypeExpr` parts speculatively (they route through
/// `eager_indices` post-pick). Required by FN / FUNCTOR overloads.
fn has_lazy_kexpr_slot(sig: &ExpressionSignature<'_>, expr: &KExpression<'_>) -> bool {
    sig.elements
        .iter()
        .zip(&expr.parts)
        .any(|(el, part)| match (el, &part.value) {
            (SignatureElement::Argument(arg), ExpressionPart::Expression(_)) => {
                matches!(arg.ktype, KType::KExpression)
            }
            _ => false,
        })
}

/// Per-slot strict admission — the element walk body of
/// [`signature_admits_strict`] and the per-slot gate the relaxed pass leans on
/// when it rejects.
fn slot_admits_strict<'e>(
    el: &SignatureElement<'_>,
    part_value: &ExpressionPart<'e>,
    i: usize,
    has_lazy_kexpr_slot: bool,
    bare_outcomes: &[Option<NameOutcome<'e>>],
) -> bool {
    match (el, part_value) {
        (SignatureElement::Keyword(s), ExpressionPart::Keyword(t)) => s == t,
        (SignatureElement::Keyword(_), _) => false,
        (SignatureElement::Argument(arg), part_value) => {
            // Binder declaration slot: the slot owns the name, so admission
            // is shape-only. SigiledTypeExpr / RecordType still admit speculatively
            // (they sub-dispatch to a type-side carrier — e.g. the FN record-schema
            // overload's `ProperType` signature slot taking a `:{…}`).
            if matches!(
                arg.ktype,
                KType::Identifier | KType::OfKind(KKind::ProperType)
            ) {
                if matches!(
                    part_value,
                    ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_)
                ) {
                    return true;
                }
                return arg.matches(part_value);
            }
            // A sigil / record-type part in a slot that is neither `:KExpression` nor the
            // *other* lazy raw-capture kind sub-dispatches to a type-side carrier. The two
            // lazy raw-capture slots (`:SigiledTypeExpr`, `:RecordType`) are part-kind-exact
            // and mutually exclusive — a `:{…}` must not be admitted to a `:SigiledTypeExpr`
            // slot, nor a `:(…)` to a `:RecordType` slot — else the two overloads tie
            // incomparably and the eager fallback wins (dropping the lazy raw capture).
            match part_value {
                ExpressionPart::SigiledTypeExpr(_)
                    if !matches!(arg.ktype, KType::KExpression | KType::RecordType) =>
                {
                    return true;
                }
                ExpressionPart::RecordType(_)
                    if !matches!(arg.ktype, KType::KExpression | KType::SigiledTypeExpr) =>
                {
                    return true;
                }
                _ => {}
            }
            // Lazy-candidate relaxation (see `has_lazy_kexpr_slot`). The `:SigiledTypeExpr`
            // and `:RecordType` slots are part-kind-strict like `:KExpression` — each admits
            // only its own part shape, so the return-type overloads stay disjoint.
            if has_lazy_kexpr_slot
                && matches!(part_value, ExpressionPart::Expression(_))
                && !matches!(
                    arg.ktype,
                    KType::KExpression | KType::SigiledTypeExpr | KType::RecordType
                )
            {
                return true;
            }
            match bare_outcomes.get(i).and_then(|o| o.as_ref()) {
                Some(NameOutcome::Resolved(c)) => {
                    arg.ktype.accepts_part(&ExpressionPart::Spliced(*c))
                }
                // Speculative admit so the splice/park walk can surface the
                // precise per-slot diagnostic.
                Some(NameOutcome::Parked(_)) | Some(NameOutcome::Unbound(_)) => {
                    arg.matches(part_value)
                }
                Some(NameOutcome::Cycle(_)) | Some(NameOutcome::ProducerErrored(_)) => false,
                None => arg.matches(part_value),
            }
        }
    }
}

/// True iff this part shape is one the scheduler's eager loop would schedule as a
/// sub-Dispatch.
fn is_eager_part(part: &ExpressionPart<'_>) -> bool {
    matches!(
        part,
        ExpressionPart::Expression(_)
            | ExpressionPart::SigiledTypeExpr(_)
            | ExpressionPart::RecordType(_)
            | ExpressionPart::ListLiteral(_)
            | ExpressionPart::DictLiteral(_)
            | ExpressionPart::RecordLiteral(_)
    )
}

/// True iff `expr` carries any part shape the scheduler's eager loop would
/// schedule as a sub-Dispatch.
fn expr_has_eager_part(expr: &KExpression<'_>) -> bool {
    expr.parts.iter().any(|p| is_eager_part(&p.value))
}

/// Sole producer of the embedded `slots`; disjointness lives in
/// [`KFunction::classify_for_pick`].
fn build_resolved<'step, 'e>(
    picked: &'step KFunction<'step>,
    expr: &KExpression<'e>,
) -> Resolved<'step> {
    Resolved {
        function: picked,
        placeholder: picked
            .binder_name
            .and_then(|(extractor, kind)| extractor(expr).map(|name| (name, kind))),
        pending_overload_bucket: picked.binder_bucket.and_then(|extractor| extractor(expr)),
        slots: picked.classify_for_pick(expr),
    }
}
