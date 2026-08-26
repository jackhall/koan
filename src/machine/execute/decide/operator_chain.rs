//! Operator-chain dispatch arm: resolve the operator group for a `Slot (Keyword Slot)+` chain and
//! reduce the run by the group's declared mode.
//!
//! Recognition is structural and parse-cached (see
//! [`crate::machine::model::ast::classify_dispatch_shape`]); this arm resolves the
//! chain's cached operator probe against the per-scope operator registry, walked
//! through the scope chain (innermost visible wins; see
//! [the lookup protocol](../../../../design/typing/lookup-protocol.md)).
//!
//! A registry miss first probes for a **visible pending** `OP` declaration: the declaration's
//! registry write lands only when its body finalizes, so a chain that runs while a lexically
//! earlier declaration is still in flight parks on that declaration's claim and re-runs on wake
//! (see [`park_on_pending_operators`]). Visibility is the binding tables' exclusive cutoff, so the
//! wait is always lexically backward — a lexically-later declaration is not visible, and a
//! statement's own claim is invisible to its own subtree. With nothing pending the miss is real —
//! a cross-group operator mix, or an operator no visible module declared — and surfaces directly
//! as a structured [`KErrorKind::DispatchFailed`].

use crate::machine::core::RegionBrand;
use crate::machine::core::Scope;
use crate::machine::model::Part;
use crate::machine::model::labels::{KeywordSymbol, LabelInterner};
use crate::machine::model::{ExpressionPart, PartClass, WorkingExpression, WorkingPart};
use crate::machine::model::{FoldDirection, KeyElement, OperatorGroup, ReductionMode};
use crate::machine::{KError, KErrorKind, ProducerId};
use crate::scheduler::Deps;
use crate::source::{Span, Spanned};
use crate::witnessed::BumpVec;

use super::super::outcome::DepTerminal;
use super::ctx::DecideCtx;
use super::{
    Await, DeferredTraceFrame, DepPlacement, DepRequest, Outcome, become_dispatch,
    park_resume_labelled, working_frame,
};

pub(in crate::machine::execute) fn run<'step, 'b>(
    ctx: &DecideCtx<'_, 'step, '_>,
    s: &'b Scope<'b>,
    expr: &WorkingExpression<'step>,
) -> Outcome<'step> {
    let probe = expr
        .operator_probe()
        .expect("OperatorChain shape guarantees a cached operator probe");
    let chain = ctx.chain_deref();
    match s.resolve_operator_group_delivered(probe, chain) {
        None => park_on_pending_operators(ctx, s, expr),
        Some(delivered) => {
            let operators = chain_operator_symbols(expr);
            match delivered.open(|group| ChainPlan::of(group, &operators)) {
                // The powerset keys mean a hit already covers the probe, so a non-cover is a
                // registry-build bug — surface it as a clean non-match rather than a wrong fold.
                None => Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
                    expr: expr.summarize(&ctx.registries().labels),
                    reason: cross_group_reason(&rendered_operators(
                        &operators,
                        &ctx.registries().labels,
                    )),
                }))),
                Some(ChainPlan::FoldLeft) => reduce_fold_left(ctx, expr),
                Some(ChainPlan::FoldRight) => reduce_fold_right(ctx, expr),
                Some(ChainPlan::Unary) => reduce_unary(ctx, expr),
                Some(ChainPlan::Pairwise {
                    combiner,
                    direction,
                }) => reduce_pairwise(ctx, expr, combiner, direction),
            }
        }
    }
}

/// The reduction mode, read inside the delivery envelope's open so the reducers run with nothing
/// borrowed from the declaring region. Every arm is fixed-width — a combiner is its keyword symbol
/// — so the plan copies out rather than owning anything.
#[derive(Clone, Copy)]
enum ChainPlan {
    FoldLeft,
    FoldRight,
    Unary,
    Pairwise {
        combiner: KeywordSymbol,
        direction: FoldDirection,
    },
}

impl ChainPlan {
    /// The plan for a chain naming `operators`, or `None` when the hit group does not cover them
    /// all — a cross-group mix.
    fn of(group: &OperatorGroup<'_>, operators: &[KeywordSymbol]) -> Option<ChainPlan> {
        if !group.covers(operators) {
            return None;
        }
        Some(match group.mode() {
            ReductionMode::FoldLeft => ChainPlan::FoldLeft,
            ReductionMode::FoldRight => ChainPlan::FoldRight,
            ReductionMode::Unary => ChainPlan::Unary,
            ReductionMode::Pairwise {
                combiner,
                direction,
            } => ChainPlan::Pairwise {
                combiner,
                direction,
            },
        })
    }
}

/// The operator keywords of the chain, in source order (with repeats).
fn chain_operator_symbols(expr: &WorkingExpression<'_>) -> Vec<KeywordSymbol> {
    expr.parts
        .iter()
        .filter_map(|part| match part.value.class() {
            PartClass::Keyword(symbol) => Some(symbol),
            _ => None,
        })
        .collect()
}

/// The chain's distinct operators as one space-joined spelling, for the two diagnostics that name
/// the probe. Resolved here rather than cached on the node: the probe travels as symbol bits, and
/// every glyph was interned where the parse classified it. Sorted by the rendered spelling so the
/// message reads the same for a given operator set however the run was written.
fn rendered_operators(operators: &[KeywordSymbol], labels: &LabelInterner) -> String {
    let mut spellings: Vec<String> = operators
        .iter()
        .map(|operator| labels.render(operator.symbol()))
        .collect();
    spellings.sort_unstable();
    spellings.dedup();
    spellings.join(" ")
}

/// Operands (even indices) and operator keywords (odd indices), each keeping its `Spanned` wrapper
/// so source spans survive into any error message an inner dispatch produces.
fn split_chain_parts<'step>(
    expr: &WorkingExpression<'step>,
) -> (
    Vec<Spanned<WorkingPart<'step>>>,
    Vec<Spanned<WorkingPart<'step>>>,
) {
    let mut operands = Vec::with_capacity(expr.parts.len() / 2 + 1);
    let mut operator_keywords = Vec::with_capacity(expr.parts.len() / 2);
    for (i, part) in expr.parts.iter().enumerate() {
        if i % 2 == 0 {
            operands.push(*part);
        } else {
            operator_keywords.push(*part);
        }
    }
    (operands, operator_keywords)
}

/// Wraps a built-up accumulator as an operand of the next nesting level, carrying its own span
/// forward rather than inventing a fresh one.
fn wrap_as_operand<'step>(
    brand: RegionBrand<'step>,
    acc: WorkingExpression<'step>,
) -> Spanned<WorkingPart<'step>> {
    let span = acc.span;
    Spanned {
        value: WorkingPart::Expression(brand.allocator().value(acc)),
        span,
    }
}

/// Rewrites a `FoldLeft`-mode run into nested binary dispatches — a pure syntactic rewrite, since
/// every operand appears exactly once and so there is no evaluation-order question:
///
/// `a + b + c` ⇒ `[ Expression([a, +, b]), +, c ]`. The nested `Expression` operand resolves
/// through the eager-subs sub-dispatch track before the outer `+` runs as ordinary binary
/// keyworded dispatch. The outermost expression stays bare — never itself wrapped in
/// `Expression(..)` — so [`become_dispatch`] re-enters ordinary dispatch on it directly.
fn reduce_fold_left<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: &WorkingExpression<'step>,
) -> Outcome<'step> {
    let brand = ctx.current_scope().brand();
    let (operands, operators) = split_chain_parts(expr);
    debug_assert!(
        operands.len() >= 3 && operators.len() == operands.len() - 1,
        "OperatorChain shape guarantees ≥3 operands and one fewer operator"
    );
    let mut operands = operands.into_iter();
    let mut operators = operators.into_iter();

    let first_operand = operands.next().expect("chain shape guarantees ≥3 operands");
    let second_operand = operands.next().expect("chain shape guarantees ≥3 operands");
    let first_operator = operators
        .next()
        .expect("chain shape guarantees ≥2 operators");

    let mut acc = WorkingExpression::synthesized(
        brand,
        &[first_operand, first_operator, second_operand],
        expr,
    );
    for (operator, operand) in operators.zip(operands) {
        acc = WorkingExpression::synthesized(
            brand,
            &[wrap_as_operand(brand, acc), operator, operand],
            expr,
        );
    }

    become_dispatch(ctx, acc)
}

/// The mirror image of [`reduce_fold_left`], nesting right-associated:
/// `a - b - c` ⇒ `[ a, -, Expression([b, -, c]) ]`.
fn reduce_fold_right<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: &WorkingExpression<'step>,
) -> Outcome<'step> {
    let brand = ctx.current_scope().brand();
    let (operands, operators) = split_chain_parts(expr);
    debug_assert!(
        operands.len() >= 3 && operators.len() == operands.len() - 1,
        "OperatorChain shape guarantees ≥3 operands and one fewer operator"
    );
    let mut operands = operands.into_iter().rev();
    let mut operators = operators.into_iter().rev();

    let last_operand = operands.next().expect("chain shape guarantees ≥3 operands");
    let second_last_operand = operands.next().expect("chain shape guarantees ≥3 operands");
    let last_operator = operators
        .next()
        .expect("chain shape guarantees ≥2 operators");

    let mut acc = WorkingExpression::synthesized(
        brand,
        &[second_last_operand, last_operator, last_operand],
        expr,
    );
    for (operator, operand) in operators.zip(operands) {
        acc = WorkingExpression::synthesized(
            brand,
            &[operand, operator, wrap_as_operand(brand, acc)],
            expr,
        );
    }

    become_dispatch(ctx, acc)
}

/// Rewrites a `Unary`-mode run into one keyword-first call over a list literal: the prefix surface
/// `sym [x1 x2 x3]` and the infix chain `x1 sym x2 sym x3` both become the bare 2-part expression
/// `[ Keyword(sym), ListLiteral([x1, x2, x3]) ]`, the shape `HEAD [1 2 3]` dispatches through — so
/// prefix and infix coincide on one body
/// ([design/expressions-and-parsing.md](../../../../design/expressions-and-parsing.md)).
///
/// A well-formed unary run names one operator throughout, so the first operator keyword's span and
/// text stand in for the whole run. A list literal's own element scheduling resolves each element,
/// so the operands need no per-kind rewrite.
fn reduce_unary<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: &WorkingExpression<'step>,
) -> Outcome<'step> {
    let brand = ctx.current_scope().brand();
    let (operands, operators) = split_chain_parts(expr);
    debug_assert!(
        operands.len() >= 3 && operators.len() == operands.len() - 1,
        "OperatorChain shape guarantees ≥3 operands and one fewer operator"
    );
    let operator = operators
        .into_iter()
        .next()
        .expect("chain shape guarantees ≥2 operators");
    let PartClass::Keyword(sym) = operator.value.class() else {
        unreachable!("odd-index chain parts are keywords by shape")
    };
    // A chain is parsed syntax, so every operand is still a parser part.
    let list_items = brand
        .allocator()
        .slice_from_iter(operands.into_iter().map(|operand| {
            operand
                .value
                .as_ast()
                .expect("an operator chain's operands are parsed parts")
        }));
    let kw_part = Spanned {
        value: WorkingPart::Ast(ExpressionPart::Keyword(sym)),
        span: operator.span,
    };
    let list_part = Spanned {
        value: WorkingPart::Ast(ExpressionPart::ListLiteral(list_items)),
        span: expr.span,
    };
    become_dispatch(
        ctx,
        WorkingExpression::synthesized(brand, &[kw_part, list_part], expr),
    )
}

/// Reduces a `Pairwise`-mode run: `f x < g y < h z` must evaluate `g y` **once**, its value feeding
/// both the `x<y` and `y<z` pairs, so — unlike the other modes — this cannot be a pure
/// syntactic rewrite (there every operand appears exactly once in the output tree; here a middle
/// operand appears in two places). See [`install_pairwise_fold`] for the staging + finish mechanics.
fn reduce_pairwise<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: &WorkingExpression<'step>,
    combiner: KeywordSymbol,
    direction: FoldDirection,
) -> Outcome<'step> {
    let (operands, operators) = split_chain_parts(expr);
    debug_assert!(
        operands.len() >= 3 && operators.len() == operands.len() - 1,
        "OperatorChain shape guarantees ≥3 operands and one fewer operator"
    );
    let dep_error_frame = Some(working_frame("<operator-chain>", expr));
    install_pairwise_fold(
        ctx,
        operands,
        operators,
        combiner,
        direction,
        *expr,
        dep_error_frame,
    )
}

/// Stage every pairwise operand as its own single-part sub-dispatch — whatever its part kind, the
/// one-part wrapper routes it through its normal dispatch lane — then splice each resolved cell
/// into the adjacent pairs it feeds and fold the pairs through the group's combiner.
///
/// The chain shape guarantees ≥3 operands and ≥2 operators, so there are always at least 2 pairs
/// and the combiner-fold loop always runs at least once. `chain` labels the synthesized combiner
/// parts, which have no source token of their own (see [`combine`]).
fn install_pairwise_fold<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    operands: Vec<Spanned<WorkingPart<'step>>>,
    operators: Vec<Spanned<WorkingPart<'step>>>,
    combiner: KeywordSymbol,
    direction: FoldDirection,
    chain: WorkingExpression<'step>,
    dep_error_frame: Option<DeferredTraceFrame<'step>>,
) -> Outcome<'step> {
    let host = ctx.current_scope().brand();
    // The operator run and the operand spans cross the park inside the finish, so both land in the
    // host frame region rather than the step scratch the walk builds transients on.
    let operand_spans: &'step [Option<Span>] = host
        .allocator()
        .slice_from_iter(operands.iter().map(|operand| operand.span));
    let operators: &'step [Spanned<WorkingPart<'step>>] = host.allocator().slice(&operators);
    let deps: Vec<DepRequest<'step>> = operands
        .into_iter()
        .map(|operand| DepRequest::Dispatch {
            expr: WorkingExpression::synthesized(host, &[operand], &chain),
            placement: DepPlacement::OwnScope,
        })
        .collect();
    let finish = move |ctx: &DecideCtx<'_, 'step, '_>, terminals: &[DepTerminal<'_>]| {
        // Resting a shared middle operand's cell into both adjacent pairs is the splice that makes
        // evaluation once-only; the region's union bundle absorbs the repeated coverage, so a
        // middle operand costs one retention however many pairs read it.
        let scope = ctx.current_scope();
        let brand = scope.brand();
        // The pairs buffer dies inside this wake step, so it rides the step scratch arena.
        let mut pairs = BumpVec::with_capacity_in(operators.len(), ctx.scratch());
        for (i, operator) in operators.iter().copied().enumerate() {
            let left = Spanned {
                value: WorkingPart::Spliced {
                    cell: scope.rest_spliced(&terminals[i].cell),
                },
                span: operand_spans[i],
            };
            let right = Spanned {
                value: WorkingPart::Spliced {
                    cell: scope.rest_spliced(&terminals[i + 1].cell),
                },
                span: operand_spans[i + 1],
            };
            pairs.push(WorkingExpression::synthesized(
                brand,
                &[left, operator, right],
                &chain,
            ));
        }
        let acc = match direction {
            FoldDirection::Left => {
                let mut pairs = pairs.iter().copied();
                let mut acc = pairs.next().expect(PAIRWISE_HAS_TWO_PAIRS);
                for pair in pairs {
                    acc = combine(brand, combiner, acc, pair, &chain);
                }
                acc
            }
            FoldDirection::Right => {
                let mut pairs = pairs.iter().copied().rev();
                let mut acc = pairs.next().expect(PAIRWISE_HAS_TWO_PAIRS);
                for pair in pairs {
                    acc = combine(brand, combiner, pair, acc, &chain);
                }
                acc
            }
        };
        become_dispatch(ctx, acc)
    };
    Await::on(Deps::from_requests_in(deps, ctx.scratch()))
        .error_frame(dep_error_frame)
        .finish_terminal(host, finish)
}

const PAIRWISE_HAS_TWO_PAIRS: &str =
    "pairwise always has ≥2 pairs (chain shape guarantees ≥2 operators)";

/// One combiner application over two already-built pair results. The combiner is an **operator**,
/// invoked infix: the synthesized 3-part shape `[left, Keyword(<sym>), right]` re-enters ordinary
/// keyworded dispatch, so it binds its two inputs *positionally* and resolution is the ordinary
/// scope walk at the chain's *use site* — a missing, non-callable, or wrong-arity combiner surfaces
/// as an ordinary error there (see
/// [design/operators.md](../../../../design/operators.md)).
///
/// `chain` is the originating operator chain: the synthesized keyword part has no source token of
/// its own, so it takes the chain's extent, and the combined node names the chain's file.
pub(super) fn combine<'step>(
    brand: RegionBrand<'step>,
    combiner: KeywordSymbol,
    left: WorkingExpression<'step>,
    right: WorkingExpression<'step>,
    chain: &WorkingExpression<'step>,
) -> WorkingExpression<'step> {
    WorkingExpression::synthesized(
        brand,
        &[
            wrap_as_operand(brand, left),
            Spanned {
                value: WorkingPart::Ast(ExpressionPart::Keyword(combiner)),
                span: chain.span,
            },
            wrap_as_operand(brand, right),
        ],
        chain,
    )
}

/// Park the chain on every still-finalizing `OP` declaration that would register one of its
/// operators — an `OP`'s registry write lands at its body's finalize, so a miss while the
/// declaration is in flight is a wait, not an error (see `builtins::op_def`). Visibility gating
/// makes the wait always lexically backward; with no source found the miss is real and surfaces as
/// the undeclared-operator diagnostic. Whether a claim's binder has already terminalized is the
/// harness's to rule on when it installs the park.
fn park_on_pending_operators<'step, 'b>(
    ctx: &DecideCtx<'_, 'step, '_>,
    s: &'b Scope<'b>,
    expr: &WorkingExpression<'step>,
) -> Outcome<'step> {
    let to_wait = pending_operator_sources(ctx, s, expr);
    if to_wait.is_empty() {
        return Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
            expr: expr.summarize(&ctx.registries().labels),
            reason: undeclared_operator_reason(&rendered_operators(
                &chain_operator_symbols(expr),
                &ctx.registries().labels,
            )),
        })));
    }
    let parked_expr = *expr;
    let frame = working_frame("<operator-chain>", expr);
    park_resume_labelled(
        &to_wait,
        Some(frame),
        ctx,
        move |ctx: &DecideCtx<'_, 'step, '_>, _id| run(ctx, ctx.current_scope(), &parked_expr),
    )
}

/// Every still-finalizing `OP` declaration visible from `s` that would register one of this
/// chain's operators, named by its claim edge and deduped in walk order.
///
/// An in-flight `OP` is claimed in the **bucket channel** (see `builtins::op_def`), so the probe
/// reads the scope's claim store, not the operator registry. Both keys an operator
/// can be declared under are probed — binary `[Slot, Keyword(sym), Slot]` and unary
/// `[Keyword(sym), Slot]` — since the chain cannot know the declaration's arity until it lands.
fn pending_operator_sources<'b>(
    ctx: &DecideCtx<'_, '_, '_>,
    s: &'b Scope<'b>,
    expr: &WorkingExpression<'_>,
) -> Vec<ProducerId> {
    let chain = ctx.chain_deref();
    let mut operators = chain_operator_symbols(expr);
    operators.sort_unstable();
    operators.dedup();
    let mut sources: Vec<ProducerId> = Vec::new();
    for operator in operators {
        // Stack runs over the symbols the parts already carry: a probe allocates nothing, where an
        // owned key would heap-allocate twice per scope walk.
        for key in [
            [
                KeyElement::Slot,
                KeyElement::Keyword(operator),
                KeyElement::Slot,
            ]
            .as_slice(),
            [KeyElement::Keyword(operator), KeyElement::Slot].as_slice(),
        ] {
            for scope in s.ancestors() {
                let cutoff = scope.binding_cutoff(chain);
                if let Some(source) = scope.bindings().claimed_bucket_producer(key, cutoff)
                    && !sources.contains(&source)
                {
                    sources.push(source);
                }
            }
        }
    }
    sources
}

fn undeclared_operator_reason(probe: &str) -> String {
    format!(
        "no operator group declares all of `{probe}`; chainable operators must be \
         declared together in one module"
    )
}

fn cross_group_reason(probe: &str) -> String {
    format!(
        "operators `{probe}` span more than one operator group; chaining operators \
         across groups is disallowed"
    )
}
