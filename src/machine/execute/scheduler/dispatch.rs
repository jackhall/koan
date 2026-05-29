//! Dispatch shape router, classifier, and shared spine.
//!
//! The dispatch driver enters at [`Scheduler::run_dispatch`], which
//! classifies the slot via [`classify_dispatch_shape`] and routes to
//! one of the five shape handlers:
//!
//! - **Keyworded** (any keyword present, or a head that isn't a
//!   fast-lane shape) → [`keyworded::KeywordedState`]
//! - **FunctionValueCall** (lowercase Identifier head + nested-parens
//!   body) → [`fn_value::FnValueState`]
//! - **BareIdentifier**, **BareTypeLeaf**, **ConstructorCall**,
//!   **SigiledTypeExpr** → [`single_poll`] handlers
//!
//! State and transitions live with their shape. This file keeps the
//! cross-shape glue: the [`DispatchState`] enum (one variant per shape
//! plus the universal `Initialized` birth state), the helpers that
//! multiple shapes call (`install_eager_subs`,
//! `replace_with_parked_dispatch`, `resume_eager_subs`), and the
//! pure resolution helpers (`classify_dispatch_shape`,
//! `resolve_name_part`, `extract_named_call_inner`,
//! `propagate_dep_error`, `stage_all_eager_parts`).

use crate::builtins::value_lookup::coerce_type_token_value;
use crate::machine::core::kfunction::KFunction;
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeParams};
use crate::machine::model::Parseable;
use crate::machine::{
    Frame, KError, KErrorKind, NameOutcome, NodeId, Resolution, Scope,
};

use super::Scheduler;
use super::super::nodes::{NodeOutput, NodeStep, NodeWork};

pub(in crate::machine::execute::scheduler) mod fn_value;
pub(in crate::machine::execute::scheduler) mod keyworded;
pub(in crate::machine::execute::scheduler) mod single_poll;

#[cfg(test)]
mod tests;

use fn_value::FnValueState;
use keyworded::KeywordedState;
use single_poll::{BareIdState, BareTypeState, CtorState, SigilState};

/// Pre-walk classification of a `KExpression` into the four no-keyword
/// fast-lane shapes plus the catch-all keyword-bearing shape. Driven
/// by [`classify_dispatch_shape`] at the top of
/// [`Scheduler::run_dispatch`].
pub(super) enum DispatchShape {
    BareIdentifier,
    BareTypeLeaf,
    /// Type-constructor call: head is a leaf `Type` and `parts[1..]`
    /// is non-empty.
    ConstructorCall,
    /// Function-value call: head is a lowercase `Identifier`,
    /// followed by ≥1 non-keyword parts.
    FunctionValueCall,
    /// Single-part `:(...)` sigiled type-expression wrapper.
    SigiledTypeExpr,
    /// A keyword appears anywhere in `expr.parts`, OR the expression
    /// doesn't fit any fast-lane shape.
    Keyworded,
}

/// One-pass classifier. Sweeps every part for `Keyword` first so a
/// mixed shape like `(f IF x)` goes to `Keyworded`. Only when the
/// no-keyword precondition is established do we branch on head shape.
pub(super) fn classify_dispatch_shape(expr: &KExpression<'_>) -> DispatchShape {
    if expr.parts.iter().any(|p| matches!(&p.value, ExpressionPart::Keyword(_))) {
        return DispatchShape::Keyworded;
    }
    if let [only] = expr.parts.as_slice() {
        return match &only.value {
            ExpressionPart::Identifier(_) => DispatchShape::BareIdentifier,
            ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => {
                DispatchShape::BareTypeLeaf
            }
            ExpressionPart::SigiledTypeExpr(_) => DispatchShape::SigiledTypeExpr,
            _ => DispatchShape::Keyworded,
        };
    }
    let Some(head_part) = expr.parts.first() else {
        return DispatchShape::Keyworded;
    };
    match &head_part.value {
        ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => {
            DispatchShape::ConstructorCall
        }
        ExpressionPart::Identifier(_) => DispatchShape::FunctionValueCall,
        _ => DispatchShape::Keyworded,
    }
}

/// Resolve a bare-name `ExpressionPart` (Identifier or leaf Type)
/// against `scope`. Consumed by the per-`run_dispatch` `bare_outcomes`
/// cache build and the dict/list literal planner's name-keyed Slot
/// resolution.
///
/// `consumer = Some(idx)` enables the cycle check; `consumer = None`
/// skips it (used by `classify_aggregate_part` during dict/list
/// literal planning).
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
        _ => unreachable!("resolve_name_part only called on bare-name parts"),
    };
    let chain = scheduler.active_chain.as_deref();
    match scope.resolve_with_chain(name, chain) {
        Resolution::Placeholder(producer) => {
            return if scheduler.is_result_ready(producer) {
                match scheduler.read_result(producer) {
                    Err(e) => NameOutcome::ProducerErrored(e.clone_for_propagation()),
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
            return NameOutcome::Resolved(obj);
        }
        Resolution::Value(_) | Resolution::UnboundName => {
            // Fall through for Type parts and Identifier Unbound.
        }
    }
    match is_type {
        Some(t) => match coerce_type_token_value(scope, t, chain) {
            Ok(obj) => NameOutcome::Resolved(obj),
            Err(KError { kind: KErrorKind::UnboundName(n), .. }) => NameOutcome::Unbound(n),
            Err(e) => NameOutcome::ProducerErrored(e),
        },
        None => NameOutcome::Unbound(name.to_string()),
    }
}

/// Best-effort name extraction for a bare-name `ExpressionPart` —
/// used to render the `cycle in type alias <name>` sample in
/// `SchedulerDeadlock`.
pub(super) fn bare_name_of<'a>(part: &ExpressionPart<'a>) -> Option<String> {
    match part {
        ExpressionPart::Identifier(n) => Some(n.clone()),
        ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => Some(t.name.clone()),
        _ => None,
    }
}

/// One staged submission queued by the keyworded part walk.
pub(in crate::machine::execute) enum PendingSub<'a> {
    Reuse(NodeId),
    Dispatch(KExpression<'a>),
    ListLit(Vec<ExpressionPart<'a>>),
    DictLit(Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>),
}

/// Result of a successful keyworded part walk.
pub(in crate::machine::execute) struct PartWalkResult<'a> {
    pub new_parts: Vec<Spanned<ExpressionPart<'a>>>,
    pub producers_to_wait: Vec<NodeId>,
    pub staged_subs: Vec<(usize, PendingSub<'a>)>,
}

/// Pull the inner parts of a `f (...)` call out of `expr.parts[1..]`.
/// The `FunctionValueCall` classifier guarantees an Identifier head
/// and ≥1 non-keyword body part; this checks the body is exactly a
/// single nested-parens (`ExpressionPart::Expression`) and clones its
/// inner parts.
pub(super) fn extract_named_call_inner<'a>(
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

/// Centralized dep-error propagation: clone the terminal error and
/// attach a caller-chosen frame. `frame = None` is the frameless
/// variant used by `run_catch`.
pub(super) fn propagate_dep_error(e: &KError, frame: Option<Frame>) -> KError {
    let cloned = e.clone_for_propagation();
    match frame {
        Some(f) => cloned.with_frame(f),
        None => cloned,
    }
}

/// Shape a dep-error terminal with the `<bind>` surface frame keyed
/// off `working_expr`. Shared by every eager-subs install / resume
/// site.
fn bind_frame_err<'a>(e: &KError, working_expr: &KExpression<'a>) -> NodeStep<'a> {
    let frame = Frame::from_expr("<bind>", working_expr);
    NodeStep::Done(NodeOutput::Err(propagate_dep_error(e, Some(frame))))
}

/// Walk raw parts emitting an `Identifier("")` placeholder at every
/// eager slot and a parallel staged-subs Vec; non-eager parts pass
/// through unchanged. Shared by the Keyworded `Deferred` arm and the
/// FunctionValueCall fast lane.
pub(super) fn stage_all_eager_parts<'a>(
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

/// Outcome of [`Scheduler::install_eager_subs`].
pub(in crate::machine::execute::scheduler::dispatch) enum EagerSubsInstall<'a> {
    AllInline(KExpression<'a>),
    Parked(EagerSubsTrack<'a>),
    DepError(NodeStep<'a>),
}

// ---------- State carrier ----------

/// Universal birth state of a Dispatch slot — the shape before
/// classification. Embedded by value in every per-variant state
/// struct so `pre_subs` rides along structurally rather than by
/// convention.
pub(in crate::machine::execute) struct Initialized {
    /// Pre-submitted sub-Dispatches keyed by their slot index in
    /// `expr.parts`; populated by submit-time recursion for
    /// binder-shaped expressions, empty otherwise.
    pub(in crate::machine::execute) pre_subs: Vec<(usize, NodeId)>,
}

/// Track state for the eager-subs sub-Dispatches a Keyworded or
/// FunctionValueCall slot is parked on. Each `(part_idx, sub_id)` is
/// the slot index in `working_expr.parts` to splice into at track
/// completion plus the sub NodeId (the Owned dep this slot installed
/// at park-install time).
pub(in crate::machine::execute) struct EagerSubsTrack<'a> {
    pub(in crate::machine::execute) working_expr: KExpression<'a>,
    pub(in crate::machine::execute) subs: Vec<(usize, NodeId)>,
    /// `Some(f)` is the FunctionValueCall install; resume binds `f`
    /// directly. `None` is the Keyworded install; resume re-runs
    /// `resolve_dispatch_with_chain` — re-resolve is authoritative so
    /// an element-typed `Future(_)` revealed by an eager sub surfaces
    /// as `DispatchFailed` (non-match) rather than a bind-time
    /// `TypeMismatch`.
    pub(in crate::machine::execute) picked: Option<&'a KFunction<'a>>,
}

impl<'a> EagerSubsTrack<'a> {
    pub(in crate::machine::execute) fn keyworded(
        working_expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
    ) -> Self {
        Self { working_expr, subs, picked: None }
    }

    pub(in crate::machine::execute) fn fn_value(
        working_expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
        picked: &'a KFunction<'a>,
    ) -> Self {
        Self { working_expr, subs, picked: Some(picked) }
    }
}

/// One variant per [`DispatchShape`], plus the pre-classification
/// `Initialized` birth state. `Keyworded` and `FunctionValueCall` are
/// boxed because each carries multiple independent `Option<Track>`
/// fields; inlining would push every `DispatchState`-carrying type
/// past clippy's `large_enum_variant` threshold.
pub(in crate::machine::execute) enum DispatchState<'a> {
    Initialized(Initialized),
    BareIdentifier(BareIdState<'a>),
    BareTypeLeaf(BareTypeState<'a>),
    ConstructorCall(CtorState<'a>),
    FunctionValueCall(Box<FnValueState<'a>>),
    SigiledTypeExpr(SigilState<'a>),
    Keyworded(Box<KeywordedState<'a>>),
}

impl<'a> DispatchState<'a> {
    /// Construct the universal birth state. Every submission and
    /// re-park site goes through this constructor so `pre_subs` is the
    /// only field any caller names. PhantomData markers are touched
    /// here as a hidden invariant: `_ph` is the unit-typed lifetime
    /// witness on the empty shape variants.
    pub(in crate::machine::execute) fn initialized(pre_subs: Vec<(usize, NodeId)>) -> Self {
        DispatchState::Initialized(Initialized { pre_subs })
    }

    /// Expression carried by the state itself for parked `Keyworded`
    /// or `FunctionValueCall` slots. The Track installers drop
    /// `NodeWork::Dispatch.expr` to an empty placeholder once the slot
    /// transitions to a parked variant, so the drain-end
    /// cycle-detection guard (`NodeStore::unresolved`) prefers this
    /// state-carried expression when summarizing a parked sample.
    pub(in crate::machine::execute) fn parked_carrier_expr(
        &self,
    ) -> Option<&KExpression<'a>> {
        match self {
            DispatchState::Keyworded(ks) => {
                if let Some(track) = &ks.overload_park {
                    return Some(&track.expr);
                }
                if let Some(track) = &ks.bare_name_park {
                    return Some(&track.working_expr);
                }
                if let Some(track) = &ks.eager_subs {
                    return Some(&track.working_expr);
                }
                None
            }
            DispatchState::FunctionValueCall(fs) => {
                if let Some(track) = &fs.eager_subs {
                    return Some(&track.working_expr);
                }
                if let Some(track) = &fs.head_placeholder {
                    return Some(&track.expr);
                }
                None
            }
            _ => None,
        }
    }
}

// ---------- Scheduler shared spine ----------

impl<'a> Scheduler<'a> {
    /// Build the per-part `bare_outcomes` cache consulted by strict
    /// admission and the fused splice/park walk. One
    /// `resolve_name_part` per bare-name part; non-bare-name parts
    /// get `None`. Built with `consumer = None` so cycle detection is
    /// deferred to the splice walk.
    pub(in crate::machine::execute::scheduler::dispatch) fn build_bare_outcomes(
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

    /// Submit each `PendingSub`, splice already-terminal subs inline,
    /// install an Owned dep_edge from each in-flight sub to this slot,
    /// and return the routed [`EagerSubsInstall`]. Shared by both the
    /// Keyworded driver (`picked = None`) and the FunctionValueCall
    /// fast lane (`picked = Some(f)`).
    pub(in crate::machine::execute::scheduler::dispatch) fn install_eager_subs(
        &mut self,
        mut working_expr: KExpression<'a>,
        staged_subs: Vec<(usize, PendingSub<'a>)>,
        picked: Option<&'a KFunction<'a>>,
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
            EagerSubsInstall::Parked(EagerSubsTrack { working_expr, subs: pending_subs, picked })
        }
    }

    /// Build the standard `NodeStep::Replace` shell every parked-
    /// Dispatch install site uses: drop the entry expression to an
    /// empty placeholder (the state carries the evolving `working_expr`
    /// from here on) and zero the four invoke-shape fields.
    pub(in crate::machine::execute::scheduler::dispatch) fn replace_with_parked_dispatch(
        &self,
        state: DispatchState<'a>,
    ) -> NodeStep<'a> {
        NodeStep::Replace {
            work: NodeWork::Dispatch { expr: KExpression::new(Vec::new()), state },
            frame: None,
            function: None,
            block_entry: None,
            body_index: 0,
        }
    }

    /// Track-completion continuation shared between the Keyworded and
    /// FunctionValueCall `eager_subs` tracks. Reads each sub's
    /// terminal, splices `Future(value)` into `working_expr.parts[i]`,
    /// frees the sub, then routes on `track.picked`:
    ///
    /// - `None` (Keyworded install) — tail into
    ///   [`KeywordedState::finish`], which re-resolves dispatch.
    /// - `Some(f)` (FunctionValueCall install) — bind `f` directly.
    pub(in crate::machine::execute::scheduler::dispatch) fn resume_eager_subs(
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
            None => KeywordedState::finish(self, working_expr, scope, idx),
            Some(f) => match f.bind(working_expr) {
                Ok(future) => Ok(self.invoke_to_step_pinned(future, scope, idx)),
                Err(e) => Ok(NodeStep::Done(NodeOutput::Err(e))),
            },
        }
    }

    /// Stateful dispatch driver. Classifies the slot's shape and
    /// routes to the matching per-shape entry. Fast-lane variants
    /// terminalize (or single-producer-park) in one poll; only
    /// `Keyworded` and `FunctionValueCall` carry tracks that can
    /// re-enter via the resume arms.
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
        // Drain the wake side-channel on entry.
        let _wakes = self.store.take_recent_wakes(NodeId(idx));
        let init = match state {
            DispatchState::Initialized(i) => i,
            DispatchState::Keyworded(ks) => return ks.resume(self, scope, idx),
            DispatchState::FunctionValueCall(fs) => return fs.resume(self, scope, idx),
            _ => unreachable!(
                "remaining fast-lane stateful variants terminalize in one poll; \
                 only Keyworded and FunctionValueCall re-enter from a parked track"
            ),
        };
        match classify_dispatch_shape(&expr) {
            DispatchShape::BareTypeLeaf => {
                debug_assert!(init.pre_subs.is_empty());
                let t = match &expr.parts[0].value {
                    ExpressionPart::Type(t) => t.clone(),
                    _ => unreachable!("BareTypeLeaf shape implies single leaf Type part"),
                };
                Ok(single_poll::bare_type_leaf(self, &t, scope))
            }
            DispatchShape::BareIdentifier => {
                debug_assert!(init.pre_subs.is_empty());
                let name = match &expr.parts[0].value {
                    ExpressionPart::Identifier(n) => n.clone(),
                    _ => unreachable!("BareIdentifier shape implies single Identifier part"),
                };
                Ok(single_poll::bare_identifier(self, name, scope, idx))
            }
            DispatchShape::FunctionValueCall => {
                debug_assert!(init.pre_subs.is_empty());
                let _ = init;
                FnValueState::initial(self, expr, scope, idx)
            }
            DispatchShape::ConstructorCall => {
                debug_assert!(init.pre_subs.is_empty());
                Ok(single_poll::constructor_call(self, expr, scope, idx))
            }
            DispatchShape::Keyworded => KeywordedState::initial(self, expr, init.pre_subs, scope, idx),
            DispatchShape::SigiledTypeExpr => {
                debug_assert!(init.pre_subs.is_empty());
                Ok(single_poll::sigiled_type_expr(expr))
            }
        }
    }
}
