use crate::builtins::value_lookup::coerce_type_token_value;
use crate::builtins::{struct_value, tagged_union};
use crate::machine::core::source::Spanned;
use crate::machine::model::{KObject, Parseable};
use crate::machine::{
    BindingIndex, Frame, KError, KErrorKind, NameOutcome, NodeId, ResolveOutcome, Resolution, Scope,
};
use crate::machine::core::kfunction::BodyResult;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeExpr, TypeParams};

use super::super::nodes::{LiftState, NodeOutput, NodeStep, NodeWork};
use super::dispatch_state::{
    BareIdState, BareTypeState, DispatchState, FnValueState, Initialized, KeywordedState,
    SigilState, TyCtorState,
};
use super::Scheduler;

/// Pre-walk classification of a `KExpression` into the four no-keyword fast-lane
/// shapes plus the catch-all keyword-bearing shape. Driven by [`classify_dispatch_shape`]
/// at the top of [`Scheduler::run_dispatch`]; the four no-keyword variants run their own
/// handlers and never enter `resolve_dispatch_with_chain`. `Keyworded` falls into the
/// existing Phase 1-4 candidate pipeline unchanged.
///
/// Rationale: only `Keyworded` calls have candidates in `bindings.functions`. The other
/// shapes (a bare identifier, a leaf type token, a type-call like `(List Number)`, a
/// function-value call like `(f 7)`) all resolve through scope value/type lookups, not
/// the overload bucket — so the candidate machinery does no useful work for them.
///
/// See the unified-walk roadmap item D4 / D5.
/// Pre-walk classification of a `KExpression` into the four no-keyword fast-lane
/// shapes plus the catch-all keyword-bearing shape. The variants carry per-shape
/// indices into `expr.parts` rather than reference data — borrowing references would
/// keep `expr` borrowed for the dispatch driver's whole match, which conflicts with
/// the keyworded arm needing to move `expr` into `resolve_dispatch_with_chain`.
pub(super) enum DispatchShape {
    /// Single bare `Identifier` part. Handler reads the name out of
    /// `expr.parts[0]`.
    BareIdentifier,
    /// Single bare leaf `Type` part. Handler reads the `TypeExpr` out of
    /// `expr.parts[0]`.
    BareTypeLeaf,
    /// Type-constructor call: head (index 0) is a leaf `Type` and `parts[1..]`
    /// is non-empty. Handler resolves the head type-side and routes
    /// Struct / Tagged / Newtype / TypeConstructor heads directly through their
    /// construction primitives; opaque / Module / unbound heads fall through to
    /// the keyworded `type_call` builtin. With the legacy positional
    /// `(List Number)` / `(Dict K V)` shape deleted (the `TypeCall` arm is
    /// gone), this variant covers every Type-headed multi-part call. Phase 2
    /// of `scratch/plan-fast-lane-subsume.md`.
    TypeConstructorCall,
    /// Function-value call: head (index 0) is a lowercase `Identifier`, followed by
    /// ≥1 non-keyword parts. Handler resolves the head and falls back to the
    /// keyworded path when it doesn't bind to a `KFunction`.
    FunctionValueCall,
    /// Single-part `:(...)` sigiled type-expression wrapper. Handler recursively
    /// dispatches the inner `KExpression` and asserts the result is a type-side
    /// carrier (`KTypeValue`, `Module`, `Signature`, `UserType`, `KFunctor`). The
    /// recursive sub-dispatch sees the same classifier — `Keyworded` for new
    /// `LIST OF` / `MAP _ -> _` / `FN` / `FUNCTOR` shapes, `TypeCall` for legacy
    /// positional `:(List Number)`. See [design/typing/type-language-via-dispatch.md].
    SigiledTypeExpr,
    /// A keyword appears anywhere in `expr.parts`, OR the expression doesn't fit any
    /// fast-lane shape (parameterized Type head, literal head, `Future` head, etc.).
    /// Drives the existing candidate-walk pipeline.
    Keyworded,
}

/// One-pass classifier. **Sweeps every part for `Keyword` first** so a mixed shape
/// like `(f IF x)` (lowercase head, keyword in body) goes to `Keyworded` — only the
/// candidate machinery knows how to dispatch against the `(_ IF _)` bucket. Only when
/// the no-keyword precondition is established do we branch on head shape.
///
/// Single-part fast-lane: a parens-wrapped bare name (`(some_var)`) or leaf type
/// (`(Number)`) maps to `BareIdentifier` / `BareTypeLeaf`. Anything else single-part
/// (literal, `Future`, parameterized Type) falls to `Keyworded` — the parser keeps
/// those for the candidate path.
///
/// Multi-part fast-lane: head is leaf `Type` → `TypeConstructorCall` (the legacy
/// positional `(List Number)` shape is gone — type-language parameterization runs
/// through the keyworded `LIST OF` / `MAP _ -> _` / `FN` / `FUNCTOR` overloads).
/// Head is lowercase `Identifier` → `FunctionValueCall`.
pub(super) fn classify_dispatch_shape(expr: &KExpression<'_>) -> DispatchShape {
    // 1. Any Keyword part anywhere ⇒ Keyworded. Head position is not special — D4's
    // "sweep first, branch on head second" ordering.
    if expr.parts.iter().any(|p| matches!(&p.value, ExpressionPart::Keyword(_))) {
        return DispatchShape::Keyworded;
    }
    // 2. Single-part cases.
    if let [only] = expr.parts.as_slice() {
        return match &only.value {
            ExpressionPart::Identifier(_) => DispatchShape::BareIdentifier,
            ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => {
                DispatchShape::BareTypeLeaf
            }
            // Sigiled type-expression wrapper — the dispatcher unwraps and re-runs
            // classification on the inner expression. See [`Self::fast_lane_sigiled_type_expr`].
            ExpressionPart::SigiledTypeExpr(_) => DispatchShape::SigiledTypeExpr,
            // Parenthesized literal, Future, parameterized Type, ListLiteral, ...:
            // not a fast-lane shape; the keyworded path surfaces today's Deferred /
            // Unmatched / DispatchFailed for these.
            _ => DispatchShape::Keyworded,
        };
    }
    // 3. Multi-part, no-keyword. Branch on head-token shape. An empty parts list
    // (which the parser never produces but the scheduler can construct via splicing)
    // falls through to `Keyworded` so the candidate path surfaces today's
    // `DispatchFailed`.
    let Some(head_part) = expr.parts.first() else {
        return DispatchShape::Keyworded;
    };
    match &head_part.value {
        ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => {
            // Head is a leaf `Type` → `TypeConstructorCall`. The legacy positional
            // `(List Number)` shape (leaf-Type-only args) used to route through a
            // separate `TypeCall` arm that elaborated `TypeExpr { params: List(_) }`;
            // that arm is deleted now that the keyworded `LIST OF` / `MAP _ -> _` /
            // `FN` / `FUNCTOR` overloads serve every parameterized-type form.
            DispatchShape::TypeConstructorCall
        }
        ExpressionPart::Identifier(_) => DispatchShape::FunctionValueCall,
        _ => DispatchShape::Keyworded,
    }
}

/// Resolve a bare-name `ExpressionPart` (Identifier or leaf Type) against `scope`. The
/// returned [`NameOutcome`] is a small ADT consumed by:
/// 1. The per-`run_dispatch` `bare_outcomes` cache build (every bare-name part is
///    resolved exactly once, then the cache feeds strict admission and the fused
///    splice/park walk).
/// 2. The dict/list literal planner's name-keyed Slot resolution (see
///    [`crate::machine::execute::scheduler::literal`]).
///
/// Non-bare-name parts (`Expression`, literals, parens, `Future`, etc.) are never
/// passed here — the dispatch driver filters them out with a `matches!` guard
/// before populating the cache.
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

/// Best-effort name extraction for a bare-name `ExpressionPart` — used to render
/// the `cycle in type alias <name>` sample in `SchedulerDeadlock` when the
/// fused splice/park walk detects a wake cycle. Returns `None` for non-bare-name
/// parts (which shouldn't reach the cycle arm by classification).
fn bare_name_of<'a>(part: &ExpressionPart<'a>) -> Option<String> {
    match part {
        ExpressionPart::Identifier(n) => Some(n.clone()),
        ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => Some(t.name.clone()),
        _ => None,
    }
}

/// Pull the inner parts of a `f (...)` call out of `expr.parts[1..]`. The
/// `FunctionValueCall` classifier guarantees an Identifier head and ≥1 non-keyword
/// body part; this checks the body is exactly a single nested-parens
/// (`ExpressionPart::Expression`) and clones its inner parts. Anything else
/// surfaces a `DispatchFailed` `KError` — koan has no positional call shape for
/// function values, so any non-paren body is a genuine shape error. Free
/// function (not on `Scheduler`) so the caller's `&mut self` borrow is unaffected
/// during the extraction.
fn extract_named_call_inner<'a>(
    expr: &KExpression<'a>,
) -> Result<Vec<Spanned<ExpressionPart<'a>>>, KError> {
    let [Spanned { value: ExpressionPart::Expression(inner), .. }] = expr.parts[1..].as_ref()
    else {
        return Err(KError::new(KErrorKind::DispatchFailed {
            expr: expr.summarize(),
            reason: "no matching function".to_string(),
        }));
    };
    Ok(inner.parts.clone())
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
    /// Dispatch driver. Opens with [`classify_dispatch_shape`]; the no-keyword shapes
    /// (`BareIdentifier`, `BareTypeLeaf`, `FunctionValueCall`, `TypeConstructorCall`,
    /// `SigiledTypeExpr`) run their fast-lane handlers and never enter
    /// `resolve_dispatch_with_chain`. The `Keyworded` arm — the only shape with
    /// candidates in `bindings.functions` — drives the strict-only pipeline:
    ///
    /// 1. **Build the bare-name cache.** One `resolve_name_part` per bare-name part
    ///    (`Identifier` or leaf `Type`) into `bare_outcomes: Vec<Option<NameOutcome>>`
    ///    — non-bare-name parts get `None`. Built with `consumer = None` so cycle
    ///    detection is deferred to Step 4 (where it runs only on slots the picked
    ///    function classifies as references). Shared with the resolver's strict
    ///    admission *and* the fused splice/park walk below, so each bare name
    ///    resolves exactly once per `run_dispatch` invocation.
    /// 2. **Upfront short-circuit.** Sweep `bare_outcomes` for `ProducerErrored`;
    ///    these trump any overload choice and surface a `<wrap-resolve>`-framed
    ///    propagation directly. (No `Cycle` sweep — cycle detection is in Step 4.)
    /// 3. **`Scope::resolve_dispatch_with_chain`** — one chain walk yielding a
    ///    [`Resolved`], `Ambiguous(n)`, `Deferred`, `ParkOnProducers`, `UnboundName`,
    ///    or `Unmatched`. Admission is strict-only and reads `bare_outcomes`; the
    ///    post-walk fallback (placeholders > eager > unbound > pending overload >
    ///    Unmatched) also reads it. A keyword-headed call to a not-yet-registered
    ///    function with no eager parts and no Parked / Unbound bare-name args still
    ///    consults `pending_overloads` by the full bucket key as a last-step fallback.
    ///    3.5: **Placeholder install** — if the picked function carried a
    ///    `binder_name` extractor, install its dispatch-time name placeholder against
    ///    this slot's `NodeId`. Same for `pending_overload_bucket`.
    /// 4. **Fused splice / park / eager-sub walk.** One iteration over `expr.parts`
    ///    that reads `bare_outcomes[i]` for wrap-slot splice (Resolved ⇒ rewrite to
    ///    `Future(obj)`; Parked ⇒ cycle-check then push producer; Unbound ⇒
    ///    surface `UnboundName` as a slot terminal) and ref-name-slot park (Parked
    ///    ⇒ cycle-check then push producer), and the part's shape for the
    ///    eager-sub schedule (Expression / SigiledTypeExpr /
    ///    ListLiteral / DictLiteral). Index buckets are disjoint by
    ///    [`ClassifiedSlots`](crate::machine::core::kfunction::ClassifiedSlots). If
    ///    any producer parked, install one combined park before submitting any subs;
    ///    otherwise either bind directly (no subs) or build a `Bind` slot.
    ///
    /// Fast lane is one-pass: `BareTypeLeaf` doesn't fall back; its failure surfaces
    /// directly. Only `FunctionValueCall` falls back to the `Keyworded` path when the
    /// head doesn't resolve to a function — a keyword-headed overload may still match.
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
        // Fast lane: classify before any walk. The five no-keyword shapes route around
        // `resolve_dispatch_with_chain` entirely (no candidates to consider for them);
        // `Keyworded` falls into the cache-driven pipeline below.
        match classify_dispatch_shape(&expr) {
            DispatchShape::BareIdentifier => {
                // Classifier guarantees expr.parts is single-element Identifier.
                let name = match &expr.parts[0].value {
                    ExpressionPart::Identifier(n) => n.clone(),
                    _ => unreachable!("BareIdentifier shape implies single Identifier part"),
                };
                if let Some(step) = self.fast_lane_bare_identifier(&name, scope, idx) {
                    return Ok(step);
                }
                // Unbound bare identifier falls through to the keyworded path so
                // `value_lookup::body_identifier` produces the structured `UnboundName`
                // surface (preserves today's contract).
            }
            DispatchShape::BareTypeLeaf => {
                let t = match &expr.parts[0].value {
                    ExpressionPart::Type(t) => t.clone(),
                    _ => unreachable!("BareTypeLeaf shape implies single leaf Type part"),
                };
                return Ok(self.fast_lane_bare_type_leaf(&t, scope));
            }
            DispatchShape::FunctionValueCall => {
                return Ok(self.fast_lane_function_value_call(&expr, scope, idx));
            }
            DispatchShape::TypeConstructorCall => {
                // Phase 2 commit 1 of the fast-lane subsumption
                // (`scratch/plan-fast-lane-subsume.md`): the variant is added to the
                // classifier and routed here, but the handler is intentionally empty
                // — we fall through to Keyworded so the `type_call` builtin still
                // serves construction. Commits 2-3 add per-head-type arms; commits
                // 4-6 migrate tests, trim `type_call.rs`, and relocate
                // `dispatch_constructor`.
            }
            DispatchShape::SigiledTypeExpr => {
                let inner = match expr.parts.into_iter().next() {
                    Some(Spanned { value: ExpressionPart::SigiledTypeExpr(boxed), .. }) => *boxed,
                    _ => unreachable!("SigiledTypeExpr shape implies single SigiledTypeExpr part"),
                };
                return Ok(self.fast_lane_sigiled_type_expr(inner, scope, idx));
            }
            DispatchShape::Keyworded => {}
        }

        // Step 1: build the bare-name outcome cache. Non-bare-name parts get `None`;
        // strict admission falls back to `arg.matches(part)` for them. Cache lives
        // through Step 3 (strict admission) and Step 4 (fused splice/park/eager walk).
        //
        // `consumer = None`: cycle detection is **deferred** to the fused walk in
        // Step 4, where it runs only on wrap / ref-name slots (slots the picked
        // function says are references). A binder declaration slot like `x` in
        // `LET x = …` has the dispatching slot as the producer of its own
        // `x → idx` placeholder; running cycle detection upfront would surface a
        // false-positive `SchedulerDeadlock` because the declaration slot looks
        // like a self-park before we know it's a declaration.
        let bare_outcomes: Vec<Option<NameOutcome<'a>>> = expr
            .parts
            .iter()
            .map(|p| match &p.value {
                ExpressionPart::Identifier(_) => {
                    Some(resolve_name_part(scope, &p.value, self, None))
                }
                ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => {
                    Some(resolve_name_part(scope, &p.value, self, None))
                }
                _ => None,
            })
            .collect();

        // Step 2: upfront short-circuit for `ProducerErrored`. A bare-name arg whose
        // producer has already terminalized with `Err` can never resolve; surface the
        // error with a `<wrap-resolve>` frame before the candidate walk. (No `Cycle`
        // sweep — cycle detection is deferred to Step 4 by the `consumer = None`
        // above.)
        for outcome in bare_outcomes.iter().flatten() {
            if let NameOutcome::ProducerErrored(e) = outcome {
                let frame = Frame::from_expr("<wrap-resolve>", &expr);
                return Ok(NodeStep::Done(NodeOutput::Err(
                    propagate_dep_error(e, Some(frame)),
                )));
            }
        }

        // Step 3: chain-gated, cache-driven dispatch resolution.
        let chain = self.active_chain.as_deref();
        let resolved = match scope.resolve_dispatch_with_chain(&expr, chain, &bare_outcomes) {
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
                // eager parts whose evaluation may surface matching types. Schedule
                // every Expression-shaped part as a sub-Dispatch; the receiving
                // `run_bind` re-dispatches after the subs resolve. `pre_subs` is
                // empty by construction: recursive submission only runs when a binder
                // is picked at submit time, and Deferred means no overload picked.
                debug_assert!(
                    pre_subs.is_empty(),
                    "Deferred resolve_dispatch implies no binder pick at submit time; \
                     `pre_subs` must be empty here",
                );
                return Ok(self.schedule_eager_only(expr, scope, idx));
            }
            ResolveOutcome::ParkOnProducers(producers) => {
                // No bucket admitted; ≥1 bare-name arg parks on a forward-reference
                // placeholder (or an in-flight FN/FUNCTOR sibling's pending_overloads
                // entry). Re-dispatch on wake, when strict admission rebuilds the
                // cache against the now-bound type.
                return Ok(self.park_pending_and_redispatch(producers, expr, pre_subs, idx));
            }
            ResolveOutcome::UnboundName(name) => {
                return Err(KError::new(KErrorKind::UnboundName(name)));
            }
        };

        // Step 3.5: install dispatch-time placeholders for the binder slot.
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

        // Step 4: fused walk over `expr.parts`. Per part, exactly one arm fires:
        // - Pre-sub splice (binder recursive submission): reuse the recorded NodeId.
        // - Wrap slot (`resolved.slots.wrap_indices`): read `bare_outcomes[i]` —
        //   Resolved ⇒ rewrite to `Future(obj)`; Parked ⇒ push producer; Unbound ⇒
        //   surface `UnboundName` as a slot terminal.
        // - Ref-name slot (`resolved.slots.ref_name_indices`): read `bare_outcomes[i]`
        //   — Parked ⇒ push producer; Resolved / Unbound ⇒ no-op (literal-name slot
        //   keeps the bare token; the receiving builtin resolves it).
        // - Eager-sub slot: schedule a sub-Dispatch (Expression / SigiledTypeExpr)
        //   or aggregate (ListLiteral / DictLiteral). Filtered by
        //   `resolved.slots.eager_indices` when the picked function is a lazy
        //   candidate; otherwise every eager-shaped part schedules.
        //
        // Park-precedence guard: collect subs into a staging vec first. If any
        // producer parked, install the combined park *before* submitting the subs to
        // the scheduler — submitting would leak nodes on the re-Dispatch wake path.
        let wrap_set = &resolved.slots.wrap_indices;
        let ref_name_set = &resolved.slots.ref_name_indices;
        let eager_filter = resolved.slots.eager_indices.as_deref();
        let mut new_parts: Vec<Spanned<ExpressionPart<'a>>> =
            Vec::with_capacity(expr.parts.len());
        let mut producers_to_wait: Vec<NodeId> = Vec::new();
        // Pending subs: queued submissions, applied only after the park check.
        // `PendingSub` discriminates the four schedulable shapes so we don't run
        // `self.add(...)` until producers_to_wait is finalized.
        enum PendingSub<'a> {
            Reuse(NodeId),
            Dispatch(KExpression<'a>),
            ListLit(Vec<ExpressionPart<'a>>),
            DictLit(Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>),
        }
        let mut staged_subs: Vec<(usize, PendingSub<'a>)> = Vec::new();
        for (i, part) in expr.parts.into_iter().enumerate() {
            let span = part.span;
            // Pre-sub splice (binder recursive submission): reuse the NodeId.
            if let Some(&(_, sub_id)) = pre_subs.iter().find(|(j, _)| *j == i) {
                staged_subs.push((i, PendingSub::Reuse(sub_id)));
                new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                continue;
            }
            // Wrap slot: splice Resolved → Future, park Parked, surface Unbound.
            // Cycle detection runs here (the cache is built with `consumer = None`):
            // a `Parked(p)` outcome where `would_create_cycle(p, idx)` is a wake
            // cycle (e.g. `LET y = y` where `y`'s value slot self-parks on the LET
            // slot itself); surface `SchedulerDeadlock` rather than installing the
            // cycle-creating park edge.
            if wrap_set.contains(&i) {
                match &bare_outcomes[i] {
                    Some(NameOutcome::Resolved(obj)) => {
                        new_parts.push(Spanned { value: ExpressionPart::Future(obj), span });
                    }
                    Some(NameOutcome::Parked(p)) => {
                        if self.deps.would_create_cycle(*p, NodeId(idx)) {
                            let name = bare_name_of(&part.value).unwrap_or_default();
                            return Ok(NodeStep::Done(NodeOutput::Err(KError::new(
                                KErrorKind::SchedulerDeadlock {
                                    pending: 1,
                                    sample: format!("cycle in type alias `{name}`"),
                                },
                            ))));
                        }
                        if !producers_to_wait.contains(p) {
                            producers_to_wait.push(*p);
                        }
                        new_parts.push(Spanned { value: part.value, span });
                    }
                    Some(NameOutcome::Unbound(name)) => {
                        // Match the pre-PR-C surface: an unbound wrap-slot name was a
                        // slot terminal (`BodyResult::Err(UnboundName)`), not a
                        // propagated scheduler error. Parent slots (MODULE / FN / LET
                        // binders) catch that terminal through their Combine's
                        // dep-error short-circuit; surfacing as `Err` from `execute`
                        // would break that catch.
                        return Ok(NodeStep::Done(NodeOutput::Err(KError::new(
                            KErrorKind::UnboundName(name.clone()),
                        ))));
                    }
                    Some(NameOutcome::Cycle(_)) => {
                        // Cache was built with `consumer = None`, so `Cycle` is never
                        // produced; defensive arm only.
                        unreachable!("cache built with consumer=None never yields Cycle");
                    }
                    Some(NameOutcome::ProducerErrored(_)) => {
                        unreachable!("ProducerErrored short-circuited upfront in Step 2");
                    }
                    None => {
                        debug_assert!(false, "wrap_indices implies bare-name part");
                        new_parts.push(Spanned { value: part.value, span });
                    }
                }
                continue;
            }
            // Ref-name slot: literal-name slots keep the bare token; only the park
            // outcome matters here. Same cycle-detection guard as the wrap-slot arm.
            if ref_name_set.contains(&i) {
                let park_eligible = matches!(
                    &part.value,
                    ExpressionPart::Identifier(_)
                ) || matches!(
                    &part.value,
                    ExpressionPart::Type(t) if matches!(t.params, TypeParams::None)
                );
                if park_eligible {
                    if let Some(NameOutcome::Parked(p)) = &bare_outcomes[i] {
                        if self.deps.would_create_cycle(*p, NodeId(idx)) {
                            let name = bare_name_of(&part.value).unwrap_or_default();
                            return Ok(NodeStep::Done(NodeOutput::Err(KError::new(
                                KErrorKind::SchedulerDeadlock {
                                    pending: 1,
                                    sample: format!("cycle in type alias `{name}`"),
                                },
                            ))));
                        }
                        if !producers_to_wait.contains(p) {
                            producers_to_wait.push(*p);
                        }
                    }
                }
                new_parts.push(Spanned { value: part.value, span });
                continue;
            }
            // Eager-sub slot: schedule a sub-Dispatch (or aggregate). Filtered by
            // `eager_filter` when the picked function is a lazy candidate.
            let in_eager_filter = eager_filter.is_none_or(|idxs| idxs.contains(&i));
            if in_eager_filter {
                match part.value {
                    ExpressionPart::Expression(boxed) => {
                        staged_subs.push((i, PendingSub::Dispatch(*boxed)));
                        new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                        continue;
                    }
                    ExpressionPart::SigiledTypeExpr(boxed) => {
                        let wrapped = KExpression::new(vec![Spanned::bare(
                            ExpressionPart::SigiledTypeExpr(boxed),
                        )]);
                        staged_subs.push((i, PendingSub::Dispatch(wrapped)));
                        new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                        continue;
                    }
                    ExpressionPart::ListLiteral(items) => {
                        staged_subs.push((i, PendingSub::ListLit(items)));
                        new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                        continue;
                    }
                    ExpressionPart::DictLiteral(pairs) => {
                        staged_subs.push((i, PendingSub::DictLit(pairs)));
                        new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                        continue;
                    }
                    other => new_parts.push(Spanned { value: other, span }),
                }
            } else {
                new_parts.push(Spanned { value: part.value, span });
            }
        }
        let new_expr = KExpression::new(new_parts);
        // Park precedence: if any producer parked, install the combined park before
        // submitting any subs (otherwise the re-Dispatch on wake would re-stage them
        // and the original sub-nodes would leak).
        if !producers_to_wait.is_empty() {
            return Ok(self.install_combined_park(producers_to_wait, new_expr, pre_subs, idx));
        }
        // No park — submit the staged subs and build a Bind slot (or bind directly).
        let mut subs: Vec<(usize, NodeId)> = Vec::with_capacity(staged_subs.len());
        for (i, pending) in staged_subs {
            let node_id = match pending {
                PendingSub::Reuse(id) => id,
                PendingSub::Dispatch(expr) => self.add(NodeWork::dispatch(expr), scope),
                PendingSub::ListLit(items) => self.schedule_list_literal(items, scope),
                PendingSub::DictLit(pairs) => self.schedule_dict_literal(pairs, scope),
            };
            subs.push((i, node_id));
        }
        if subs.is_empty() {
            // No subs: bind the picked function directly. Spliced `Future(&'a KObject)`
            // references survive `results[dep] = None` because the objects live in
            // arenas tied to lexical scope.
            match resolved.function.bind(new_expr) {
                Ok(future) => Ok(self.invoke_to_step(future, scope, idx)),
                Err(e) => Ok(NodeStep::Done(NodeOutput::Err(e))),
            }
        } else {
            let bind_id = self.add(NodeWork::Bind { expr: new_expr, subs }, scope);
            Ok(self.defer_to_lift(idx, bind_id))
        }
    }

    /// Eager-only schedule used by the `Deferred` arm of `run_dispatch`. No picked
    /// function, no eager filter, no `bare_outcomes` consultation: bare names ride
    /// the re-dispatch after the subs land. Schedule every `Expression` /
    /// `SigiledTypeExpr` / `ListLiteral` / `DictLiteral` part as a sub-Dispatch (or
    /// aggregate) and build a `Bind` slot.
    ///
    /// `Deferred ⇒ at least one eager part`, so the empty-subs branch would be a
    /// `resolve_dispatch` invariant break — `debug_assert!` would catch it but the
    /// real protection is the caller's surface contract.
    fn schedule_eager_only(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        let mut new_parts = Vec::with_capacity(expr.parts.len());
        let mut subs: Vec<(usize, NodeId)> = Vec::new();
        for (i, part) in expr.parts.into_iter().enumerate() {
            let span = part.span;
            match part.value {
                ExpressionPart::Expression(boxed) => {
                    let sub_id = self.add(NodeWork::dispatch(*boxed), scope);
                    subs.push((i, sub_id));
                    new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                }
                ExpressionPart::SigiledTypeExpr(boxed) => {
                    let wrapped = KExpression::new(vec![Spanned::bare(
                        ExpressionPart::SigiledTypeExpr(boxed),
                    )]);
                    let sub_id = self.add(NodeWork::dispatch(wrapped), scope);
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
        debug_assert!(
            !subs.is_empty(),
            "schedule_eager_only invoked from Deferred arm; resolve_dispatch contract \
             requires at least one eager part"
        );
        let bind_id = self.add(NodeWork::Bind { expr: new_expr, subs }, scope);
        self.defer_to_lift(idx, bind_id)
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
        //
        // Re-Dispatch rebuilds the carrier in `Initialized` — the slot re-enters
        // the driver as if it were a fresh submission, modulo the preserved
        // `pre_subs`. Step 1 of the stateful-dispatch refactor keeps this
        // re-classify-on-wake behavior; later steps cache the classified shape
        // in a per-variant state so wakes can skip re-classification.
        NodeStep::Replace {
            work: NodeWork::Dispatch {
                expr,
                state: DispatchState::initialized(pre_subs),
            },
            frame: None,
            function: None,
            block_entry: None,
            body_index: 0,
        }
    }

    /// Park `idx` on each still-pending producer and rebuild it as a re-Dispatch of `expr`.
    /// Shares the producer-error / cycle / install guards with the fused splice/park walk
    /// in `run_dispatch`: a producer that already terminalized with an error propagates
    /// (parking on a dead slot would deadlock); one that would close a cycle is skipped;
    /// if no parkable producer remains, the call is a genuine no-match. On wake the
    /// re-Dispatch rebuilds the bare-name cache so strict admission can read the now-bound
    /// type. Drives the [`ResolveOutcome::ParkOnProducers`] path from both `run_dispatch`
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

    /// Fast lane for `DispatchShape::BareIdentifier`. Resolves the name against the
    /// dispatching scope. `Some(step)` fires on `Value` (terminate with the bound value)
    /// or `Placeholder` (install park edge, rewrite the slot to a `Lift`). `Unbound`
    /// returns `None`, letting the caller fall through to the keyworded path so
    /// `value_lookup::body_identifier` produces the structured `UnboundName` error.
    ///
    /// The Lift transition is unique to this single-bare-name short-circuit (no other
    /// phase rewrites the slot to a Lift on park) — combined-park phases call
    /// [`Self::install_combined_park`] instead, which keeps the slot as a Dispatch and
    /// re-runs the full pipeline on wake.
    ///
    /// This is the post-classifier home for the old `try_short_circuit` — folded into
    /// the shape-driven dispatch routing per the unified-walk roadmap.
    fn fast_lane_bare_identifier(
        &mut self,
        name: &str,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Option<NodeStep<'a>> {
        // Chain-gated: a later-sibling binding is invisible to this consumer, so
        // a name that lexically does not yet exist falls through to the standard
        // dispatch / `UnboundName` path rather than short-circuiting on a hidden
        // value.
        match scope.resolve_with_chain(name, self.active_chain.as_deref()) {
            Resolution::Value(obj) => Some(NodeStep::Done(NodeOutput::Value(obj))),
            Resolution::Placeholder(producer_id) => {
                // Notify edge, not Owned: the producer is a sibling slot this Lift only
                // parks on for a wake — it is not part of this slot's reclaim subtree.
                // `add_park_edge` installs the forward wake on `notify_list[producer]`
                // and bumps `pending_deps[idx]` in the same atomic body; `free` skips
                // past Notify edges via `owned_children`. Producer-not-terminal
                // precondition: `Resolution::Placeholder` is only returned between
                // submission and terminalization of the placeholder's slot, so
                // `producer_id` is not yet terminal here.
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
    }

    /// Fast lane for `DispatchShape::BareTypeLeaf` (`(Number)`, `(IntOrd)`, etc.).
    /// Routes through `coerce_type_token_value` so the dispatch-phase carrier matches
    /// what `value_lookup::body_type_expr` would synthesize — `KTypeValue` for builtin
    /// leaves and aliases, paired carrier (`KSignature` / `KModule` / `StructType` /
    /// `TaggedUnionType`) for nominal identities via the paired-carrier lookup.
    ///
    /// `UnboundName` surfaces directly here rather than falling back: bare leaf-Type
    /// dispatch has no candidate-machinery alternative, and the candidate path would
    /// surface the same `UnboundName` after a wasted bucket walk.
    fn fast_lane_bare_type_leaf(
        &mut self,
        t: &TypeExpr,
        scope: &'a Scope<'a>,
    ) -> NodeStep<'a> {
        let chain = self.active_chain.as_deref();
        match coerce_type_token_value(scope, t, chain) {
            Ok(obj) => NodeStep::Done(NodeOutput::Value(obj)),
            Err(KError { kind: KErrorKind::UnboundName(n), .. }) => {
                NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::UnboundName(n))))
            }
            Err(e) => NodeStep::Done(NodeOutput::Err(e)),
        }
    }

    /// Fast lane for `DispatchShape::FunctionValueCall` — Identifier-headed calls of
    /// the surface form `f (...)` where `f` resolves to a `KFunction`, a `StructType`,
    /// or a `TaggedUnionType` carrier. Phase 1 of the `call_by_name` subsumption
    /// (`roadmap/dispatch_fix/unified-walk.md`): this handler now covers every outcome
    /// the deleted `call_by_name` builtin used to serve.
    ///
    /// **Admission rule for the call shape.** `expr.parts[1..]` must be exactly
    /// `[Spanned(Expression(inner))]` — a single nested-parens body. Named-argument
    /// calls are the only valid `FunctionValueCall` surface; koan has no `f 1 2`
    /// positional call syntax for function values. Anything else (bare positional
    /// arg, multiple parts) surfaces `DispatchFailed`, matching the keyworded-path
    /// surface the `call_by_name` typed-slot bind used to produce.
    ///
    /// **Head resolution branches (four admission types per D1.1 of the plan):**
    /// - `KFunction(f, _)` → reconstruct the positional expression via
    ///   [`KFunction::reconstruct_positional`] (interleaves signature `Keyword`
    ///   elements between picked-by-name argument values) and dispatch through
    ///   `schedule_picked_eager` with the picked function. Any error from
    ///   reconstruction (`MissingArg`, `ShapeError` for malformed / unknown / duplicate)
    ///   surfaces directly as `NodeOutput::Err`.
    /// - `StructType { .. }` → [`struct_value::apply`] returns a `BodyResult::Tail`
    ///   re-dispatching through the `struct_construct` primitive. Tail expressions
    ///   become a `NodeWork::Dispatch` replacement on this slot, identical to how
    ///   `run_combine` / `invoke_to_step` decode `BodyResult::Tail`.
    /// - `TaggedUnionType { .. }` → same shape via [`tagged_union::apply`] → the
    ///   `tagged_union_construct` primitive.
    /// - Anything else (`KNumber`, `KString`, `Bool`, instance `Struct`, `Module`, …)
    ///   → `TypeMismatch { arg: "verb", expected: "KFunction or Type", got }` — same
    ///   wording the deleted `call_by_name` body produced.
    ///
    /// **Forward-reference park** (`Placeholder(producer)`) installs a combined park
    /// and rebuilds this slot as a re-Dispatch; on wake the fast lane re-runs against
    /// the now-bound carrier.
    ///
    /// **Unbound head** surfaces `UnboundName(name)` directly — D1.2: no more
    /// fall-through to Keyworded for this shape.
    fn fast_lane_function_value_call(
        &mut self,
        expr: &KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        // Classifier guarantees expr.parts[0] is a lowercase Identifier.
        let head = match &expr.parts[0].value {
            ExpressionPart::Identifier(n) => n.clone(),
            _ => unreachable!("FunctionValueCall shape implies Identifier head"),
        };
        let chain = self.active_chain.as_deref();
        match scope.resolve_with_chain(&head, chain) {
            Resolution::Value(obj) => self.dispatch_callable_value(expr, obj, scope, idx),
            Resolution::Placeholder(producer_id) => {
                // Forward-reference park: install a park edge and rebuild this slot as
                // a re-Dispatch so the now-bound carrier reaches the fast lane on
                // wake. Uses the same combined-park machinery as Phase 3.
                self.install_combined_park(vec![producer_id], expr.clone(), Vec::new(), idx)
            }
            Resolution::UnboundName => NodeStep::Done(NodeOutput::Err(KError::new(
                KErrorKind::UnboundName(head),
            ))),
        }
    }

    /// Branch on the resolved head carrier of a `FunctionValueCall`. Split out from
    /// [`Self::fast_lane_function_value_call`] so the head-resolution match arm stays
    /// readable; per D1.1 of `scratch/plan-fast-lane-subsume.md` only three carrier
    /// shapes admit, and everything else surfaces a `TypeMismatch` with the wording
    /// the deleted `call_by_name` body produced.
    fn dispatch_callable_value(
        &mut self,
        expr: &KExpression<'a>,
        head_obj: &'a KObject<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        // Extract the inner parts from the single-nested-parens body. Anything else
        // is not a koan call shape — surface `DispatchFailed` to match today's
        // keyworded-path bind-error surface.
        let inner_parts = match extract_named_call_inner(expr) {
            Ok(parts) => parts,
            Err(e) => return NodeStep::Done(NodeOutput::Err(e)),
        };
        match head_obj {
            KObject::KFunction(f, _) => {
                // D1.4: error precedence (missing → unknown → malformed) is enforced
                // by `reconstruct_positional`; propagate its `Err(KError)` directly
                // rather than running a separate admission check first. The bind step
                // re-validates types per arg.
                match f.reconstruct_positional(inner_parts) {
                    Ok(rebuilt) => self.schedule_picked_eager(rebuilt, f, scope, idx),
                    Err(e) => NodeStep::Done(NodeOutput::Err(e)),
                }
            }
            KObject::StructType { .. } => {
                self.schedule_constructor_tail(struct_value::apply(head_obj, inner_parts))
            }
            KObject::TaggedUnionType { .. } => {
                self.schedule_constructor_tail(tagged_union::apply(head_obj, inner_parts))
            }
            other => NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "KFunction or Type".to_string(),
                got: other.summarize(),
            }))),
        }
    }

    /// Fast lane for `DispatchShape::SigiledTypeExpr` — the `:(...)` parse-context
    /// marker. Tail-replaces this slot with a `Dispatch` of the inner `KExpression`,
    /// so the inner expression runs through the same classifier and produces the same
    /// carrier shape any other dispatch site does.
    ///
    /// The inner classifier sees:
    /// - `Keyworded` for new keyworded shapes (`:(LIST OF Number)`,
    ///   `:(MAP Str -> Number)`, `:(FN (x :Number) -> Bool)`, `:(FUNCTOR (T :S) -> M)`)
    ///   served by the registered `LIST OF` / `MAP _ -> _` / `FN` / `FUNCTOR` overloads
    ///   (see [`crate::builtins::type_constructors`]).
    /// - `TypeCall` for legacy positional inputs (`:(List Number)`,
    ///   `:(Dict Str Number)`) served by `resolve_type_expr` — preserved for source
    ///   compatibility with annotations that haven't migrated to the keyworded form.
    /// - `BareTypeLeaf` for single-name sigils (`:(Number)`).
    /// - `BareIdentifier` for sigiled identifier references that resolve to a
    ///   type-side carrier through the standard bare-name path.
    /// - `FunctionValueCall` for user-functor application
    ///   (`:(MyFunctor (T = IntOrd))`) — the head `MyFunctor` resolves to a `KFunction`
    ///   carrier and the value-side `FunctionValueCall` machinery handles the kwarg
    ///   bind exactly as for any other function value.
    ///
    /// The sigil boundary — "the returned carrier must be type-side
    /// (`KTypeValue` / `Module` / `Signature` / `UserType` / `KFunctor`)" — is
    /// enforced by the consumer slot's KType check at Bind / Combine. A
    /// value-side carrier (number, instance struct, plain function value) in a
    /// sigil slot reaches a TypeExprRef / Type / Any{Module,Signature} slot
    /// and surfaces a standard `TypeMismatch`. No dedicated boundary tail is
    /// needed at the sigil itself; the existing slot-type machinery does the
    /// job.
    fn fast_lane_sigiled_type_expr(
        &mut self,
        inner: KExpression<'a>,
        _scope: &'a Scope<'a>,
        _idx: usize,
    ) -> NodeStep<'a> {
        // Tail-replace this slot with a Dispatch of the inner expression. No
        // frame / function / block_entry — the sigil itself is scope-neutral; the
        // inner expression carries whatever context it needs.
        NodeStep::Replace {
            work: NodeWork::dispatch(inner),
            frame: None,
            function: None,
            block_entry: None,
            body_index: 0,
        }
    }

    /// Decode a constructor `BodyResult` returned by `struct_value::apply` /
    /// `tagged_union::apply` into a `NodeStep`. `Tail(expr)` rewrites the slot as a
    /// `Dispatch(expr)` re-dispatch through the construction primitive — same
    /// `BodyResult::Tail` decode the `run_combine` / `invoke_to_step` paths use, but
    /// without a `frame` / `function` / `block_entry` (constructors are scope-neutral
    /// builtin tails). `Value` / `DeferTo` are unreachable for these constructor
    /// `apply` helpers but handled defensively to avoid a future-bug-hidden-by-panic.
    fn schedule_constructor_tail(&mut self, body: BodyResult<'a>) -> NodeStep<'a> {
        match body {
            BodyResult::Tail { expr, frame, function, block_entry, body_index } => {
                NodeStep::Replace {
                    work: NodeWork::dispatch(expr),
                    frame,
                    function,
                    block_entry,
                    body_index,
                }
            }
            BodyResult::Value(v) => NodeStep::Done(NodeOutput::Value(v)),
            BodyResult::DeferTo(_) => NodeStep::Done(NodeOutput::Err(KError::new(
                KErrorKind::ShapeError(
                    "constructor apply returned DeferTo (scheduler invariant break)".to_string(),
                ),
            ))),
            BodyResult::Err(e) => NodeStep::Done(NodeOutput::Err(e)),
        }
    }

    /// Phase 4. Schedule eager sub-Dispatches for `Expression` / `ListLiteral` /
    /// Schedule eager sub-Dispatches for a reconstructed-positional expression and
    /// build a `Bind` slot (or bind directly if no eager parts schedule). Used by the
    /// `fast_lane_function_value_call` arm to dispatch a kwarg-reconstructed
    /// expression against a `KFunction` head. No eager filter (every Expression /
    /// SigiledTypeExpr / ListLiteral / DictLiteral part schedules) and no
    /// bare-name cache consultation — the reconstructed expression's bare names
    /// (if any) ride the standard sub-Dispatch wrap-slot path.
    fn schedule_picked_eager(
        &mut self,
        expr: KExpression<'a>,
        picked: &'a crate::machine::core::kfunction::KFunction<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        let mut new_parts = Vec::with_capacity(expr.parts.len());
        let mut subs: Vec<(usize, NodeId)> = Vec::new();
        for (i, part) in expr.parts.into_iter().enumerate() {
            let span = part.span;
            match part.value {
                ExpressionPart::Expression(boxed) => {
                    let sub_id = self.add(NodeWork::dispatch(*boxed), scope);
                    subs.push((i, sub_id));
                    new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                }
                ExpressionPart::SigiledTypeExpr(boxed) => {
                    let wrapped = KExpression::new(vec![Spanned::bare(
                        ExpressionPart::SigiledTypeExpr(boxed),
                    )]);
                    let sub_id = self.add(NodeWork::dispatch(wrapped), scope);
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
            match picked.bind(new_expr) {
                Ok(future) => self.invoke_to_step(future, scope, idx),
                Err(e) => NodeStep::Done(NodeOutput::Err(e)),
            }
        } else {
            let bind_id = self.add(NodeWork::Bind { expr: new_expr, subs }, scope);
            self.defer_to_lift(idx, bind_id)
        }
    }

    /// Stateful dispatch driver. Step 1 of the stateful-dispatch refactor:
    /// classify the slot's shape, transition `Initialized → <variant>`,
    /// then delegate to the legacy `run_dispatch` for the actual step. No
    /// behavior change — the per-variant transition exists only to exercise
    /// the carrier wiring later steps fill in. Steps 3+ replace each
    /// variant's delegation with a real per-variant handler.
    ///
    /// Reached only when `self.use_stateful_dispatch` is `true` — see the
    /// `NodeWork::Dispatch` arm of [`Scheduler::execute`].
    pub(super) fn run_dispatch_stateful(
        &mut self,
        expr: KExpression<'a>,
        state: DispatchState<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        // Step 2 drains the wake side-channel on entry so producers
        // that fired since the slot's last poll don't accumulate across
        // re-park. The per-variant handlers introduced in step 3+ read
        // these wakes to pick per-edge callbacks; step 2's classify-
        // and-delegate stub discards the list. The drain still runs
        // unconditionally so the side-channel never grows stale —
        // `take_recent_wakes` resets the slot's Vec to empty in O(1).
        // See `roadmap/dispatch_fix/stateful-dispatch-02-recent-wakes.md`.
        let _wakes = self.store.take_recent_wakes(NodeId(idx));
        // Step 1 contract: a Dispatch slot enters the stateful driver in
        // `Initialized`. The park-rebuild sites all reconstruct as
        // `Initialized`, so per-variant states are not yet reachable; the
        // `_ => unreachable!` arm guards the invariant until step 2/3 wires
        // up re-entry from a parked per-variant state.
        let init = match state {
            DispatchState::Initialized(i) => i,
            _ => unreachable!(
                "stateful dispatch step 1: only Initialized is reachable; \
                 per-variant states are introduced in step 3+"
            ),
        };
        // Classify and transition. The per-variant state structs are empty
        // in step 1, but constructing them now exercises the variant-
        // transition shape later steps depend on. The `_classified`
        // binding is intentionally unused — it documents the transition
        // wiring later steps fill in (when each arm delegates to a real
        // per-variant handler instead of falling through to
        // `run_dispatch`).
        //
        // `pre_subs.clone()` here pays one Vec clone per stateful dispatch
        // entry; step 1 accepts this cost because the legacy delegate
        // below still needs the original. Step 3+ moves `init` into the
        // per-variant state and the clone goes away.
        let _classified: DispatchState<'a> = match classify_dispatch_shape(&expr) {
            DispatchShape::BareIdentifier => DispatchState::BareIdentifier(
                BareIdState::from_init(Initialized { pre_subs: init.pre_subs.clone() }),
            ),
            DispatchShape::BareTypeLeaf => DispatchState::BareTypeLeaf(
                BareTypeState::from_init(Initialized { pre_subs: init.pre_subs.clone() }),
            ),
            DispatchShape::TypeConstructorCall => DispatchState::TypeConstructorCall(
                TyCtorState::from_init(Initialized { pre_subs: init.pre_subs.clone() }),
            ),
            DispatchShape::FunctionValueCall => DispatchState::FunctionValueCall(
                FnValueState::from_init(Initialized { pre_subs: init.pre_subs.clone() }),
            ),
            DispatchShape::SigiledTypeExpr => DispatchState::SigiledTypeExpr(
                SigilState::from_init(Initialized { pre_subs: init.pre_subs.clone() }),
            ),
            DispatchShape::Keyworded => DispatchState::Keyworded(
                KeywordedState::from_init(Initialized { pre_subs: init.pre_subs.clone() }),
            ),
        };
        // All variants delegate to the legacy driver in step 1. `pre_subs`
        // rides through unchanged from the original `Initialized`.
        self.run_dispatch(expr, init.pre_subs, scope, idx)
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_name_part;
    use crate::machine::NameOutcome;
    use super::super::super::nodes::{LiftState, NodeOutput, NodeWork};
    use crate::builtins::default_scope;
    use crate::machine::core::source::Spanned;
    use crate::machine::execute::Scheduler;
    use crate::machine::model::ast::{ExpressionPart, KExpression, TypeExpr};
    use crate::machine::model::{KObject, KType};
    use crate::machine::{BindingIndex, NodeId, RuntimeArena};

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

    /// Builder-toggle smoke test: `with_stateful_dispatch(true)` routes the
    /// dispatch arm through `run_dispatch_stateful` without requiring the
    /// `KOAN_STATEFUL_DISPATCH` env var. The trivial program `LET x = 1`
    /// runs to a value under the new driver — proving the classify-and-
    /// delegate stub doesn't lose any state crossing the toggle boundary.
    /// This is step 1's "cheap insurance against the env-var read silently
    /// failing" — not a behavioral acceptance criterion (toggle-on whole-
    /// suite parity is the acceptance gate).
    #[test]
    fn builder_toggle_routes_through_stateful_driver() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new().with_stateful_dispatch(true);
        assert!(sched.use_stateful_dispatch);
        let exprs = crate::parse::parse("LET x = 1").expect("parse succeeds");
        for e in exprs {
            sched.add_dispatch(e, scope);
        }
        sched.execute().expect("LET x = 1 runs cleanly under the stateful toggle");
        // LET binds; the value lands in the scope, the slot's terminal is
        // not what this test guards — just that the program terminalized
        // without the toggle dropping its state.
        assert!(matches!(scope.lookup("x"), Some(KObject::Number(n)) if *n == 1.0));
    }

    /// Step 2 of the stateful-dispatch refactor: the `recent_wakes`
    /// side-channel is `Dispatch`-only. A non-`Dispatch` consumer (here
    /// a `Lift(Pending(producer))` slot) parked on a `Dispatch`
    /// producer must drain to an empty Vec — `push_recent_wake` filters
    /// non-Dispatch work via the same peek-discriminator pattern as
    /// `stamp_lift_ready`. The Lift's stamp-then-enqueue path stays
    /// intact (asserted indirectly through full-suite parity).
    #[test]
    fn recent_wakes_empty_for_non_dispatch_consumer() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let producer = sched.add_dispatch(
            KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("_p".into()))]),
            scope,
        );
        // Lift consumer parked on the Dispatch producer. We use the
        // submission path to install both the slot and a park edge —
        // identical shape to the fast-lane bare-identifier short-circuit
        // that produces this Lift carrier in practice.
        let consumer = sched.add(
            NodeWork::Lift(LiftState::Pending(producer)),
            scope,
        );
        // The Lift was installed via `add` which already wired the
        // Owned read-dep; finalize must drain the notify edge. Drive
        // finalize directly with a synthetic Value so the test is
        // independent of the producer's expression body.
        let value = arena.alloc(KObject::Number(1.0));
        sched.finalize(producer.index(), NodeOutput::Value(value));
        // Non-Dispatch consumer: `push_recent_wake` no-ops, so the
        // side-channel stays empty.
        assert!(sched.store.take_recent_wakes(consumer).is_empty());
    }

    /// Step 2: a `Dispatch` consumer parked on a `Dispatch` producer
    /// records the producer's `NodeId` in `recent_wakes` when the
    /// producer finalizes. The drained list is what step 3+ will key
    /// per-edge callbacks off; step 2's `run_dispatch_stateful` still
    /// discards it (the classify-and-delegate stub falls through to
    /// the legacy driver).
    #[test]
    fn recent_wakes_records_producer_for_dispatch_consumer() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let producer = sched.add_dispatch(
            KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("_p".into()))]),
            scope,
        );
        let consumer = sched.add_dispatch(
            KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("_c".into()))]),
            scope,
        );
        // Park the consumer on the producer with a `Notify` edge — the
        // shape `install_combined_park` would install on a forward-
        // reference re-Dispatch wake path.
        sched.deps.add_park_edge(producer, consumer);
        // Finalize the producer with a synthetic Value. The notify-walk
        // drains the edge and fans out: side-channel append for every
        // consumer plus queue push for counter-zero consumers.
        let value = arena.alloc(KObject::Number(1.0));
        sched.finalize(producer.index(), NodeOutput::Value(value));
        let wakes = sched.store.take_recent_wakes(consumer);
        assert_eq!(wakes, vec![producer]);
        // Repeat-drain is empty: `take_recent_wakes` resets the Vec on
        // each call.
        assert!(sched.store.take_recent_wakes(consumer).is_empty());
    }

    /// Sanity: a `NodeId` constructed via `add_dispatch` indexes the
    /// freshly-grown `recent_wakes` slot — drain on a never-woken
    /// Dispatch returns empty.
    #[test]
    fn recent_wakes_drain_default_empty() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let id: NodeId = sched.add_dispatch(
            KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("_".into()))]),
            scope,
        );
        assert!(sched.store.take_recent_wakes(id).is_empty());
    }
}
