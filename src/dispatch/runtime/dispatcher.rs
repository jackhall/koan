//! Overload resolution for `Scope`. The public `Scope::dispatch` and `Scope::lazy_candidate`
//! methods are thin forwarders to the free functions here; everything else (the `Pick` enum,
//! the per-scope `pick` lookup, the specificity tournament, and the lazy-candidate shape
//! check) is private. Splitting these out of `scope.rs` keeps the storage- and
//! lookup-oriented `Scope` impl block compact and lets the dispatch logic evolve without
//! churning the storage code's borrow plumbing.
//!
//! The `outer`-chain recursion shape matches the previous in-impl version: when a scope's
//! own bucket has no match, dispatch recurses into the outer scope; an in-bucket *ambiguity*
//! does not fall through (silently shadowing it would hide a real conflict from the author).

use super::kerror::{KError, KErrorKind};
use super::scope::{KFuture, Scope, ShapePick};
use crate::dispatch::kfunction::KFunction;
use crate::dispatch::types::{KType, Parseable, SignatureElement, Specificity};
use crate::parse::kexpression::{ExpressionPart, KExpression};

/// Resolve `expr` against `scope`'s functions, walking `outer` on miss so child scopes
/// inherit from their parents. Ambiguity does *not* fall through to `outer` — the inner
/// scope had a real conflict, and silently shadowing it would hide it from the author.
///
/// Function-as-value calls (e.g., `LET f = (FN ...)` then `f (args)`) do not live here:
/// they go through the [`call_by_name`](crate::dispatch::builtins::call_by_name) builtin,
/// whose signature `[Identifier, KExpression]` matches identifier-leading expressions and
/// synthesizes a re-dispatchable expression by weaving the looked-up function's keyword
/// tokens back in.
pub(crate) fn dispatch<'a>(scope: &Scope<'a>, expr: KExpression<'a>) -> Result<KFuture<'a>, KError> {
    match pick(scope, &expr) {
        Pick::One(f) => return f.bind(expr),
        Pick::Ambiguous(n) => {
            return Err(KError::new(KErrorKind::AmbiguousDispatch {
                expr: expr.summarize(),
                candidates: n,
            }));
        }
        Pick::None => {}
    }
    if let Some(outer) = scope.outer {
        return dispatch(outer, expr);
    }
    Err(KError::new(KErrorKind::DispatchFailed {
        expr: expr.summarize(),
        reason: "no matching function".to_string(),
    }))
}

/// Find a "lazy candidate" for `expr`: a matching function with at least one
/// `KType::KExpression` slot bound by an `ExpressionPart::Expression`. Returns the indices
/// of the *eager* `Expression` parts — the caller schedules those as deps and leaves the
/// lazy ones in place for the receiving builtin to dispatch itself. Walks `outer` like
/// `dispatch` does.
///
/// TODO(lazy-list-of-expressions): once user functions exist, `[e1 e2 e3]` will need to
/// ride into the parent as `KExpression` data rather than be eagerly scheduled. Today
/// every list-literal element resolves eagerly via `schedule_list_literal`.
pub(crate) fn lazy_candidate<'a>(scope: &Scope<'a>, expr: &KExpression<'_>) -> Option<Vec<usize>> {
    if !expr.parts.iter().any(|p| matches!(p, ExpressionPart::Expression(_))) {
        return None;
    }
    let functions = scope.functions.borrow();
    let mut viable: Vec<(&KFunction<'a>, Vec<usize>)> = functions
        .get(&expr.untyped_key())
        .into_iter()
        .flatten()
        .filter_map(|f| lazy_eager_indices(f, expr).map(|e| (*f, e)))
        .collect();
    if !viable.is_empty() {
        let funcs: Vec<&KFunction<'_>> = viable.iter().map(|(f, _)| *f).collect();
        // Ambiguous → return None and let `dispatch` surface the actual error at execute
        // time. Falling back to the eager pipeline here would misevaluate the lazy slot.
        return pick_most_specific_index(&funcs).map(|i| viable.swap_remove(i).1);
    }
    drop(functions);
    scope.outer.and_then(|outer| lazy_candidate(outer, expr))
}

/// Pick within `scope`'s own bucket only. Returns `None` if the bucket is missing or has no
/// matching candidates; the caller decides whether to walk `outer`.
fn pick<'a>(scope: &Scope<'a>, expr: &KExpression<'a>) -> Pick<'a> {
    let key = expr.untyped_key();
    let functions = scope.functions.borrow();
    let bucket = match functions.get(&key) {
        Some(b) => b,
        None => return Pick::None,
    };
    let candidates: Vec<&'a KFunction<'a>> = bucket
        .iter()
        .filter(|f| f.signature.matches(expr))
        .copied()
        .collect();
    match pick_most_specific_index(&candidates) {
        Some(i) => Pick::One(candidates[i]),
        None if candidates.is_empty() => Pick::None,
        None => Pick::Ambiguous(candidates.len()),
    }
}

enum Pick<'a> {
    One(&'a KFunction<'a>),
    Ambiguous(usize),
    None,
}

/// Pairwise specificity tournament: returns `Some(i)` iff `candidates[i]` is strictly more
/// specific than every other candidate. Returns `None` if the bucket is empty or if no
/// candidate dominates every peer (callers distinguish via `candidates.is_empty()`).
fn pick_most_specific_index(candidates: &[&KFunction<'_>]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .find(|(i, a)| {
            candidates.iter().enumerate().all(|(j, b)| {
                *i == j
                    || matches!(a.signature.specificity_vs(&b.signature), Specificity::StrictlyMore)
            })
        })
        .map(|(i, _)| i)
}

/// `lazy_candidate` shape check for a single function: is this a viable lazy match for `expr`,
/// and if so what are the indices of its eager-Expression parts? Returns `None` when the
/// function isn't a lazy candidate (length mismatch, fixed-token mismatch, no `KExpression`
/// slot binding an `Expression` part, or any other arg-type mismatch).
fn lazy_eager_indices(f: &KFunction<'_>, expr: &KExpression<'_>) -> Option<Vec<usize>> {
    let sig = &f.signature;
    if sig.elements.len() != expr.parts.len() {
        return None;
    }
    let mut eager_indices: Vec<usize> = Vec::new();
    let mut has_lazy_slot = false;
    for (i, (el, part)) in sig.elements.iter().zip(expr.parts.iter()).enumerate() {
        match (el, part) {
            (SignatureElement::Keyword(s), ExpressionPart::Keyword(t)) if s == t => {}
            (SignatureElement::Keyword(_), _) => return None,
            (SignatureElement::Argument(arg), part) => match (&arg.ktype, part) {
                (KType::KExpression, ExpressionPart::Expression(_)) => {
                    has_lazy_slot = true;
                }
                (KType::KExpression, _) => return None,
                (_, ExpressionPart::Expression(_)) => {
                    // Speculative: assume the eager-evaluated result will type-match at late
                    // dispatch. If not, dispatch will fail at that point.
                    eager_indices.push(i);
                }
                (_, other) => {
                    if !arg.matches(other) {
                        return None;
                    }
                }
            },
        }
    }
    if has_lazy_slot { Some(eager_indices) } else { None }
}

/// Shape-pick: produces a [`ShapePick`] for `expr` by selecting the unique most-specific
/// matching function in `scope` (or `outer` chain). Tries strict matching first
/// (mirroring the real dispatcher's `Argument::matches` check); only falls back to a
/// tentative-accept of bare-Identifier-in-value-slot when no strict candidate exists.
///
/// `eager_indices` carries `lazy_eager_indices`' result for the lazy path (or empty for
/// non-lazy candidates). `wrap_indices` are bare-Identifier parts in value-typed slots
/// that should be auto-wrapped as sub-Dispatches per §7. `ref_name_indices` are
/// bare-Identifier parts in literal-name slots (`KType::Identifier` /
/// `KType::TypeExprRef`) of a non-pre_run function, used by §8 replay-park.
///
/// Returns `None` when:
/// - no candidate function in any scope on the chain matches `expr`'s shape, or
/// - more than one candidate ties at the same specificity (ambiguous — `dispatch` will
///   surface it later; the wrap pass shouldn't speculatively transform an ambiguous expr).
pub(crate) fn shape_pick<'a>(scope: &Scope<'a>, expr: &KExpression<'_>) -> Option<ShapePick> {
    let key = expr.untyped_key();
    let functions = scope.functions.borrow();
    // First: strict matching (only literal-Identifier-in-Identifier-slot etc.). This is
    // the dispatch-time match the real dispatcher would make.
    let strict: Vec<&KFunction<'_>> = functions
        .get(&key)
        .into_iter()
        .flatten()
        .copied()
        .filter(|f| f.signature.matches(expr))
        .collect();
    let strict_pick = pick_most_specific_index(&strict);
    if let Some(i) = strict_pick {
        let pick = classify_for_pick(strict[i], expr);
        drop(functions);
        return Some(pick);
    }
    // Second: tentative-accept for the §7 auto-wrap case — bare-Identifier in a
    // non-literal-name slot. The wrap rewrites that Identifier into a sub-Dispatch whose
    // resolved value will type-match at late dispatch. Only fires when strict picked
    // nothing AND tentative produces a unique candidate.
    let tentative: Vec<&KFunction<'_>> = functions
        .get(&key)
        .into_iter()
        .flatten()
        .copied()
        .filter(|f| accepts_for_wrap(f, expr))
        .collect();
    let tentative_pick = pick_most_specific_index(&tentative);
    drop(functions);
    match tentative_pick {
        Some(i) => Some(classify_for_pick(tentative[i], expr)),
        None if strict.is_empty() && tentative.is_empty() => match scope.outer {
            Some(outer) => shape_pick(outer, expr),
            None => None,
        },
        None => None,
    }
}

/// Auto-wrap-permissive shape check: bare-Identifier parts in a slot whose declared type is
/// neither `Identifier` nor `TypeExprRef` are tentatively accepted (the §7 auto-wrap will
/// rewrite them into sub-Dispatches, whose results type-check at late dispatch). All other
/// slot/part pairings reuse the normal `Argument::matches` check. Mirrors the strict
/// matcher except for the bare-Identifier-in-value-slot allowance.
fn accepts_for_wrap(f: &KFunction<'_>, expr: &KExpression<'_>) -> bool {
    let sig = &f.signature;
    if sig.elements.len() != expr.parts.len() {
        return false;
    }
    for (el, part) in sig.elements.iter().zip(expr.parts.iter()) {
        match (el, part) {
            (SignatureElement::Keyword(s), ExpressionPart::Keyword(t)) if s == t => {}
            (SignatureElement::Keyword(_), _) => return false,
            (SignatureElement::Argument(arg), part) => {
                if matches!(part, ExpressionPart::Identifier(_))
                    && !matches!(arg.ktype, KType::Identifier | KType::TypeExprRef)
                {
                    continue;
                }
                if !arg.matches(part) {
                    return false;
                }
            }
        }
    }
    true
}

/// Build a [`ShapePick`] from the picked function and the matching expression. `eager_indices`
/// reuses `lazy_eager_indices`' result for the lazy path (or empty for non-lazy candidates);
/// `wrap_indices` and `ref_name_indices` classify bare-Identifier parts per §7 / §8.
fn classify_for_pick(f: &KFunction<'_>, expr: &KExpression<'_>) -> ShapePick {
    let eager_indices = lazy_eager_indices(f, expr).unwrap_or_default();
    let mut wrap_indices: Vec<usize> = Vec::new();
    let mut ref_name_indices: Vec<usize> = Vec::new();
    let picked_has_pre_run = f.pre_run.is_some();
    for (i, (el, part)) in f.signature.elements.iter().zip(expr.parts.iter()).enumerate() {
        let SignatureElement::Argument(arg) = el else { continue };
        let ExpressionPart::Identifier(_) = part else { continue };
        match arg.ktype {
            KType::Identifier | KType::TypeExprRef => {
                // Literal-name slot — §8 candidate iff the picked function isn't a binder.
                // Binders' Identifier/TypeExprRef slots are *declarations* (the name being
                // bound), not references that need to look anything up.
                if !picked_has_pre_run {
                    ref_name_indices.push(i);
                }
            }
            _ => {
                // Value-typed slot — §7 auto-wrap target. Wrap regardless of whether the
                // picked function has a pre_run; binders take their value from `parts[3]`
                // (LET) or other Any-typed positions, so `LET y = z` with `z` an identifier
                // wraps just like `(F z)`.
                wrap_indices.push(i);
            }
        }
    }
    ShapePick {
        eager_indices,
        wrap_indices,
        ref_name_indices,
        picked_has_pre_run,
    }
}

#[cfg(test)]
mod tests {
    use super::super::arena::RuntimeArena;
    use super::super::scope::Scope;
    use crate::dispatch::builtins::default_scope;
    use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

    #[test]
    fn dispatch_walks_outer_chain_to_find_builtin() {
        // Parent owns the LET builtin; child has no functions of its own. Dispatching LET
        // against the child must climb to the parent.
        let arena = RuntimeArena::new();
        let outer = default_scope(&arena, Box::new(std::io::sink()));
        let inner = arena.alloc_scope(outer.child_for_call());

        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("LET".into()),
                ExpressionPart::Identifier("x".into()),
                ExpressionPart::Keyword("=".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };

        assert!(inner.dispatch(expr).is_ok(), "child scope should inherit LET from outer");
    }

    #[test]
    fn dispatch_with_no_outer_and_no_match_errors() {
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        let expr = KExpression {
            parts: vec![ExpressionPart::Identifier("nope".into())],
        };
        assert!(scope.dispatch(expr).is_err());
    }

    // --- specificity / bucketing / shadowing tests ---

    use crate::dispatch::builtins::register_builtin;
    use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
    use crate::dispatch::types::{Argument, ExpressionSignature, KType, SignatureElement};
    use crate::dispatch::values::KObject;
    use crate::execute::scheduler::Scheduler;

    // Sentinel-returning bodies. Each produces a distinct `KString` so a test can tell which
    // overload won dispatch. Allocate the marker into the call's scope arena so it drops with
    // the run — Miri's leak detector flagged earlier `Box::leak`-based markers as the only
    // post-stage-1 audit-slate leak.
    fn marker<'a>(s: &'a Scope<'a>, label: &'static str) -> &'a KObject<'a> {
        s.arena.alloc_object(KObject::KString(label.into()))
    }

    fn body_identifier<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "identifier")) }
    fn body_any<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "any")) }
    fn body_number_any<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "number_any")) }
    fn body_any_number<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "any_number")) }
    fn body_inner_any<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "inner_any")) }
    fn body_outer_number<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "outer_number")) }
    fn body_lowercase<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "lowercase")) }

    fn one_slot_sig(name: &str, kt: KType) -> ExpressionSignature {
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![SignatureElement::Argument(Argument {
                name: name.into(),
                ktype: kt,

            })],
        }
    }

    /// `<a:A> OP <b:B>` — a binary-operator shape that includes a fixed token so the
    /// expression doesn't get caught by list-shape detection (which would treat any
    /// fixed-token-free multi-part expression as a list construction).
    fn two_slot_sig(a: KType, b: KType) -> ExpressionSignature {
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Argument(Argument {
                    name: "a".into(),
                    ktype: a,

                }),
                SignatureElement::Keyword("OP".into()),
                SignatureElement::Argument(Argument {
                    name: "b".into(),
                    ktype: b,

                }),
            ],
        }
    }

    /// Register the `Identifier` overload AFTER the `Any` overload (the opposite of
    /// `default_scope`'s declaration order). Specificity-based dispatch should still pick
    /// `Identifier` for an identifier-shaped input.
    #[test]
    fn dispatch_picks_identifier_over_any_regardless_of_registration_order() {
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        register_builtin(scope, "any_first", one_slot_sig("v", KType::Any), body_any);
        register_builtin(scope, "ident_second", one_slot_sig("v", KType::Identifier), body_identifier);

        let expr = KExpression { parts: vec![ExpressionPart::Identifier("foo".into())] };
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().unwrap();
        let result = sched.read(id);
        assert!(matches!(result, KObject::KString(s) if s == "identifier"),
            "Identifier overload should win on an identifier input, got {:?}", summarize_marker(result));
    }

    /// Inner scope's `Any` overload shadows the outer scope's more-specific `Number` overload.
    /// Pure lexical shadowing — innermost match wins regardless of specificity at outer levels.
    #[test]
    fn dispatch_inner_scope_shadows_outer_more_specific() {
        let arena = RuntimeArena::new();
        let outer = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        register_builtin(outer, "outer_specific", one_slot_sig("v", KType::Number), body_outer_number);

        let inner = arena.alloc_scope(outer.child_for_call());
        register_builtin(inner, "inner_loose", one_slot_sig("v", KType::Any), body_inner_any);

        let expr = KExpression { parts: vec![ExpressionPart::Literal(KLiteral::Number(7.0))] };
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, inner);
        sched.execute().unwrap();
        let result = sched.read(id);
        assert!(matches!(result, KObject::KString(s) if s == "inner_any"),
            "inner Any must shadow outer Number (lexical shadowing > specificity), got {:?}",
            summarize_marker(result));
    }

    /// `<Number> OP <Any>` and `<Any> OP <Number>` are incomparable for an input matching
    /// both (`5 OP 7`): each is more specific in one slot and less in the other. Dispatch
    /// must error rather than silently picking one. The fixed `OP` token keeps the
    /// expression out of the list-shape short-circuit.
    #[test]
    fn dispatch_errors_on_ambiguous_overlap() {
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        register_builtin(scope, "number_any", two_slot_sig(KType::Number, KType::Any), body_number_any);
        register_builtin(scope, "any_number", two_slot_sig(KType::Any, KType::Number), body_any_number);

        let expr = KExpression {
            parts: vec![
                ExpressionPart::Literal(KLiteral::Number(5.0)),
                ExpressionPart::Keyword("OP".into()),
                ExpressionPart::Literal(KLiteral::Number(7.0)),
            ],
        };
        let result = scope.dispatch(expr);
        match result {
            Err(e) => assert!(
                matches!(e.kind, crate::dispatch::runtime::KErrorKind::AmbiguousDispatch { .. }),
                "expected ambiguity error, got: {e}",
            ),
            Ok(_) => panic!("equally-specific overloads should produce an ambiguity error"),
        }
    }

    /// A lowercase fixed token in a registered signature is coerced to uppercase, so
    /// dispatching the uppercase form from a source program still hits the registered
    /// function. (Once monadic effects exist, this should also produce a warning effect.)
    #[test]
    fn registration_coerces_lowercase_fixed_tokens_to_uppercase() {
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        let sig = ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("foo".into()), // lowercase — should be coerced
                SignatureElement::Argument(Argument {
                    name: "v".into(),
                    ktype: KType::Number,

                }),
            ],
        };
        register_builtin(scope, "FOO", sig, body_lowercase);

        // The source-side caller writes `FOO 1` (uppercase), which must match the coerced
        // `FOO <v>` registration.
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("FOO".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().unwrap();
        let result = sched.read(id);
        assert!(matches!(result, KObject::KString(s) if s == "lowercase"));
    }

    fn summarize_marker(obj: &KObject<'_>) -> String {
        match obj {
            KObject::KString(s) => s.clone(),
            KObject::Null => "null".into(),
            _ => "<other>".into(),
        }
    }

    // -------------------- §7 / §8 shape_pick coverage --------------------

    /// A function whose signature is `OP <v:Number>` matched against `OP someName` (where
    /// `someName` is a bare Identifier in a Number-typed slot) returns `wrap_indices = [1]`
    /// and no ref_name_indices — the dispatcher will wrap `someName` as a sub-Dispatch so
    /// it resolves through `value_lookup` (or §1's short-circuit, if the name is bound).
    #[test]
    fn shape_pick_returns_wrap_indices_for_value_slot_identifiers() {
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        let sig = ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("OP".into()),
                SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Number }),
            ],
        };
        register_builtin(scope, "OP", sig, body_any);
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("OP".into()),
                ExpressionPart::Identifier("someName".into()),
            ],
        };
        let pick = scope.shape_pick(&expr).expect("OP <Number> should pick");
        assert_eq!(pick.wrap_indices, vec![1]);
        assert!(pick.ref_name_indices.is_empty());
        assert!(!pick.picked_has_pre_run);
    }

    /// `call_by_name`'s shape — `<verb:Identifier> <args:KExpression>` — picked against
    /// `myFn (x: 1)` returns ref_name_indices = [0]: the Identifier slot is a literal-name
    /// reference and the function has no pre_run, so §8 will check whether `myFn` resolves
    /// to a placeholder.
    #[test]
    fn shape_pick_returns_ref_name_indices_for_non_pre_run_function() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        // call_by_name is registered by default_scope.
        let inner = KExpression {
            parts: vec![
                ExpressionPart::Identifier("x".into()),
                ExpressionPart::Keyword(":".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Identifier("myFn".into()),
                ExpressionPart::Expression(Box::new(inner)),
            ],
        };
        let pick = scope
            .shape_pick(&expr)
            .expect("call_by_name should pick on Identifier-leading expression");
        assert!(pick.ref_name_indices.contains(&0));
        assert!(!pick.picked_has_pre_run);
    }

    /// LET's name slot is `Identifier` (or `TypeExprRef`), but LET has `pre_run = Some(_)` —
    /// so shape_pick should NOT include the name slot in ref_name_indices. Binder Identifier
    /// slots are declarations, not references; §8 must skip them.
    #[test]
    fn shape_pick_skips_ref_name_indices_for_pre_run_function() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("LET".into()),
                ExpressionPart::Identifier("x".into()),
                ExpressionPart::Keyword("=".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };
        let pick = scope.shape_pick(&expr).expect("LET should pick");
        assert!(pick.picked_has_pre_run);
        assert!(
            pick.ref_name_indices.is_empty(),
            "LET's Identifier name slot is a declaration, not a reference; should not be ref_name_index. Got {:?}",
            pick.ref_name_indices,
        );
    }

    /// Ambiguous shape (two equally-specific overloads matching) returns `None` — the wrap
    /// pass mustn't speculatively transform an ambiguous expression.
    #[test]
    fn shape_pick_returns_none_when_ambiguous() {
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        register_builtin(scope, "OP_NA", two_slot_sig(KType::Number, KType::Any), body_number_any);
        register_builtin(scope, "OP_AN", two_slot_sig(KType::Any, KType::Number), body_any_number);
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Literal(KLiteral::Number(5.0)),
                ExpressionPart::Keyword("OP".into()),
                ExpressionPart::Literal(KLiteral::Number(7.0)),
            ],
        };
        assert!(scope.shape_pick(&expr).is_none(), "ambiguous overlap → None");
    }
}
