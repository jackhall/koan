//! Keyworded dispatch shape: the catch-all for any expression with a
//! keyword present, or a head that isn't a fast-lane shape.

use crate::machine::ProducerId;
use crate::machine::core::OpenedFunction;
use crate::machine::core::location_from_expr;
use crate::machine::model::labels::BinderSymbol;
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart, diagnose_miss};
use crate::machine::{DispatchOutcome, KError, KErrorKind};
use crate::source::Spanned;

use super::super::nodes::WorkLabel;

use super::super::outcome::DepTerminal;
use super::super::outcome::continue_inline;
use super::super::{NodeContinuation, decide_only, erase_bumped};
use super::ctx::DecideCtx;
use super::{
    Await, Outcome, PartWalk, Resolution, StagedSubs, park_resume_labelled, working_frame,
};

/// Entry from the dispatch router.
///
/// Staging runs **first**: every eager-shaped part the node's lazy-slot stamp does not keep raw is
/// submitted as a sub-dispatch before any overload is considered. So by the time resolution runs,
/// each of those children has already evaluated and dispatch selects on landed values, never on
/// which overload would have left a child unevaluated. The subs' finish re-enters here, where the
/// spliced run stages nothing further and falls through to resolution.
///
/// Resolution failures are slot-terminal (TRY-catchable), uniform with the bare-identifier and
/// head-deferred lanes; `ParkOnProducers` re-runs this function on wake.
pub(super) fn initial<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let brand = ctx.current_scope().brand();
    match super::stage_all_eager_parts(brand, &expr, &[], ctx.scratch()) {
        PartWalk::Respliced {
            expr: staged_expr,
            staged,
        } => install_eager_subs(ctx, staged_expr, staged, None),
        PartWalk::Unchanged => resolve_and_invoke(ctx, expr),
    }
}

/// Resolve `expr` against the lexical scope chain and route the outcome. Every child that
/// evaluates has landed by now, so this is the single dispatch resolution: one pick, then the
/// invoke.
fn resolve_and_invoke<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    debug_assert!(
        expr.parts.iter().enumerate().all(|(i, part)| {
            !super::is_eager_working_part(&part.value) || super::stays_raw(&expr, i, &part.value)
        }),
        "every eager part the stamp leaves eager is staged before dispatch resolves",
    );
    let bare_outcomes = ctx.build_bare_outcomes(expr.parts);
    let chain = ctx.chain_deref();
    // Resolving against the cart scope puts the pick at the cart `'step` lifetime, so it reaches
    // `invoke_continue` with no re-anchor.
    let scope = ctx.current_scope();
    let outcome = scope.resolve_dispatch(
        &expr,
        chain,
        &bare_outcomes,
        ctx.registries(),
        ctx.scratch(),
    );
    let resolved = match outcome {
        DispatchOutcome::Resolved(r) => r,
        DispatchOutcome::Ambiguous(n) => {
            let named = splice_resolved_names(ctx, &expr, &bare_outcomes).unwrap_or(expr);
            return Outcome::Done(Err(KError::new(KErrorKind::AmbiguousDispatch {
                expr: named.summarize(ctx.registries()),
                candidates: n,
                location: location_from_expr(&named),
            })));
        }
        DispatchOutcome::Unmatched { quote_hint } => {
            if let Some(diagnosed) = diagnose_miss(&expr, ctx.registries()) {
                return Outcome::Done(Err(KError::new(KErrorKind::ShapeError(diagnosed))));
            }
            let named = splice_resolved_names(ctx, &expr, &bare_outcomes).unwrap_or(expr);
            return Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
                expr: named.summarize(ctx.registries()),
                reason: unmatched_reason(quote_hint),
                location: location_from_expr(&named),
            })));
        }
        DispatchOutcome::UnboundName(name, role) => {
            // A diagnosable shape speaks first: the name a mis-spelled slot holds is *why* nothing
            // matched, and reporting it as merely unbound would bury the mistake. `-> er` is the
            // standing case — a parameter name is unbound in the defining scope, so the shape
            // reaches this arm rather than the miss above.
            if let Some(diagnosed) = diagnose_miss(&expr, ctx.registries()) {
                return Outcome::Done(Err(KError::new(KErrorKind::ShapeError(diagnosed))));
            }
            let spelling = crate::machine::model::render_label(name.symbol(), ctx.registries());
            // A slot that declared a role keeps the pointed noun its body used to render before the
            // lane owned the resolution; every other slot reports the bare unbound name.
            return Outcome::Done(Err(KError::new(match role {
                Some(role) => {
                    KErrorKind::ShapeError(format!("{role} `{spelling}` is not a known type"))
                }
                None => KErrorKind::UnboundName(spelling),
            })));
        }
        DispatchOutcome::ParkOnProducers(sources) => {
            return park_on_claims(&sources, expr, ctx);
        }
    };
    // Binder name claims / pending overload slots are installed at statement submission from the
    // enclosing statement's parse-time aggregate (see `submit_expression`); nothing installs here.
    let expr =
        splice_wrap_slots(ctx, &expr, &bare_outcomes, &resolved.wrap_indices).unwrap_or(expr);
    super::exec::invoke_continue(ctx, resolved.function, expr)
}

/// Resplice every bare name the pre-dispatch scan resolved *except* the binder name, for a miss the
/// caller is about to render. [`splice_wrap_slots`] does the same for the picked wrap set on the
/// success path; a miss has no pick, so it splices the rest of the lot.
///
/// Without this a diagnostic renders the parts run as it stood before resolution, where a bare name
/// is still its own token — so a summary that names the type dispatch matched each slot on would
/// report the *token's* type (`Identifier`) rather than the type of the value the name is bound to.
/// The resolution has already happened by then; it just lives beside the parts, in `bare_outcomes`.
///
/// A binder name slot is the one position dispatch does not match on a type: it is the name being
/// declared, and a summary spells it out. Splicing there would replace the spelling the render
/// wants with a value the position never denoted — so the exclusion mirrors the wrap
/// classification's, which skips the same slot on the success path.
fn splice_resolved_names<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: &WorkingExpression<'step>,
    bare_outcomes: &[Option<Resolution>],
) -> Option<WorkingExpression<'step>> {
    let binder_name_slot = expr.binder_name_slot();
    let splices = |index: usize, outcome: &Option<Resolution>| {
        Some(index) != binder_name_slot && matches!(outcome, Some(Resolution::Resolved(_)))
    };
    if !bare_outcomes
        .iter()
        .enumerate()
        .any(|(index, outcome)| splices(index, outcome))
    {
        return None;
    }
    let brand = ctx.current_scope().brand();
    Some(expr.respliced(
        brand,
        expr.parts.iter().enumerate().map(|(i, part)| {
            let Some(outcome) = bare_outcomes.get(i).filter(|o| splices(i, o)) else {
                return *part;
            };
            let Some(Resolution::Resolved(cell)) = outcome else {
                return *part;
            };
            Spanned {
                value: WorkingPart::Spliced {
                    cell: ctx.current_scope().rest_delivered(cell),
                    from_name: bare_part_name(&part.value),
                },
                span: part.span,
            }
        }),
    ))
}

/// Why nothing matched. A slot typed `:KExpression` in the bucket that took an evaluated value
/// instead gets the pointed hint: the argument ran before dispatch, so quoting it is the fix.
fn unmatched_reason(quote_hint: bool) -> String {
    if quote_hint {
        "no matching function: an argument evaluated before dispatch; write #(…) to pass the code \
         itself"
            .to_string()
    } else {
        "no matching function".to_string()
    }
}

/// Park on the claims dispatch resolution leaned on — a still-finalizing bare-name producer from
/// the pre-admission scan, a visible pending overload slot, or a forward-reference producer a
/// relaxed candidate needs — and re-run [`initial`] against `expr` on wake. The claims are
/// lexically-earlier binders still in flight (the binding tables' exclusive cutoff keeps a
/// statement's own claim out of its own subtree), so the wait is always well-founded.
fn park_on_claims<'step>(
    sources: &[ProducerId],
    expr: WorkingExpression<'step>,
    view: &DecideCtx<'_, 'step, '_>,
) -> Outcome<'step> {
    let frame = working_frame("<dispatch-park>", &expr);
    park_resume_labelled(
        sources,
        Some(frame),
        view,
        move |ctx: &DecideCtx<'_, 'step, '_>, _id| initial(ctx, expr),
    )
}

/// Fold the post-eager-subs re-entry into a dep-free decide that re-runs [`initial`] on the next
/// pop. The spliced run stages nothing further, so the re-entry is the resolution the staging pass
/// deferred. The slot holds no contract of its own, so the continuation carries the ambient
/// obligation and the declared-return checker survives the hop.
fn redispatch_continue<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    working_expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let label = WorkLabel::of(&working_expr);
    let continuation = NodeContinuation::new(
        view.current_obligation(),
        erase_bumped(
            view.current_scope().brand(),
            decide_only(move |ctx: &DecideCtx<'_, 'step, '_>, _id| initial(ctx, working_expr)),
        ),
    );
    continue_inline(continuation, label)
}

/// Park each staged eager dep, then rebuild `working_expr` with the resolved carriers in its
/// staged slots and route on `picked`. Nothing is read and spliced inline before the park — that
/// would embed a producer's frame-local terminal, which its per-call frame frees at Done (it never
/// lifts), so it would dangle. Pure: every write the outcome implies is the harness's.
///
/// Shared with the post-pick lane
/// [`install_eager_subs_track`](super::apply_callable::install_eager_subs_track), which commits a
/// `picked`.
pub(super) fn install_eager_subs<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    working_expr: WorkingExpression<'step>,
    staged: StagedSubs<'step>,
    picked: Option<OpenedFunction<'step>>,
) -> Outcome<'step> {
    if staged.is_empty() {
        return finish_eager_subs(ctx, working_expr, picked);
    }
    let StagedSubs { part_indices, deps } = staged;
    // The slot run crosses the park inside the finish, so it lands in the host frame region — the
    // scratch arena the walk built it on is reset at the next drain pop.
    let host = ctx.current_scope().brand();
    let part_indices: &'step [usize] = host.allocator().slice(&part_indices);
    let dep_error_frame = working_frame("<bind>", &working_expr);
    let finish = move |ctx: &DecideCtx<'_, 'step, '_>, terminals: &[DepTerminal<'_>]| {
        // A parts run is frozen once its door bumps it, so the whole batch must land in one
        // rebuild. Deps land in staging order, so `part_indices` ascends 1:1 with `terminals` and
        // a single cursor over it places every cell in one pass.
        let scope = ctx.current_scope();
        let parts = working_expr.parts;
        let mut filled = part_indices.iter().copied().zip(terminals).peekable();
        let spliced = working_expr.respliced(
            scope.brand(),
            parts.iter().enumerate().map(|(i, part)| {
                // Resting the dep's cell into this step's region is what keeps the value's backing
                // retained until the bind reads it — which happens in this same step, on the
                // decide that folds the resolved call, so the cell is never read across a tail hop.
                match filled.next_if(|(slot, _)| *slot == i) {
                    Some((_, terminal)) => Spanned {
                        value: WorkingPart::Spliced {
                            cell: scope.rest_spliced(&terminal.cell),
                            from_name: None,
                        },
                        span: part.span,
                    },
                    None => *part,
                }
            }),
        );
        finish_eager_subs(ctx, spliced, picked)
    };
    Await::on(deps)
        .error_frame(dep_error_frame)
        .finish_terminal(host, finish)
}

/// Route a fully-spliced eager-subs `working_expr` to its continuation. Under `None` the resolution
/// sees the element types the subs revealed, so a `Spliced(_)` no candidate satisfies surfaces
/// there as a slot-terminal `DispatchFailed` rather than as a bind-time error.
fn finish_eager_subs<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    working_expr: WorkingExpression<'step>,
    picked: Option<OpenedFunction<'step>>,
) -> Outcome<'step> {
    match picked {
        Some(f) => super::exec::invoke_continue(view, f, working_expr),
        None => redispatch_continue(view, working_expr),
    }
}

/// Splice each wrap slot of `expr` with the carrier its bare name resolved to, returning the
/// rebuilt node — `None` when the pick named no wrap slot and the run is already the one to invoke
/// over.
///
/// No staging happens here: [`initial`] stages before it resolves, so by the time a pick exists
/// every child that evaluates has landed and the only slot still to move is a bare name the pick
/// classified. Infallible for the same reason resolution is: admission rejects an unbound name and
/// parks on a still-finalizing one before any pick, so every name the wrap set names is `Resolved`
/// in `bare_outcomes`.
fn splice_wrap_slots<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: &WorkingExpression<'step>,
    bare_outcomes: &[Option<Resolution>],
    wrap_indices: &[usize],
) -> Option<WorkingExpression<'step>> {
    if wrap_indices.is_empty() {
        return None;
    }
    let brand = ctx.current_scope().brand();
    Some(expr.respliced(
        brand,
        expr.parts.iter().enumerate().map(|(i, part)| {
            if !wrap_indices.contains(&i) {
                return *part;
            }
            // A name bound in this region rests for free (the self rule strips this region from
            // what is retained); one bound further out lodges its binding scope's coverage, which
            // is what keeps the read live.
            let Some(Resolution::Resolved(cell)) = bare_outcomes.get(i).and_then(|o| o.as_ref())
            else {
                unreachable!("a picked wrap slot's bare name has a Resolved cache entry");
            };
            Spanned {
                value: WorkingPart::Spliced {
                    cell: ctx.current_scope().rest_delivered(cell),
                    from_name: bare_part_name(&part.value),
                },
                span: part.span,
            }
        }),
    ))
}

/// The bare name a part holds, for a splice to carry forward as the operand's surface spelling.
/// Every other part shape answers `None`: it was never a name, so no diagnostic can quote one.
fn bare_part_name(part: &WorkingPart<'_>) -> Option<BinderSymbol> {
    match part.as_ast()? {
        ExpressionPart::Identifier(v) => Some(BinderSymbol::Value(v)),
        ExpressionPart::Type(t) => Some(BinderSymbol::Type(t)),
        _ => None,
    }
}
