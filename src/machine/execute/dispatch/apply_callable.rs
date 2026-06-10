//! The shared apply-a-callable tail.
//!
//! Every dispatch lane that resolves a head to *something callable* — `TypeCall`
//! (bare-`Type` head), `FunctionValueCall` (lowercase-identifier head), and the
//! head-deferred lanes (a head expression / `:(...)` sigil that is evaluated
//! first) — funnels its resolved callable through [`apply_callable`]. The lane
//! does the resolution; this tail does the body-shape branching and launches
//! construction or a function call.
//!
//! A [`ResolvedCallable`] has exactly two execution arms:
//!
//! - `Constructor(&KType)` — build a value from a type schema (struct / tagged /
//!   newtype / `TypeConstructor` identity). Reuses `CtorState`/`CtorTrack`.
//! - `Function(&KFunction)` — call a `KFunction` by name. A functor is a
//!   `KFunction` whose result is a module, so functor application *is* this arm;
//!   the functor/function distinction survives only at classification (for
//!   `KType::KFunctor` typing and the `TypeHeadDeferred` diagnostic gate), never
//!   here at execution.

use std::rc::Rc;

use crate::machine::core::kfunction::{KFunction, SchedulerHandle};
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{KType, ProjectedSchema, RecursiveSet};
use crate::machine::{KError, KErrorKind};

use super::super::nodes::{NodeOutput, NodeStep};
use super::{
    body_shape_err, constructors, extract_call_body, stage_all_eager_parts, CallBody, DispatchCtx,
    DispatchState, EagerSubsInstall, FnValueState, Initialized, NAMED_ONLY, POSITIONAL_ONLY,
};

/// A head resolved to something callable. The lane decides which arm; the tail
/// branches on the body surface and launches.
pub(in crate::machine::execute) enum ResolvedCallable<'a> {
    /// Build from a sealed nominal member (`KType::SetRef` — struct / tagged / newtype /
    /// `TypeConstructor`).
    Constructor(&'a KType<'a>),
    /// Call a `KFunction` by name — functor or not; a functor's result is a module.
    Function(&'a KFunction<'a>),
}

/// Body-shape-branch the resolved callable and launch. `expr.parts[1..]` is the
/// call body; `extract_call_body` admits one `{name = value}` record literal
/// (`Named`) or one `(value)` paren group (`Positional`).
pub(in crate::machine::execute) fn apply_callable<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    callable: ResolvedCallable<'a>,
    expr: &KExpression<'a>,
    idx: usize,
) -> NodeStep<'a> {
    match callable {
        // A constructor branches on the projected schema before deciding what body shape it
        // admits; the newtype arm in particular takes the trailing parts directly (so
        // `(Point r)` works), so body extraction lives per-arm inside `apply_constructor`
        // rather than here.
        ResolvedCallable::Constructor(identity) => apply_constructor(ctx, identity, expr, idx),
        ResolvedCallable::Function(f) => {
            let body = match extract_call_body(expr) {
                Ok(b) => b,
                Err(e) => return NodeStep::Done(NodeOutput::Err(e)),
            };
            apply_function(ctx, f, expr, body, idx)
        }
    }
}

/// Construct from a `KType::SetRef` member identity. A newtype (record-repr or scalar) takes
/// the trailing parts as its value expression — `(Point {x = 1, y = 2})` builds a record,
/// `(Point r)` / `(Distance 3.0)` wrap a value — so it bypasses the `{name = value}` /
/// `(value)` body split entirely. Tagged / `TypeConstructor` take a positional `(value)` body;
/// a named body is a loud `DispatchFailed`. A non-constructible identity is a `TypeMismatch`.
/// The schema is projected off the member (sibling `SetLocal`s resolved to external
/// `SetRef`s); `(set, index)` is stamped onto a tagged value.
fn apply_constructor<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    identity: &'a KType<'a>,
    expr: &KExpression<'a>,
    idx: usize,
) -> NodeStep<'a> {
    let KType::SetRef { set, index } = identity else {
        return NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::TypeMismatch {
            arg: "verb".to_string(),
            expected: "constructible Type".to_string(),
            got: identity.name(),
        })));
    };
    match RecursiveSet::projected_schema(set, *index) {
        // Newtype construction. A named record-literal body (`Point {x = 1, y = 2}`) builds a
        // record per-field (so literal fields bind synchronously); any other trailing
        // expression (`(Point r)`, `(Distance 3.0)`) is wrapped as a single positional value.
        ProjectedSchema::Newtype(_) => match expr.parts.get(1..) {
            Some(
                [Spanned {
                    value: ExpressionPart::RecordLiteral(fields),
                    ..
                }],
            ) => {
                constructors::dispatch_construct_record_newtype(ctx, identity, fields.clone(), idx)
            }
            _ => constructors::dispatch_construct_newtype(
                ctx,
                identity,
                expr.parts[1..].to_vec(),
                idx,
            ),
        },
        // A bare variant-tag token with no payload (`Maybe Some`) names the variant
        // *type*, reached through its union — distinct from construction `Maybe (Some v)`,
        // which wraps the tag in a paren group. Yielded as a first-class type value.
        ProjectedSchema::Tagged(schema) => {
            if let [Spanned {
                value: ExpressionPart::Type(t),
                ..
            }] = expr.parts[1..].as_ref()
            {
                let tag = t.render();
                if !schema.contains_key(&tag) {
                    return NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::ShapeError(
                        format!(
                            "`{tag}` is not a variant of `{}` (variants: {})",
                            set.member(*index).name,
                            sorted_variant_names(&schema),
                        ),
                    ))));
                }
                let variant = KType::Variant {
                    set: Rc::clone(set),
                    index: *index,
                    tag,
                };
                return NodeStep::Done(NodeOutput::ktype(
                    ctx.current_scope().arena.alloc_ktype(variant),
                ));
            }
            // Positional construction: `Outcome (Error "x")` (paren-group body). Tagged
            // unions and higher-kinded `TypeConstructor`s both construct positionally.
            match extract_call_body(expr) {
                Ok(CallBody::Positional(parts)) => constructors::dispatch_construct_tagged(
                    ctx,
                    Rc::clone(set),
                    *index,
                    Rc::new(schema),
                    parts,
                    idx,
                ),
                Ok(CallBody::Named(_)) => body_shape_err(expr, POSITIONAL_ONLY),
                Err(e) => NodeStep::Done(NodeOutput::Err(e)),
            }
        }
        ProjectedSchema::TypeConstructor { schema, .. } => match extract_call_body(expr) {
            Ok(CallBody::Positional(parts)) => constructors::dispatch_construct_tagged(
                ctx,
                Rc::clone(set),
                *index,
                Rc::new(schema),
                parts,
                idx,
            ),
            Ok(CallBody::Named(_)) => body_shape_err(expr, POSITIONAL_ONLY),
            Err(e) => NodeStep::Done(NodeOutput::Err(e)),
        },
    }
}

/// Sorted, comma-joined variant tags of a projected tagged schema — for the
/// "not a variant of …" diagnostic.
fn sorted_variant_names(schema: &std::collections::HashMap<String, KType<'_>>) -> String {
    let mut names: Vec<&str> = schema.keys().map(|s| s.as_str()).collect();
    names.sort_unstable();
    names.join(", ")
}

/// Call a `KFunction` by name. Named args reconstruct the exact-arity positional
/// expression and eager-resolve the value slots before binding; a positional body
/// is a loud `DispatchFailed` (functions and functors take `{name = value}` only).
fn apply_function<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    f: &'a KFunction<'a>,
    expr: &KExpression<'a>,
    body: CallBody<'a>,
    idx: usize,
) -> NodeStep<'a> {
    match body {
        CallBody::Named(fields) => match f.reconstruct_positional(fields) {
            Ok(rebuilt) => match install_eager_subs_track(ctx, rebuilt, f, idx) {
                Ok(step) => step,
                Err(e) => NodeStep::Done(NodeOutput::Err(e)),
            },
            Err(e) => NodeStep::Done(NodeOutput::Err(e)),
        },
        CallBody::Positional(_) => body_shape_err(expr, NAMED_ONLY),
    }
}

/// Stage every eager part of the reconstructed call as a sub-Dispatch, splice
/// already-terminal subs inline, and either bind `picked` directly (all inline)
/// or park as a `FunctionValueCall` eager-subs track. Shared by the
/// `FunctionValueCall` lane and every head-deferred / type-call function arm.
pub(in crate::machine::execute) fn install_eager_subs_track<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    expr: KExpression<'a>,
    picked: &'a KFunction<'a>,
    idx: usize,
) -> Result<NodeStep<'a>, KError> {
    // `picked` is already committed (the head uniquely resolved to it), so bare-name
    // value slots resolve by sub-Dispatch rather than the keyword path's pre-pick
    // `bare_outcomes` lookup — their resolved carrier then reaches `accepts_part` at bind.
    let wrap_indices = picked.classify_for_pick(&expr).wrap_indices;
    let (new_parts, staged_subs) = stage_all_eager_parts(expr.parts, &wrap_indices);
    let working_expr = KExpression::new(new_parts);
    match ctx.install_eager_subs(working_expr, staged_subs, Some(picked), idx) {
        EagerSubsInstall::DepError(step) => Ok(step),
        EagerSubsInstall::AllInline(working_expr) => match picked.bind(working_expr) {
            Ok(future) => Ok(ctx.invoke_to_step(future, idx)),
            Err(e) => Ok(NodeStep::Done(NodeOutput::Err(e))),
        },
        EagerSubsInstall::Parked(track) => {
            // The function arm is non-binder; `pre_subs` is always empty.
            let init = Initialized {
                pre_subs: Vec::new(),
            };
            Ok(
                ctx.replace_with_parked_dispatch(DispatchState::FunctionValueCall(Box::new(
                    FnValueState::with_eager_subs(init, track),
                ))),
            )
        }
    }
}
