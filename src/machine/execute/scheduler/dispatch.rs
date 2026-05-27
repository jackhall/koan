use crate::builtins::value_lookup::coerce_type_token_value;
use crate::machine::core::source::Spanned;
use crate::machine::model::{KObject, Parseable};
use crate::machine::{
    BindingIndex, Frame, KError, KErrorKind, NodeId, ResolveOutcome, Resolution, Scope,
};
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeParams};

use super::super::nodes::{LiftState, NodeOutput, NodeStep, NodeWork};
use super::Scheduler;

/// Outcome of resolving a bare-name part (`Identifier` or leaf `Type`) against the
/// dispatching scope. Shared between Phase 3 (wrap-slot eager resolve) and Phase 4
/// (literal-name slot replay-park). Callers branch on the variant; `Resolved` is spliced
/// or ignored, `Parked` feeds the combined park-producer list, and the error variants
/// short-circuit with the standard `clone_for_propagation` shape.
pub(super) enum NameOutcome<'a> {
    /// Bare name resolved to a value-side binding. The caller decides whether to splice
    /// the carrier into the slot (wrap-slot) or leave the bare token alone (ref_name).
    Resolved(&'a KObject<'a>),
    /// Bare name resolves to a still-pending placeholder; caller pushes the producer onto
    /// the shared `producers_to_wait` list after `would_create_cycle` filtering.
    Parked(NodeId),
    /// The producer this name resolved to has already terminalized with `Err`. Surfaces
    /// for caller-side propagation with a frame attached (`<wrap-resolve>` /
    /// `<replay-park>` etc.).
    ProducerErrored(KError),
    /// Bare name has no binding anywhere on the scope chain.
    Unbound(String),
    /// Caller-side parking would close a wake cycle (trivial `LET Ty = Ty` etc.). The
    /// dispatch phase surfaces this as `SchedulerDeadlock` (the cycle-specific error
    /// kind) rather than deadlocking on the park edge at finalize.
    Cycle(String),
}

/// Resolve a bare-name `ExpressionPart` (Identifier or leaf Type) against `scope`. The
/// returned `NameOutcome` is a small ADT the dispatch driver branches on; non-bare-name
/// parts (`Expression`, literals, parens, `Future`, etc.) are never passed here — slot
/// classification (`ClassifiedSlots`) filters them out before this point.
///
/// Type-token coercion (`KTypeValue` synthesis or paired-carrier recovery) routes through
/// [`coerce_type_token_value`] so the dispatch-phase path produces the same carrier the
/// `value_lookup::body_type_expr` builtin would.
///
/// `consumer = Some(idx)` enables the cycle check (returning `NameOutcome::Cycle` when
/// the resolved placeholder is forward-reachable from `idx` along the wake graph).
/// `consumer = None` skips the check — used by `classify_aggregate_part` during
/// dict/list literal planning, where the Combine slot doesn't yet exist (and so
/// cannot be the producer in a forward-reachable walk).
pub(super) fn resolve_name_part<'a>(
    scope: &'a Scope<'a>,
    part: &ExpressionPart<'a>,
    scheduler: &Scheduler<'a>,
    consumer: Option<NodeId>,
) -> NameOutcome<'a> {
    let (name, is_type) = match part {
        ExpressionPart::Identifier(n) => (n.as_str(), None),
        ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => {
            (t.name.as_str(), Some(t))
        }
        // Classification already filters non-bare-name parts out before this helper is
        // called. Anything else here would be a classifier bug.
        _ => unreachable!("resolve_name_part only called on bare-name parts"),
    };
    // Placeholder check first: a forward-declared name shadows any outer-scope binding,
    // and parking on the producer must precede the resolved-value check. The
    // `scope.resolve` chain consults `bindings.data` then `bindings.placeholders` per
    // scope — for value-side bindings under the same name (e.g. `LET ty = …`) a `Value`
    // hit wins; for forward `STRUCT Foo = …` references the binder's `binder_name` installs
    // a `Placeholder` we park on.
    //
    // Chain-gated: visibility against the consumer's lexical chain filters out
    // later-sibling bindings (and placeholders) so the eager-resolve / replay-park
    // passes agree with the gated `resolve_dispatch` walk that ran in Phase 2.
    let chain = scheduler.active_chain.as_deref();
    match scope.resolve_with_chain(name, chain) {
        Resolution::Placeholder(producer) => {
            return if scheduler.is_result_ready(producer) {
                // Terminal-while-placeholder-set means the producer errored (success
                // would have cleared the placeholder via `bind_value`); propagate rather
                // than park on a dead slot.
                match scheduler.read_result(producer) {
                    Err(e) => NameOutcome::ProducerErrored(e.clone_for_propagation()),
                    // Defensive: a finalized-Ok producer with the placeholder still set
                    // would be a bindings invariant break. Treat as Unbound rather than
                    // panicking.
                    Ok(_) => NameOutcome::Unbound(name.to_string()),
                }
            } else if matches!(consumer, Some(c) if scheduler.deps.would_create_cycle(producer, c))
            {
                NameOutcome::Cycle(name.to_string())
            } else {
                NameOutcome::Parked(producer)
            };
        }
        Resolution::Value(obj) if is_type.is_none() => {
            // Identifier leaves bind directly to the scope value.
            return NameOutcome::Resolved(obj);
        }
        Resolution::Value(_) | Resolution::UnboundName => {
            // Fall through for Type parts (which need `coerce_type_token_value`'s
            // type-side resolution against `bindings.types`) and for the Identifier
            // Unbound case (caller produces `UnboundName`). Type parts that miss
            // `bindings.data` may still hit `bindings.types` — `Number` is a builtin
            // type with no value-side binding, for instance.
        }
    }
    match is_type {
        // Type-token leaves route through `coerce_type_token_value` so the dispatch
        // phase produces the same paired-carrier / `KTypeValue` synthesis shape that
        // the pre-removal sub-Dispatch through `value_lookup::body_type_expr` did. The
        // helper returns its own `UnboundName` on a `resolve_type` miss; that error is
        // returned to the caller as `Unbound` rather than `ProducerErrored` so the
        // wrap-slot phase surfaces the standard unbound surface.
        Some(t) => match coerce_type_token_value(scope, t, chain) {
            Ok(obj) => NameOutcome::Resolved(obj),
            Err(KError { kind: KErrorKind::UnboundName(n), .. }) => NameOutcome::Unbound(n),
            Err(e) => NameOutcome::ProducerErrored(e),
        },
        None => NameOutcome::Unbound(name.to_string()),
    }
}

/// Centralized propagation: clone a dep's terminal error and attach a caller-chosen
/// frame. `frame = None` is the `run_catch` frameless variant — passing a `None`-shaped
/// label keeps the propagation chain consistent without inventing an empty frame.
pub(super) fn propagate_dep_error(e: &KError, frame: Option<Frame>) -> KError {
    let cloned = e.clone_for_propagation();
    match frame {
        Some(f) => cloned.with_frame(f),
        None => cloned,
    }
}

impl<'a> Scheduler<'a> {
    /// Dispatch driver: a linear pipeline.
    ///
    /// 1. **`try_short_circuit`** — bare-name match in the current scope. A `Value` hit
    ///    terminates immediately; a `Placeholder` hit installs a park edge and rewrites the
    ///    slot to a `Lift`. `Unbound` and non-bare-name shapes fall through.
    /// 2. **`Scope::resolve_dispatch`** — one chain walk yielding a [`Resolved`],
    ///    `Ambiguous(n)`, `Deferred`, or `Unmatched`. `Ambiguous` and `Unmatched` surface
    ///    as structured errors. `Deferred` jumps to schedule-deps; `Resolved` continues.
    ///    A keyword-headed call to a not-yet-registered function with no eager parts
    ///    consults the `pending_overloads` table by the *full* inner-call bucket key
    ///    as a last-step fallback: a sibling FN / FUNCTOR binder still parked on its
    ///    own Combine will have installed a `binder_bucket` entry under that key.
    ///    The walk parks on that producer rather than failing, so the bare-arg shape
    ///    `(MAKESET IntOrd)` doesn't race the FIFO submission order. Keying by the
    ///    full bucket (not just the lead keyword) keeps overloads with shared head
    ///    keywords but different signatures from colliding. Value / type slot
    ///    bare-name forward-reference parks ride phases 1 and 3 of this driver via
    ///    `Scope::resolve` and the name-keyed `placeholders` table.
    ///
    ///    2.5: **Placeholder install** — if the picked function carried a `binder_name`
    ///    extractor, install its dispatch-time name placeholder against this slot's
    ///    `NodeId`. Conceptually between phase 2 and phase 3; numbered with a fractional
    ///    step so the surrounding list reads as the canonical four-phase pipeline.
    /// 3. **Eager name resolve** — walk `resolved.slots.wrap_indices` and
    ///    `ref_name_indices` through a shared [`resolve_name_part`] helper. Wrap-slot
    ///    `Resolved` results splice `Future(obj)` directly into the slot (replacing the
    ///    old `apply_auto_wrap` + sub-Dispatch detour); ref-name-slot `Resolved` is a
    ///    no-op. `Parked` outcomes feed a shared producers-to-wait list; one
    ///    `install_combined_park` call covers both phases.
    /// 4. **`schedule_deps`** — schedule the resolution's `eager_indices` plus any other
    ///    `Expression` / `ListLiteral` / `DictLiteral` parts as sub-nodes, building a
    ///    `Bind` slot. If no subs needed, bind the function directly and step to its
    ///    body.
    ///
    /// See [design/execution-model.md § Dispatch-time name placeholders](../../../../design/execution-model.md#dispatch-time-name-placeholders)
    /// for the bare-name short-circuit, placeholder install, and forward-name park rules
    /// referenced above.
    pub(super) fn run_dispatch(
        &mut self,
        expr: KExpression<'a>,
        pre_subs: Vec<(usize, NodeId)>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        // Phase 1.
        if let Some(step) = self.try_short_circuit(&expr, scope, idx) {
            return Ok(step);
        }

        // Phase 2. `Ambiguous` / `Unmatched` propagate as `Err` (rather than
        // `NodeStep::Done(NodeOutput::Err(_))`) so they surface at `Scheduler::execute`'s
        // return value, matching today's `scope.dispatch(...)?` shape.
        //
        // Chain-gated: every dispatched node carries an active chain by invariant, so
        // pass it in so the resolver / bucket-pre-filter / pending-overload walk filter
        // candidates by visibility against this consumer's lexical position. See
        // `LexicalFrame::index_for` and `core::scope::visible`.
        let chain = self.active_chain.as_deref();
        let resolved = match scope.resolve_dispatch_with_chain(&expr, chain) {
            ResolveOutcome::Resolved(r) => r,
            ResolveOutcome::Ambiguous(n) => {
                return Err(KError::new(KErrorKind::AmbiguousDispatch {
                    expr: expr.summarize(),
                    candidates: n,
                }));
            }
            ResolveOutcome::Unmatched => {
                return Err(KError::new(KErrorKind::DispatchFailed {
                    expr: expr.summarize(),
                    reason: "no matching function".to_string(),
                }));
            }
            ResolveOutcome::Deferred => {
                // No overload picks against the bare shape, but the expression carries
                // eager parts whose evaluation may surface matching types. Schedule them
                // through the standard eager loop (no eager_indices filter, no picked
                // function — the receiving `run_bind` re-dispatches after the subs
                // resolve). `pre_subs` is empty by construction: recursive submission
                // only runs when a binder is picked at submit time, and Deferred means
                // no overload picked.
                debug_assert!(
                    pre_subs.is_empty(),
                    "Deferred resolve_dispatch implies no binder pick at submit time; \
                     `pre_subs` must be empty here",
                );
                return Ok(self.schedule_deps_filtered(expr, None, None, pre_subs, scope, idx));
            }
            ResolveOutcome::ParkOnProducers(producers) => {
                // A tentative tie hinged on a forward-referenced bare name. Park on its
                // producer(s) and re-dispatch on wake, when the strict-pass peek can read
                // the bound type. Preserve `pre_subs` across the re-Dispatch.
                return Ok(self.park_pending_and_redispatch(producers, expr, pre_subs, idx));
            }
            ResolveOutcome::UnboundName(name) => {
                return Err(KError::new(KErrorKind::UnboundName(name)));
            }
        };

        // Phase 2.5: install dispatch-time placeholders for the binder slot.
        // Two parallel installs:
        // - `placeholder_name` -> name-keyed `placeholders[name]`, consulted by
        //   `Scope::resolve` for forward-reference *name* resolution. Set by every
        //   binder builtin's `binder_name` hook (LET, FN, FUNCTOR, STRUCT, UNION, SIG,
        //   MODULE).
        // - `pending_overload_bucket` -> bucket-keyed `pending_overloads[key]`,
        //   consulted by `resolve_dispatch`'s no-bucket fallback for forward-reference
        //   *dispatch* parks. Set only by FN / FUNCTOR's `binder_bucket` hook (the
        //   binders that register a callable function). Keying by the full inner-call
        //   bucket — not the lead keyword — keeps overloads with shared heads but
        //   different keyword shapes from colliding on the park edge.
        // Both installs carry the executing slot's lexical index and the picked
        // function's `is_nominal_binder` flag — the submission-time install in
        // `submit::add_with_chain` used the same pair, so the placeholder→bind
        // transition keeps a consistent visibility tag.
        let lex_index = self
            .active_chain
            .as_ref()
            .expect("dispatching slot must have an active chain")
            .index;
        let bind_index = BindingIndex {
            idx: lex_index,
            nominal_binder: resolved.function.is_nominal_binder,
        };
        if let Some(name) = resolved.placeholder_name.as_ref() {
            if let Err(e) = scope.install_placeholder(name.clone(), NodeId(idx), bind_index) {
                return Ok(NodeStep::Done(NodeOutput::Err(e)));
            }
        }
        if let Some(bucket) = resolved.pending_overload_bucket.as_ref() {
            if let Err(e) =
                scope.install_pending_overload(bucket.clone(), NodeId(idx), bind_index)
            {
                return Ok(NodeStep::Done(NodeOutput::Err(e)));
            }
        }

        // Phase 3: eager name resolve. One pass over `wrap_indices` (splice on hit) and
        // `ref_name_indices` (no-op on hit) feeding a combined `producers_to_wait` list.
        // If any producer was parked we install one combined park and re-dispatch on
        // wake; otherwise the resolved values are in-place and we proceed to schedule
        // deps for the eager parts.
        let mut expr = expr;
        let mut producers_to_wait: Vec<NodeId> = Vec::new();
        for &i in &resolved.slots.wrap_indices {
            match resolve_name_part(scope, &expr.parts[i].value, self, Some(NodeId(idx))) {
                NameOutcome::Resolved(obj) => {
                    expr.parts[i].value = ExpressionPart::Future(obj);
                }
                NameOutcome::Parked(producer) => {
                    if !producers_to_wait.contains(&producer) {
                        producers_to_wait.push(producer);
                    }
                }
                NameOutcome::ProducerErrored(e) => {
                    let frame = Frame::from_expr("<wrap-resolve>", &expr);
                    return Ok(NodeStep::Done(NodeOutput::Err(
                        propagate_dep_error(&e, Some(frame)),
                    )));
                }
                NameOutcome::Unbound(name) => {
                    // Match the pre-removal surface: an unbound wrap-slot name became a
                    // sub-Dispatch through `value_lookup::body_identifier` /
                    // `body_type_expr`, both of which return `BodyResult::Err(UnboundName)`
                    // — a slot terminal rather than a propagated scheduler error. Parent
                    // slots (MODULE / FN / LET binders) catch that terminal through
                    // their Combine's dep-error short-circuit; surfacing it as an Err
                    // from `execute` here would break that catch.
                    return Ok(NodeStep::Done(NodeOutput::Err(KError::new(
                        KErrorKind::UnboundName(name),
                    ))));
                }
                NameOutcome::Cycle(name) => {
                    // Trivial self-park (`LET x = x`, `LET Ty = Ty`). The eager-resolve
                    // pass catches the placeholder-points-at-self condition before any
                    // sub-Dispatch, and surfaces it as `SchedulerDeadlock` (the
                    // cycle-specific error kind) on the slot's terminal — unifying the
                    // Identifier-LHS and Type-LHS cycle surfaces. The legacy resolver
                    // path that emitted this as `ShapeError("cycle in type alias …")`
                    // has been removed; see `resolver::elaborate_type_expr`.
                    return Ok(NodeStep::Done(NodeOutput::Err(KError::new(
                        KErrorKind::SchedulerDeadlock {
                            pending: 1,
                            sample: format!("cycle in type alias `{name}`"),
                        },
                    ))));
                }
            }
        }
        for &i in &resolved.slots.ref_name_indices {
            // Literal-name slots keep the bare token; only the park outcome matters here.
            // Non-bare-name parts (e.g. an already-spliced `Future`) shouldn't appear in
            // a `ref_name_indices` slot by classification, but defensive: skip them.
            let part = &expr.parts[i].value;
            if !matches!(
                part,
                ExpressionPart::Identifier(_)
                    | ExpressionPart::Type(_)
            ) {
                continue;
            }
            // Skip parameterized Type parts — only leaf names park.
            if let ExpressionPart::Type(t) = part {
                if !matches!(t.params, TypeParams::None) {
                    continue;
                }
            }
            match resolve_name_part(scope, part, self, Some(NodeId(idx))) {
                NameOutcome::Resolved(_) | NameOutcome::Unbound(_) => {}
                NameOutcome::Parked(producer) => {
                    if !producers_to_wait.contains(&producer) {
                        producers_to_wait.push(producer);
                    }
                }
                NameOutcome::ProducerErrored(e) => {
                    let frame = Frame::from_expr("<replay-park>", &expr);
                    return Ok(NodeStep::Done(NodeOutput::Err(
                        propagate_dep_error(&e, Some(frame)),
                    )));
                }
                NameOutcome::Cycle(name) => {
                    // ref_name-slot cycle (e.g. ATTR-form self-reference); same
                    // `SchedulerDeadlock` surface as the wrap-slot cycle arm above.
                    return Ok(NodeStep::Done(NodeOutput::Err(KError::new(
                        KErrorKind::SchedulerDeadlock {
                            pending: 1,
                            sample: format!("cycle in type alias `{name}`"),
                        },
                    ))));
                }
            }
        }
        if !producers_to_wait.is_empty() {
            return Ok(self.install_combined_park(producers_to_wait, expr, pre_subs, idx));
        }

        // Phase 4: schedule eager subs from the resolution's indices. Commit to the
        // tentative pick from Phase 2 even when wrap-slots were spliced — a mismatch
        // between the splicing and the picked overload surfaces as `TypeMismatch` from
        // `bind` (more specific than the generic `DispatchFailed` an extra
        // `resolve_dispatch` would emit), and the eager path avoids the cost of a
        // second resolution walk per wrap-bearing dispatch.
        Ok(self.schedule_deps_filtered(
            expr,
            resolved.slots.eager_indices.as_deref(),
            Some(resolved.function),
            pre_subs,
            scope,
            idx,
        ))
    }

    /// Install park edges from `idx` onto each producer in `producers` and rebuild this
    /// slot as a re-Dispatch of `expr`. Shared between Phase 3's combined park, Phase 2's
    /// `ParkOnProducers` arm (via `park_pending_and_redispatch`), and the bind-time
    /// `ParkOnProducers` path. Caller has already filtered through `would_create_cycle`
    /// and producer-error propagation; this just installs the edges and the Replace step.
    fn install_combined_park(
        &mut self,
        producers: Vec<NodeId>,
        expr: KExpression<'a>,
        pre_subs: Vec<(usize, NodeId)>,
        idx: usize,
    ) -> NodeStep<'a> {
        for p in &producers {
            self.deps.add_park_edge(*p, NodeId(idx));
        }
        // Preserve `pre_subs` across re-Dispatch: the recursive submission only
        // happens at the *original* `add_with_chain`, so a parked binder dispatch
        // that wakes and re-runs must still see its pre-submitted children to
        // avoid double-submission in Phase 4. See
        // `roadmap/dispatch_fix/nested-binder-submission.md`.
        NodeStep::Replace {
            work: NodeWork::Dispatch { expr, pre_subs },
            frame: None,
            function: None,
            block_entry: None,
            body_index: 0,
        }
    }

    /// Park `idx` on each still-pending producer and rebuild it as a re-Dispatch of `expr`.
    /// Shares the producer-error / cycle / install guards with Phase 3 of `run_dispatch`:
    /// a producer that already terminalized with an error propagates (parking on a dead
    /// slot would deadlock); one that would close a cycle is skipped; if no parkable
    /// producer remains, the call is a genuine no-match. On wake the re-Dispatch re-runs
    /// resolution, where the now-bound name lets the strict-pass peek pick. Drives the
    /// tentative-tie [`ResolveOutcome::ParkOnProducers`] path from both `run_dispatch`
    /// and `run_bind`.
    pub(super) fn park_pending_and_redispatch(
        &mut self,
        producers: Vec<NodeId>,
        expr: KExpression<'a>,
        pre_subs: Vec<(usize, NodeId)>,
        idx: usize,
    ) -> NodeStep<'a> {
        let mut to_wait: Vec<NodeId> = Vec::new();
        for p in producers {
            if self.is_result_ready(p) {
                // Terminal while its placeholder is still set ⇒ the producer errored
                // (success clears the placeholder); propagate rather than park on a dead
                // slot.
                if let Err(e) = self.read_result(p) {
                    let frame = Frame::from_expr("<dispatch-park>", &expr);
                    return NodeStep::Done(NodeOutput::Err(
                        propagate_dep_error(e, Some(frame)),
                    ));
                }
            } else if !self.deps.would_create_cycle(p, NodeId(idx))
                && !to_wait.contains(&p) {
                    to_wait.push(p);
                }
        }
        if to_wait.is_empty() {
            return NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::DispatchFailed {
                expr: expr.summarize(),
                reason: "no matching function".to_string(),
            })));
        }
        self.install_combined_park(to_wait, expr, pre_subs, idx)
    }

    /// Phase 1. Bare-name short-circuit. `Some(step)` only fires on `Value` (terminate
    /// with the bound value) or `Placeholder` (install park edge, rewrite to `Lift`).
    /// `Unbound` and non-bare-name shapes return `None` for the caller to continue.
    ///
    /// The Lift transition is unique to single-bare-name short-circuit (no other phase
    /// rewrites the slot to a Lift on park) — combined-park phases call
    /// [`Self::install_combined_park`] instead, which keeps the slot as a Dispatch and
    /// re-runs the full pipeline on wake.
    fn try_short_circuit(
        &mut self,
        expr: &KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Option<NodeStep<'a>> {
        if let [Spanned { value: ExpressionPart::Identifier(name), .. }] = expr.parts.as_slice() {
            // Chain-gated: a later-sibling binding is invisible to this consumer, so
            // a name that lexically does not yet exist falls through to the standard
            // dispatch / `UnboundName` path rather than short-circuiting on a hidden
            // value.
            match scope.resolve_with_chain(name, self.active_chain.as_deref()) {
                Resolution::Value(obj) => Some(NodeStep::Done(NodeOutput::Value(obj))),
                Resolution::Placeholder(producer_id) => {
                    // Notify edge, not Owned: the producer is a sibling slot this Lift
                    // only parks on for a wake — it is not part of this slot's reclaim
                    // subtree. `add_park_edge` installs the forward wake on
                    // `notify_list[producer]` and bumps `pending_deps[idx]` in the same
                    // atomic body; `free` skips past Notify edges via `owned_children`.
                    // Producer-not-terminal precondition: `Resolution::Placeholder` is
                    // only returned between submission and terminalization of the
                    // placeholder's slot, so `producer_id` is not yet terminal here.
                    self.deps.add_park_edge(producer_id, NodeId(idx));
                    Some(NodeStep::Replace {
                        work: NodeWork::Lift(LiftState::Pending(producer_id)),
                        frame: None,
                        function: None,
                        block_entry: None,
                        body_index: 0,
                    })
                }
                // Unbound falls through so `value_lookup`'s body produces the structured
                // `UnboundName` error.
                Resolution::UnboundName => None,
            }
        } else {
            None
        }
    }

    /// Phase 4. Schedule eager sub-Dispatches for `Expression` / `ListLiteral` /
    /// `DictLiteral` parts of `expr` and build a `Bind` slot. The collapse-into-one of
    /// the old `schedule_deps` (lazy / strict resolved) and `schedule_eager_fallthrough`
    /// (deferred) — only difference between the call sites is whether an `eager_filter`
    /// restricts which parts schedule:
    /// - `Some(eager_indices)` — **Lazy-candidate arm.** The picked function has a
    ///   `KType::KExpression` slot bound by an `ExpressionPart::Expression`; only the
    ///   carried `eager_indices` (Expression parts in *non-*`KExpression` slots) schedule.
    ///   Every other part rides through unchanged, including lazy `Expression` parts in
    ///   `KExpression` slots, which the receiving builtin dispatches itself. Caller
    ///   supplies the picked function via `picked` so the no-subs branch can bind
    ///   directly.
    /// - `None` — **Schedule-all arm.** Schedule every `Expression` / `ListLiteral` /
    ///   `DictLiteral` part as a sub. Used by:
    ///   - The strict `Resolved` arm of `run_dispatch` (no lazy slot — `picked` is the
    ///     resolved function).
    ///   - The `Deferred` arm — `picked = None`; the receiving `run_bind` re-dispatches
    ///     with `Future(_)` parts after the subs resolve. `Deferred ⇒ at least one eager
    ///     part`, so the empty-subs branch is unreachable; `debug_assert!` pins that
    ///     invariant.
    ///
    /// On the empty-subs branch, bind `picked` directly via `invoke_to_step`. Required
    /// for the wrap-slot fast path: a `MAKESET IntOrd`-shape call resolves bare names in
    /// Phase 3, leaves no eager parts to schedule, and binds the picked function in one
    /// step — no Bind detour.
    fn schedule_deps_filtered(
        &mut self,
        expr: KExpression<'a>,
        eager_filter: Option<&[usize]>,
        picked: Option<&'a crate::machine::core::kfunction::KFunction<'a>>,
        pre_subs: Vec<(usize, NodeId)>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        let mut new_parts = Vec::with_capacity(expr.parts.len());
        let mut subs: Vec<(usize, NodeId)> = Vec::new();
        for (i, part) in expr.parts.into_iter().enumerate() {
            let span = part.span;
            // The lazy-candidate arm only schedules parts named in `eager_filter`. The
            // schedule-all arm passes `None` and schedules every Expression-shaped part.
            let in_filter = eager_filter.is_none_or(|idxs| idxs.contains(&i));
            if !in_filter {
                new_parts.push(Spanned { value: part.value, span });
                continue;
            }
            // Pre-submission splice: if this slot was recursively pre-submitted at
            // outermost-submission time (binder-shaped expression — see
            // `submit::add_with_chain`), reuse that NodeId instead of allocating a
            // fresh sub-Dispatch. Pre-empts the Expression arm to avoid
            // double-submission. See `roadmap/dispatch_fix/nested-binder-submission.md`.
            if let Some(&(_, sub_id)) = pre_subs.iter().find(|(j, _)| *j == i) {
                subs.push((i, sub_id));
                new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                continue;
            }
            match part.value {
                ExpressionPart::Expression(boxed) => {
                    let sub_id = self.add(NodeWork::dispatch(*boxed), scope);
                    subs.push((i, sub_id));
                    new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                }
                ExpressionPart::ListLiteral(items) => {
                    let agg_id = self.schedule_list_literal(items, scope);
                    subs.push((i, agg_id));
                    new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                }
                ExpressionPart::DictLiteral(pairs) => {
                    let agg_id = self.schedule_dict_literal(pairs, scope);
                    subs.push((i, agg_id));
                    new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                }
                other => new_parts.push(Spanned { value: other, span }),
            }
        }
        let new_expr = KExpression::new(new_parts);
        if subs.is_empty() {
            // No subs: bind the picked function directly. Spliced `Future(&'a KObject)`
            // references survive `results[dep] = None` because the objects live in
            // arenas tied to lexical scope.
            let function = picked.expect(
                "schedule_deps_filtered: empty-subs branch requires `picked`; Deferred arm \
                 must carry at least one eager part",
            );
            match function.bind(new_expr) {
                Ok(future) => self.invoke_to_step(future, scope, idx),
                Err(e) => NodeStep::Done(NodeOutput::Err(e)),
            }
        } else {
            debug_assert!(
                picked.is_some() || eager_filter.is_none(),
                "lazy-candidate arm must supply `picked` (eager_filter = Some ⇒ picked = Some)",
            );
            let bind_id = self.add(NodeWork::Bind { expr: new_expr, subs }, scope);
            self.defer_to_lift(idx, bind_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_name_part, NameOutcome};
    use super::super::super::nodes::NodeWork;
    use crate::builtins::default_scope;
    use crate::machine::core::source::Spanned;
    use crate::machine::execute::Scheduler;
    use crate::machine::model::ast::{ExpressionPart, KExpression, TypeExpr};
    use crate::machine::model::{KObject, KType};
    use crate::machine::{BindingIndex, RuntimeArena};

    /// Resolved-Identifier path: bare Identifier in scope.bindings.data returns
    /// `NameOutcome::Resolved(&obj)` pointing at the bound carrier.
    #[test]
    fn resolve_name_part_identifier_resolved() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let bound = arena.alloc(KObject::Number(7.0));
        scope.bind_value("x".to_string(), bound, BindingIndex::BUILTIN).unwrap();
        let part = ExpressionPart::Identifier("x".to_string());
        let sched = Scheduler::new();
        match resolve_name_part(scope, &part, &sched, None) {
            NameOutcome::Resolved(KObject::Number(n)) => assert_eq!(*n, 7.0),
            _ => panic!("expected NameOutcome::Resolved(Number)"),
        }
    }

    /// Resolved-Type path: bare leaf `Type` token whose name lives in
    /// `bindings.types` routes through `coerce_type_token_value` and returns the
    /// `KTypeValue` synthesis. The builtin `Number` registered at default_scope
    /// satisfies this without extra setup.
    #[test]
    fn resolve_name_part_type_resolved() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let part = ExpressionPart::Type(TypeExpr::leaf("Number".to_string()));
        let sched = Scheduler::new();
        match resolve_name_part(scope, &part, &sched, None) {
            NameOutcome::Resolved(KObject::KTypeValue(KType::Number)) => {}
            other => {
                let kind = match other {
                    NameOutcome::Resolved(_) => "Resolved(other)",
                    NameOutcome::Parked(_) => "Parked",
                    NameOutcome::ProducerErrored(_) => "ProducerErrored",
                    NameOutcome::Unbound(_) => "Unbound",
                    NameOutcome::Cycle(_) => "Cycle",
                };
                panic!("expected Resolved(KTypeValue(Number)), got {kind}");
            }
        }
    }

    /// Parked path: a Dispatch slot installed as a `binder_name` placeholder against the
    /// name resolves to `NameOutcome::Parked(producer)`. Mimics a forward LET binder
    /// by manually installing a placeholder against a fresh slot.
    #[test]
    fn resolve_name_part_parked() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        // A fresh Dispatch slot to back the placeholder. The expression contents don't
        // matter for this test; we never run it.
        let producer = sched.add_dispatch(
            KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("_".into()))]),
            scope,
        );
        scope.install_placeholder("fwd".to_string(), producer, BindingIndex::BUILTIN).unwrap();
        let part = ExpressionPart::Identifier("fwd".to_string());
        match resolve_name_part(scope, &part, &sched, None) {
            NameOutcome::Parked(p) => assert_eq!(p, producer),
            _ => panic!("expected NameOutcome::Parked(producer)"),
        }
    }

    /// Unbound path: a name with no binding and no placeholder returns
    /// `NameOutcome::Unbound(name)`.
    #[test]
    fn resolve_name_part_unbound() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let part = ExpressionPart::Identifier("missing".to_string());
        let sched = Scheduler::new();
        match resolve_name_part(scope, &part, &sched, None) {
            NameOutcome::Unbound(name) => assert_eq!(name, "missing"),
            _ => panic!("expected NameOutcome::Unbound"),
        }
    }

    /// Cycle path: when `consumer` is provided and matches the producer (self-park),
    /// returns `NameOutcome::Cycle(name)` rather than `Parked`.
    #[test]
    fn resolve_name_part_self_park_is_cycle() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        // Submit a Dispatch slot and install a placeholder for `self_ref` pointing at
        // itself. Then resolve "self_ref" with `consumer = Some(that_slot)`.
        let slot = sched.add(
            NodeWork::dispatch(KExpression::new(vec![
                Spanned::bare(ExpressionPart::Identifier("self_ref".into())),
            ])),
            scope,
        );
        scope.install_placeholder("self_ref".to_string(), slot, BindingIndex::BUILTIN).unwrap();
        let part = ExpressionPart::Identifier("self_ref".to_string());
        match resolve_name_part(scope, &part, &sched, Some(slot)) {
            NameOutcome::Cycle(name) => assert_eq!(name, "self_ref"),
            _ => panic!("expected NameOutcome::Cycle"),
        }
    }
}
