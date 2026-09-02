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
use crate::machine::core::{FunctionLookup, LexicalFrame, Scope};
use crate::machine::core::{OpenedFunction, WrapIndices};
use crate::machine::model::KeyElement;
use crate::machine::model::labels::BinderSymbol;
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::machine::model::{ExpressionSignature, KType, SignatureElement};
use crate::witnessed::{BumpAllocator, BumpVec};

use super::resolve::Resolution;
use crate::machine::model::RunRegistries;

#[cfg(test)]
mod tests;

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

/// Picked function plus the auto-wrap classification the dispatch driver splices before the
/// invoke.
///
/// `function` is the pick **in use**: adopted into its own binding region, so the mint that
/// re-anchored it named its reach there and the region retains the pins — which is what lets the
/// callable ride the `'step` lifetime across argument evaluation and into the invoke. The escape
/// into the call chain [`reseal`](crate::witnessed::Opened::reseal)s it back to rest.
pub struct Resolved<'step> {
    pub function: OpenedFunction<'step>,
    pub wrap_indices: WrapIndices<'step>,
}

pub enum DispatchOutcome<'step> {
    Resolved(Resolved<'step>),
    Ambiguous(usize),
    /// Waits on existing producers (forward-reference placeholders, or a claim on `key`) without
    /// scheduling new work.
    ParkOnProducers(BumpVec<'step, ProducerId>),
    /// A bare-name arg resolves to nothing — no binding and no placeholder. The unbound name is the
    /// precise cause, so it surfaces here rather than as a dispatch miss. It travels as the symbol
    /// the lookup held; the spelling is read back where the error is built. The second field is
    /// the rejecting slot's [`Argument::role`](crate::machine::model::Argument), when it declared
    /// one, so the raise can name the form and role the name failed at.
    UnboundName(BinderSymbol, Option<&'static str>),
    /// No candidate admits the expression. `quote_hint` is set when a forgotten `#(…)` explains
    /// the miss — see [`quote_would_help`].
    Unmatched {
        quote_hint: bool,
    },
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
        let mut dead_lean: Option<(BinderSymbol, Option<&'static str>)> = None;
        for scope in self.ancestors() {
            let lookup =
                scope
                    .bindings()
                    .lookup_function_stored(key, scope.binding_cutoff(chain), scratch);
            match decide_scope(scope, &lookup, expr, bare_outcomes, registries, scratch) {
                ScopeDecision::Terminal(outcome) => return outcome,
                ScopeDecision::DeadLean(name, role) => {
                    if dead_lean.is_none() {
                        dead_lean = Some((name, role));
                    }
                }
                ScopeDecision::Continue => {}
            }
        }
        match dead_lean {
            Some((name, role)) => DispatchOutcome::UnboundName(name, role),
            None => DispatchOutcome::Unmatched {
                quote_hint: quote_would_help(
                    self,
                    key,
                    chain,
                    expr,
                    bare_outcomes,
                    registries,
                    scratch,
                ),
            },
        }
    }
}

/// Whether a forgotten `#(…)` explains this dispatch miss: some overload in the bucket types a slot
/// `:KExpression`, and the expression carries an evaluated value there that is not code. The
/// argument ran, its side effects happened, and only then did nothing match — a failure mode
/// pointed enough to name its fix.
///
/// No provenance is tracked on the evaluated part: the trigger cannot tell a bare group from a bare
/// name that was bound to one, and the hint reads correctly for both. Walked only on the miss path,
/// so a successful dispatch pays nothing for it.
fn quote_would_help<'step>(
    scope: &Scope<'step>,
    key: &[KeyElement],
    chain: Option<&LexicalFrame>,
    expr: &WorkingExpression<'_>,
    bare_outcomes: &[Option<Resolution>],
    registries: &RunRegistries,
    scratch: BumpAllocator<'step>,
) -> bool {
    scope.ancestors().any(|scope| {
        let lookup =
            scope
                .bindings()
                .lookup_function_stored(key, scope.binding_cutoff(chain), scratch);
        lookup.overloads.iter().any(|sealed| {
            sealed
                .open_at()
                .value()
                .signature
                .elements()
                .iter()
                .zip(expr.parts)
                .enumerate()
                .any(|(i, (el, part))| {
                    matches!(el, SignatureElement::Argument(arg) if matches!(arg.ktype, KType::KEXPRESSION))
                        && holds_evaluated_non_code(&part.value, i, bare_outcomes, registries)
                })
        })
    })
}

/// Whether this slot holds a landed value that is not a `KExpression` — the staged group's result,
/// or the binding a bare name resolved to.
fn holds_evaluated_non_code(
    part: &WorkingPart<'_>,
    index: usize,
    bare_outcomes: &[Option<Resolution>],
    registries: &RunRegistries,
) -> bool {
    if let WorkingPart::Spliced { cell, .. } = part {
        return !KType::KEXPRESSION.accepts_cell(cell, registries);
    }
    match bare_outcomes.get(index).and_then(|o| o.as_ref()) {
        Some(Resolution::Resolved(delivered)) => {
            !KType::KEXPRESSION.accepts_carried(delivered.open_at().value(), registries)
        }
        _ => false,
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
    // A `Parked` entry can only come from `bare_outcomes[i]`, one per part, so the deduped list
    // never exceeds the part count — see `decide_scope` on why the reserve must be exact.
    let mut producers: BumpVec<'s, ProducerId> =
        BumpVec::with_capacity_in(expr.parts.len(), scratch);
    for (i, (part, outcome)) in expr.parts.iter().zip(bare_outcomes).enumerate() {
        let Some(Resolution::Parked(p)) = outcome else {
            continue;
        };
        if expr.park_exempt_slot(i, &part.value) {
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
    DeadLean(BinderSymbol, Option<&'static str>),
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
            let function = scope.open_function(&lookup.overloads[index]);
            // One flag per part, exactly the parts run's length, so the classification indexes it
            // positionally alongside the signature's elements.
            let mut parked: BumpVec<'_, bool> =
                BumpVec::with_capacity_in(expr.parts.len(), scratch);
            for i in 0..expr.parts.len() {
                parked.push(matches!(
                    bare_outcomes.get(i).and_then(|o| o.as_ref()),
                    Some(Resolution::Parked(_))
                ));
            }
            let wrap_indices =
                function
                    .value()
                    .classify_for_pick(expr, &parked, &registries.types, scratch);
            ScopeDecision::Terminal(DispatchOutcome::Resolved(Resolved {
                function,
                wrap_indices,
            }))
        }
        PickPass::Tie(n) => ScopeDecision::Terminal(DispatchOutcome::Ambiguous(n)),
        PickPass::Empty => decide_relaxed(&bucket, expr, bare_outcomes, registries, scratch),
    }
}

/// Strict-Empty relaxed pass: one assume-every-unresolved-slot-satisfiable pass per candidate.
///
/// A dead unbound lean never parks, since an unbound name never arrives, so it only records a
/// `DeadLean` blocker.
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
    let mut dead_name: Option<(BinderSymbol, Option<&'static str>)> = None;
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
                Lean::Dead(name, role) => {
                    if dead_name.is_none() {
                        dead_name = Some((name, role));
                    }
                }
            }
        }
    }
    if !parked.is_empty() {
        return ScopeDecision::Terminal(DispatchOutcome::ParkOnProducers(parked));
    }
    match dead_name {
        Some((name, role)) => ScopeDecision::DeadLean(name, role),
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

/// Policy-free outcome of one filter→`most_specific` pass: the `Tie` → `Ambiguous` translation is
/// decided outside. `Picked` names the winner's bucket index rather than the callable, so the pick
/// is re-anchored exactly once.
enum PickPass {
    Picked(usize),
    Tie(usize),
    Empty,
}

/// Which unresolved-slot kind the relaxed pass leaned on at a rejecting slot. `Dead` names an
/// unbound bare name: no producer will ever bind it, so it labels the `UnboundName` terminal and
/// never waits. It carries the rejecting slot's declared role along with the name, so the terminal
/// can render the form-and-role noun the slot registered.
#[derive(Clone, Copy)]
enum Lean {
    Parked(ProducerId),
    Dead(BinderSymbol, Option<&'static str>),
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
    sig.elements()
        .iter()
        .zip(expr.parts)
        .enumerate()
        .all(|(i, (el, part))| {
            slot_admits_strict(
                el,
                &part.value,
                i,
                expr.park_exempt_slot(i, &part.value),
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
    // At most one lean per slot; reserved after the length check so a rejected candidate takes
    // no scratch.
    let mut leans: BumpVec<'s, Lean> = BumpVec::with_capacity_in(sig.elements().len(), scratch);
    for (i, (el, part)) in sig.elements().iter().zip(expr.parts).enumerate() {
        if slot_admits_strict(
            el,
            &part.value,
            i,
            expr.park_exempt_slot(i, &part.value),
            bare_outcomes,
            registries,
        ) {
            continue;
        }
        match bare_outcomes.get(i).and_then(|o| o.as_ref()) {
            Some(Resolution::Parked(p)) => leans.push(Lean::Parked(*p)),
            Some(Resolution::Unbound(name)) => {
                let role = match el {
                    SignatureElement::Argument(arg) => arg.role,
                    SignatureElement::Keyword(_) => None,
                };
                leans.push(Lean::Dead(*name, role));
            }
            // Hard reject: no arriving input or binding can flip it.
            _ => return None,
        }
    }
    Some(leans)
}

/// Per-slot strict admission, shared by [`signature_admits_strict`] and [`relaxed_admits`] so the
/// two passes cannot drift on what "strict rejects here" means.
fn slot_admits_strict<'e>(
    el: &SignatureElement,
    slot: &WorkingPart<'e>,
    i: usize,
    park_exempt: bool,
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
            WorkingPart::Spliced { cell, .. } => arg.ktype.accepts_cell(cell, registries),
            _ => false,
        },
        (SignatureElement::Argument(arg), Some(part_value)) => {
            let part_value = &part_value;
            // A union carrier slot routes by its matching exact carrier member: the part is
            // captured raw at bind, so admission is shape-only here exactly as it is for the
            // bare carrier constants below. The verdict still comes from `matches`, which
            // distributes over the union — the member lookup decides only the routing.
            if arg.ktype.raw_capture_member(part_value, types).is_some() {
                return arg.matches(part_value, types);
            }
            // `ProperType` admits a `:(…)` / `:{…}` part on shape alone — whether that part reaches
            // the body raw or as a resolved type-side carrier is the node's lazy-slot stamp's call,
            // not this predicate's — and the same holds for a `ProperType` member inside a union.
            // A bare `Type` token gets no such pass: the slot is a kind expectation asking for a
            // type *value*, so the token auto-wraps and admission reads its resolution below,
            // exactly as at any other eager slot.
            if arg.ktype.union_has_member(KType::PROPER_TYPE, types) {
                if matches!(
                    part_value,
                    ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_)
                ) {
                    return true;
                }
                // A binder form's own `Type`-token operand naming a *still-finalizing* type. The
                // pre-admission park skipped it because the binder body owns the resolve-or-park
                // protocol through the declaration window, so admission is on shape and the token
                // rides raw — the one bare `Type` token a kind expectation does not resolve.
                // Parking it instead would deadlock the declaration group the two names share.
                //
                // Only `Parked` takes this door. A resolved name wraps and rides the lane like any
                // other; an unbound one rejects, so the relaxed pass's dead lean raises against the
                // slot's registered role. And only a *kind* slot: `LET Alias = Cell` reads its
                // sibling through an `:Any` slot, which parks below and waits for the seal.
                if park_exempt
                    && matches!(part_value, ExpressionPart::Type(_))
                    && matches!(
                        bare_outcomes.get(i).and_then(|o| o.as_ref()),
                        Some(Resolution::Parked(_))
                    )
                {
                    return true;
                }
            }
            // A declaration slot owns the name, so admission is shape-only. `Identifier` stays
            // part-kind-exact, so a `:{…}` return type is never mistaken for a value-named one.
            if matches!(
                arg.ktype,
                KType::IDENTIFIER | KType::NAME_TOKEN | KType::TYPE_NAME_TOKEN
            ) {
                return arg.matches(part_value, types);
            }
            // The two lazy raw-capture slots are part-kind-exact and mutually exclusive: admitting
            // a `:{…}` to a `:SigiledTypeExpr` slot (or a `:(…)` to a `:RecordType` one) would tie
            // the two overloads incomparably, and the eager fallback would win — dropping the lazy
            // raw capture. Anywhere else, such a part sub-dispatches to a type-side carrier. The
            // exclusion distributes over union members: a union capturing one type-expression
            // kind raw must not speculatively eat the other, or a sibling overload's raw capture
            // ties away just the same.
            let excludes = |needle| arg.ktype.union_has_member(needle, types);
            match part_value {
                ExpressionPart::SigiledTypeExpr(_)
                    if !(excludes(KType::KEXPRESSION) || excludes(KType::RECORD_TYPE)) =>
                {
                    return true;
                }
                ExpressionPart::RecordType(_)
                    if !(excludes(KType::KEXPRESSION) || excludes(KType::SIGILED_TYPE_EXPR)) =>
                {
                    return true;
                }
                _ => {}
            }
            match bare_outcomes.get(i).and_then(|o| o.as_ref()) {
                Some(Resolution::Resolved(delivered)) => arg
                    .ktype
                    .accepts_carried(delivered.open_at().value(), registries),
                // The relaxed pass's `Dead` lean carries the precise `UnboundName`. `Parked`
                // reaching here means the slot did not shape-admit the token above, so the relaxed
                // pass parks on it — the wait a consumer of an unsealed sibling needs.
                Some(Resolution::Parked(_)) | Some(Resolution::Unbound(_)) => false,
                None => arg.matches(part_value, types),
            }
        }
    }
}
