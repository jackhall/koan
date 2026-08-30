//! The shared apply-a-callable tail.
//!
//! Every dispatch lane that resolves a head to *something callable* — `TypeCall`
//! (bare-`Type` head), `FunctionValueCall` (lowercase-identifier head), and the
//! head-deferred lanes (a head expression / `:(...)` sigil that is evaluated
//! first) — funnels its resolved callable through [`apply_callable`]. The lane
//! does the resolution; this tail does the body-shape branching and launches
//! construction or a function call.

use crate::machine::core::{DepPlacement, OpenedFunction};
use crate::machine::model::labels::{BinderSymbol, LabelInterner, Symbol, TypeSymbol};
use crate::machine::model::render_label;
use crate::machine::model::{Carried, Record, TypeMemberMap, constructor_param_names};
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::machine::model::{KType, NodeSchema, TypeNode};
use crate::machine::{KError, KErrorKind};
use crate::scheduler::Deps;
use crate::source::Spanned;

use super::super::outcome::DepTerminal;
use super::super::outcome::dep_error_frame;
use super::ctx::DecideCtx;
use super::{Await, DepRequest, Outcome};
use super::{PartWalk, StagedSubs, constructors, stage_all_eager_parts};
use crate::machine::model::RunRegistries;

#[cfg(test)]
mod tests;

/// The argument body of a `head (...)` / `head {...}` call, classified by surface shape.
///
/// The resolved verb-carrier decides which shape it admits; the mismatched shape surfaces a
/// loud `DispatchFailed`.
enum CallBody<'step> {
    /// A `{x = 1}` record literal — named arguments.
    Named(&'step [(BinderSymbol, ExpressionPart<'step>)]),
    /// A `(Error "x")` paren group — positional construction (tagged unions, newtypes).
    Positional(&'step [Spanned<ExpressionPart<'step>>]),
}

/// Classify the single body part of a `head (...)` / `head {...}` call from
/// `expr.parts[1..]`. The body must be exactly one nested-parens (`Positional`) or one
/// record literal (`Named`); anything else is a non-match.
fn extract_call_body<'step>(
    expr: &WorkingExpression<'step>,
    labels: &LabelInterner,
) -> Result<CallBody<'step>, KError> {
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
            expr: expr.summarize(labels),
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

fn body_shape_err<'step>(
    expr: &WorkingExpression<'step>,
    reason: &str,
    labels: &LabelInterner,
) -> Outcome<'step> {
    Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
        expr: expr.summarize(labels),
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
            let body = match extract_call_body(expr, &ctx.registries().labels) {
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
    if ctx.types().is_union(identity) {
        return apply_union_construct(ctx, identity, expr);
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
        let name = render_label(name.symbol(), ctx.registries());
        return Outcome::Done(Err(KError::new(KErrorKind::ShapeError(format!(
            "`{name}` is an abstract constructor slot declared by TYPE; only a \
                 NEWTYPE-declared constructor can construct values"
        )))));
    }
    let TypeNode::SetMember { schema, name, .. } = ctx.types().node(identity) else {
        return Outcome::Done(Err(KError::new(KErrorKind::TypeMismatch {
            arg: "verb".to_string(),
            expected: "constructible Type".to_string(),
            got: identity.name(ctx.registries()),
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
            ) => constructors::dispatch_construct_record_newtype(
                brand,
                identity,
                fields,
                ctx.scratch(),
            ),
            _ => constructors::dispatch_construct_newtype(
                brand,
                identity,
                &expr.parts[1..],
                ctx.scratch(),
            ),
        },
        // A non-empty schema is `Result`'s variant schema — the sealed tagged-union path. An
        // empty schema is a declared constructor family (`NEWTYPE (Elem AS Wrapper)`); it
        // constructs an identity-wrapper `Wrapped` value.
        NodeSchema::TypeConstructor {
            schema: variant_schema,
            ..
        } if !variant_schema.is_empty() => {
            match extract_call_body(expr, &ctx.registries().labels) {
                Ok(CallBody::Positional(parts)) => constructors::dispatch_construct_tagged(
                    brand,
                    identity,
                    &variant_schema,
                    parts,
                    ctx.registries(),
                    ctx.scratch(),
                ),
                Ok(CallBody::Named(_)) => {
                    body_shape_err(expr, POSITIONAL_ONLY, &ctx.registries().labels)
                }
                Err(e) => Outcome::Done(Err(e)),
            }
        }
        // An identity wrapper wraps one value and infers one type argument from it, so value
        // construction is an arity-1 surface; a wider family applies by name only.
        NodeSchema::TypeConstructor { param_names, .. } if param_names.len() > 1 => {
            Outcome::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                "`{}` takes {} type parameters; constructing values of a multi-parameter \
                 family is not yet supported",
                render_label(name.symbol(), ctx.registries()),
                param_names.len(),
            )))))
        }
        NodeSchema::TypeConstructor { .. } => {
            match extract_call_body(expr, &ctx.registries().labels) {
                Ok(CallBody::Positional(parts)) => {
                    constructors::dispatch_construct_apply(brand, identity, parts, ctx.scratch())
                }
                Ok(CallBody::Named(_)) => {
                    body_shape_err(expr, POSITIONAL_ONLY, &ctx.registries().labels)
                }
                Err(e) => Outcome::Done(Err(e)),
            }
        }
    }
}

/// One supplied type argument on its way to [`build_apply_args`]: the name's bare symbol bits as
/// the record-literal key carries them, and the resolved argument. A miss renders the name from
/// the interner at the error site.
type SuppliedTypeArgument = (Symbol, KType);

/// Apply a type constructor to a record of named type arguments — `:(Result {Ok = Number, Error =
/// MyError})`. Each field value rides its own sub-Dispatch, so a compound argument like
/// `{Elem = (LIST OF Number)}` elaborates through the ordinary type-expression lanes and the slot
/// parks until it lands.
fn apply_named_type_args<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    identity: KType,
    param_names: Vec<TypeSymbol>,
    fields: &[(BinderSymbol, ExpressionPart<'step>)],
) -> Outcome<'step> {
    // An empty argument record supplies no dep to park on, so it decides here — against the same
    // key check the non-empty path runs.
    if fields.is_empty() {
        return Outcome::Done(
            build_apply_args(identity, &param_names, Vec::new(), ctx.registries()).map(|args| {
                ctx.step_ctx()
                    .type_carried(ctx.types().constructor_apply(identity, args))
            }),
        );
    }
    let brand = ctx.current_scope().brand();
    // Argument names are record-literal keys, minted at parse and matched by symbol bits from
    // here on. A reference is not a declaration, so this is a bare probe through the recovery
    // door; a name matching no declared parameter renders its own text in the miss.
    // Both name runs cross the park inside the finish, so they land in the host frame region.
    let names: &'step [Symbol] = brand
        .allocator()
        .slice_from_iter(fields.iter().map(|(name, _)| name.symbol()));
    let param_names: &'step [TypeSymbol] = brand.allocator().slice(&param_names);
    let value_parts: Vec<ExpressionPart<'step>> = fields.iter().map(|(_, part)| *part).collect();
    let deps: Vec<DepRequest<'step>> = value_parts
        .into_iter()
        .map(|part| DepRequest::Dispatch {
            expr: WorkingExpression::new(brand, &[Spanned::bare(WorkingPart::Ast(part))]),
            placement: DepPlacement::OwnScope,
        })
        .collect();
    let finish = move |view: &DecideCtx<'_, 'step, '_>, terminals: &[DepTerminal<'_>]| {
        // Each argument is a type value cloned out as owned data, so the applied type embeds no
        // borrow of a producer's region and needs no carrier fold.
        let supplied: Result<Vec<SuppliedTypeArgument>, KError> = terminals
            .iter()
            .zip(names)
            .map(|(terminal, symbol)| match terminal.cell.open_at().value() {
                Carried::Type(kt) => Ok((*symbol, kt)),
                Carried::Object(object) => Err(KError::new(KErrorKind::TypeMismatch {
                    arg: render_label(*symbol, view.registries()),
                    expected: "Type".to_string(),
                    got: object.ktype().name(view.registries()),
                })),
                Carried::UnresolvedType(ti) => Err(KError::new(KErrorKind::UnboundName(
                    render_label(ti.symbol(), view.registries()),
                ))),
            })
            .collect();
        Outcome::Done(supplied.and_then(|supplied| {
            let args = build_apply_args(identity, param_names, supplied, view.registries())?;
            Ok(view
                .step_ctx()
                .type_carried(view.types().constructor_apply(identity, args)))
        }))
    };
    Await::on(Deps::from_requests_in(deps, ctx.scratch()))
        .error_frame(dep_error_frame())
        .finish_terminal(brand, finish)
}

/// Key-check the supplied type arguments against the constructor's declared parameters and
/// re-order them into that declaration order. `Record` identity is order-blind, so the declared
/// order is presentation — it is what `KType::name()` renders and re-parses.
fn build_apply_args(
    identity: KType,
    param_names: &[TypeSymbol],
    supplied: Vec<SuppliedTypeArgument>,
    registries: &RunRegistries,
) -> Result<Record<KType>, KError> {
    // The supplied names carry bare symbol bits; a declared parameter that matches contributes its
    // own classified key, and a name matching none joins the misspellings in `unknown`.
    let mut matched = TypeMemberMap::default();
    let mut unknown: Vec<Symbol> = Vec::new();
    for (symbol, kt) in &supplied {
        match param_names.iter().find(|param| param.symbol() == *symbol) {
            Some(param) => {
                matched.insert(*param, *kt);
            }
            None => unknown.push(*symbol),
        }
    }
    let missing: Vec<TypeSymbol> = param_names
        .iter()
        .copied()
        .filter(|name| !matched.contains_key(name))
        .collect();
    if !missing.is_empty() || !unknown.is_empty() {
        // The declared names resolve through the interner, which recorded them at the
        // constructor's declaration — on the miss path only, so a satisfied key check renders
        // nothing at all.
        let render = |names: &[TypeSymbol]| -> Vec<String> {
            names
                .iter()
                .map(|name| render_label(name.symbol(), registries))
                .collect()
        };
        fn borrow(names: &[String]) -> Vec<&str> {
            names.iter().map(String::as_str).collect()
        }
        let missing = render(&missing);
        let declared = render(param_names);
        let mut unknown: Vec<String> = unknown
            .iter()
            .map(|symbol| render_label(*symbol, registries))
            .collect();
        unknown.sort_unstable();
        let mut problems = Vec::new();
        if !missing.is_empty() {
            problems.push(format!("missing {}", quoted_list(&borrow(&missing))));
        }
        if !unknown.is_empty() {
            problems.push(format!("unknown {}", quoted_list(&borrow(&unknown))));
        }
        return Err(KError::new(KErrorKind::ShapeError(format!(
            "`{}` takes type parameters {} — {}",
            identity.display_name(registries),
            quoted_list(&borrow(&declared)),
            problems.join(", "),
        ))));
    }
    // A constructor parameter is declared by a `Type` token, so the symbol it already carries
    // keys the record with its class intact.
    Ok(Record::from_pairs(param_names.iter().map(|name| {
        let arg = matched
            .remove(name)
            .expect("every declared parameter is supplied — the key check passed");
        (BinderSymbol::Type(*name), arg)
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
/// symbol through the shared `TaggedByTag` path.
fn apply_union_construct<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    union: KType,
    expr: &WorkingExpression<'step>,
) -> Outcome<'step> {
    if let [
        Spanned {
            value: WorkingPart::Ast(ExpressionPart::Type(t)),
            ..
        },
    ] = &expr.parts[1..]
    {
        // The token names a variant, so it probes the members by bare symbol bits — the
        // recovery door. A bare-token reference is a lookup, not a declaration, and the miss
        // renders the token through the interner the parser recorded it in.
        return match ctx.types().union_member_named(union, t.symbol()) {
            Some(member) => Outcome::Done(Ok(ctx.step_ctx().type_carried(member))),
            None => Outcome::Done(Err(unknown_variant_error(union, *t, ctx.registries()))),
        };
    }
    // The tag names which member; the built value's `identity` is that member's own sealed handle.
    match extract_call_body(expr, &ctx.registries().labels) {
        Ok(CallBody::Positional(parts)) => {
            let (tag, value_part) = match constructors::prepare_args(parts, ctx.registries()) {
                Ok(v) => v,
                Err(e) => return Outcome::Done(Err(e)),
            };
            match ctx.types().union_variant_target(union, tag.symbol()) {
                Some((member, expected)) => constructors::construct_tagged(
                    ctx.current_scope().brand(),
                    member,
                    expected,
                    tag,
                    value_part,
                    ctx.scratch(),
                ),
                None => Outcome::Done(Err(unknown_variant_error(union, tag, ctx.registries()))),
            }
        }
        Ok(CallBody::Named(_)) => body_shape_err(expr, POSITIONAL_ONLY, &ctx.registries().labels),
        Err(e) => Outcome::Done(Err(e)),
    }
}

/// A schema error for a name that is not one of the union's variants, listing the members. `name`
/// is the probed tag's **source text**, so the message names what the expression spelled even when
/// nothing interned it.
fn unknown_variant_error(union: KType, name: TypeSymbol, registries: &RunRegistries) -> KError {
    KError::new(KErrorKind::ShapeError(format!(
        "`{}` is not a variant of the union (variants: {})",
        render_label(name.symbol(), registries),
        union_member_names(union, registries),
    )))
}

/// Sorted, comma-joined names of the union's constructible variants — its sealed `NewType`
/// members. A member declaring any other schema names no tag payload, so it is no variant and goes
/// unlisted. Each name resolves through the run's label interner, which recorded it at its
/// declaration. A cold diagnostic path, so it reads the member list out of the node by clone
/// rather than through the construction lane's allocation-free probes.
fn union_member_names(union: KType, registries: &RunRegistries) -> String {
    let members = match registries.types.node(union) {
        TypeNode::Union { members } => members,
        _ => Vec::new(),
    };
    let mut names: Vec<String> = members
        .iter()
        .filter_map(|m| match registries.types.node(*m) {
            TypeNode::SetMember {
                name,
                schema: NodeSchema::NewType(_),
                ..
            } => Some(render_label(name.symbol(), registries)),
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
            // A named-argument label carries its parse-minted symbol, which matches the
            // parameter symbol the signature already carries.
            let fields = fields
                .iter()
                .map(|(name, part)| (name.symbol(), *part))
                .collect();
            match f.value().reconstruct_positional(
                brand,
                fields,
                expr.source_ref(),
                ctx.registries(),
            ) {
                Ok(rebuilt) => install_eager_subs_track(ctx, rebuilt, f),
                Err(e) => Outcome::Done(Err(e)),
            }
        }
        CallBody::Positional(_) => body_shape_err(expr, NAMED_ONLY, &ctx.registries().labels),
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
        .classify_for_pick(&expr, ctx.registries(), ctx.scratch())
        .wrap_indices;
    // A call whose slots are all filled stages nothing, so the node handed to the walk is the one
    // the committed call folds over — no rebuild.
    let (working_expr, staged) =
        match stage_all_eager_parts(brand, &expr, &wrap_indices, ctx.scratch()) {
            PartWalk::Unchanged => (expr, StagedSubs::new_in(ctx.scratch())),
            PartWalk::Respliced { expr, staged } => (expr, staged),
        };
    super::keyworded::install_eager_subs(ctx, working_expr, staged, Some(picked))
}
