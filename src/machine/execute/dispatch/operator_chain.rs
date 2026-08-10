//! Operator-chain dispatch arm: resolve the operator group for a
//! `Slot (Keyword Slot)+` chain.
//!
//! Recognition is structural and parse-cached (see
//! [`crate::machine::model::ast::classify_dispatch_shape`]); this arm resolves the
//! chain's cached operator probe against the per-scope operator registry, walked
//! through the scope chain (innermost visible wins, like every other name; see
//! [the lookup protocol](../../../../design/typing/lookup-protocol.md)).
//!
//! A miss first probes the chain's operators for a still-finalizing `OP` declaration — a
//! pending-overload entry under either bucket key an operator body registers — and parks on it
//! rather than erroring, so an operator declared earlier in the same submitted block resolves
//! whatever order the scheduler pops the statements in. With nothing pending, a miss — a
//! cross-group operator mix, or an operator no module declared — surfaces a structured
//! [`KErrorKind::DispatchFailed`]. A hit reduces the run by the resolved group's
//! declared mode: [`ReductionMode::FoldLeft`] rewrites the chain into nested binary dispatches
//! (see [`reduce_fold_left`]), [`ReductionMode::FoldRight`] mirrors it right-associated (see
//! [`reduce_fold_right`]), [`ReductionMode::Unary`] rewrites it into one keyword-first call over
//! a list literal (see [`reduce_unary`]), and [`ReductionMode::Pairwise`] stages every operand as
//! its own dispatch and, once they all resolve, splices each result into the up-to-two adjacent
//! pairs it feeds before folding the pairs through the group's combiner in its declared direction
//! (see [`reduce_pairwise`] and [`combine`]) — the one mode that actually runs sub-dispatches
//! itself rather than purely rewriting syntax, since a shared middle operand must evaluate exactly
//! once.

use crate::machine::core::RegionBrand;
use crate::machine::core::Scope;
use crate::machine::model::Part;
use crate::machine::model::{ExpressionPart, PartClass, WorkingExpression, WorkingPart};
use crate::machine::model::{FoldDirection, OperatorGroup, ReductionMode, StoredElement};
use crate::machine::{KError, KErrorKind, NodeId};
use crate::scheduler::ResolvedDeps;
use crate::source::{Span, Spanned};

use super::ctx::SchedulerView;
use super::{
    become_dispatch, park_resume, propagate_dep_error, working_frame, Outcome, ProducerDisposition,
};

/// The probe is `Some` for every `OperatorChain` (the classifier guarantees it), so a
/// `None` probe is a classification bug.
///
/// Every path but the pending-`OP` park is terminal (no scheduler write), so this decides against a
/// read-only [`SchedulerView`] and returns [`Outcome::Done`]. `idx` is this slot's own node, needed
/// to classify a park edge's producers.
pub(in crate::machine::execute) fn run<'step, 'b>(
    ctx: &SchedulerView<'_, 'step, '_>,
    s: &'b Scope<'b>,
    expr: &WorkingExpression<'step>,
    idx: usize,
) -> Outcome<'step> {
    let probe = expr
        .operator_probe()
        .expect("OperatorChain shape guarantees a cached operator probe");
    let chain = ctx.chain_deref();
    match s.resolve_operator_group_delivered(probe, chain) {
        None => park_on_pending_operators(ctx, s, expr, idx, probe),
        Some(delivered) => {
            // Everything the reducers need is read out inside the envelope's one open, so the
            // record's own borrow never escapes and the envelope's pins drop before the reduce.
            let operators = chain_operators(expr);
            match delivered.open(|group| ChainPlan::of(group, &operators)) {
                // Guard against a registry-build bug: a hit whose probe operators aren't all
                // members surfaces as a clean cross-group non-match rather than a wrong fold.
                None => Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
                    expr: expr.summarize(),
                    reason: cross_group_reason(probe),
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

/// What a resolved group tells this arm to do, as owned data — the reduction mode with a pairwise
/// combiner copied out of the region-hosted record. Read inside the delivery envelope's open, so the
/// reducers run with nothing borrowed from the declaring region.
enum ChainPlan {
    FoldLeft,
    FoldRight,
    Unary,
    Pairwise {
        combiner: String,
        direction: FoldDirection,
    },
}

impl ChainPlan {
    /// The plan for a chain naming `operators`, or `None` when the hit group does not cover them
    /// all — a cross-group mix.
    fn of(group: &OperatorGroup<'_>, operators: &[&str]) -> Option<ChainPlan> {
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
                combiner: combiner.to_string(),
                direction,
            },
        })
    }
}

/// The operator keywords of the chain, in source order (with repeats).
fn chain_operators<'a>(expr: &WorkingExpression<'a>) -> Vec<&'a str> {
    expr.parts
        .iter()
        .filter_map(|part| match part.value.class() {
            PartClass::Keyword(s) => Some(s),
            _ => None,
        })
        .collect()
}

/// Splits `expr.parts` into operands (even indices) and operator keywords (odd indices),
/// cloning each `Spanned` wrapper whole — not just the inner value — so source spans survive
/// into any error message the inner dispatch produces.
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

/// Wraps a built-up accumulator as the next level's leading operand, carrying its own span
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

/// Rewrites a `FoldLeft`-mode run into nested binary dispatches — a pure syntactic rewrite,
/// since every operand appears exactly once there is no evaluation-order question:
///
/// `a + b + c` ⇒ `[ Expression([a, +, b]), +, c ]`, a bare 3-part expression whose nested
/// `Expression` operand resolves through the existing eager-subs sub-dispatch track before the
/// outer `+` runs as ordinary binary keyworded dispatch (the bodies `arithmetic::register`
/// installs). The outermost expression stays a bare 3-part expression — never itself wrapped in
/// `Expression(..)` — so [`become_dispatch`] re-enters ordinary dispatch on it directly.
fn reduce_fold_left<'step>(
    ctx: &SchedulerView<'_, 'step, '_>,
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

    let mut acc =
        WorkingExpression::new(brand, vec![first_operand, first_operator, second_operand]);
    for (operator, operand) in operators.zip(operands) {
        acc = WorkingExpression::new(brand, vec![wrap_as_operand(brand, acc), operator, operand]);
    }

    become_dispatch(ctx, acc)
}

/// Rewrites a `FoldRight`-mode run into nested binary dispatches — the mirror image of
/// [`reduce_fold_left`], nesting right-associated instead of left-associated:
///
/// `a - b - c` ⇒ `[ a, -, Expression([b, -, c]) ]`, a bare 3-part expression whose nested
/// `Expression` operand resolves through the existing eager-subs sub-dispatch track before the
/// outer `-` runs as ordinary binary keyworded dispatch. The outermost expression stays a bare
/// 3-part expression — never itself wrapped in `Expression(..)` — so [`become_dispatch`]
/// re-enters ordinary dispatch on it directly.
fn reduce_fold_right<'step>(
    ctx: &SchedulerView<'_, 'step, '_>,
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

    let mut acc = WorkingExpression::new(
        brand,
        vec![second_last_operand, last_operator, last_operand],
    );
    for (operator, operand) in operators.zip(operands) {
        acc = WorkingExpression::new(brand, vec![operand, operator, wrap_as_operand(brand, acc)]);
    }

    become_dispatch(ctx, acc)
}

/// Rewrites a `Unary`-mode run into one keyword-first call over a list literal — the prefix
/// surface `sym [x1 x2 x3]` and the infix chain `x1 sym x2 sym x3` are one dispatch shape for a
/// unary operator (the roadmap's "prefix and infix coincide" direction): both become the bare
/// 2-part expression `[ Keyword(sym), ListLiteral([x1, x2, x3]) ]`, the same shape `HEAD [1 2 3]`
/// dispatches through (a keyword-first call whose single slot carries a list). Unary is
/// homogeneous — a well-formed run names one operator throughout — so the first operator
/// keyword's span and text stand in for the whole run. Each operand rides into the literal
/// verbatim: a list literal's own element scheduling resolves a bare name against scope and
/// dispatches a parenthesized element, so a run's operands need no per-kind rewrite.
fn reduce_unary<'step>(
    ctx: &SchedulerView<'_, 'step, '_>,
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
    // A chain is parsed syntax, so every operand is still a parser part and rides straight into the
    // literal run the brand bumps.
    let list_items: Vec<ExpressionPart<'step>> = operands
        .into_iter()
        .map(|operand| {
            operand
                .value
                .as_ast()
                .expect("an operator chain's operands are parsed parts")
        })
        .collect();
    let kw_part = Spanned {
        value: WorkingPart::Ast(ExpressionPart::Keyword(sym)),
        span: operator.span,
    };
    let list_part = Spanned {
        value: WorkingPart::Ast(ExpressionPart::ListLiteral(
            brand.allocator().slice(&list_items),
        )),
        span: expr.span,
    };
    become_dispatch(ctx, WorkingExpression::new(brand, vec![kw_part, list_part]))
}

/// Reduces a `Pairwise`-mode run: `f x < g y < h z` must evaluate `g y` **once**, its value
/// feeding both the `x<y` and `y<z` pairs, so — unlike the three modes above — this cannot be a
/// pure syntactic rewrite (each operand there appears exactly once in the output tree; here a
/// middle operand appears in two places). Every operand is staged as its own owned dep
/// (whatever its part kind — a bare identifier, a literal, or a parenthesized sub-expression all
/// dispatch through their normal lane via the one-part wrapper `install_pairwise_fold` builds);
/// once every operand resolves, the finish splices each resolved cell into the up-to-two pair
/// expressions it feeds (a `.duplicate()` per embed site) and folds the pairs through the group's
/// combiner in the declared direction. See [`SchedulerView::install_pairwise_fold`] for the
/// staging + finish mechanics (mirrors the shared eager-subs pattern in `ctx.rs`, but splices
/// into a fresh pair-tree rather than back into the original expression's own slots).
fn reduce_pairwise<'step>(
    ctx: &SchedulerView<'_, 'step, '_>,
    expr: &WorkingExpression<'step>,
    combiner: String,
    direction: FoldDirection,
) -> Outcome<'step> {
    let (operands, operators) = split_chain_parts(expr);
    debug_assert!(
        operands.len() >= 3 && operators.len() == operands.len() - 1,
        "OperatorChain shape guarantees ≥3 operands and one fewer operator"
    );
    let dep_error_frame = Some(working_frame("<operator-chain>", expr));
    ctx.install_pairwise_fold(
        operands,
        operators,
        combiner,
        direction,
        expr.span,
        dep_error_frame,
    )
}

/// One combiner application over two already-built sub-expressions — the fold step
/// [`SchedulerView::install_pairwise_fold`] repeats over a pairwise run's pair results.
///
/// The combiner is an **operator**, invoked infix: the synthesized shape is the 3-part keyworded
/// expression `[left, Keyword(<sym>), right]`, which re-enters ordinary keyworded dispatch and so
/// binds its two inputs *positionally*, by signature shape — the builtin comparison group's `AND`,
/// or a group member declared `OP #(<sym>) OVER <PairResult> = (…)`. Resolution is the ordinary
/// scope walk at the chain's *use site* (a group's `USING` window surfaces the combiner alongside
/// the operator bodies), so a missing, non-callable, or wrong-arity combiner surfaces as an
/// ordinary error there.
///
/// `span` labels the synthesized keyword part, which has no source token of its own.
pub(super) fn combine<'step>(
    brand: RegionBrand<'step>,
    combiner: &str,
    left: WorkingExpression<'step>,
    right: WorkingExpression<'step>,
    span: Option<Span>,
) -> WorkingExpression<'step> {
    WorkingExpression::new(
        brand,
        vec![
            wrap_as_operand(brand, left),
            Spanned {
                value: WorkingPart::Ast(ExpressionPart::Keyword(brand.allocator().text(combiner))),
                span,
            },
            wrap_as_operand(brand, right),
        ],
    )
}

/// Registry miss: an operator of this chain may still be *being declared*. An `OP` binder installs
/// a pending-overload entry under each bucket key its body will register (see
/// `builtins::op_def`), and the declaration's registry write lands only when its body finalizes —
/// so a chain that misses the registry probes those same buckets for a visible pending producer and
/// parks on it, re-running this arm on wake. With nothing pending, the miss is real and surfaces as
/// the undeclared-operator diagnostic.
///
/// The scope walk mirrors `resolve_dispatch`'s read of `FunctionLookup::pending`: per-scope,
/// visibility-gated by the chain's cutoff, innermost first. Both keys an operator can be declared
/// under are probed — binary `[Slot, Keyword(sym), Slot]` and unary `[Keyword(sym), Slot]` — since
/// the chain cannot know the declaration's arity until it lands.
fn park_on_pending_operators<'step, 'b>(
    ctx: &SchedulerView<'_, 'step, '_>,
    s: &'b Scope<'b>,
    expr: &WorkingExpression<'step>,
    idx: usize,
    probe: &str,
) -> Outcome<'step> {
    let mut to_wait = ResolvedDeps::new();
    for producer in pending_operator_producers(ctx, s, expr) {
        match ctx.producer_disposition(producer, NodeId(idx)) {
            ProducerDisposition::Errored(e) => {
                let frame = working_frame("<operator-chain>", expr);
                return Outcome::Done(Err(propagate_dep_error(e, Some(frame))));
            }
            ProducerDisposition::Ready | ProducerDisposition::Cycle => {}
            ProducerDisposition::Park => {
                to_wait.park_on(producer);
            }
        }
    }
    if to_wait.is_empty() {
        return Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
            expr: expr.summarize(),
            reason: undeclared_operator_reason(probe),
        })));
    }
    let carrier = expr.summarize();
    let parked_expr = *expr;
    park_resume(
        to_wait.parks().to_vec(),
        Some(carrier),
        Box::new(move |ctx, idx| ctx.with_current_scope(|s| run(ctx, s, &parked_expr, idx))),
    )
}

/// Every still-finalizing `OP` declaration visible from `s` that would register one of this
/// chain's operators, deduped in walk order.
fn pending_operator_producers<'b>(
    ctx: &SchedulerView<'_, '_, '_>,
    s: &'b Scope<'b>,
    expr: &WorkingExpression<'_>,
) -> Vec<NodeId> {
    let chain = ctx.chain_deref();
    let mut operators = chain_operators(expr);
    operators.sort_unstable();
    operators.dedup();
    let mut producers: Vec<NodeId> = Vec::new();
    for operator in operators {
        // Both shapes as stack runs over the operator's own borrowed text — a stored probe needs
        // no allocation at all, where an owned key would clone the symbol twice per scope walk.
        for key in [
            [
                StoredElement::Slot,
                StoredElement::Keyword(operator),
                StoredElement::Slot,
            ]
            .as_slice(),
            [StoredElement::Keyword(operator), StoredElement::Slot].as_slice(),
        ] {
            for scope in s.ancestors() {
                let cutoff = scope.binding_cutoff(chain);
                if let Some(producer) = scope.bindings().lookup_function_stored(key, cutoff).pending
                {
                    if !producers.contains(&producer) {
                        producers.push(producer);
                    }
                }
            }
        }
    }
    producers
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
