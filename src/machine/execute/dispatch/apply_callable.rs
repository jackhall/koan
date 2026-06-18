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
//!   newtype / `TypeConstructor` identity). Reuses the `constructors` module
//!   (`CtorKind` + `launch`).
//! - `Function(&KFunction)` — call a `KFunction` by name. A functor is a
//!   `KFunction` whose result is a module, so functor application *is* this arm;
//!   the functor/function distinction survives only at classification (for
//!   `KType::KFunctor` typing and the `TypeHeadDeferred` diagnostic gate), never
//!   here at execution.

use std::rc::Rc;

use crate::machine::core::kfunction::KFunction;
use crate::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{KType, ProjectedSchema, RecursiveSet};
use crate::machine::{KError, KErrorKind};

use super::ctx::SchedulerView;
use super::Outcome;
use super::{
    body_shape_err, constructors, extract_call_body, stage_all_eager_parts, CallBody, NAMED_ONLY,
    POSITIONAL_ONLY,
};
use crate::machine::model::Carried;

/// A head resolved to something callable. The lane decides which arm; the tail
/// branches on the body surface and launches.
pub(in crate::machine::execute) enum ResolvedCallable<'step> {
    /// Build from a sealed nominal member (`KType::SetRef` — struct / tagged / newtype /
    /// `TypeConstructor`).
    Constructor(&'step KType<'step>),
    /// Call a `KFunction` by name — functor or not; a functor's result is a module.
    Function(&'step KFunction<'step>),
}

/// Body-shape-branch the resolved callable and launch. `expr.parts[1..]` is the
/// call body; `extract_call_body` admits one `{name = value}` record literal
/// (`Named`) or one `(value)` paren group (`Positional`).
pub(in crate::machine::execute) fn apply_callable<'step>(
    ctx: &SchedulerView<'step, '_>,
    callable: ResolvedCallable<'step>,
    expr: &KExpression<'step>,
) -> Outcome<'step> {
    match callable {
        // A constructor branches on the projected schema before deciding what body shape it
        // admits; the newtype arm in particular takes the trailing parts directly (so
        // `(Point r)` works), so body extraction lives per-arm inside `apply_constructor`
        // rather than here.
        ResolvedCallable::Constructor(identity) => apply_constructor(ctx, identity, expr),
        ResolvedCallable::Function(f) => {
            let body = match extract_call_body(expr) {
                Ok(b) => b,
                Err(e) => return Outcome::Done(Err(e)),
            };
            apply_function(ctx, f, expr, body)
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
fn apply_constructor<'step>(
    ctx: &SchedulerView<'step, '_>,
    identity: &'step KType<'step>,
    expr: &KExpression<'step>,
) -> Outcome<'step> {
    let KType::SetRef { set, index } = identity else {
        return Outcome::Done(Err(KError::new(KErrorKind::TypeMismatch {
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
            ) => constructors::dispatch_construct_record_newtype(identity, fields.clone()),
            _ => constructors::dispatch_construct_newtype(identity, expr.parts[1..].to_vec()),
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
                    return Outcome::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                        "`{tag}` is not a variant of `{}` (variants: {})",
                        set.member(*index).name,
                        sorted_variant_names(&schema),
                    )))));
                }
                let variant = KType::Variant {
                    set: Rc::clone(set),
                    index: *index,
                    tag,
                };
                return Outcome::Done(Ok(Carried::Type(
                    ctx.current_scope().arena.alloc_ktype(variant),
                )));
            }
            // Positional construction: `Outcome (Error "x")` (paren-group body). Tagged
            // unions and higher-kinded `TypeConstructor`s both construct positionally.
            match extract_call_body(expr) {
                Ok(CallBody::Positional(parts)) => constructors::dispatch_construct_tagged(
                    Rc::clone(set),
                    *index,
                    Rc::new(schema),
                    parts,
                ),
                Ok(CallBody::Named(_)) => body_shape_err(expr, POSITIONAL_ONLY),
                Err(e) => Outcome::Done(Err(e)),
            }
        }
        ProjectedSchema::TypeConstructor { schema, .. } => match extract_call_body(expr) {
            Ok(CallBody::Positional(parts)) => constructors::dispatch_construct_tagged(
                Rc::clone(set),
                *index,
                Rc::new(schema),
                parts,
            ),
            Ok(CallBody::Named(_)) => body_shape_err(expr, POSITIONAL_ONLY),
            Err(e) => Outcome::Done(Err(e)),
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
fn apply_function<'step>(
    ctx: &SchedulerView<'step, '_>,
    f: &'step KFunction<'step>,
    expr: &KExpression<'step>,
    body: CallBody<'step>,
) -> Outcome<'step> {
    match body {
        CallBody::Named(fields) => match f.reconstruct_positional(fields) {
            Ok(rebuilt) => install_eager_subs_track(ctx, rebuilt, f),
            Err(e) => Outcome::Done(Err(e)),
        },
        CallBody::Positional(_) => body_shape_err(expr, NAMED_ONLY),
    }
}

/// Stage every eager part of the reconstructed call as a sub-Dispatch, splice already-terminal
/// subs inline, and park the slot on the in-flight ones as a `AwaitDeps` whose finish binds
/// `picked`. Shared by the `FunctionValueCall` lane and every head-deferred / type-call function
/// arm.
pub(in crate::machine::execute) fn install_eager_subs_track<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: KExpression<'step>,
    picked: &'step KFunction<'step>,
) -> Outcome<'step> {
    // `picked` is already committed (the head uniquely resolved to it), so bare-name
    // value slots resolve by sub-Dispatch rather than the keyword path's pre-pick
    // `bare_outcomes` lookup — their resolved carrier then reaches `accepts_part` at bind.
    let wrap_indices = picked.classify_for_pick(&expr).wrap_indices;
    let (new_parts, staged_subs) = stage_all_eager_parts(expr.parts, &wrap_indices);
    let working_expr = KExpression::new(new_parts);
    ctx.install_eager_subs(working_expr, staged_subs, Some(picked))
}
