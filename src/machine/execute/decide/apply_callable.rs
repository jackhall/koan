//! The shared apply-a-callable tail.
//!
//! Every dispatch lane that resolves a head to *something callable* — `TypeCall`
//! (bare-`Type` head), `FunctionValueCall` (lowercase-identifier head), and the
//! head-deferred lanes (a head expression / `:(...)` sigil that is evaluated
//! first) — funnels its resolved callable through [`apply_callable`]. The lane
//! does the resolution; this tail does the body-shape branching and launches
//! construction or a function call.

use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::{DepPlacement, OpenedFunction};
use crate::machine::model::{Carried, Record, constructor_param_names};
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::machine::model::{KType, NodeSchema, TypeNode, TypeRegistry};
use crate::machine::{KError, KErrorKind};
use crate::scheduler::Deps;
use crate::source::Spanned;

use super::super::TerminalDepFinish;
use super::super::outcome::dep_error_frame;
use super::ctx::DecideCtx;
use super::{Await, DepRequest, Outcome};
use super::{PartWalk, constructors, stage_all_eager_parts};

#[cfg(test)]
mod tests;

/// The argument body of a `head (...)` / `head {...}` call, classified by surface shape.
///
/// The resolved verb-carrier decides which shape it admits; the mismatched shape surfaces a
/// loud `DispatchFailed`.
enum CallBody<'step> {
    /// A `{x = 1}` record literal — named arguments.
    Named(&'step [(&'step str, ExpressionPart<'step>)]),
    /// A `(Error "x")` paren group — positional construction (tagged unions, newtypes).
    Positional(&'step [Spanned<ExpressionPart<'step>>]),
}

/// Classify the single body part of a `head (...)` / `head {...}` call from
/// `expr.parts[1..]`. The body must be exactly one nested-parens (`Positional`) or one
/// record literal (`Named`); anything else is a non-match.
fn extract_call_body<'step>(expr: &WorkingExpression<'step>) -> Result<CallBody<'step>, KError> {
    match &expr.parts[1..] {
        [
            Spanned {
                value: WorkingPart::Ast(ExpressionPart::RecordLiteral(fields)),
                ..
            },
        ] => Ok(CallBody::Named(fields)),
        [
            Spanned {
                value: WorkingPart::Ast(ExpressionPart::Expression(inner)),
                ..
            },
        ] => Ok(CallBody::Positional(inner.parts)),
        _ => Err(KError::new(KErrorKind::DispatchFailed {
            expr: expr.summarize(),
            reason: "no matching function".to_string(),
        })),
    }
}

/// Reason strings for the loud `DispatchFailed` raised when a call body's surface shape
/// doesn't match what the resolved verb-carrier admits.
const NAMED_ONLY: &str =
    "named arguments use a record literal `{name = value}`, not a parenthesized group";
const POSITIONAL_ONLY: &str =
    "positional construction takes `(value)`, not a record literal `{name = value}`";

fn body_shape_err<'step>(expr: &WorkingExpression<'step>, reason: &str) -> Outcome<'step> {
    Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
        expr: expr.summarize(),
        reason: reason.to_string(),
    })))
}

/// A head resolved to something callable. The resolving lane decides which arm.
pub(in crate::machine::execute) enum ResolvedCallable<'step> {
    /// Build from a type identity — a sealed nominal member, an anonymous union, or a type
    /// constructor applied to named type arguments.
    Constructor { identity: KType },
    /// Call a callable by name, in the **in-use** carrier state its resolving lane adopted it into
    /// — so the callable rides the apply tail fused to the reach that proves it.
    Function(OpenedFunction<'step>),
}

/// Body-shape-branch the resolved callable and launch. `expr.parts[1..]` is the call body.
pub(in crate::machine::execute) fn apply_callable<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    callable: ResolvedCallable<'step>,
    expr: &WorkingExpression<'step>,
) -> Outcome<'step> {
    match callable {
        // A constructor decides its admitted body shape per schema arm — the newtype arm takes
        // the trailing parts directly, so `(Point r)` works — hence body extraction lives inside
        // `apply_constructor` rather than here.
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

/// Construct from a type identity, or apply a type constructor to named type arguments.
///
/// A record-literal body on a constructor-kind head (`Wrap {Elem = Number}`) is *type
/// application*, yielding a `ConstructorApply` type value. Otherwise a newtype bypasses the
/// `{name = value}` / `(value)` body split — it takes the trailing parts directly as its
/// value expression, so `(Point {x = 1, y = 2})` builds a record and `(Point r)` /
/// `(Distance 3.0)` wrap a value. The tagged and identity-wrapper arms take a positional
/// `(value)` body; a named one there is a loud `DispatchFailed`.
fn apply_constructor<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    identity: KType,
    expr: &WorkingExpression<'step>,
) -> Outcome<'step> {
    let brand = ctx.current_scope().brand();
    // A user `UNION` binds an anonymous union of per-variant newtype members. `Maybe Some`
    // names the variant type; `Maybe (Some v)` newtype-constructs the named member.
    if let TypeNode::Union { members } = ctx.types().node(identity) {
        return apply_union_construct(ctx, members, expr);
    }
    // Named type application precedes every construction arm: on a type-constructor head the
    // record body is a type-argument list, not a value, and the two surfaces are disjoint.
    if let Some(param_names) = constructor_param_names(identity, ctx.types())
        && let Some(
            [
                Spanned {
                    value: WorkingPart::Ast(ExpressionPart::RecordLiteral(fields)),
                    ..
                },
            ],
        ) = expr.parts.get(1..)
    {
        return apply_named_type_args(ctx, identity, param_names, fields);
    }
    // A SIG's abstract constructor slot names a kind; it has no representation to build values
    // over. Its first-order sibling carries no parameters and falls to the generic mismatch.
    if let TypeNode::AbstractType {
        name, param_names, ..
    } = ctx.types().node(identity)
        && !param_names.is_empty()
    {
        return Outcome::Done(Err(KError::new(KErrorKind::ShapeError(format!(
            "`{name}` is an abstract constructor slot declared by TYPE; only a \
                 NEWTYPE-declared constructor can construct values"
        )))));
    }
    let TypeNode::SetMember { schema, name, .. } = ctx.types().node(identity) else {
        return Outcome::Done(Err(KError::new(KErrorKind::TypeMismatch {
            arg: "verb".to_string(),
            expected: "constructible Type".to_string(),
            got: identity.name(ctx.types()),
        })));
    };
    match schema {
        // A record-literal body builds per-field; any other trailing expression is wrapped as a
        // single positional value.
        NodeSchema::NewType(_) => match expr.parts.get(1..) {
            Some(
                [
                    Spanned {
                        value: WorkingPart::Ast(ExpressionPart::RecordLiteral(fields)),
                        ..
                    },
                ],
            ) => constructors::dispatch_construct_record_newtype(brand, identity, fields),
            _ => constructors::dispatch_construct_newtype(brand, identity, &expr.parts[1..]),
        },
        // A non-empty schema is `Result`'s variant schema — the sealed tagged-union path. An
        // empty schema is a declared constructor family (`NEWTYPE (Elem AS Wrapper)`); it
        // constructs an identity-wrapper `Wrapped` value.
        NodeSchema::TypeConstructor {
            schema: variant_schema,
            ..
        } if !variant_schema.is_empty() => match extract_call_body(expr) {
            Ok(CallBody::Positional(parts)) => constructors::dispatch_construct_tagged(
                brand,
                identity,
                Rc::new(variant_schema),
                parts,
            ),
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
                constructors::dispatch_construct_apply(brand, identity, parts)
            }
            Ok(CallBody::Named(_)) => body_shape_err(expr, POSITIONAL_ONLY),
            Err(e) => Outcome::Done(Err(e)),
        },
    }
}

/// Apply a type constructor to a record of named type arguments — `:(Result {Ok = Number, Error =
/// MyError})`. Each field value rides its own sub-Dispatch, so a compound argument like
/// `{Elem = (LIST OF Number)}` elaborates through the ordinary type-expression lanes and the slot
/// parks until it lands.
fn apply_named_type_args<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    identity: KType,
    param_names: Vec<String>,
    fields: &[(&'step str, ExpressionPart<'step>)],
) -> Outcome<'step> {
    // An empty argument record supplies no dep to park on, so it decides here — against the same
    // key check the non-empty path runs.
    if fields.is_empty() {
        return Outcome::Done(
            build_apply_args(identity, &param_names, Vec::new(), ctx.types()).map(|args| {
                ctx.step_ctx()
                    .type_carried(ctx.types().constructor_apply(identity, args))
            }),
        );
    }
    let brand = ctx.current_scope().brand();
    let (names, value_parts): (Vec<String>, Vec<ExpressionPart<'step>>) = fields
        .iter()
        .map(|(name, part)| ((*name).to_string(), *part))
        .unzip();
    let deps: Vec<DepRequest<'step>> = value_parts
        .into_iter()
        .map(|part| DepRequest::Dispatch {
            expr: WorkingExpression::new(brand, &[Spanned::bare(WorkingPart::Ast(part))]),
            placement: DepPlacement::OwnScope,
        })
        .collect();
    let finish: TerminalDepFinish<'step> = Box::new(move |view, terminals| {
        // Each argument is a type value cloned out as owned data, so the applied type embeds no
        // borrow of a producer's region and needs no carrier fold.
        let supplied: Result<Vec<(String, KType)>, KError> = terminals
            .iter()
            .zip(&names)
            .map(|(terminal, name)| match terminal.cell.open_at().value() {
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
    Await::on(Deps::from_requests(deps))
        .error_frame(dep_error_frame())
        .finish_terminal(finish)
}

/// Key-check the supplied type arguments against the constructor's declared parameters and
/// re-order them into that declaration order. `Record` identity is order-blind, so the declared
/// order is presentation — it is what `KType::name()` renders and re-parses.
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
/// string through the shared `TaggedByTag` path.
fn apply_union_construct<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    members: Vec<KType>,
    expr: &WorkingExpression<'step>,
) -> Outcome<'step> {
    if let [
        Spanned {
            value: WorkingPart::Ast(ExpressionPart::Type(t)),
            ..
        },
    ] = &expr.parts[1..]
    {
        let name = t.render();
        return match union_member(&members, &name, ctx.types()) {
            Some(member) => Outcome::Done(Ok(ctx.step_ctx().type_carried(member))),
            None => Outcome::Done(Err(unknown_variant_error(&members, &name, ctx.types()))),
        };
    }
    // The tag names which member; the built value's `identity` is that member's own sealed handle.
    match extract_call_body(expr) {
        Ok(CallBody::Positional(parts)) => {
            let (tag, value_part) = match constructors::prepare_args(parts) {
                Ok(v) => v,
                Err(e) => return Outcome::Done(Err(e)),
            };
            match union_member(&members, &tag, ctx.types()) {
                Some(member) => constructors::construct_tagged(
                    ctx.current_scope().brand(),
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
/// `Tagged` finish reads (`schema[tag]`).
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

/// Apply a resolved function to its call body. A function takes `{name = value}` only; a
/// positional body is a loud `DispatchFailed`. The named record reconstructs the exact-arity
/// keyworded expression, whose value slots then eager-resolve before binding.
fn apply_function<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    f: OpenedFunction<'step>,
    expr: &WorkingExpression<'step>,
    body: CallBody<'step>,
) -> Outcome<'step> {
    match body {
        CallBody::Named(fields) => {
            let brand = ctx.current_scope().brand();
            let fields = fields
                .iter()
                .map(|(name, part)| ((*name).to_string(), *part))
                .collect();
            match f.value().reconstruct_positional(brand, fields) {
                Ok(rebuilt) => install_eager_subs_track(ctx, rebuilt, f),
                Err(e) => Outcome::Done(Err(e)),
            }
        }
        CallBody::Positional(_) => body_shape_err(expr, NAMED_ONLY),
    }
}

/// Stage every eager part of a reconstructed call as a sub-Dispatch and hand the staged call to
/// [`install_eager_subs`](super::keyworded::install_eager_subs) with the committed pick.
pub(in crate::machine::execute) fn install_eager_subs_track<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
    picked: OpenedFunction<'step>,
) -> Outcome<'step> {
    // `picked` is already committed, so bare-name value slots resolve by sub-Dispatch rather than
    // the keyword path's pre-pick `bare_outcomes` lookup — each rides `bare_identifier`'s reach
    // carrier through the eager-subs finish and reaches `accepts_part` at bind.
    let brand = ctx.current_scope().brand();
    let wrap_indices = picked
        .value()
        .classify_for_pick(&expr, ctx.types())
        .wrap_indices;
    // A call whose slots are all filled stages nothing, so the node handed to the walk is the one
    // the committed call folds over — no rebuild.
    let (working_expr, staged_subs) = match stage_all_eager_parts(brand, &expr, &wrap_indices) {
        PartWalk::Unchanged => (expr, Vec::new()),
        PartWalk::Respliced { expr, staged_subs } => (expr, staged_subs),
    };
    super::keyworded::install_eager_subs(ctx, working_expr, staged_subs, Some(picked))
}
