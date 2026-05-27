use std::rc::Rc;

use crate::machine::core::LexicalFrame;
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CallArena, KError, KErrorKind, RuntimeArena, Scope,
    SchedulerHandle,
};

use crate::machine::core::kfunction::argument_bundle::extract_kexpression;
use crate::machine::core::kfunction::body::split_body_statements;
use super::branch_walk::find_branch_body;
use super::{arg, err, kw, register_builtin, sig};

/// `MATCH <value:Any> WITH <branches:KExpression>` — branch by tag.
///
/// `value` may be a `Tagged` (user-defined tagged union) or a `Bool`. For `Bool`, the
/// value is projected at entry into a synthetic `(tag, value)` pair where `tag` is
/// `"true"` or `"false"` and the inner value is `Null`; the rest of the branch-walking
/// machinery is the same path used by `Tagged`. Other input types are a `TypeMismatch`.
///
/// `branches` is the parens-wrapped body whose parts are repeated `<tag> -> <body>`
/// triples (arrow-pair syntax). The tag part is normally a bare identifier; for `Bool`
/// matches it is the literal `true` or `false` (which the parser tokenizes as
/// `KLiteral::Boolean`, accepted here in the same position). The body of the first
/// branch whose tag matches `value.tag` is dispatched as a tail expression; the others
/// are never touched. `it` is bound to the inner value in a per-MATCH child scope,
/// modeled on `KFunction::invoke`'s per-call frame so the binding doesn't leak into
/// the surrounding scope. For `Bool` matches `it` is `Null` — accurate, since there
/// is no payload.
///
/// No matching branch → `ShapeError("inexhaustive match = no branch for `X`")` — same
/// rule for `Bool` as for `Tagged`, so `MATCH cond WITH (true -> ...)` against a
/// `false` value is an error rather than a silent null. Malformed branch shape (not
/// `<tag> -> <body>` triples) → `ShapeError`.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let (tag, value) = match bundle.get("value") {
        Some(KObject::Tagged { tag, value, .. }) => (tag.clone(), Rc::clone(value)),
        Some(KObject::Bool(b)) => (
            if *b { "true".to_string() } else { "false".to_string() },
            Rc::new(KObject::Null),
        ),
        Some(other) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "value".to_string(),
                expected: "Tagged or Bool".to_string(),
                got: other.ktype().name().to_string(),
            }));
        }
        None => return err(KError::new(KErrorKind::MissingArg("value".to_string()))),
    };
    let branches_expr = match extract_kexpression(&mut bundle, "branches") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "MATCH branches slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    let branch_body = match find_branch_body(&branches_expr, &tag, false) {
        Ok(Some(body)) => body,
        Ok(None) => {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "inexhaustive match = no branch for `{}`",
                tag
            ))));
        }
        Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
    };
    // Per-MATCH frame for the `it` binding — same pattern as `KFunction::invoke`. The
    // child scope's `outer` is the MATCH call site, so free names in the branch body
    // resolve against the surrounding scope. `it` is bound only in the child, so it
    // disappears when the frame drops. The call-site frame Rc is chained onto the new
    // frame's `outer_frame`: the call-site scope lives in *some* arena, and if that arena
    // is per-call (e.g., MATCH inside a user-fn body), it must stay alive while the new
    // frame's child scope's `outer` pointer is in use. `current_frame` returns `None` when
    // the call site is run-root (which outlives the run, so no chain needed).
    let frame: Rc<CallArena> = CallArena::new(scope, sched.current_frame());
    let arena_ptr: *const RuntimeArena = frame.arena();
    let scope_ptr: *const Scope<'_> = frame.scope();
    // SAFETY: heap-pinning makes both pointers valid for the Rc's lifetime. The
    // re-borrow ends before the `frame` move into `BodyResult::Tail`.
    let inner_arena: &'a RuntimeArena = unsafe { &*(arena_ptr as *const _) };
    let child: &'a Scope<'a> = unsafe { &*(scope_ptr as *const _) };
    let it_obj: &'a KObject<'a> = inner_arena.alloc(value.deep_clone());
    // Fresh per-call child scope: the `it` binding never collides. `bind_value`'s rebind
    // check therefore always passes; the `_` swallow is intentional.
    // `it` is the first (and only) sibling in the freshly minted per-MATCH child
    // scope. Tag it at lexical index 0 — the arm body runs at index 0 too, but
    // visibility takes care of itself: the consumer's chain prepends `(child.id, 0)`
    // for the arm body, and the `it` entry's `idx: 0 < c: 0` would fail. We need
    // `it` to be visible to the arm body, so install with the nominal-binder
    // carve-out (semantically: it's not a sibling reference, it's the entire
    // surrounding context for the arm — same logic the FN parameter path uses).
    let _ = child.bind_value(
        "it".to_string(),
        it_obj,
        BindingIndex { idx: 0, nominal_binder: true },
    );
    // The arm body enters a fresh lexical block (its scope is the per-MATCH child
    // scope, distinct from the call-site scope). For multi-statement arm bodies
    // (`tag -> ((s_0) (s_1) ... (s_{N-1}))`) split into N statements: submit the
    // first N-1 as siblings into the arm scope at chain indices `1..N-1`, then
    // tail-replace into the last statement at index `N`. Single-statement bodies
    // pass through unchanged at index 0.
    let arm_scope_id = child.id;
    let statements = split_body_statements(branch_body);
    let n = statements.len();
    if n >= 2 {
        let call_site_chain = sched
            .current_lexical_chain()
            .expect("MATCH body runs inside an enter_block / active_chain");
        let mut stmts = statements;
        let last = stmts.pop().expect("n >= 2");
        for (i, stmt) in stmts.into_iter().enumerate() {
            let chain = LexicalFrame::push(
                Some(call_site_chain.clone()),
                arm_scope_id,
                i + 1,
            );
            sched.with_active_frame(frame.clone(), &mut |s| {
                s.add_dispatch_with_chain(stmt.clone(), child, chain.clone());
            });
        }
        BodyResult::tail_with_block_at_index(last, Some(frame), arm_scope_id, n)
    } else {
        let only = statements.into_iter().next().expect("n >= 1");
        BodyResult::tail_with_block(only, Some(frame), arm_scope_id)
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "MATCH",
        sig(KType::Any, vec![
            kw("MATCH"),
            arg("value", KType::Any),
            kw("WITH"),
            arg("branches", KType::KExpression),
        ]),
        body,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one_err, run_root_silent, run_root_with_buf};
    use crate::machine::{KErrorKind, RuntimeArena};

    fn run_program(source: &str) -> Vec<u8> {
        let arena = RuntimeArena::new();
        let (scope, captured) = run_root_with_buf(&arena);
        run(scope, source);
        let bytes = captured.borrow().clone();
        bytes
    }

    #[test]
    fn match_dispatches_branch_for_matching_tag() {
        let bytes = run_program(
            "UNION Maybe = (some :Number none :Null)\n\
             LET m = (Maybe (some 42))\n\
             MATCH (m) WITH (some -> (PRINT \"got\") none -> (PRINT \"no\"))",
        );
        assert_eq!(bytes, b"got\n");
    }

    #[test]
    fn match_binds_inner_value_to_it() {
        // `it` resolves to the inner value through the per-MATCH child scope; PRINT's
        // `msg:Str` slot picks up the binding at dispatch time.
        let bytes = run_program(
            "UNION Outcome = (ok :Str err :Str)\n\
             LET r = (Outcome (ok \"all good\"))\n\
             MATCH (r) WITH (ok -> (PRINT it) err -> (PRINT \"failed\"))",
        );
        assert_eq!(bytes, b"all good\n");
    }

    #[test]
    fn match_does_not_run_unmatched_branches() {
        // Lazy: the `none` branch's PRINT must not fire when the value is `some`.
        let bytes = run_program(
            "UNION Maybe = (some :Number none :Null)\n\
             LET m = (Maybe (some 1))\n\
             MATCH (m) WITH (some -> (PRINT \"yes\") none -> (PRINT \"NO_SHOULD_NOT_APPEAR\"))",
        );
        assert_eq!(bytes, b"yes\n");
    }

    #[test]
    fn match_inexhaustive_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "UNION Maybe = (some :Number none :Null)\nLET m = (Maybe (none null))",
        );
        let err = run_one_err(scope, parse_one("MATCH (m) WITH (some -> (PRINT \"yes\"))"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("inexhaustive") && msg.contains("`none`")),
            "expected inexhaustive ShapeError, got {err}",
        );
    }

    #[test]
    fn match_other_branch_runs_when_tag_matches() {
        let bytes = run_program(
            "UNION Maybe = (some :Number none :Null)\n\
             LET m = (Maybe (none null))\n\
             MATCH (m) WITH (some -> (PRINT \"yes\") none -> (PRINT \"nothing\"))",
        );
        assert_eq!(bytes, b"nothing\n");
    }

    #[test]
    fn match_on_bool_true_takes_true_branch() {
        let bytes = run_program(
            "MATCH true WITH (true -> (PRINT \"yes\") false -> (PRINT \"no\"))",
        );
        assert_eq!(bytes, b"yes\n");
    }

    #[test]
    fn match_on_bool_false_takes_false_branch() {
        let bytes = run_program(
            "MATCH false WITH (true -> (PRINT \"yes\") false -> (PRINT \"no\"))",
        );
        assert_eq!(bytes, b"no\n");
    }

    #[test]
    fn recursive_tagged_match_no_uaf() {
        // Regression: a recursive HOP through a tagged value triggered a use-after-free
        // during writer drop. Root cause was structural in the scheduler/MATCH frame
        // chain: MATCH built a per-call `CallArena` whose child scope's `outer` pointed
        // into the call-site (the per-call arena of the enclosing user-fn). The
        // enclosing-fn frame was dropped on TCO replace before MATCH's deferred lift
        // ran, so the value-lift read `scope.outer.arena` through a freed pointer.
        // Fixed by chaining the call-site frame's Rc onto the new `CallArena` via
        // `SchedulerHandle::current_frame` + `outer_frame`.
        let bytes = run_program(
            "UNION Bit = (one :Null zero :Null)\n\
             FN (HOP b :Tagged) -> Any = (MATCH (b) WITH (\
                 one -> (HOP (Bit (zero null)))\
                 zero -> (PRINT \"done\")\
             ))\n\
             HOP (Bit (one null))",
        );
        assert_eq!(bytes, b"done\n");
    }

    #[test]
    fn match_on_bool_inexhaustive_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("MATCH true WITH (false -> (PRINT \"x\"))"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("inexhaustive") && msg.contains("`true`")),
            "expected inexhaustive ShapeError for missing true branch, got {err}",
        );
    }

    /// Multi-statement MATCH arm body: each statement runs and the arm's terminal
    /// is the last statement's value. Effect ordering between statements is
    /// topological (sibling sub-slots), not strict source-order.
    #[test]
    fn multi_statement_match_branch_returns_last_value() {
        let bytes = run_program(
            "UNION Maybe = (some :Number none :Null)\n\
             LET m = (Maybe (some 5))\n\
             MATCH (m) WITH (\
                 some -> ((PRINT \"got\") (PRINT it))\
                 none -> (PRINT \"no\")\
             )",
        );
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("got"), "missing 'got' in {s:?}");
        assert!(s.contains("5"), "missing 'it' value in {s:?}");
    }

    /// FN recursion through a multi-statement MATCH arm: the recursive HOP call is
    /// the last statement of the `one` arm and gets tail-replaced. Without TCO,
    /// deep recursion would blow the scheduler.
    #[test]
    fn fn_recursion_with_multi_statement_body_via_match_terminates() {
        let bytes = run_program(
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
}
