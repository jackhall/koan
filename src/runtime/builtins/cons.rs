//! `CONS <head:KExpression> <tail:KExpression>` — sequence two expressions: dispatch
//! `head` as a sibling slot for its side effects, then tail-call into `tail`. CONS is the
//! desugar target for multi-statement bodies; FN and MATCH right-fold their parts-list
//! into a chain of CONS calls at construction time so the scheduler always sees a single
//! expression as the body / branch.
//!
//! Dispatch shape: `head` runs in parallel with `tail` (head is `add_dispatch`ed against
//! the caller scope; tail is the slot's tail-replace target). Data dependencies between
//! statements are carried by the existing dispatch-time placeholder mechanism — `LET`'s
//! `pre_run` installs the placeholder synchronously at `add_dispatch` time, so a later
//! statement that names the binding parks on the producer in the standard way. Forward
//! references (an earlier statement naming a binding declared later) do **not** work:
//! the later CONS step's `add_dispatch` happens only after the outer slot has tail-replaced
//! and the head's slot has already started, so the later binding's placeholder is not yet
//! installed when the head dispatches. This is a known trade-off vs. the parallel
//! `plan_body_statements` path used by MODULE / SIG.
//!
//! Effect ordering between head and tail is topological, not source-order: head is a
//! sibling slot in the ready queue, tail is the slot's replaced work. Either may run
//! first depending on the queue. Use placeholder-bearing statements (`LET`) to enforce
//! ordering when needed.

use crate::runtime::machine::model::KType;
use crate::runtime::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};
use crate::runtime::machine::model::ast::{ExpressionPart, KExpression};

use crate::runtime::machine::core::kfunction::argument_bundle::extract_kexpression;
use super::{arg, err, kw, register_builtin, sig};

pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let head = match extract_kexpression(&mut bundle, "head") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "CONS head slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    let tail = match extract_kexpression(&mut bundle, "tail") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "CONS tail slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    // Head's value is discarded; its purpose is the side effects (PRINT, LET-binding) it
    // performs and the placeholder its `pre_run` may have installed at `add_dispatch` time.
    sched.add_dispatch(head, scope);
    BodyResult::tail(tail)
}

/// Right-fold a multi-statement body into a CONS chain. Input shape is the parens-content
/// of an FN body or MATCH branch — `KExpression { parts: [Expression(s_0), Expression(s_1),
/// ..., Expression(s_{n-1})] }`. Output for `n >= 2`:
///
/// ```text
/// (CONS s_0 (CONS s_1 ... (CONS s_{n-2} s_{n-1})))
/// ```
///
/// Bodies with `n < 2` parts, or any non-`Expression` part, pass through unchanged — the
/// stricter all-`Expression` rule mirrors `SchedulerHandle::plan_body_statements` so a single
/// statement like `(LET x = (FN ...))` doesn't get mis-split (its inner `Expression`
/// would otherwise look like a second statement).
pub(crate) fn fold_multi_statement<'a>(body: KExpression<'a>) -> KExpression<'a> {
    if body.parts.len() < 2 {
        return body;
    }
    let (mut preceding, mut acc) = match body.try_take_inner_expressions_split() {
        Ok(t) => t,
        Err(body) => return body,
    };
    while let Some(stmt) = preceding.pop() {
        acc = KExpression {
            parts: vec![
                ExpressionPart::Keyword("CONS".into()),
                ExpressionPart::Expression(Box::new(stmt)),
                ExpressionPart::Expression(Box::new(acc)),
            ],
        };
    }
    acc
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "CONS",
        sig(KType::Any, vec![
            kw("CONS"),
            arg("head", KType::KExpression),
            arg("tail", KType::KExpression),
        ]),
        body,
    );
}

#[cfg(test)]
mod tests {
    use crate::runtime::builtins::test_support::{run, run_one, parse_one, run_root_silent, run_root_with_buf};
    use crate::runtime::machine::model::KObject;
    use crate::runtime::machine::RuntimeArena;

    fn capture(source: &str) -> Vec<u8> {
        let arena = RuntimeArena::new();
        let (scope, captured) = run_root_with_buf(&arena);
        run(scope, source);
        let bytes = captured.borrow().clone();
        bytes
    }

    #[test]
    fn multi_statement_fn_body_returns_last_value() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "FN (FOO) -> Number = ((LET x = 1) (LET y = 2) (y))",
        );
        let v = run_one(scope, parse_one("FOO"));
        assert!(matches!(v, KObject::Number(n) if *n == 2.0));
    }

    #[test]
    fn multi_statement_fn_body_runs_each_statement() {
        let bytes = capture(
            "FN (FOO) -> Str = ((PRINT \"a\") (PRINT \"b\") (PRINT \"c\"))\nFOO",
        );
        // Effect ordering between siblings is topological; we only assert all three ran.
        assert!(bytes.windows(2).any(|w| w == b"a\n"), "missing 'a' in {:?}", String::from_utf8_lossy(&bytes));
        assert!(bytes.windows(2).any(|w| w == b"b\n"), "missing 'b' in {:?}", String::from_utf8_lossy(&bytes));
        assert!(bytes.windows(2).any(|w| w == b"c\n"), "missing 'c' in {:?}", String::from_utf8_lossy(&bytes));
    }

    #[test]
    fn multi_statement_match_branch_returns_last_value() {
        let bytes = capture(
            "UNION Maybe = (some :Number none :Null)\n\
             LET m = (Maybe (some 5))\n\
             MATCH (m) WITH (\
                 some -> ((PRINT \"got\") (PRINT it))\
                 none -> (PRINT \"no\")\
             )",
        );
        // Both PRINTs should run; topological ordering means we can't assert which first.
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("got"), "missing 'got' in {s:?}");
        assert!(s.contains("5"), "missing 'it' value in {s:?}");
    }

    #[test]
    fn fn_recursion_with_multi_statement_body_via_match_terminates() {
        // Recursive last-statement TCO under multi-statement body: the MATCH branch's last
        // statement is the recursive call, and CONS's tail-replace preserves the FN slot.
        // Without TCO, deep recursion would blow the scheduler.
        let bytes = capture(
            "UNION Bit = (one :Null zero :Null)\n\
             FN (HOP b :Tagged) -> Any = (\
                 (PRINT \"step\")\
                 (MATCH (b) WITH (\
                     one -> (HOP (Bit (zero null)))\
                     zero -> (PRINT \"done\")\
                 ))\
             )\n\
             HOP (Bit (one null))",
        );
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("done"), "expected 'done' to print, got {s:?}");
    }

    #[test]
    fn backward_reference_across_statements_works() {
        // Standard data-dep via LET placeholder parking: stmt 2 reads `a` bound by stmt 1.
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "FN (FOO) -> Number = ((LET a = 10) (LET b = (a)) (b))",
        );
        let v = run_one(scope, parse_one("FOO"));
        assert!(matches!(v, KObject::Number(n) if *n == 10.0));
    }

    #[test]
    fn single_statement_body_unchanged() {
        // Fold should pass through a single-statement body identical to today.
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET v = 7\nFN (FOO) -> Number = (v)");
        let v = run_one(scope, parse_one("FOO"));
        assert!(matches!(v, KObject::Number(n) if *n == 7.0));
    }
}
