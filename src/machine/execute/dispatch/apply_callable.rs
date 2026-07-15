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
//! - `Function(&KFunction)` — call a `KFunction` by name. Every function rides this
//!   arm, whatever it returns.

use std::rc::Rc;

use crate::machine::core::KFunction;
use crate::machine::core::StoredReach;
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::model::{KType, ProjectedSchema, RecursiveSet};
use crate::machine::{KError, KErrorKind};
use crate::source::Spanned;

use super::ctx::SchedulerView;
use super::Outcome;
use super::{
    body_shape_err, constructors, extract_call_body, stage_all_eager_parts, CallBody, NAMED_ONLY,
    POSITIONAL_ONLY,
};

/// A head resolved to something callable. The lane decides which arm; the tail
/// branches on the body surface and launches.
pub(in crate::machine::execute) enum ResolvedCallable<'step> {
    /// Build from a sealed nominal member (`KType::SetRef` — struct / tagged / newtype /
    /// `TypeConstructor`). `reach` is the identity's stored per-binding type token (home-omitted
    /// foreign reach + home-borrow bit), threaded to the construction finish so the built value's
    /// operand names the identity's own region — empty while `RecursiveSet` is heap-`Rc`'d, the set's
    /// region once it is region-allocated.
    Constructor {
        identity: &'step KType<'step>,
        reach: StoredReach<'step>,
    },
    /// Call a `KFunction` by name.
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
        ResolvedCallable::Constructor { identity, reach } => {
            apply_constructor(ctx, identity, reach, expr)
        }
        ResolvedCallable::Function(f) => {
            let body = match extract_call_body(expr) {
                Ok(b) => b,
                Err(e) => return Outcome::Done(Err(e)),
            };
            apply_function(ctx, f, expr, body)
        }
    }
}

/// Construct from a `KType::SetRef` member identity. A newtype bypasses the
/// `{name = value}` / `(value)` body split — it takes the trailing parts directly as its
/// value expression, so `(Point {x = 1, y = 2})` builds a record and `(Point r)` /
/// `(Distance 3.0)` wrap a value. Tagged / `TypeConstructor` take a positional `(value)` body
/// (named is a loud `DispatchFailed`); a non-constructible identity is a `TypeMismatch`.
fn apply_constructor<'step>(
    ctx: &SchedulerView<'step, '_>,
    identity: &'step KType<'step>,
    reach: StoredReach<'step>,
    expr: &KExpression<'step>,
) -> Outcome<'step> {
    // A user `UNION` binds an anonymous union of per-variant newtype `SetRef`s. `Maybe Some`
    // names the variant type; `Maybe (Some v)` newtype-constructs the named member.
    if let KType::Union { members, .. } = identity {
        return apply_union_construct(ctx, members, reach, expr);
    }
    let KType::SetRef { set, index } = identity else {
        return Outcome::Done(Err(KError::new(KErrorKind::TypeMismatch {
            arg: "verb".to_string(),
            expected: "constructible Type".to_string(),
            got: identity.name(),
        })));
    };
    match RecursiveSet::projected_schema(set, *index) {
        // A record-literal body builds per-field (literal fields bind synchronously); any
        // other trailing expression is wrapped as a single positional value.
        ProjectedSchema::NewType(_) => match expr.parts.get(1..) {
            Some(
                [Spanned {
                    value: ExpressionPart::RecordLiteral(fields),
                    ..
                }],
            ) => constructors::dispatch_construct_record_newtype(identity, reach, fields.clone()),
            _ => {
                constructors::dispatch_construct_newtype(identity, reach, expr.parts[1..].to_vec())
            }
        },
        ProjectedSchema::TypeConstructor { schema, .. } => match extract_call_body(expr) {
            Ok(CallBody::Positional(parts)) => constructors::dispatch_construct_tagged(
                Rc::clone(set),
                *index,
                Rc::new(schema),
                reach,
                parts,
            ),
            Ok(CallBody::Named(_)) => body_shape_err(expr, POSITIONAL_ONLY),
            Err(e) => Outcome::Done(Err(e)),
        },
    }
}

/// Construct from an anonymous union of per-variant newtype `SetRef`s (a user `UNION`). `Maybe Some`
/// (a bare `Type` token body) yields the variant member's type value, reached through its union;
/// `Maybe (Some v)` (a paren-group body) newtype-constructs the named member — an ordinary
/// `KObject::Wrapped` over the member `SetRef`, never a `KObject::Tagged`. An unknown variant name in
/// either form is a schema error listing the union's members.
fn apply_union_construct<'step>(
    ctx: &SchedulerView<'step, '_>,
    members: &'step [KType<'step>],
    reach: StoredReach<'step>,
    expr: &KExpression<'step>,
) -> Outcome<'step> {
    // Bare variant-tag token with no payload (`Maybe Some`) names the variant *type*, reached
    // through its union — yielded as a first-class type value.
    if let [Spanned {
        value: ExpressionPart::Type(t),
        ..
    }] = expr.parts[1..].as_ref()
    {
        let name = t.render();
        return match union_member(members, &name) {
            Some(member) => {
                let step_ctx = ctx.step_ctx();
                // A region-free union member takes the compile-enforced `'static` tier; a member
                // that borrows a region (a `SetRef` variant) takes the runtime-checked seal.
                let sealed = match member.to_static() {
                    Some(owned) => Ok(step_ctx.alloc_type(owned)),
                    None => step_ctx.alloc_type_checked(member.clone()),
                };
                match sealed {
                    Ok(sealed) => Outcome::Done(Ok(sealed)),
                    Err(e) => Outcome::Done(Err(e)),
                }
            }
            None => Outcome::Done(Err(unknown_variant_error(members, &name))),
        };
    }
    // Payload construction: `Maybe (Some v)` (paren-group body) newtype-constructs the member.
    match extract_call_body(expr) {
        Ok(CallBody::Positional(parts)) => {
            let (tag, value_part) = match constructors::tagged_union::prepare_args(parts) {
                Ok(v) => v,
                Err(e) => return Outcome::Done(Err(e)),
            };
            match union_member(members, &tag) {
                Some(member) => constructors::dispatch_construct_newtype(
                    member,
                    reach,
                    vec![Spanned::bare(value_part)],
                ),
                None => Outcome::Done(Err(unknown_variant_error(members, &tag))),
            }
        }
        Ok(CallBody::Named(_)) => body_shape_err(expr, POSITIONAL_ONLY),
        Err(e) => Outcome::Done(Err(e)),
    }
}

/// The union member whose newtype `SetRef` is named `name`, if any.
fn union_member<'step>(members: &'step [KType<'step>], name: &str) -> Option<&'step KType<'step>> {
    members
        .iter()
        .find(|m| matches!(m, KType::SetRef { set, index } if set.member(*index).name == name))
}

/// A schema error for a name that is not one of the union's variants, listing the members.
fn unknown_variant_error(members: &[KType<'_>], name: &str) -> KError {
    KError::new(KErrorKind::ShapeError(format!(
        "`{name}` is not a variant of the union (variants: {})",
        union_member_names(members),
    )))
}

/// Sorted, comma-joined member names of an anonymous union of newtype `SetRef`s.
fn union_member_names(members: &[KType<'_>]) -> String {
    let mut names: Vec<&str> = members
        .iter()
        .filter_map(|m| match m {
            KType::SetRef { set, index } => Some(set.member(*index).name.as_str()),
            _ => None,
        })
        .collect();
    names.sort_unstable();
    names.join(", ")
}

/// Call a `KFunction` by name. A function takes `{name = value}` only; a
/// positional body is a loud `DispatchFailed`. Named args reconstruct the exact-arity
/// positional expression, then eager-resolve the value slots before binding.
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

/// Stage every eager part of the reconstructed call as a sub-Dispatch and park the slot on the
/// in-flight ones as an `AwaitDeps` whose finish binds `picked`. Shared by the
/// `FunctionValueCall` lane and every head-deferred / type-call function arm.
pub(in crate::machine::execute) fn install_eager_subs_track<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: KExpression<'step>,
    picked: &'step KFunction<'step>,
) -> Outcome<'step> {
    // `picked` is already committed (the head uniquely resolved to it), so bare-name value slots
    // resolve by sub-Dispatch rather than the keyword path's pre-pick `bare_outcomes` lookup —
    // each rides `bare_identifier`'s reach carrier through the eager-subs finish and reaches
    // `accepts_part` at bind. No slot resolves inline here.
    let wrap_indices = picked.classify_for_pick(&expr).wrap_indices;
    let (new_parts, staged_subs) = stage_all_eager_parts(expr.parts, &wrap_indices);
    let working_expr = KExpression::new(new_parts);
    ctx.install_eager_subs(working_expr, staged_subs, Some(picked))
}
