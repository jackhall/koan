//! Overload resolution for a [`WorkingExpression`] against the lexical scope chain.
//!
//! Read-only consumer of the dispatch table. The caller builds a `bare_outcomes` cache (one
//! [`Resolution`] per bare-name part) consulted by admission instead of re-resolving each part per
//! scope. A `Parked` entry pre-empts everything: resolution parks on the still-finalizing producers
//! *before* any admission runs, so every candidate decides against the same landed facts and no
//! pick commits ahead of a value it depends on. Exempt from that pre-admission park are the slots a
//! binder form's own machinery owns — the declared-name position
//! ([`WorkingExpression::binder_name_slot`]) and a binder form's `Type`-token operands, which the
//! binder body resolves through the declaration-window / type-resolution protocol.
//!
//! Scopes are then decided in walk order, innermost first. Only a *dead* unbound bare-name lean and
//! total non-admission are post-walk terminals — a dead lean must not pre-empt an outer scope that
//! could strict-pick the bare name as an `:Identifier` slot.

use crate::machine::ProducerId;
use crate::machine::core::{ClassifiedSlots, OpenedFunction};
use crate::machine::core::{FunctionLookup, LexicalFrame, Scope};
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::machine::model::{ExpressionSignature, KType, SignatureElement};
use crate::witnessed::{BumpAllocator, BumpVec};

use super::is_eager_working_part;

use super::resolve::Resolution;
use crate::machine::model::RunRegistries;

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

/// Picked function plus the per-slot classification the dispatch driver needs for auto-wrap and
/// eager-sub scheduling.
///
/// `function` is the pick **in use**: adopted into its own binding region, so the mint that
/// re-anchored it named its reach there and the region retains the pins — which is what lets the
/// callable ride the `'step` lifetime across argument evaluation and into the invoke. The escape
/// into the call chain [`reseal`](crate::witnessed::Opened::reseal)s it back to rest.
pub struct Resolved<'step> {
    pub function: OpenedFunction<'step>,
    pub slots: ClassifiedSlots<'step>,
}

pub enum DispatchOutcome<'step> {
    Resolved(Resolved<'step>),
    Ambiguous(usize),
    Deferred,
    /// Distinct from `Deferred`: waits on existing producers (forward-reference placeholders, or a
    /// claim on `key`) without scheduling new work.
    ParkOnProducers(BumpVec<'step, ProducerId>),
    /// A bare-name arg resolves to nothing — no binding and no placeholder. The unbound name is the
    /// precise cause, so it surfaces here rather than as a dispatch miss.
    UnboundName(String),
    Unmatched,
}

impl<'step> Scope<'step> {
    /// Chain-gated, cache-driven dispatch resolution.
    ///
    /// Each candidate is filtered against the visibility predicate before admission — per-overload
    /// tagging matters because overloads in a bucket may sit at different lexical positions.
    /// `chain = None` (no active payload names a frame) leaves every scope complete to the reader,
    /// and an empty `bare_outcomes` reverts admission to shape-only `arg.matches`.
    ///
    /// `scratch` hosts every buffer the walk builds — the per-scope overload copy-out, the opened
    /// candidates, the two pick buffers, and the outcome's own lists. It is the step's own arena
    /// (`ctx.scratch()`), because the picked candidate's classification buckets and a park's
    /// producer list ride out of the walk inside [`DispatchOutcome`]. The walk's cost stays
    /// independent of how many scopes it visits: the bytes are the drain's, reclaimed wholesale at
    /// the next pop.
    pub(crate) fn resolve_dispatch<'e>(
        &self,
        expr: &WorkingExpression<'e>,
        chain: Option<&LexicalFrame>,
        bare_outcomes: &[Option<Resolution>],
        registries: &RunRegistries,
        scratch: BumpAllocator<'step>,
    ) -> DispatchOutcome<'step> {
        #[cfg(test)]
        RESOLVE_DISPATCH_ENTRIES.with(|c| c.set(c.get() + 1));
        // Which overload wins can depend on the carried type a still-finalizing producer has yet to
        // land, so committing a pick before it does would make dispatch a function of drain order.
        let parked = parked_producers(expr, bare_outcomes, scratch);
        if !parked.is_empty() {
            return DispatchOutcome::ParkOnProducers(parked);
        }
        // Read where it rests: dispatch is the hottest read in the machine, and materializing an
        // owned key per call would clone every keyword's text.
        let key = expr.stored_key();
        // Builtin dispatch buckets are unshadowable, so the root bucket is authoritative. A
        // non-terminal root falls through to the full walk, preserving precedence.
        let root = self.root_scope();
        if root.bindings().has_builtin_function_stored(key) {
            let lookup =
                root.bindings()
                    .lookup_function_stored(key, root.binding_cutoff(chain), scratch);
            if let ScopeDecision::Terminal(outcome) =
                decide_scope(root, &lookup, expr, bare_outcomes, registries, scratch)
            {
                return outcome;
            }
        }
        let mut dead_lean: Option<String> = None;
        for scope in self.ancestors() {
            let lookup =
                scope
                    .bindings()
                    .lookup_function_stored(key, scope.binding_cutoff(chain), scratch);
            match decide_scope(scope, &lookup, expr, bare_outcomes, registries, scratch) {
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

/// Deduped producers behind every non-exempt `Parked` bare-name part — the pre-admission park
/// scan. Exempt are the slots a binder form's own machinery resolves:
///
/// - the declared-name position (`binder_name_slot`, cached off
///   [`BinderSpec::name_slot`](crate::machine::model::binder::BinderSpec::name_slot)): the slot
///   *owns* the name, so an inner shadowing binder must not wait on a same-named outer binder still
///   in flight (its own claim is already invisible to it by the exclusive visibility cutoff);
/// - a binder form's `Type`-token operands (`NEWTYPE Meters = Length`, `LET Thing = OtherThing`, a
///   combined form's return type): the binder body resolves these through the declaration-window /
///   type-resolution protocol, which answers a declarator's reference to a co-declared sibling
///   without waiting — parking here instead could deadlock a recursive declaration group.
///
/// A non-binder expression has no exemptions: its `Type`-token operands wait here, on exactly the
/// producers the type-resolution walk would park on downstream.
fn parked_producers<'s>(
    expr: &WorkingExpression<'_>,
    bare_outcomes: &[Option<Resolution>],
    scratch: BumpAllocator<'s>,
) -> BumpVec<'s, ProducerId> {
    let name_slot = expr.binder_name_slot();
    // A `Parked` entry can only come from `bare_outcomes[i]`, one per part, so the deduped list
    // never exceeds the part count — see `decide_scope` on why the reserve must be exact.
    let mut producers: BumpVec<'s, ProducerId> =
        BumpVec::with_capacity_in(expr.parts.len(), scratch);
    for (i, (part, outcome)) in expr.parts.iter().zip(bare_outcomes).enumerate() {
        let Some(Resolution::Parked(p)) = outcome else {
            continue;
        };
        if let Some(pos) = name_slot
            && (i == pos || matches!(part.value.as_ast(), Some(ExpressionPart::Type(_))))
        {
            continue;
        }
        if !producers.contains(p) {
            producers.push(*p);
        }
    }
    producers
}

/// Per-scope precedence: the innermost scope with a `Terminal` decision wins. `DeadLean` records an
/// unbound bare-name blocker *without* terminating, since an outer scope may still strict-Pick the
/// bare name.
enum ScopeDecision<'step> {
    Terminal(DispatchOutcome<'step>),
    DeadLean(String),
    Continue,
}

/// Decide one scope's contribution from its [`FunctionLookup`].
///
/// The candidate walk reads signatures only, so it opens each dormant overload at `scope`'s own
/// region-owner borrow — a candidate that loses the tournament is never re-anchored past this call.
/// Only the **pick** escapes, through [`Scope::open_function`], which adopts it at `scope`'s region
/// lifetime; `scope` is the very scope the bucket was read from, whose arena hosts the descriptions.
fn decide_scope<'step, 'e>(
    scope: &Scope<'step>,
    lookup: &FunctionLookup<'_, BumpAllocator<'_>>,
    expr: &WorkingExpression<'e>,
    bare_outcomes: &[Option<Resolution>],
    registries: &RunRegistries,
    scratch: BumpAllocator<'step>,
) -> ScopeDecision<'step> {
    // Exact capacity, filled by a push loop: a grown bump buffer abandons its old bytes as dead
    // scratch, so every scratch-hosted buffer here is built at the length it will reach.
    let mut candidates: BumpVec<'_, OpenedFunction<'_>> =
        BumpVec::with_capacity_in(lookup.overloads.len(), scratch);
    for sealed in lookup.overloads.iter() {
        candidates.push(sealed.open_at());
    }
    let bucket = OverloadBucket {
        candidates: &candidates,
    };
    // Pending parks at its scope even over a finalized Pick: the pending sibling would shadow once
    // it finalizes, so resolve nothing until it does (Decision 5 in
    // ../../../../design/typing/scheduler.md). The relaxed pass's parked producers union in so a
    // single wake re-runs the full resolution.
    if let Some(pending) = lookup.pending {
        // The pending overload slot plus at most one distinct producer per part.
        let mut producers: BumpVec<'step, ProducerId> =
            BumpVec::with_capacity_in(expr.parts.len() + 1, scratch);
        producers.push(pending);
        for p in bucket.relaxed_parked_producers(expr, bare_outcomes, registries, scratch) {
            if !producers.contains(&p) {
                producers.push(p);
            }
        }
        return ScopeDecision::Terminal(DispatchOutcome::ParkOnProducers(producers));
    }
    match bucket.pick_strict(expr, bare_outcomes, registries, scratch) {
        PickPass::Picked(index) => {
            ScopeDecision::Terminal(DispatchOutcome::Resolved(build_resolved(
                scope.open_function(&lookup.overloads[index]),
                expr,
                registries,
                scratch,
            )))
        }
        // A tie may break once an unevaluated eager part lands: the typed `Spliced(List …)`
        // re-dispatch is element-aware where the bare literal is shape-only. A genuine tie
        // resurfaces as `Ambiguous` on the post-eager-subs pass.
        PickPass::Tie(n) if expr_has_eager_part(expr) => {
            let _ = n;
            ScopeDecision::Terminal(DispatchOutcome::Deferred)
        }
        PickPass::Tie(n) => ScopeDecision::Terminal(DispatchOutcome::Ambiguous(n)),
        PickPass::Empty => decide_relaxed(&bucket, expr, bare_outcomes, registries, scratch),
    }
}

/// Strict-Empty relaxed pass: one assume-every-unresolved-slot-satisfiable pass per candidate.
///
/// Parked beats eager; a dead unbound lean never parks, since an unbound name never arrives, so it
/// only records a `DeadLean` blocker.
fn decide_relaxed<'step, 'e>(
    bucket: &OverloadBucket<'_, '_>,
    expr: &WorkingExpression<'e>,
    bare_outcomes: &[Option<Resolution>],
    registries: &RunRegistries,
    scratch: BumpAllocator<'step>,
) -> ScopeDecision<'step> {
    // Every `Lean::Parked` names a producer read out of `bare_outcomes`, one slot each, so the
    // deduped accumulator is bounded by the part count no matter how many candidates lean.
    let mut parked: BumpVec<'step, ProducerId> =
        BumpVec::with_capacity_in(expr.parts.len(), scratch);
    let mut any_eager_lean = false;
    let mut dead_name: Option<String> = None;
    for f in bucket.candidates.iter() {
        let Some(leans) = relaxed_admits(
            &f.value().signature,
            expr,
            bare_outcomes,
            registries,
            scratch,
        ) else {
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

/// View over a single scope's visibility-pre-filtered overload bucket, opened at the scope's own
/// pin. Encapsulates the filter-then-[`ExpressionSignature::most_specific`] dance.
struct OverloadBucket<'p, 'b> {
    candidates: &'b [OpenedFunction<'p>],
}

impl OverloadBucket<'_, '_> {
    /// The winner's index into the bucket, so the caller re-anchors only the pick — these opens
    /// borrow the walk's own pin and none of them may leave it.
    fn pick_strict<'e>(
        &self,
        expr: &WorkingExpression<'e>,
        bare_outcomes: &[Option<Resolution>],
        registries: &RunRegistries,
        scratch: BumpAllocator<'_>,
    ) -> PickPass {
        // `candidates.len()` bounds the survivors and the survivor count is exactly the signature
        // count, so neither buffer reallocates — see `decide_scope` on why that matters in a bump.
        let mut survivors: BumpVec<'_, usize> =
            BumpVec::with_capacity_in(self.candidates.len(), scratch);
        for (i, f) in self.candidates.iter().enumerate() {
            if signature_admits_strict(&f.value().signature, expr, bare_outcomes, registries) {
                survivors.push(i);
            }
        }
        let mut sigs: BumpVec<'_, &ExpressionSignature> =
            BumpVec::with_capacity_in(survivors.len(), scratch);
        for i in survivors.iter() {
            sigs.push(&self.candidates[*i].value().signature);
        }
        // `most_specific` keeps its contiguous-slice signature: a `BumpVec` derefs to `[T]`.
        match ExpressionSignature::most_specific(&sigs, registries) {
            Some(i) => PickPass::Picked(survivors[i]),
            None if !survivors.is_empty() => PickPass::Tie(survivors.len()),
            None => PickPass::Empty,
        }
    }

    /// Deduped so a single wake re-runs the whole resolution.
    fn relaxed_parked_producers<'e, 's>(
        &self,
        expr: &WorkingExpression<'e>,
        bare_outcomes: &[Option<Resolution>],
        registries: &RunRegistries,
        scratch: BumpAllocator<'s>,
    ) -> BumpVec<'s, ProducerId> {
        // Bounded by the part count for the same reason as `decide_relaxed`'s accumulator.
        let mut producers: BumpVec<'s, ProducerId> =
            BumpVec::with_capacity_in(expr.parts.len(), scratch);
        for f in self.candidates.iter() {
            let Some(leans) = relaxed_admits(
                &f.value().signature,
                expr,
                bare_outcomes,
                registries,
                scratch,
            ) else {
                continue;
            };
            for lean in leans {
                if let Lean::Parked(p) = lean
                    && !producers.contains(&p)
                {
                    producers.push(p);
                }
            }
        }
        producers
    }
}

/// Policy-free outcome of one filter→`most_specific` pass: the `Tie` → `Ambiguous` / `Deferred`
/// translation is decided outside. `Picked` names the winner's bucket index rather than the
/// callable, so the pick is re-anchored exactly once.
enum PickPass {
    Picked(usize),
    Tie(usize),
    Empty,
}

/// Which unresolved-slot kind the relaxed pass leaned on at a rejecting slot. `Dead` names an
/// unbound bare name: no producer will ever bind it, so it labels the `UnboundName` terminal and
/// never waits.
enum Lean {
    Parked(ProducerId),
    Eager,
    Dead(String),
}

/// Strict admission against the `bare_outcomes` cache. Rule table at
/// [design/typing/elaboration.md § Strict admission rules](../../../../design/typing/elaboration.md#strict-admission-rules).
fn signature_admits_strict<'e>(
    sig: &ExpressionSignature<'_>,
    expr: &WorkingExpression<'e>,
    bare_outcomes: &[Option<Resolution>],
    registries: &RunRegistries,
) -> bool {
    if sig.elements().len() != expr.parts.len() {
        return false;
    }
    let has_lazy_kexpr_slot = has_lazy_kexpr_slot(sig, expr);
    sig.elements()
        .iter()
        .zip(expr.parts)
        .enumerate()
        .all(|(i, (el, part))| {
            slot_admits_strict(
                el,
                &part.value,
                i,
                has_lazy_kexpr_slot,
                bare_outcomes,
                registries,
            )
        })
}

/// Relaxed admission: assume every *unresolved* slot satisfiable and report which kinds were leaned
/// on. `None` ⇒ the candidate rejects even relaxed, on a hard already-resolved / literal / keyword
/// slot no arriving input or binding can flip.
///
/// "Leaned on" = strict rejects at the slot but the assume-satisfiable relaxation passes it. A
/// `Parked` lean can only arise on a pre-admission-park-exempt slot, the park having already
/// pre-empted every other `Parked` part.
fn relaxed_admits<'e, 's>(
    sig: &ExpressionSignature<'_>,
    expr: &WorkingExpression<'e>,
    bare_outcomes: &[Option<Resolution>],
    registries: &RunRegistries,
    scratch: BumpAllocator<'s>,
) -> Option<BumpVec<'s, Lean>> {
    if sig.elements().len() != expr.parts.len() {
        return None;
    }
    let has_lazy_kexpr_slot = has_lazy_kexpr_slot(sig, expr);
    // At most one lean per slot; reserved after the length check so a rejected candidate takes
    // no scratch.
    let mut leans: BumpVec<'s, Lean> = BumpVec::with_capacity_in(sig.elements().len(), scratch);
    for (i, (el, part)) in sig.elements().iter().zip(expr.parts).enumerate() {
        if slot_admits_strict(
            el,
            &part.value,
            i,
            has_lazy_kexpr_slot,
            bare_outcomes,
            registries,
        ) {
            continue;
        }
        // An unevaluated eager part in an *argument* slot routes through `eager_indices` post-pick;
        // a keyword element can never be satisfied by an eager part.
        if is_eager_working_part(&part.value) && matches!(el, SignatureElement::Argument(_)) {
            leans.push(Lean::Eager);
            continue;
        }
        match bare_outcomes.get(i).and_then(|o| o.as_ref()) {
            Some(Resolution::Parked(p)) => leans.push(Lean::Parked(*p)),
            Some(Resolution::Unbound(name)) => leans.push(Lean::Dead(name.clone())),
            // Hard reject: no arriving input or binding can flip it.
            _ => return None,
        }
    }
    Some(leans)
}

/// Lazy-candidate gate: a `KType::KEXPRESSION` slot bound by an `ExpressionPart::Expression`
/// relaxes the other slots to admit parts speculatively (they route through `eager_indices`
/// post-pick), which is what lets the `FN` overloads capture an unevaluated body.
fn has_lazy_kexpr_slot(sig: &ExpressionSignature<'_>, expr: &WorkingExpression<'_>) -> bool {
    sig.elements()
        .iter()
        .zip(expr.parts)
        .any(|(el, part)| match (el, part.value.as_ast()) {
            (SignatureElement::Argument(arg), Some(ExpressionPart::Expression(_))) => {
                matches!(arg.ktype, KType::KEXPRESSION)
            }
            _ => false,
        })
}

/// Per-slot strict admission, shared by [`signature_admits_strict`] and [`relaxed_admits`] so the
/// two passes cannot drift on what "strict rejects here" means.
fn slot_admits_strict<'e>(
    el: &SignatureElement,
    slot: &WorkingPart<'e>,
    i: usize,
    has_lazy_kexpr_slot: bool,
    bare_outcomes: &[Option<Resolution>],
    registries: &RunRegistries,
) -> bool {
    let types = &registries.types;
    match (el, slot.as_ast()) {
        (SignatureElement::Keyword(s), Some(ExpressionPart::Keyword(t))) => {
            s.symbol() == t.symbol()
        }
        (SignatureElement::Keyword(_), _) => false,
        // A slot the scheduler filled classifies by its carried value, never by a part shape: a
        // resolved cell opens at its own brand, and a synthesized node / staging hole names no value
        // an argument slot can admit.
        (SignatureElement::Argument(arg), None) => match slot {
            WorkingPart::Spliced { cell } => arg.ktype.accepts_cell(cell, registries),
            _ => false,
        },
        (SignatureElement::Argument(arg), Some(part_value)) => {
            let part_value = &part_value;
            // A declaration slot owns the name, so admission is shape-only. `ProperType` still
            // admits SigiledTypeExpr / RecordType speculatively, since they sub-dispatch to a
            // type-side carrier; `Identifier` stays part-kind-exact, so a `:{…}` return type is
            // never mistaken for a value-named one.
            if matches!(arg.ktype, KType::PROPER_TYPE) {
                if matches!(
                    part_value,
                    ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_)
                ) {
                    return true;
                }
                return arg.matches(part_value, types);
            }
            if matches!(arg.ktype, KType::IDENTIFIER) {
                return arg.matches(part_value, types);
            }
            // The two lazy raw-capture slots are part-kind-exact and mutually exclusive: admitting
            // a `:{…}` to a `:SigiledTypeExpr` slot (or a `:(…)` to a `:RecordType` one) would tie
            // the two overloads incomparably, and the eager fallback would win — dropping the lazy
            // raw capture. Anywhere else, such a part sub-dispatches to a type-side carrier.
            match part_value {
                ExpressionPart::SigiledTypeExpr(_)
                    if !matches!(arg.ktype, KType::KEXPRESSION | KType::RECORD_TYPE) =>
                {
                    return true;
                }
                ExpressionPart::RecordType(_)
                    if !matches!(arg.ktype, KType::KEXPRESSION | KType::SIGILED_TYPE_EXPR) =>
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
                    KType::KEXPRESSION | KType::SIGILED_TYPE_EXPR | KType::RECORD_TYPE
                )
            {
                return true;
            }
            match bare_outcomes.get(i).and_then(|o| o.as_ref()) {
                Some(Resolution::Resolved(delivered)) => arg
                    .ktype
                    .accepts_carried(delivered.open_at().value(), registries),
                // The relaxed pass's `Dead` lean carries the precise `UnboundName`. `Parked` reaches
                // here only on a slot exempt from the pre-admission park (a binder form's own
                // operand), where rejecting leaves the pick to the shape-only binder slots.
                Some(Resolution::Parked(_)) | Some(Resolution::Unbound(_)) => false,
                None => arg.matches(part_value, types),
            }
        }
    }
}

fn expr_has_eager_part(expr: &WorkingExpression<'_>) -> bool {
    expr.parts.iter().any(|p| is_eager_working_part(&p.value))
}

/// The one place `Resolved` is built; the disjoint `(eager_indices | wrap_indices)` invariant its
/// `slots` carries is established by
/// [`KFunction::classify_for_pick`](crate::machine::core::KFunction::classify_for_pick).
fn build_resolved<'step, 'e>(
    picked: OpenedFunction<'step>,
    expr: &WorkingExpression<'e>,
    registries: &RunRegistries,
    scratch: BumpAllocator<'step>,
) -> Resolved<'step> {
    let slots = picked.value().classify_for_pick(expr, registries, scratch);
    Resolved {
        function: picked,
        slots,
    }
}
