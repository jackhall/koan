use crate::builtins::newtype_def::newtype_construct;
use crate::builtins::value_lookup::coerce_type_token_value;
use crate::builtins::{dispatch_constructor, struct_value, tagged_union};
use crate::machine::model::types::UserTypeKind;
use crate::machine::core::source::Spanned;
use crate::machine::model::{KObject, KType, Parseable};
use crate::machine::{
    BindingIndex, Frame, KError, KErrorKind, NameOutcome, NodeId, ResolveOutcome, Resolution, Scope,
};
use crate::machine::core::kfunction::BodyResult;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeExpr, TypeParams};

use super::super::nodes::{LiftState, NodeOutput, NodeStep, NodeWork};
use super::dispatch_state::{
    BareNameParkTrack, DispatchState, EagerSubsTrack, FnValueHeadPlaceholderTrack, FnValueState,
    KeywordedState, OverloadParkTrack,
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
    ConstructorCall,
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
/// Multi-part fast-lane: head is leaf `Type` → `ConstructorCall` (the legacy
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
            // classification on the inner expression. See the
            // `DispatchShape::SigiledTypeExpr` arm of `run_dispatch`.
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
            // Head is a leaf `Type` → `ConstructorCall`. The legacy positional
            // `(List Number)` shape (leaf-Type-only args) used to route through a
            // separate `TypeCall` arm that elaborated `TypeExpr { params: List(_) }`;
            // that arm is deleted now that the keyworded `LIST OF` / `MAP _ -> _` /
            // `FN` / `FUNCTOR` overloads serve every parameterized-type form.
            DispatchShape::ConstructorCall
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

/// One staged submission queued by [`Scheduler::keyworded_part_walk`]. The walk
/// collects these into a staging Vec so the park-precedence guard runs before
/// any sub-node hits the scheduler (otherwise a producer-parked dispatch would
/// leak the eager sub-nodes on the re-Dispatch wake path). The four variants
/// match the four schedulable shapes the walk recognizes: a recorded sub-Dispatch
/// reuse (binder recursive submission), a fresh `Dispatch` (`Expression` /
/// `SigiledTypeExpr` part), or a list / dict aggregate.
pub(in crate::machine::execute) enum PendingSub<'a> {
    Reuse(NodeId),
    Dispatch(KExpression<'a>),
    ListLit(Vec<ExpressionPart<'a>>),
    DictLit(Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>),
}

/// Result of a successful [`Scheduler::keyworded_part_walk`]. The fields land in
/// the stateful Keyworded driver's one-shot path; the three buckets feed
/// the splice / park / eager-sub install respectively.
///
/// - `new_parts`: post-splice parts with `Future(obj)` substituted for resolved
///   wrap slots and bare placeholder strings substituted for scheduled subs.
/// - `producers_to_wait`: producers the slot must park on before the picked
///   function can bind. Deduplicated.
/// - `staged_subs`: per-index submissions, applied only after the park check.
pub(in crate::machine::execute) struct PartWalkResult<'a> {
    pub new_parts: Vec<Spanned<ExpressionPart<'a>>>,
    pub producers_to_wait: Vec<NodeId>,
    pub staged_subs: Vec<(usize, PendingSub<'a>)>,
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

/// Shape a dep-error terminal with the `<bind>` surface frame keyed off
/// `working_expr`. Shared by every eager-subs install / resume site so the
/// surface stays identical with `run_bind`'s `reclaim_deps` propagation.
fn bind_frame_err<'a>(e: &KError, working_expr: &KExpression<'a>) -> NodeStep<'a> {
    let frame = Frame::from_expr("<bind>", working_expr);
    NodeStep::Done(NodeOutput::Err(propagate_dep_error(e, Some(frame))))
}

/// Walk raw parts emitting an `Identifier("")` placeholder at every eager
/// slot (`Expression` / `SigiledTypeExpr` / `ListLiteral` / `DictLiteral`)
/// and a parallel staged-subs Vec; non-eager parts pass through unchanged.
/// Shared by the Keyworded-Deferred arm and the FunctionValueCall fast
/// lane — both treat *every* eager part as a sub (no slot filter).
fn stage_all_eager_parts<'a>(
    parts: Vec<Spanned<ExpressionPart<'a>>>,
) -> (Vec<Spanned<ExpressionPart<'a>>>, Vec<(usize, PendingSub<'a>)>) {
    let mut new_parts: Vec<Spanned<ExpressionPart<'a>>> = Vec::with_capacity(parts.len());
    let mut staged: Vec<(usize, PendingSub<'a>)> = Vec::new();
    for (i, part) in parts.into_iter().enumerate() {
        let span = part.span;
        match part.value {
            ExpressionPart::Expression(boxed) => {
                staged.push((i, PendingSub::Dispatch(*boxed)));
                new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
            }
            ExpressionPart::SigiledTypeExpr(boxed) => {
                let wrapped = KExpression::new(vec![Spanned::bare(
                    ExpressionPart::SigiledTypeExpr(boxed),
                )]);
                staged.push((i, PendingSub::Dispatch(wrapped)));
                new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
            }
            ExpressionPart::ListLiteral(items) => {
                staged.push((i, PendingSub::ListLit(items)));
                new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
            }
            ExpressionPart::DictLiteral(pairs) => {
                staged.push((i, PendingSub::DictLit(pairs)));
                new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
            }
            other => new_parts.push(Spanned { value: other, span }),
        }
    }
    (new_parts, staged)
}

/// Outcome of [`Scheduler::install_eager_subs`]. The caller routes:
/// - `AllInline` — every sub terminalized at install time, no parking needed.
///   Keyworded finishes via `stateful_keyworded_finish` (re-resolve);
///   FunctionValueCall binds `picked` directly.
/// - `Parked` — at least one sub is in flight. Caller wraps the carried
///   track in the driver-specific state and uses [`Scheduler::replace_with_parked_dispatch`].
/// - `DepError` — an already-terminal sub errored. Already shaped as
///   `NodeStep::Done(Err)` with the `<bind>` surface frame; return as-is.
enum EagerSubsInstall<'a> {
    AllInline(KExpression<'a>),
    Parked(EagerSubsTrack<'a>),
    DepError(NodeStep<'a>),
}

impl<'a> Scheduler<'a> {
    /// Build the per-part `bare_outcomes` cache consulted by strict admission
    /// and the fused splice/park walk. One `resolve_name_part` per bare-name
    /// part (`Identifier` or leaf `Type`); non-bare-name parts get `None`. Built
    /// with `consumer = None` so cycle detection is deferred to the splice walk
    /// (which runs the check only on slots the picked function classifies as
    /// references) — see Step 4 of [`Self::run_dispatch`] for the rationale.
    pub(in crate::machine::execute) fn build_bare_outcomes(
        &self,
        parts: &[Spanned<ExpressionPart<'a>>],
        scope: &'a Scope<'a>,
    ) -> Vec<Option<NameOutcome<'a>>> {
        parts
            .iter()
            .map(|p| match &p.value {
                ExpressionPart::Identifier(_) => Some(resolve_name_part(scope, &p.value, self, None)),
                ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => {
                    Some(resolve_name_part(scope, &p.value, self, None))
                }
                _ => None,
            })
            .collect()
    }

    /// Fused splice / park / eager-sub walk over `parts`. Per part, exactly one
    /// arm fires:
    /// - Pre-sub splice (binder recursive submission): reuse the recorded NodeId.
    /// - Wrap slot: read `bare_outcomes[i]` — Resolved ⇒ rewrite to `Future(obj)`;
    ///   Parked ⇒ cycle-check then push producer; Unbound ⇒ slot-terminal
    ///   `UnboundName`.
    /// - Ref-name slot: read `bare_outcomes[i]` — Parked ⇒ cycle-check then push
    ///   producer; Resolved / Unbound ⇒ no-op (literal-name slot keeps the bare
    ///   token; the receiving builtin resolves it).
    /// - Eager-sub slot: stage a sub-Dispatch (`Expression` / `SigiledTypeExpr`)
    ///   or aggregate (`ListLiteral` / `DictLiteral`). Filtered by
    ///   `slots.eager_indices` when the picked function is a lazy candidate.
    ///
    /// Pure: no scheduler submission, no park-edge installation. Caller decides
    /// on the result whether to install a combined park (when
    /// `producers_to_wait` is non-empty) or submit the staged subs.
    ///
    /// `Err(KError)` here surfaces a *slot terminal* error (cycle / unbound
    /// wrap), not a scheduler-level error — callers wrap it as
    /// `NodeStep::Done(NodeOutput::Err(_))`.
    pub(in crate::machine::execute) fn keyworded_part_walk(
        &mut self,
        parts: Vec<Spanned<ExpressionPart<'a>>>,
        pre_subs: &[(usize, NodeId)],
        bare_outcomes: &[Option<NameOutcome<'a>>],
        slots: &crate::machine::core::kfunction::ClassifiedSlots,
        idx: usize,
    ) -> Result<PartWalkResult<'a>, KError> {
        let wrap_set = &slots.wrap_indices;
        let ref_name_set = &slots.ref_name_indices;
        let eager_filter = slots.eager_indices.as_deref();
        let mut new_parts: Vec<Spanned<ExpressionPart<'a>>> = Vec::with_capacity(parts.len());
        let mut producers_to_wait: Vec<NodeId> = Vec::new();
        let mut staged_subs: Vec<(usize, PendingSub<'a>)> = Vec::new();
        for (i, part) in parts.into_iter().enumerate() {
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
            // cycle; surface `SchedulerDeadlock` rather than installing the
            // cycle-creating park edge.
            if wrap_set.contains(&i) {
                match &bare_outcomes[i] {
                    Some(NameOutcome::Resolved(obj)) => {
                        new_parts.push(Spanned { value: ExpressionPart::Future(obj), span });
                    }
                    Some(NameOutcome::Parked(p)) => {
                        if self.deps.would_create_cycle(*p, NodeId(idx)) {
                            let name = bare_name_of(&part.value).unwrap_or_default();
                            return Err(KError::new(KErrorKind::SchedulerDeadlock {
                                pending: 1,
                                sample: format!("cycle in type alias `{name}`"),
                            }));
                        }
                        if !producers_to_wait.contains(p) {
                            producers_to_wait.push(*p);
                        }
                        new_parts.push(Spanned { value: part.value, span });
                    }
                    Some(NameOutcome::Unbound(name)) => {
                        // Pre-PR-C surface: an unbound wrap-slot name is a slot
                        // terminal (`BodyResult::Err(UnboundName)`), not a
                        // propagated scheduler error. Parent slots catch that
                        // terminal through their Combine's dep-error short-circuit;
                        // surfacing as `Err` from `execute` would break that catch.
                        return Err(KError::new(KErrorKind::UnboundName(name.clone())));
                    }
                    Some(NameOutcome::Cycle(_)) => {
                        unreachable!("cache built with consumer=None never yields Cycle");
                    }
                    Some(NameOutcome::ProducerErrored(_)) => {
                        unreachable!("ProducerErrored short-circuited upfront");
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
                let park_eligible = matches!(&part.value, ExpressionPart::Identifier(_))
                    || matches!(
                        &part.value,
                        ExpressionPart::Type(t) if matches!(t.params, TypeParams::None)
                    );
                if park_eligible {
                    if let Some(NameOutcome::Parked(p)) = &bare_outcomes[i] {
                        if self.deps.would_create_cycle(*p, NodeId(idx)) {
                            let name = bare_name_of(&part.value).unwrap_or_default();
                            return Err(KError::new(KErrorKind::SchedulerDeadlock {
                                pending: 1,
                                sample: format!("cycle in type alias `{name}`"),
                            }));
                        }
                        if !producers_to_wait.contains(p) {
                            producers_to_wait.push(*p);
                        }
                    }
                }
                new_parts.push(Spanned { value: part.value, span });
                continue;
            }
            // Eager-sub slot: stage a sub-Dispatch (or aggregate). Filtered by
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
        Ok(PartWalkResult { new_parts, producers_to_wait, staged_subs })
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

    /// Decode a constructor `BodyResult` returned by `struct_value::apply` /
    /// `tagged_union::apply` / `newtype_construct` / `dispatch_constructor` into a
    /// `NodeStep`. The three terminal-decode shapes all appear here:
    ///
    /// - `Tail(expr)` rewrites this slot as a `Dispatch(expr)` re-dispatch through
    ///   the construction primitive — same `BodyResult::Tail` decode the
    ///   `run_combine` / `invoke_to_step` paths use, but without a `frame` /
    ///   `function` / `block_entry` (constructors are scope-neutral builtin tails).
    /// - `Value(v)` terminalizes this slot directly.
    /// - `DeferTo(combine_id)` lifts this slot's terminal off the Combine the
    ///   construction primitive registered. `newtype_construct` returns this shape
    ///   (the value sub-expression is scheduled through `add_dispatch` and a
    ///   `Combine` wraps it after type-checking); routes through
    ///   [`Self::defer_to_lift`] to install the Owned read-dep and rewrite the
    ///   slot as a `Lift`.
    /// - `Err(e)` surfaces the construction error directly.
    fn schedule_constructor_body(&mut self, body: BodyResult<'a>, idx: usize) -> NodeStep<'a> {
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
            BodyResult::DeferTo(combine_id) => self.defer_to_lift(idx, combine_id),
            BodyResult::Err(e) => NodeStep::Done(NodeOutput::Err(e)),
        }
    }

    /// Stateful dispatch driver. Classifies the slot's shape and routes to
    /// the matching per-variant handler. Fast-lane variants
    /// (`BareTypeLeaf`, `BareIdentifier`, `FunctionValueCall`,
    /// `ConstructorCall`, `SigiledTypeExpr`) terminalize (or
    /// single-producer-park) in one poll; the `Keyworded` shape and the
    /// FnValue track may re-enter from a parked per-variant state, which
    /// is routed via the `DispatchState::Keyworded` /
    /// `DispatchState::FunctionValueCall` resume arms below.
    ///
    /// Called from the `NodeWork::Dispatch` arm of
    /// [`Scheduler::execute`]; this is the only dispatch driver.
    pub(super) fn run_dispatch(
        &mut self,
        expr: KExpression<'a>,
        state: DispatchState<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        // Drain the wake side-channel on entry so producers that fired
        // since the slot's last poll don't accumulate across re-park.
        // The keyworded / FunctionValueCall resume handlers read the
        // `subs` Vec from the installed track directly rather than the
        // wakes side-channel — at pop time `pending_deps` is zero, so
        // every recorded sub is terminal. The drain still runs
        // unconditionally so the side-channel never grows stale —
        // `take_recent_wakes` resets the slot's Vec to empty in O(1).
        let _wakes = self.store.take_recent_wakes(NodeId(idx));
        // Initial entry vs re-entry from a parked per-variant state.
        // Fast-lane variants all terminalize in one poll, so their
        // states are never re-entered; only `Keyworded` and
        // `FunctionValueCall` carry tracks that re-enter on track
        // completion.
        let init = match state {
            DispatchState::Initialized(i) => i,
            DispatchState::Keyworded(ks) => {
                return self.stateful_keyworded_resume(*ks, scope, idx);
            }
            DispatchState::FunctionValueCall(fs) => {
                return self.stateful_fn_value_resume(*fs, scope, idx);
            }
            _ => unreachable!(
                "remaining fast-lane stateful variants (BareIdentifier, BareTypeLeaf, \
                 ConstructorCall, SigiledTypeExpr) terminalize in one poll; \
                 only Keyworded and FunctionValueCall re-enter from a parked track"
            ),
        };
        // Classify once at entry and route per-variant. The fast-lane
        // handlers each terminalize (or single-producer-park) in one poll;
        // only `Keyworded` and `FunctionValueCall` carry per-variant tracks
        // that can re-enter via the resume arms above.
        match classify_dispatch_shape(&expr) {
            DispatchShape::BareTypeLeaf => {
                // Single-part by classifier; `pre_subs` cannot have been
                // populated by submit-time recursion (no nested
                // sub-expressions to pre-submit).
                debug_assert!(
                    init.pre_subs.is_empty(),
                    "BareTypeLeaf is single-part — submit-time recursion cannot \
                     populate pre_subs for this shape",
                );
                let t = match &expr.parts[0].value {
                    ExpressionPart::Type(t) => t.clone(),
                    _ => unreachable!("BareTypeLeaf shape implies single leaf Type part"),
                };
                Ok(self.fast_lane_bare_type_leaf(&t, scope))
            }
            DispatchShape::BareIdentifier => {
                debug_assert!(
                    init.pre_subs.is_empty(),
                    "BareIdentifier is single-part — submit-time recursion cannot \
                     populate pre_subs for this shape",
                );
                let name = match &expr.parts[0].value {
                    ExpressionPart::Identifier(n) => n.clone(),
                    _ => unreachable!("BareIdentifier shape implies single Identifier part"),
                };
                Ok(self.stateful_bare_identifier(name, scope, idx))
            }
            DispatchShape::FunctionValueCall => {
                // Submit-time recursion only fires for binder-shaped expressions
                // (LET / FN / FUNCTOR / STRUCT / UNION / SIG / MODULE binders), all
                // of which classify as `Keyworded`. A `FunctionValueCall` (Identifier
                // head + nested-parens body) is never produced by a binder, so
                // `pre_subs` is empty here. The stateful fast lane allocates a
                // fresh empty `Initialized` for any track install per the
                // structural-embedding rule's destructure-and-discard contract.
                debug_assert!(
                    init.pre_subs.is_empty(),
                    "FunctionValueCall is non-binder — submit-time recursion cannot \
                     populate pre_subs for this shape",
                );
                let _ = init;
                self.stateful_fast_lane_function_value_call(expr, scope, idx)
            }
            DispatchShape::ConstructorCall => {
                // Same non-binder reasoning as `FunctionValueCall` — a leaf-Type
                // head with nested-parens body is never produced by a binder.
                debug_assert!(
                    init.pre_subs.is_empty(),
                    "ConstructorCall is non-binder — submit-time recursion cannot \
                     populate pre_subs for this shape",
                );
                Ok(self.stateful_constructor_call(expr, scope, idx))
            }
            DispatchShape::Keyworded => {
                self.stateful_keyworded_initial(expr, init.pre_subs, scope, idx)
            }
            DispatchShape::SigiledTypeExpr => {
                // SigiledTypeExpr is single-part by classifier; `pre_subs` cannot
                // have been populated by submit-time recursion.
                debug_assert!(
                    init.pre_subs.is_empty(),
                    "SigiledTypeExpr is single-part — submit-time recursion cannot \
                     populate pre_subs for this shape",
                );
                // Tail-replace this slot with a Dispatch of the inner expression.
                // No frame / function / block_entry — the sigil itself is
                // scope-neutral; the inner expression carries its own context.
                let inner = match expr.parts.into_iter().next() {
                    Some(Spanned { value: ExpressionPart::SigiledTypeExpr(boxed), .. }) => *boxed,
                    _ => unreachable!(
                        "SigiledTypeExpr shape implies single SigiledTypeExpr part"
                    ),
                };
                Ok(NodeStep::Replace {
                    work: NodeWork::dispatch(inner),
                    frame: None,
                    function: None,
                    block_entry: None,
                    body_index: 0,
                })
            }
        }
    }

    /// Stateful-driver handler for `DispatchShape::Keyworded`. Routes the
    /// *one-shot* (Resolved, no parks, no eager subs) case to a direct
    /// terminate that inlines the placeholder install and the
    /// `function.bind` call without going through any per-variant state.
    /// The Resolved-with-parks and Resolved-with-eager-subs sub-cases
    /// install the bare-name-park / eager-subs Track on
    /// `KeywordedState`; `Deferred` folds into the eager-subs Track with
    /// no captured function; `ParkOnProducers` installs the overload-park
    /// Track. All re-entry routes through the resume handlers in
    /// `stateful_keyworded_resume`.
    ///
    /// Why this isn't a thin pre-walk that delegates: doing so would
    /// re-enter `resolve_dispatch_with_chain` a second time on the same
    /// slot, inflating the per-call resolve count. The Resolved-* sub-
    /// cases reuse the already-built `bare_outcomes` and `resolved`
    /// carrier and pay the resolve cost exactly once.
    ///
    /// The walk consumes `expr.parts` (via `into_iter`); we keep a clone
    /// of `expr` only for the `Deferred` / `ParkOnProducers` arms, which
    /// install per-variant tracks rather than rebuilding the slot as a
    /// fresh `Initialized` re-Dispatch.
    fn stateful_keyworded_initial(
        &mut self,
        expr: KExpression<'a>,
        pre_subs: Vec<(usize, NodeId)>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let bare_outcomes = self.build_bare_outcomes(&expr.parts, scope);
        // ProducerErrored short-circuit: a bare-name arg whose producer has
        // already terminalized with `Err` can never resolve; surface the error
        // with a `<wrap-resolve>` frame before the candidate walk.
        for outcome in bare_outcomes.iter().flatten() {
            if let NameOutcome::ProducerErrored(e) = outcome {
                let frame = Frame::from_expr("<wrap-resolve>", &expr);
                return Ok(NodeStep::Done(NodeOutput::Err(propagate_dep_error(e, Some(frame)))));
            }
        }
        let chain = self.active_chain.as_deref();
        let outcome = scope.resolve_dispatch_with_chain(&expr, chain, &bare_outcomes);
        let resolved = match outcome {
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
            ResolveOutcome::UnboundName(name) => {
                return Err(KError::new(KErrorKind::UnboundName(name)));
            }
            // Deferred folds onto `KeywordedState.eager_subs` (function = None)
            // in 4b. ParkOnProducers folds onto `KeywordedState.overload_park`
            // in 4d (both the bare-name-Placeholder and the
            // pending-overload-entry sub-cases — `resolve_dispatch_with_chain`
            // surfaces them through the same return variant, so the install
            // path is shared).
            ResolveOutcome::Deferred => {
                debug_assert!(
                    pre_subs.is_empty(),
                    "Deferred resolve_dispatch implies no binder pick at submit time; \
                     `pre_subs` must be empty here",
                );
                return self.stateful_install_eager_only(expr, scope, idx);
            }
            ResolveOutcome::ParkOnProducers(producers) => {
                return Ok(self.stateful_install_overload_park(producers, expr, pre_subs, idx));
            }
        };
        // Step 3.5: install dispatch-time placeholders. Both carry the
        // dispatching slot's lexical index and the picked function's
        // `is_nominal_binder` flag. Mirrors `run_dispatch`'s Step 3.5.
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
        // Step 4: part walk. Slot-terminal errors (cycle / unbound wrap) come
        // back as `Err(KError)`.
        let walk = match self.keyworded_part_walk(
            expr.parts,
            &pre_subs,
            &bare_outcomes,
            &resolved.slots,
            idx,
        ) {
            Ok(w) => w,
            Err(e) => return Ok(NodeStep::Done(NodeOutput::Err(e))),
        };
        let PartWalkResult { new_parts, producers_to_wait, staged_subs } = walk;
        let new_expr = KExpression::new(new_parts);
        if !producers_to_wait.is_empty() {
            // Park-precedence guard: the part walk already deferred submitting
            // `staged_subs` to the scheduler (they're staged, not added), and the
            // bare-name park installer drops them on the floor — re-Dispatch on
            // wake re-runs the walk and re-stages them, so submitting now would
            // leak nodes on the wake path.
            let _ = staged_subs;
            return Ok(self.stateful_install_bare_name_park(
                producers_to_wait,
                new_expr,
                pre_subs,
                idx,
            ));
        }
        // No park.
        if staged_subs.is_empty() {
            // One-shot path: bind directly without allocating a tracking
            // slot. Spliced `Future(&'a KObject)` references survive
            // `results[dep] = None` because the objects live in arenas tied
            // to lexical scope.
            return match resolved.function.bind(new_expr) {
                Ok(future) => Ok(self.invoke_to_step(future, scope, idx)),
                Err(e) => Ok(NodeStep::Done(NodeOutput::Err(e))),
            };
        }
        // Eager-subs Track: submit each sub, install Owned edges, and
        // transition this slot to `Keyworded(WaitingEagerSubs)`. The
        // resume handler re-resolves dispatch against the spliced
        // expression and binds on track completion — no intervening
        // `NodeWork::Bind` allocation. The initial pick's function is
        // not carried into the track: re-resolve is authoritative so
        // an element-typed `Future(_)` that narrows the typed-slot
        // admission surfaces `DispatchFailed` (non-match) rather than
        // a bind-time `TypeMismatch` (see `EagerSubsTrack`'s doc).
        let _ = resolved; // discard the speculative pick.
        self.stateful_install_eager_subs_track(new_expr, staged_subs, pre_subs, scope, idx)
    }

    /// Eager-only Track installer for the `ResolveOutcome::Deferred` arm.
    /// No picked function, no eager filter, no `bare_outcomes` consultation:
    /// schedule every `Expression` / `SigiledTypeExpr` / `ListLiteral` /
    /// `DictLiteral` part as a sub-Dispatch (or aggregate) and park the slot
    /// on them via `KeywordedState::WaitingEagerSubs`. On track completion
    /// the resume handler re-resolves dispatch against the spliced expression
    /// — Deferred means "the call shape needs a fresh resolve once the subs
    /// land".
    ///
    /// `Deferred ⇒ at least one eager part`, so the empty-subs branch would
    /// be a `resolve_dispatch` invariant break — `debug_assert!` would catch
    /// it but the real protection is the caller's surface contract.
    fn stateful_install_eager_only(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let (new_parts, staged_subs) = stage_all_eager_parts(expr.parts);
        debug_assert!(
            !staged_subs.is_empty(),
            "stateful_install_eager_only invoked from Deferred arm; \
             resolve_dispatch contract requires at least one eager part",
        );
        let new_expr = KExpression::new(new_parts);
        self.stateful_install_eager_subs_track(new_expr, staged_subs, Vec::new(), scope, idx)
    }

    /// Realize the bare-name park Track: install a `Notify` park edge from
    /// each producer to this slot via `DepGraph::add_park_edge` (producers
    /// are sibling forward references, not children of this slot, so the
    /// slot's reclaim walk must not transit into them), then transition to
    /// `Keyworded(WaitingBareNamePark)`. The cycle check ran inside
    /// `keyworded_part_walk` at the time the producer was added to the
    /// wait list, so this installer doesn't re-check.
    ///
    /// On track completion `stateful_keyworded_resume_bare_name_park`
    /// re-runs `stateful_keyworded_initial` against `working_expr` and
    /// `pre_subs`. The producers' now-bound values surface through the
    /// rebuilt `bare_outcomes` cache and the wrap-slot splice fires
    /// `Future(obj)` for them on the second pass.
    fn stateful_install_bare_name_park(
        &mut self,
        producers: Vec<NodeId>,
        working_expr: KExpression<'a>,
        pre_subs: Vec<(usize, NodeId)>,
        idx: usize,
    ) -> NodeStep<'a> {
        for p in &producers {
            self.deps.add_park_edge(*p, NodeId(idx));
        }
        let track = BareNameParkTrack::new(working_expr, producers);
        let init = super::dispatch_state::Initialized { pre_subs };
        self.replace_with_parked_dispatch(DispatchState::Keyworded(Box::new(
            KeywordedState::with_bare_name_park(init, track),
        )))
    }

    /// Realize the overload-park Track: filter `producers` for cycles
    /// and already-errored terminals, install a `Notify` park edge from
    /// each surviving producer via `DepGraph::add_park_edge`, and
    /// transition to `Keyworded(WaitingOverloadPark)`. Two
    /// `resolve_dispatch_with_chain` outcomes fold into this installer:
    /// bare-name `Placeholder`s the strict walk couldn't admit, and an
    /// innermost-visible `pending_overloads[key]` entry an FN /
    /// FUNCTOR sibling installed.
    ///
    /// An empty filtered list (every producer either errored or would
    /// close a cycle) surfaces the standard `DispatchFailed` —
    /// installing no park edge and falling through would deadlock the
    /// drain-end cycle-detection guard.
    ///
    /// On track completion `stateful_keyworded_resume_overload_park`
    /// re-runs `stateful_keyworded_initial` against the carried `expr`
    /// and preserved `pre_subs`. The producers' finalized state (an
    /// overload now registered in `bindings.functions`, or a bound bare
    /// name) feeds the rebuilt resolve.
    pub(super) fn stateful_install_overload_park(
        &mut self,
        producers: Vec<NodeId>,
        expr: KExpression<'a>,
        pre_subs: Vec<(usize, NodeId)>,
        idx: usize,
    ) -> NodeStep<'a> {
        let mut to_wait: Vec<NodeId> = Vec::new();
        for p in producers {
            if self.is_result_ready(p) {
                // Terminal while its placeholder is still set ⇒ the
                // producer errored (success clears the placeholder);
                // propagate rather than park on a dead slot, with a
                // `<dispatch-park>` frame for surface attribution.
                if let Err(e) = self.read_result(p) {
                    let frame = Frame::from_expr("<dispatch-park>", &expr);
                    return NodeStep::Done(NodeOutput::Err(
                        propagate_dep_error(e, Some(frame)),
                    ));
                }
            } else if !self.deps.would_create_cycle(p, NodeId(idx))
                && !to_wait.contains(&p)
            {
                to_wait.push(p);
            }
        }
        if to_wait.is_empty() {
            return NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::DispatchFailed {
                expr: expr.summarize(),
                reason: "no matching function".to_string(),
            })));
        }
        for p in &to_wait {
            self.deps.add_park_edge(*p, NodeId(idx));
        }
        let track = OverloadParkTrack::new(expr, to_wait);
        let init = super::dispatch_state::Initialized { pre_subs };
        self.replace_with_parked_dispatch(DispatchState::Keyworded(Box::new(
            KeywordedState::with_overload_park(init, track),
        )))
    }

    /// Submit each `PendingSub`, splice already-terminal subs inline,
    /// install an Owned dep_edge from each in-flight sub to this slot, and
    /// route the outcome via [`EagerSubsInstall`]. Shared by both the
    /// Keyworded driver (`picked = None`, finishes via re-resolve) and the
    /// FunctionValueCall fast lane (`picked = Some(f)`, binds directly on
    /// resume). The submission-time `is_result_ready` short-circuit avoids
    /// re-entering the dispatch loop when every sub was terminal at install.
    fn install_eager_subs(
        &mut self,
        mut working_expr: KExpression<'a>,
        staged_subs: Vec<(usize, PendingSub<'a>)>,
        picked: Option<&'a crate::machine::core::kfunction::KFunction<'a>>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> EagerSubsInstall<'a> {
        let mut pending_subs: Vec<(usize, NodeId)> = Vec::with_capacity(staged_subs.len());
        for (i, pending) in staged_subs {
            let sub_id = match pending {
                PendingSub::Reuse(id) => id,
                PendingSub::Dispatch(sub_expr) => self.add(NodeWork::dispatch(sub_expr), scope),
                PendingSub::ListLit(items) => self.schedule_list_literal(items, scope),
                PendingSub::DictLit(pairs) => self.schedule_dict_literal(pairs, scope),
            };
            if self.is_result_ready(sub_id) {
                match self.read_result(sub_id) {
                    Err(e) => return EagerSubsInstall::DepError(bind_frame_err(e, &working_expr)),
                    Ok(value) => {
                        working_expr.parts[i].value = ExpressionPart::Future(value);
                        self.free(sub_id.index());
                    }
                }
            } else {
                self.deps.add_owned_edge(sub_id, NodeId(idx));
                pending_subs.push((i, sub_id));
            }
        }
        if pending_subs.is_empty() {
            EagerSubsInstall::AllInline(working_expr)
        } else {
            EagerSubsInstall::Parked(EagerSubsTrack {
                working_expr,
                subs: pending_subs,
                picked,
            })
        }
    }

    /// Build the standard `NodeStep::Replace` shell every parked-`Dispatch`
    /// install site uses: drop the entry expression to an empty placeholder
    /// (the state carries the evolving `working_expr` from here on) and
    /// zero the four invoke-shape fields. The driver's resume routes by
    /// state variant and never reads the entry `expr` field, but
    /// `NodeWork::Dispatch` still requires it structurally.
    pub(super) fn replace_with_parked_dispatch(&self, state: DispatchState<'a>) -> NodeStep<'a> {
        NodeStep::Replace {
            work: NodeWork::Dispatch {
                expr: KExpression::new(Vec::new()),
                state,
            },
            frame: None,
            function: None,
            block_entry: None,
            body_index: 0,
        }
    }

    /// Keyworded eager-subs install: route the shared install outcome by
    /// the `AllInline`/`Parked`/`DepError` shape. The `AllInline` arm tails
    /// into `stateful_keyworded_finish` (re-resolve against the spliced
    /// expression); the `Parked` arm wraps the track in a
    /// `KeywordedState::with_eager_subs` and replaces the slot.
    fn stateful_install_eager_subs_track(
        &mut self,
        working_expr: KExpression<'a>,
        staged_subs: Vec<(usize, PendingSub<'a>)>,
        pre_subs: Vec<(usize, NodeId)>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        match self.install_eager_subs(working_expr, staged_subs, None, scope, idx) {
            EagerSubsInstall::DepError(step) => Ok(step),
            EagerSubsInstall::AllInline(working_expr) => {
                self.stateful_keyworded_finish(working_expr, scope, idx)
            }
            EagerSubsInstall::Parked(track) => {
                let init = super::dispatch_state::Initialized { pre_subs };
                Ok(self.replace_with_parked_dispatch(DispatchState::Keyworded(Box::new(
                    KeywordedState::with_eager_subs(init, track),
                ))))
            }
        }
    }

    /// Resume entry for a `Keyworded` slot. The three Keyworded tracks
    /// (`eager_subs`, `bare_name_park`, `overload_park`) are mutually
    /// exclusive at install time: `overload_park` fires when
    /// `resolve_dispatch_with_chain` returns `ParkOnProducers` *before*
    /// the part walk runs (so neither sibling has staged), and the part
    /// walk's park-precedence guard installs `bare_name_park` *before*
    /// staging any subs (so `eager_subs` cannot coexist with it). The
    /// resume routing tests each track in install order: `overload_park`,
    /// then `bare_name_park`, then `eager_subs`.
    fn stateful_keyworded_resume(
        &mut self,
        state: KeywordedState<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let KeywordedState { init, eager_subs, bare_name_park, overload_park } = state;
        if let Some(track) = overload_park {
            debug_assert!(
                eager_subs.is_none() && bare_name_park.is_none(),
                "overload_park is mutually exclusive with eager_subs and bare_name_park \
                 at install time (overload_park installs from the `ParkOnProducers` arm \
                 of `resolve_dispatch_with_chain`, before the part walk could stage \
                 either sibling track); resume must never see them coexisting",
            );
            return self.stateful_keyworded_resume_overload_park(track, init, scope, idx);
        }
        if let Some(track) = bare_name_park {
            debug_assert!(
                eager_subs.is_none(),
                "bare_name_park and eager_subs are mutually exclusive at install time \
                 (the part walk's park-precedence guard installs the bare-name park \
                 before staging any subs); resume must never see both",
            );
            return self.stateful_keyworded_resume_bare_name_park(track, init, scope, idx);
        }
        // Eager-subs resume doesn't consume `pre_subs` — the bind /
        // re-resolve path doesn't re-Dispatch the slot, so the
        // carry-through is dropped here per the structural-embedding
        // rule's destructure-and-discard contract.
        let _ = init;
        let track = eager_subs.expect(
            "Keyworded resume is only entered after a track is installed; \
             the install sites set `eager_subs`, `bare_name_park`, or `overload_park`",
        );
        self.stateful_resume_eager_subs(track, scope, idx)
    }

    /// Track-completion continuation for the `bare_name_park` track.
    /// Every producer this slot parked on has terminalized (the slot
    /// pops only on `pending_deps == 0`), so the bare names they backed
    /// now resolve through `scope.resolve_with_chain` to a bound value.
    /// Re-entering `stateful_keyworded_initial` rebuilds the
    /// `bare_outcomes` cache against the now-bound scope, picks the
    /// overload (Resolved-with-eager-subs on a typed bare name lands
    /// the eager-subs track on this same slot; Resolved-with-no-parks
    /// terminalizes one-shot), and proceeds.
    fn stateful_keyworded_resume_bare_name_park(
        &mut self,
        track: BareNameParkTrack<'a>,
        init: super::dispatch_state::Initialized,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        // `dep_edges[idx]` carries the Notify entries from
        // `add_park_edge`. They're harmless (Notify is skipped by
        // `owned_children` at free time) and stay in place across the
        // re-Dispatch wake.
        let BareNameParkTrack { working_expr, producers, .. } = track;
        let _ = producers;
        self.stateful_keyworded_initial(working_expr, init.pre_subs, scope, idx)
    }

    /// Track-completion continuation for the `overload_park` track.
    /// Every producer the install filtered into `to_wait` has
    /// terminalized (the slot pops only on `pending_deps == 0`), so a
    /// forward-overload sibling has registered its function in
    /// `bindings.functions` (or a bare-name placeholder has cleared to
    /// a bound value). Re-entering `stateful_keyworded_initial`
    /// rebuilds `bare_outcomes`, re-runs
    /// `resolve_dispatch_with_chain` against the now-populated bucket,
    /// and proceeds.
    ///
    /// If the wake didn't actually produce a matching overload (a
    /// later-sibling registered a different bucket key, or the
    /// re-resolve still finds another forward overload registered
    /// after this slot installed its park), the rebuilt resolve fires
    /// `ParkOnProducers` again and the re-entry installs a fresh
    /// `overload_park` track on the next-earliest sibling.
    fn stateful_keyworded_resume_overload_park(
        &mut self,
        track: OverloadParkTrack<'a>,
        init: super::dispatch_state::Initialized,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        // Same Notify-edge contract as the bare-name resume:
        // `dep_edges[idx]` keeps the entries across the re-Dispatch
        // wake; `owned_children` skips them at free time.
        let OverloadParkTrack { expr, producers, .. } = track;
        let _ = producers;
        self.stateful_keyworded_initial(expr, init.pre_subs, scope, idx)
    }

    /// Track-completion continuation shared between the Keyworded and
    /// FunctionValueCall `eager_subs` tracks. Reads each sub's terminal,
    /// splices `Future(value)` into `working_expr.parts[i]`, frees the
    /// sub, then routes on `track.picked`:
    ///
    /// - `None` (Keyworded install) — tail into
    ///   [`Self::stateful_keyworded_finish`], which re-resolves dispatch
    ///   against the spliced expression. Re-resolve is authoritative; an
    ///   element-typed `Future(_)` that narrows a typed-slot admission
    ///   surfaces `DispatchFailed` (non-match) rather than a bind-time
    ///   `TypeMismatch`.
    /// - `Some(f)` (FunctionValueCall install) — bind `f` directly. The
    ///   head was a single `KFunction` value carrier, not a candidate
    ///   bucket, so no re-resolve narrows the pick.
    ///
    /// On dep-error surfaces the `<bind>` frame to match the keyworded
    /// `run_bind` driver's `reclaim_deps` surface. Eager-free on the
    /// success path: clear the slot's dep_edges then free each sub —
    /// `dep_edges[idx]` at resume time holds only the Owned entries the
    /// install set up, so clearing is sound.
    fn stateful_resume_eager_subs(
        &mut self,
        track: EagerSubsTrack<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let EagerSubsTrack { mut working_expr, subs, picked } = track;
        for (_, sub_id) in &subs {
            if let Err(e) = self.read_result(*sub_id) {
                return Ok(bind_frame_err(e, &working_expr));
            }
        }
        let dep_indices: Vec<usize> = subs.iter().map(|(_, d)| d.index()).collect();
        for (part_idx, dep_id) in subs {
            let value = self.read(dep_id);
            working_expr.parts[part_idx].value = ExpressionPart::Future(value);
        }
        self.deps.clear_dep_edges(idx);
        for d in dep_indices {
            self.free(d);
        }
        match picked {
            None => self.stateful_keyworded_finish(working_expr, scope, idx),
            Some(f) => match f.bind(working_expr) {
                Ok(future) => Ok(self.invoke_to_step_pinned(future, scope, idx)),
                Err(e) => Ok(NodeStep::Done(NodeOutput::Err(e))),
            },
        }
    }

    /// Re-resolve completion shared between the parked-track resume and the
    /// all-subs-terminal-at-install short-circuit in
    /// `stateful_install_eager_subs_track`. Outcome shape: `Resolved` →
    /// bind + invoke, `Deferred` / `Unmatched` → `DispatchFailed`,
    /// `ParkOnProducers` → re-park (empty `pre_subs`, since the eager
    /// subs have already terminalized).
    ///
    /// Re-resolving is authoritative even when the initial pre-eager-resolve
    /// already picked an overload: an element-typed `Future(_)` that narrows
    /// a typed-slot admission rules the speculative pick out, so we surface
    /// `DispatchFailed` (non-match) rather than committing and surfacing a
    /// bind-time `TypeMismatch`. See `EagerSubsTrack`'s doc.
    ///
    /// `Err(KError)` (rather than `NodeStep::Done(NodeOutput::Err(_))`) for
    /// dispatch-failure cases — the execute loop's `?` bubbles the failure
    /// to the caller of `Scheduler::execute`. A bind-time `KError` from the
    /// picked function's admission likewise bubbles.
    fn stateful_keyworded_finish(
        &mut self,
        working_expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        match scope.resolve_dispatch(&working_expr) {
            ResolveOutcome::Resolved(r) => {
                let future = r.function.bind(working_expr)?;
                Ok(self.invoke_to_step_pinned(future, scope, idx))
            }
            ResolveOutcome::Ambiguous(n) => Err(KError::new(KErrorKind::AmbiguousDispatch {
                expr: working_expr.summarize(),
                candidates: n,
            })),
            ResolveOutcome::Deferred | ResolveOutcome::Unmatched => {
                Err(KError::new(KErrorKind::DispatchFailed {
                    expr: working_expr.summarize(),
                    reason: "no matching function".to_string(),
                }))
            }
            ResolveOutcome::ParkOnProducers(producers) => {
                Ok(self.stateful_install_overload_park(producers, working_expr, Vec::new(), idx))
            }
            ResolveOutcome::UnboundName(name) => Err(KError::new(KErrorKind::UnboundName(name))),
        }
    }

    /// Stateful fast lane for `DispatchShape::FunctionValueCall`. Routes
    /// the `KFunction` carrier through the eager-subs Track installer and
    /// the `Resolution::Placeholder` head park through the head-placeholder
    /// Track installer — both inline into the slot's `DispatchState` rather
    /// than spawning a separate `NodeWork::Bind`.
    ///
    /// **Forward-reference park** (`Placeholder(producer)`) installs an
    /// `add_park_edge` (Notify shape — producer is a sibling) and
    /// transitions the slot to `FunctionValueCall(WaitingHeadPlaceholder)`.
    /// On wake the resume re-runs this fast lane against the carried
    /// expression so the now-bound carrier reaches the
    /// `Resolution::Value` arm.
    ///
    /// **Unbound head** surfaces `UnboundName(name)` directly.
    fn stateful_fast_lane_function_value_call(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        // Classifier guarantees expr.parts[0] is a lowercase Identifier.
        let head = match &expr.parts[0].value {
            ExpressionPart::Identifier(n) => n.clone(),
            _ => unreachable!("FunctionValueCall shape implies Identifier head"),
        };
        let chain = self.active_chain.as_deref();
        match scope.resolve_with_chain(&head, chain) {
            Resolution::Value(obj) => self.stateful_dispatch_callable_value(expr, obj, scope, idx),
            Resolution::Placeholder(producer_id) => {
                Ok(self.stateful_install_fn_value_head_park(producer_id, expr, idx))
            }
            Resolution::UnboundName => Ok(NodeStep::Done(NodeOutput::Err(KError::new(
                KErrorKind::UnboundName(head),
            )))),
        }
    }

    /// Branch on the resolved head carrier of a `FunctionValueCall`.
    /// Routes the `KFunction` arm through
    /// `stateful_install_fn_value_eager_subs_track`; Struct / Tagged
    /// construction stays on `schedule_constructor_body` (those return
    /// `Tail` shapes, not Bind, and don't need eager scheduling at this
    /// level).
    fn stateful_dispatch_callable_value(
        &mut self,
        expr: KExpression<'a>,
        head_obj: &'a KObject<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let inner_parts = match extract_named_call_inner(&expr) {
            Ok(parts) => parts,
            Err(e) => return Ok(NodeStep::Done(NodeOutput::Err(e))),
        };
        match head_obj {
            KObject::KFunction(f, _) => match f.reconstruct_positional(inner_parts) {
                Ok(rebuilt) => {
                    self.stateful_install_fn_value_eager_subs_track(rebuilt, f, scope, idx)
                }
                Err(e) => Ok(NodeStep::Done(NodeOutput::Err(e))),
            },
            KObject::StructType { .. } => Ok(self
                .schedule_constructor_body(struct_value::apply(head_obj, inner_parts), idx)),
            KObject::TaggedUnionType { .. } => Ok(self.schedule_constructor_body(
                tagged_union::apply(head_obj, inner_parts),
                idx,
            )),
            other => Ok(NodeStep::Done(NodeOutput::Err(KError::new(
                KErrorKind::TypeMismatch {
                    arg: "verb".to_string(),
                    expected: "KFunction or Type".to_string(),
                    got: other.summarize(),
                },
            )))),
        }
    }

    /// Realize the FunctionValueCall eager-subs Track: walk the
    /// reconstructed-positional expression, stage each eager part
    /// (`Expression` / `SigiledTypeExpr` / `ListLiteral` / `DictLiteral`)
    /// as a sub-Dispatch (or aggregate) with an
    /// `Identifier("")` placeholder at its slot index, submit each sub
    /// and either splice already-terminal results inline (matching the
    /// `is_result_ready` short-circuit in
    /// `stateful_install_eager_subs_track`) or `add_owned_edge` and
    /// record in `pending_subs`, then transition to
    /// `FunctionValueCall(WaitingEagerSubs)`. If no subs schedule (the
    /// reconstructed expression has no eager parts at all) or all subs
    /// short-circuit at install time, bind `picked` directly without
    /// installing a track — mirrors the
    /// `stateful_install_eager_subs_track` all-terminal-at-install
    /// short-circuit.
    ///
    /// Mirrors `stateful_install_eager_subs_track`'s shape (Owned
    /// dep_edges + slot self-park + state-carried `working_expr`); the
    /// only structural difference is the `picked` carry-through — a
    /// `FunctionValueCall` head is a single `KFunction` value carrier,
    /// not an overload set, so re-resolving on completion would yield
    /// the same pick.
    fn stateful_install_fn_value_eager_subs_track(
        &mut self,
        expr: KExpression<'a>,
        picked: &'a crate::machine::core::kfunction::KFunction<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let (new_parts, staged_subs) = stage_all_eager_parts(expr.parts);
        let working_expr = KExpression::new(new_parts);
        match self.install_eager_subs(working_expr, staged_subs, Some(picked), scope, idx) {
            EagerSubsInstall::DepError(step) => Ok(step),
            EagerSubsInstall::AllInline(working_expr) => match picked.bind(working_expr) {
                Ok(future) => Ok(self.invoke_to_step_pinned(future, scope, idx)),
                Err(e) => Ok(NodeStep::Done(NodeOutput::Err(e))),
            },
            EagerSubsInstall::Parked(track) => {
                // FunctionValueCall is non-binder; submit-time recursion
                // never populated `pre_subs`, so the carrier carries an
                // empty `Initialized`. The structural-embedding rule's
                // destructure-and-discard contract is satisfied by
                // constructing `Initialized` fresh.
                let init = super::dispatch_state::Initialized { pre_subs: Vec::new() };
                Ok(self.replace_with_parked_dispatch(DispatchState::FunctionValueCall(Box::new(
                    FnValueState::with_eager_subs(init, track),
                ))))
            }
        }
    }

    /// Realize the FunctionValueCall head-placeholder Track: install a
    /// `Notify` park edge from the producer to this slot via
    /// `DepGraph::add_park_edge` (the producer is a sibling forward
    /// reference, not a child of this slot, so the slot's reclaim walk
    /// must not transit into it), then transition to
    /// `FunctionValueCall(WaitingHeadPlaceholder)`.
    ///
    /// On track completion `stateful_fn_value_resume_head_placeholder`
    /// re-runs `stateful_fast_lane_function_value_call` against the
    /// carried `expr`. The producer is now bound, so head resolution
    /// succeeds on the second pass.
    fn stateful_install_fn_value_head_park(
        &mut self,
        producer: NodeId,
        expr: KExpression<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        self.deps.add_park_edge(producer, NodeId(idx));
        let track = FnValueHeadPlaceholderTrack::new(expr, producer);
        // FunctionValueCall is non-binder; `pre_subs` is always empty,
        // and the resume path's re-entry through the fast lane never
        // reads it. Construct `Initialized` fresh per the structural-
        // embedding rule's destructure-and-discard contract.
        let init = super::dispatch_state::Initialized { pre_subs: Vec::new() };
        self.replace_with_parked_dispatch(DispatchState::FunctionValueCall(Box::new(
            FnValueState::with_head_placeholder(init, track),
        )))
    }

    /// Resume entry for a `FunctionValueCall` slot. Routes by install
    /// order: `eager_subs` first, then `head_placeholder` (mutually
    /// exclusive at install time — head resolution succeeds before the
    /// part walk runs, so `eager_subs` install implies the head
    /// resolved to `Value(KFunction)`; conversely `head_placeholder`
    /// install fires from the `Resolution::Placeholder` arm before any
    /// sub could stage).
    fn stateful_fn_value_resume(
        &mut self,
        state: FnValueState<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let FnValueState { init, eager_subs, head_placeholder } = state;
        // FunctionValueCall is non-binder; `pre_subs` is always empty,
        // and the resume paths don't re-Dispatch through any path that
        // re-reads it. Drop per the structural-embedding rule's
        // destructure-and-discard contract.
        let _ = init;
        if let Some(track) = eager_subs {
            debug_assert!(
                head_placeholder.is_none(),
                "eager_subs and head_placeholder are mutually exclusive at install \
                 time (head_placeholder fires from `Resolution::Placeholder` before \
                 the part walk could stage any subs; eager_subs install implies the \
                 head resolved to `Value(KFunction)`); resume must never see both",
            );
            return self.stateful_resume_eager_subs(track, scope, idx);
        }
        let track = head_placeholder.expect(
            "FunctionValueCall resume is only entered after a track is installed; \
             the install sites set `eager_subs` or `head_placeholder`",
        );
        self.stateful_fn_value_resume_head_placeholder(track, scope, idx)
    }

    /// Track-completion continuation for the FunctionValueCall
    /// `head_placeholder` track. Re-runs the stateful fast lane
    /// against the carried (unspliced) expression. The producer is
    /// now bound, so the second-pass `scope.resolve_with_chain` lands
    /// in the `Resolution::Value` arm (or, in the rare case that the
    /// producer re-resolved to a fresh `Placeholder` from a sibling
    /// forward chain, installs a fresh head park).
    ///
    /// Mirrors `stateful_keyworded_resume_bare_name_park` /
    /// `stateful_keyworded_resume_overload_park`'s shape: the Notify
    /// dep_edge stays in `dep_edges[idx]` across the wake (it's
    /// skipped by `owned_children` at free time), and the resume
    /// hands the carried expression straight back to the initial
    /// classifier.
    fn stateful_fn_value_resume_head_placeholder(
        &mut self,
        track: FnValueHeadPlaceholderTrack<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let FnValueHeadPlaceholderTrack { expr, producer, .. } = track;
        let _ = producer;
        self.stateful_fast_lane_function_value_call(expr, scope, idx)
    }

    /// Handler for `DispatchShape::BareIdentifier`. Surfaces `UnboundName`
    /// directly for a bare identifier with no binding and no visible
    /// placeholder, rather than falling through to the keyworded
    /// `value_lookup::body_identifier` path.
    fn stateful_bare_identifier(
        &mut self,
        name: String,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        match scope.resolve_with_chain(&name, self.active_chain.as_deref()) {
            Resolution::Value(obj) => NodeStep::Done(NodeOutput::Value(obj)),
            Resolution::Placeholder(producer) => {
                // Notify edge, not Owned: the producer is a sibling slot
                // this Lift only parks on for a wake. The Lift carrier
                // holds the result so the slot terminalizes on the
                // producer's value (or error) directly.
                self.deps.add_park_edge(producer, NodeId(idx));
                NodeStep::Replace {
                    work: NodeWork::Lift(LiftState::Pending(producer)),
                    frame: None,
                    function: None,
                    block_entry: None,
                    body_index: 0,
                }
            }
            Resolution::UnboundName => NodeStep::Done(NodeOutput::Err(KError::new(
                KErrorKind::UnboundName(name),
            ))),
        }
    }

    /// Stateful-driver handler for `DispatchShape::ConstructorCall` (leaf-Type
    /// head + nested-parens body). Resolves the head identity type-side and
    /// routes by `KType::UserType { kind, .. }`:
    ///
    /// - `Struct` / `Tagged` → recover the value-side schema carrier via
    ///   `coerce_type_token_value` and apply through
    ///   [`struct_value::apply`] / [`tagged_union::apply`], decoded by
    ///   `schedule_constructor_body`.
    /// - `Newtype { .. }` → [`newtype_construct`] with the arena-resident
    ///   `&'a KType` identity. Returns a `BodyResult::DeferTo(combine_id)`
    ///   wrapping the inner value sub-expression and a type-check Combine;
    ///   the decoder calls [`Self::defer_to_lift`] to install the Owned
    ///   read-dep and rewrite this slot as a `Lift`.
    /// - `TypeConstructor { .. }` → look the value-side schema carrier up
    ///   through `scope.lookup_with_chain` and dispatch via
    ///   [`dispatch_constructor`]. A builtin parameterized type registered
    ///   at prelude (`Result`) installs that paired carrier alongside the
    ///   type identity; an opaque per-call `TypeConstructor` (SIG / functor
    ///   ascription) installs only the identity, in which case the lookup
    ///   misses and we surface `TypeMismatch { expected: "constructible Type" }`.
    /// - `Module { .. }` and any other identity → `TypeMismatch { expected:
    ///   "constructible Type", got: identity.name() }`. MODULE-as-constructor
    ///   (functor application) is future work; both drivers reject the same
    ///   way today.
    /// - `resolve_type` miss → `UnboundName(name)` directly.
    ///
    /// **Forward-reference park.** A type-side name with a visible
    /// `Placeholder` (a forward `STRUCT Foo = …` reference before finalize)
    /// installs a combined park edge and rebuilds the slot as a fresh
    /// `Dispatch` so the now-finalized carrier reaches the type-side
    /// resolution on wake. The park check runs against the value-side
    /// `resolve_with_chain` because the type-side `resolve_type` returns
    /// `None` (rather than surfacing the placeholder) when the binder
    /// hasn't finalized — we must intercept the placeholder before that miss.
    ///
    /// **Inner-args shape.** Same admission as `extract_named_call_inner`:
    /// `expr.parts[1..]` must be exactly `[Spanned(Expression(inner))]`. Any
    /// other shape surfaces `DispatchFailed` (no positional call shape for
    /// type constructors).
    fn stateful_constructor_call(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        // Classifier guarantees parts[0] is a leaf `Type` and parts[1..] is
        // non-empty. Clone the head out so we can move `expr` later for the
        // park rebuild and the DispatchFailed surface.
        let head_t = match &expr.parts[0].value {
            ExpressionPart::Type(t) => t.clone(),
            _ => unreachable!("ConstructorCall shape implies leaf Type head"),
        };
        // Inner-parts admission first. Matches `extract_named_call_inner` —
        // anything other than a single nested-parens body surfaces
        // `DispatchFailed`.
        let inner_parts = match extract_named_call_inner(&expr) {
            Ok(parts) => parts,
            Err(e) => return NodeStep::Done(NodeOutput::Err(e)),
        };
        let chain = self.active_chain.as_deref();
        // Forward-reference park: the type-side resolution returns `None` (not
        // `Placeholder`) for a not-yet-finalized binder, so we must check the
        // value-side `resolve_with_chain` first. A value-side `Value` hit for a
        // Type name is the paired-carrier shape (STRUCT / UNION install both
        // atomically); fall through to the type-side resolution below either way.
        match scope.resolve_with_chain(&head_t.name, chain) {
            Resolution::Placeholder(producer) => {
                // Forward-reference park: route through the stateful overload-park
                // track installer (single-producer is fine — the installer
                // dedupes/cycle-filters internally) so the resume rebuilds via
                // `stateful_keyworded_initial`.
                return self.stateful_install_overload_park(vec![producer], expr, Vec::new(), idx);
            }
            Resolution::Value(_) | Resolution::UnboundName => {
                // Fall through to the type-side resolution below.
            }
        }
        // Identity-first: resolve `&'a KType<'a>` directly so the `Newtype`
        // arm can hand the arena-resident reference to `newtype_construct`
        // (the `KTypeValue` synthesis in `coerce_type_token_value` clones the
        // KType, which doesn't survive across the Combine closure's `'a` bound).
        let identity = match scope.resolve_type_with_chain(&head_t.name, chain) {
            Some(kt) => kt,
            None => {
                return NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::UnboundName(
                    head_t.name.clone(),
                ))));
            }
        };
        match identity {
            KType::UserType { kind: UserTypeKind::Struct, .. }
            | KType::UserType { kind: UserTypeKind::Tagged, .. } => {
                // Paired-carrier recovery: STRUCT / UNION finalize installs the
                // value-side schema in `data` alongside the type identity.
                // `coerce_type_token_value` resolves both sides and prefers the
                // paired carrier for `UserType` identities — we lean on it
                // rather than duplicating the lookup here. A non-Struct /
                // non-Tagged carrier from `coerce_type_token_value` on a
                // Struct / Tagged identity would be a finalize bug; surface
                // the standard not-constructible error rather than panicking.
                let carrier = match coerce_type_token_value(scope, &head_t, chain) {
                    Ok(obj) => obj,
                    Err(KError { kind: KErrorKind::UnboundName(n), .. }) => {
                        return NodeStep::Done(NodeOutput::Err(KError::new(
                            KErrorKind::UnboundName(n),
                        )));
                    }
                    Err(e) => return NodeStep::Done(NodeOutput::Err(e)),
                };
                let body = match carrier {
                    KObject::StructType { .. } => struct_value::apply(carrier, inner_parts),
                    KObject::TaggedUnionType { .. } => tagged_union::apply(carrier, inner_parts),
                    other => {
                        debug_assert!(
                            false,
                            "STRUCT/UNION `{}` registered its type identity but no \
                             matching value-side schema carrier (got `{}`)",
                            head_t.name,
                            other.summarize(),
                        );
                        return NodeStep::Done(NodeOutput::Err(KError::new(
                            KErrorKind::TypeMismatch {
                                arg: "verb".to_string(),
                                expected: "constructible Type".to_string(),
                                got: identity.name(),
                            },
                        )));
                    }
                };
                self.schedule_constructor_body(body, idx)
            }
            KType::UserType { kind: UserTypeKind::Newtype { .. }, .. } => {
                // NEWTYPE installs only the type identity in `bindings.types`;
                // no value-side schema carrier. `newtype_construct` schedules
                // the value sub-expression through `SchedulerHandle::add_dispatch`
                // and registers a `Combine` whose finish closure validates the
                // resolved inner value against `repr` and produces the final
                // `KObject::Wrapped`. The returned `BodyResult::DeferTo(combine_id)`
                // routes through `schedule_constructor_body`'s `DeferTo` arm,
                // which lifts this slot's terminal off the Combine.
                let body = newtype_construct(scope, self, identity, inner_parts);
                self.schedule_constructor_body(body, idx)
            }
            KType::UserType { kind: UserTypeKind::TypeConstructor { .. }, .. } => {
                // A builtin parameterized type registered at prelude (`Result`)
                // installs a schema carrier in `data` alongside the type identity,
                // like STRUCT/UNION — route through it. An *opaque* TypeConstructor
                // minted per-call for SIG/functor ascription installs only the
                // identity; for those `lookup` misses and we surface the standard
                // not-constructible error.
                match scope
                    .lookup_with_chain(&head_t.name, chain)
                    .and_then(|c| dispatch_constructor(c, inner_parts))
                {
                    Some(body) => self.schedule_constructor_body(body, idx),
                    None => NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::TypeMismatch {
                        arg: "verb".to_string(),
                        expected: "constructible Type".to_string(),
                        got: identity.name(),
                    }))),
                }
            }
            // MODULE-as-constructor (functor application) lands with the
            // functor-binder roadmap item. Today a Module identity resolved
            // type-side has no construction semantics; both drivers reject
            // the same way. `KType::Module { .. }` post-collapse — the old
            // `UserType { kind: Module, .. }` indirection is gone.
            //
            // Any other resolved identity (builtin leaf, `LET <Type> = <KTypeValue>`
            // alias, etc.) lands here too: a non-`UserType` identity is not a
            // type *constructor*.
            _ => NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "constructible Type".to_string(),
                got: identity.name(),
            }))),
        }
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

    /// The `recent_wakes` side-channel is `Dispatch`-only. A non-`Dispatch`
    /// consumer (here a `Lift(Pending(producer))` slot) parked on a
    /// `Dispatch` producer must drain to an empty Vec — `push_recent_wake`
    /// filters non-Dispatch work via the same peek-discriminator pattern
    /// as `stamp_lift_ready`. The Lift's stamp-then-enqueue path stays
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

    /// A `Dispatch` consumer parked on a `Dispatch` producer records the
    /// producer's `NodeId` in `recent_wakes` when the producer finalizes.
    /// The drained list is the side channel the per-variant resume entries
    /// key off when waking from a track-install.
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
        // shape the stateful park-track installers use for forward-
        // reference re-Dispatch wakes.
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
