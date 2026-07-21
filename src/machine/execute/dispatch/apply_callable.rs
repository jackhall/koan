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
//! - `Constructor { identity: KType }` — build a value from a type schema (struct / tagged /
//!   newtype / `TypeConstructor` identity), reusing the `constructors` module
//!   (`CtorKind` + `launch`); or, when the head is a type constructor and the body is a
//!   record literal, apply that constructor to named type arguments
//!   (`:(Result {Ok = Number, Error = MyError})`) and yield the resulting
//!   `ConstructorApply` type as a type value.
//! - `Function(&KFunction)` — call a `KFunction` by name. Every function rides this
//!   arm, whatever it returns.

use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::{DepPlacement, KFunction};
use crate::machine::model::{constructor_param_names, Carried, Record};
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::model::{KType, NodeSchema, TypeNode, TypeRegistry};
use crate::machine::{KError, KErrorKind};
use crate::scheduler::Deps;
use crate::source::Spanned;

use super::super::outcome::dep_error_frame;
use super::super::TerminalDepFinish;
use super::ctx::SchedulerView;
use super::{
    body_shape_err, constructors, extract_call_body, stage_all_eager_parts, CallBody, NAMED_ONLY,
    POSITIONAL_ONLY,
};
use super::{Await, DepRequest, Outcome};

#[cfg(test)]
mod tests;

/// A head resolved to something callable. The lane decides which arm; the tail
/// branches on the body surface and launches.
pub(in crate::machine::execute) enum ResolvedCallable<'step> {
    /// Build from a sealed nominal member (a `SetMember` node — struct / tagged / newtype /
    /// `TypeConstructor`).
    Constructor { identity: KType },
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
        ResolvedCallable::Constructor { identity } => apply_constructor(ctx, identity, expr),
        ResolvedCallable::Function(f) => {
            let body = match extract_call_body(expr) {
                Ok(b) => b,
                Err(e) => return Outcome::Done(Err(e)),
            };
            apply_function(ctx, f, expr, body)
        }
    }
}

/// Construct from a sealed nominal member identity, or apply a type constructor to named type
/// arguments. A record-literal body on a constructor-kind head (`Wrap {Elem = Number}`) is *type
/// application*, yielding a `ConstructorApply` type value. Otherwise a newtype bypasses the
/// `{name = value}` / `(value)` body split — it takes the trailing parts directly as its
/// value expression, so `(Point {x = 1, y = 2})` builds a record and `(Point r)` /
/// `(Distance 3.0)` wrap a value. Tagged / `TypeConstructor` take a positional `(value)` body
/// (named is a loud `DispatchFailed`). A SIG's abstract constructor slot is a witness-less kind
/// and rejects construction by name; any other non-constructible identity is a `TypeMismatch`.
fn apply_constructor<'step>(
    ctx: &SchedulerView<'step, '_>,
    identity: KType,
    expr: &KExpression<'step>,
) -> Outcome<'step> {
    // A user `UNION` binds an anonymous union of per-variant newtype members. `Maybe Some`
    // names the variant type; `Maybe (Some v)` newtype-constructs the named member.
    if let TypeNode::Union { members } = ctx.types().node(identity) {
        return apply_union_construct(ctx, members, expr);
    }
    // Named type application: a type-constructor head — a declared family (`SetMember`, empty or
    // non-empty schema) or a SIG's abstract constructor slot — with a record-literal body binds
    // each of the family's parameters to a type. It precedes every construction arm: the two
    // surfaces are disjoint, and the record body is a type-argument list here, not a value.
    if let Some(param_names) = constructor_param_names(identity, ctx.types()) {
        if let Some(
            [Spanned {
                value: ExpressionPart::RecordLiteral(fields),
                ..
            }],
        ) = expr.parts.get(1..)
        {
            return apply_named_type_args(ctx, identity, param_names, fields.clone());
        }
    }
    // A SIG's abstract constructor slot names a kind; it has no representation to build values
    // over. Its first-order sibling carries no parameters and falls to the generic mismatch.
    if let TypeNode::AbstractType {
        name, param_names, ..
    } = ctx.types().node(identity)
    {
        if !param_names.is_empty() {
            return Outcome::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                "`{name}` is an abstract constructor slot declared by TYPE; only a \
                 NEWTYPE-declared constructor can construct values"
            )))));
        }
    }
    let TypeNode::SetMember { schema, name, .. } = ctx.types().node(identity) else {
        return Outcome::Done(Err(KError::new(KErrorKind::TypeMismatch {
            arg: "verb".to_string(),
            expected: "constructible Type".to_string(),
            got: identity.name(ctx.types()),
        })));
    };
    match schema {
        // A record-literal body builds per-field (literal fields bind synchronously); any
        // other trailing expression is wrapped as a single positional value.
        NodeSchema::NewType(_) => match expr.parts.get(1..) {
            Some(
                [Spanned {
                    value: ExpressionPart::RecordLiteral(fields),
                    ..
                }],
            ) => constructors::dispatch_construct_record_newtype(identity, fields.clone()),
            _ => constructors::dispatch_construct_newtype(identity, expr.parts[1..].to_vec()),
        },
        // A non-empty schema is `Result`'s variant schema — the sealed tagged-union path. An
        // empty schema is a declared constructor family (`NEWTYPE (Elem AS Wrapper)`); it
        // constructs an identity-wrapper `Wrapped` value.
        NodeSchema::TypeConstructor {
            schema: variant_schema,
            ..
        } if !variant_schema.is_empty() => match extract_call_body(expr) {
            Ok(CallBody::Positional(parts)) => {
                constructors::dispatch_construct_tagged(identity, Rc::new(variant_schema), parts)
            }
            Ok(CallBody::Named(_)) => body_shape_err(expr, POSITIONAL_ONLY),
            Err(e) => Outcome::Done(Err(e)),
        },
        // An identity wrapper wraps one value and infers one type argument from it, so value
        // construction is an arity-1 surface; a wider family applies by name only.
        NodeSchema::TypeConstructor { param_names, .. } if param_names.len() > 1 => {
            Outcome::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                "`{}` takes {} type parameters; constructing values of a multi-parameter \
                 family is not yet supported",
                name,
                param_names.len(),
            )))))
        }
        NodeSchema::TypeConstructor { .. } => match extract_call_body(expr) {
            Ok(CallBody::Positional(parts)) => {
                constructors::dispatch_construct_apply(identity, parts)
            }
            Ok(CallBody::Named(_)) => body_shape_err(expr, POSITIONAL_ONLY),
            Err(e) => Outcome::Done(Err(e)),
        },
    }
}

/// Apply a type constructor to a record of named type arguments — `:(Result {Ok = Number, Error =
/// MyError})`. Each field value rides its own sub-Dispatch (the same `DepRequest::Dispatch` shape
/// construction launches), so a compound argument like `{Elem = (LIST OF Number)}` elaborates
/// through the ordinary type-expression lanes and the slot parks until it lands. The finish checks
/// the supplied keys against `param_names` and builds the args record in the constructor's declared
/// order.
fn apply_named_type_args<'step>(
    ctx: &SchedulerView<'step, '_>,
    identity: KType,
    param_names: Vec<String>,
    fields: Vec<(String, ExpressionPart<'step>)>,
) -> Outcome<'step> {
    // An empty argument record supplies no dep to park on, so it decides here — against the same
    // key check every other arity runs.
    if fields.is_empty() {
        return Outcome::Done(
            build_apply_args(identity, &param_names, Vec::new(), ctx.types()).map(|args| {
                ctx.step_ctx()
                    .type_carried(ctx.types().constructor_apply(identity, args))
            }),
        );
    }
    let (names, value_parts): (Vec<String>, Vec<ExpressionPart<'step>>) =
        fields.into_iter().unzip();
    let deps: Vec<DepRequest<'step>> = value_parts
        .into_iter()
        .map(|part| DepRequest::Dispatch {
            expr: KExpression::new(vec![Spanned::bare(part)]),
            placement: DepPlacement::OwnScope,
        })
        .collect();
    let finish: TerminalDepFinish<'step> = Box::new(move |view, terminals| {
        // Each argument is a type value cloned out as owned data, so the applied type embeds no
        // borrow of a producer's region and needs no carrier fold.
        let supplied: Result<Vec<(String, KType)>, KError> = terminals
            .owned_slice()
            .iter()
            .zip(&names)
            .map(|(terminal, name)| match terminal.value {
                Carried::Type(kt) => Ok((name.clone(), kt)),
                Carried::Object(object) => Err(KError::new(KErrorKind::TypeMismatch {
                    arg: name.clone(),
                    expected: "Type".to_string(),
                    got: object.ktype().name(view.types()),
                })),
                Carried::UnresolvedType(ti) => {
                    Err(KError::new(KErrorKind::UnboundName(ti.render())))
                }
            })
            .collect();
        Outcome::Done(supplied.and_then(|supplied| {
            let args = build_apply_args(identity, &param_names, supplied, view.types())?;
            Ok(view
                .step_ctx()
                .type_carried(view.types().constructor_apply(identity, args)))
        }))
    });
    Await::on(Deps::from_owned(deps))
        .error_frame(dep_error_frame())
        .finish_terminal(finish)
}

/// Key-check the supplied type arguments against the constructor's declared parameters and
/// re-order them into that declaration order. The supplied key set must equal the parameter set;
/// a mismatch names the missing and the unknown keys. (`Record` identity is order-blind, so the
/// declared order is presentation — it is what `KType::name()` renders and re-parses.)
fn build_apply_args(
    identity: KType,
    param_names: &[String],
    supplied: Vec<(String, KType)>,
    types: &TypeRegistry,
) -> Result<Record<KType>, KError> {
    let mut supplied: HashMap<String, KType> = supplied.into_iter().collect();
    let missing: Vec<&str> = param_names
        .iter()
        .map(String::as_str)
        .filter(|name| !supplied.contains_key(*name))
        .collect();
    let mut unknown: Vec<&str> = supplied
        .keys()
        .map(String::as_str)
        .filter(|name| !param_names.iter().any(|p| p == name))
        .collect();
    unknown.sort_unstable();
    if !missing.is_empty() || !unknown.is_empty() {
        let mut problems = Vec::new();
        if !missing.is_empty() {
            problems.push(format!("missing {}", quoted_list(&missing)));
        }
        if !unknown.is_empty() {
            problems.push(format!("unknown {}", quoted_list(&unknown)));
        }
        let declared: Vec<&str> = param_names.iter().map(String::as_str).collect();
        return Err(KError::new(KErrorKind::ShapeError(format!(
            "`{}` takes type parameters {} — {}",
            identity.name(types),
            quoted_list(&declared),
            problems.join(", "),
        ))));
    }
    Ok(Record::from_pairs(param_names.iter().map(|name| {
        let arg = supplied
            .remove(name)
            .expect("every declared parameter is supplied — the key check passed");
        (name.clone(), arg)
    })))
}

/// Backtick-quote and comma-join names for a parameter-mismatch diagnostic.
fn quoted_list(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Construct from an anonymous union of per-variant newtype members (a user `UNION`). `Maybe Some`
/// (a bare `Type` token body) yields the variant member's type value, reached through its union;
/// `Maybe (Some v)` (a paren-group body) constructs the named member as a `KObject::Tagged` —
/// the same value shape builtin `Result` produces — so `MATCH` dispatches user unions by tag
/// string through the shared `TaggedByTag` path. An unknown variant name in either form is a
/// schema error listing the union's members.
fn apply_union_construct<'step>(
    ctx: &SchedulerView<'step, '_>,
    members: Vec<KType>,
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
        return match union_member(&members, &name, ctx.types()) {
            Some(member) => Outcome::Done(Ok(ctx.step_ctx().type_carried(member))),
            None => Outcome::Done(Err(unknown_variant_error(&members, &name, ctx.types()))),
        };
    }
    // Payload construction: `Maybe (Some v)` (paren-group body) builds the variant value. A
    // user-union variant is a `Tagged` — the same value shape builtin `Result` produces — so
    // `MATCH` dispatches user unions by tag string through the shared `TaggedByTag` path. The tag
    // names which member; the value's `identity` is that member's own sealed handle.
    match extract_call_body(expr) {
        Ok(CallBody::Positional(parts)) => {
            let (tag, value_part) = match constructors::prepare_args(parts) {
                Ok(v) => v,
                Err(e) => return Outcome::Done(Err(e)),
            };
            match union_member(&members, &tag, ctx.types()) {
                Some(member) => constructors::construct_tagged(
                    member,
                    Rc::new(union_variant_schema(&members, ctx.types())),
                    tag,
                    value_part,
                ),
                None => Outcome::Done(Err(unknown_variant_error(&members, &tag, ctx.types()))),
            }
        }
        Ok(CallBody::Named(_)) => body_shape_err(expr, POSITIONAL_ONLY),
        Err(e) => Outcome::Done(Err(e)),
    }
}

/// The variant schema of an anonymous union of sealed newtype members: each member's tag mapped
/// to its declared payload type (its `NewType` repr). This is the per-value type-check table the
/// `Tagged` finish reads (`schema[tag]`), matching the shape builtin `Result` supplies.
fn union_variant_schema(members: &[KType], types: &TypeRegistry) -> HashMap<String, KType> {
    members
        .iter()
        .filter_map(|m| match types.node(*m) {
            TypeNode::SetMember {
                name,
                schema: NodeSchema::NewType(repr),
                ..
            } => Some((name, repr)),
            _ => None,
        })
        .collect()
}

/// The union member whose sealed newtype is named `name`, if any.
fn union_member(members: &[KType], name: &str, types: &TypeRegistry) -> Option<KType> {
    members.iter().copied().find(|m| match types.node(*m) {
        TypeNode::SetMember {
            name: member_name, ..
        } => member_name == name,
        _ => false,
    })
}

/// A schema error for a name that is not one of the union's variants, listing the members.
fn unknown_variant_error(members: &[KType], name: &str, types: &TypeRegistry) -> KError {
    KError::new(KErrorKind::ShapeError(format!(
        "`{name}` is not a variant of the union (variants: {})",
        union_member_names(members, types),
    )))
}

/// Sorted, comma-joined member names of an anonymous union of sealed newtype members.
fn union_member_names(members: &[KType], types: &TypeRegistry) -> String {
    let mut names: Vec<String> = members
        .iter()
        .filter_map(|m| match types.node(*m) {
            TypeNode::SetMember { name, .. } => Some(name),
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
    let wrap_indices = picked.classify_for_pick(&expr, ctx.types()).wrap_indices;
    let (new_parts, staged_subs) = stage_all_eager_parts(expr.parts, &wrap_indices);
    let working_expr = KExpression::new(new_parts);
    ctx.install_eager_subs(working_expr, staged_subs, Some(picked))
}
