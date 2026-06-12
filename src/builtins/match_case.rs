use crate::machine::model::types::KKind;
use std::rc::Rc;

use crate::machine::core::LexicalFrame;
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CallArena, KError, KErrorKind, SchedulerHandle, Scope,
};

use super::branch_walk::{find_branch_body, resolve_arm_return_contract};
use super::{arg, err, kw, sig};
#[cfg(not(feature = "action-harness"))]
use super::register_builtin;
use crate::machine::core::kfunction::body::split_body_statements;

/// `MATCH <value:Any> -> :<T> WITH <branches:KExpression>` — branch by tag.
///
/// `value` is a `Tagged` or a `Bool`; `Bool` is projected at entry to a synthetic
/// `(true|false, Null)` pair so the shared branch-walker handles both. Other input
/// types raise `TypeMismatch`. `-> :T` is the mandatory declared return type every arm
/// must agree on; the selected arm's result is checked against it (and re-tagged to it)
/// when its value lifts, via the [`ReturnContract::Arm`](crate::machine::core::kfunction::body::ReturnContract)
/// carried on the tail. `branches` is the parens-wrapped body of repeated
/// `<tag> -> <body>` triples; the first matching arm is dispatched as a tail
/// expression with `it` bound to the inner value in a per-MATCH child scope (so
/// the binding can't leak). No matching branch → `ShapeError("inexhaustive match
/// = no branch for `X`")`; malformed shape → `ShapeError`.
pub fn body<'a, 's>(
    sched: &mut dyn SchedulerHandle<'a, 's>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let (tag, value) = match bundle.get("value") {
        Some(KObject::Tagged { tag, value, .. }) => (tag.clone(), Rc::clone(value)),
        Some(KObject::Bool(b)) => (
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            },
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
    let contract = match resolve_arm_return_contract(
        sched.current_scope(),
        &mut bundle,
        "MATCH",
        sched.current_lexical_chain(),
    ) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    let branches_expr = match bundle.extract_kexpression_or_shape_error("MATCH", "branches") {
        Ok(e) => e,
        Err(e) => return err(e),
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
    // Chain the call-site frame per per-call-arena-protocol.md § Outer-frame chain.
    let frame: Rc<CallArena> = CallArena::new(sched.current_scope(), sched.current_frame());
    // `it` binds at idx 0; the arm body's statements sit at idx >= 1, so the strict
    // `idx < cutoff` rule lets the body see it — same path the FN parameter uses. The
    // per-call re-anchor is concentrated in `with_anchored_child`; arm statements dispatch
    // via `add_dispatch_with_chain_in_frame`, which stores `Yoked` and re-projects the scope
    // from the frame cart at the read boundary — so the seed itself fabricates no `&'a`.
    frame.with_anchored_child(|arena, child| {
        let it_obj = arena.alloc_object(value.deep_clone());
        let _ = child.bind_value("it".to_string(), it_obj, BindingIndex::value(0));
    });
    // Multi-statement arms (`tag -> ((s_0) ... (s_{N-1}))`) submit the first N-1 as
    // siblings at chain indices `1..N-1` and tail-replace into the last at `N`.
    let arm_scope_id = frame.scope_for_bind().id;
    let statements = split_body_statements(branch_body);
    let n = statements.len();
    if n >= 2 {
        let call_site_chain = sched
            .current_lexical_chain()
            .expect("MATCH body runs inside an enter_block / active_chain");
        let mut stmts = statements;
        let last = stmts.pop().expect("n >= 2");
        for (i, stmt) in stmts.into_iter().enumerate() {
            let chain = LexicalFrame::push(Some(call_site_chain.clone()), arm_scope_id, i + 1);
            sched.with_active_frame(frame.clone(), &mut |s| {
                s.add_dispatch_with_chain_in_frame(stmt.clone(), chain.clone());
            });
        }
        BodyResult::tail_with_block_at_index(last, Some(frame), arm_scope_id, n, Some(contract))
    } else {
        let only = statements.into_iter().next().expect("n >= 1");
        BodyResult::tail_with_block(only, Some(frame), arm_scope_id, Some(contract))
    }
}

/// `Action`-harness twin of [`body`]: selects the matching arm, mints a per-MATCH child frame with
/// `it` bound, and tail-replaces into the arm body carrying the `-> :T` `Arm` contract.
#[cfg(feature = "action-harness")]
pub fn body_action<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use super::branch_walk::{arm_tail, resolve_arm_contract};
    use crate::machine::core::kfunction::action::{arg_object, require_kexpression, Action};

    let (tag, value) = match arg_object(ctx.args, "value") {
        Some(KObject::Tagged { tag, value, .. }) => (tag.clone(), Rc::clone(value)),
        Some(KObject::Bool(b)) => (
            if *b { "true" } else { "false" }.to_string(),
            Rc::new(KObject::Null),
        ),
        Some(other) => {
            return Action::Done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "value".to_string(),
                expected: "Tagged or Bool".to_string(),
                got: other.ktype().name(),
            })))
        }
        None => return Action::Done(Err(KError::new(KErrorKind::MissingArg("value".to_string())))),
    };
    let contract = match resolve_arm_contract(ctx, "MATCH") {
        Ok(c) => c,
        Err(e) => return Action::Done(Err(e)),
    };
    let branches_expr = match require_kexpression(ctx.args, "MATCH", "branches") {
        Ok(e) => e,
        Err(e) => return Action::Done(Err(e)),
    };
    let branch_body = match find_branch_body(&branches_expr, &tag, false) {
        Ok(Some(body)) => body,
        Ok(None) => {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                "inexhaustive match = no branch for `{}`",
                tag
            )))))
        }
        Err(msg) => return Action::Done(Err(KError::new(KErrorKind::ShapeError(msg)))),
    };
    arm_tail(
        ctx.scope,
        ctx.frame.map(Rc::clone),
        value.deep_clone(),
        branch_body,
        contract,
    )
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(
        KType::Any,
        vec![
            kw("MATCH"),
            arg("value", KType::Any),
            kw("->"),
            arg("return_type", KType::OfKind(KKind::Proper)),
            kw("WITH"),
            arg("branches", KType::KExpression),
        ],
    );
    #[cfg(feature = "action-harness")]
    crate::builtins::register_action_builtin(scope, "MATCH", signature, body_action);
    #[cfg(not(feature = "action-harness"))]
    register_builtin(scope, "MATCH", signature, body);
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{
        parse_one, run, run_one_err, run_root_silent, run_root_with_buf,
    };
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
            "UNION Maybe = (Some :Number None :Null)\n\
             LET m = (Maybe (Some 42))\n\
             MATCH (m) -> :Str WITH (Some -> (PRINT \"got\") None -> (PRINT \"no\"))",
        );
        assert_eq!(bytes, b"got\n");
    }

    #[test]
    fn match_binds_inner_value_to_it() {
        let bytes = run_program(
            "UNION Outcome = (Ok :Str Err :Str)\n\
             LET r = (Outcome (Ok \"all good\"))\n\
             MATCH (r) -> :Str WITH (Ok -> (PRINT it) Err -> (PRINT \"failed\"))",
        );
        assert_eq!(bytes, b"all good\n");
    }

    #[test]
    fn match_does_not_run_unmatched_branches() {
        let bytes = run_program(
            "UNION Maybe = (Some :Number None :Null)\n\
             LET m = (Maybe (Some 1))\n\
             MATCH (m) -> :Str WITH (Some -> (PRINT \"yes\") None -> (PRINT \"NO_SHOULD_NOT_APPEAR\"))",
        );
        assert_eq!(bytes, b"yes\n");
    }

    #[test]
    fn match_inexhaustive_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "UNION Maybe = (Some :Number None :Null)\nLET m = (Maybe (None null))",
        );
        let err = run_one_err(
            scope,
            parse_one("MATCH (m) -> :Str WITH (Some -> (PRINT \"yes\"))"),
        );
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("inexhaustive") && msg.contains("`None`")),
            "expected inexhaustive ShapeError, got {err}",
        );
    }

    #[test]
    fn match_arm_violating_declared_return_type_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "UNION Maybe = (Some :Number None :Null)\nLET m = (Maybe (Some 1))",
        );
        // Declared `:Number`, but the taken arm returns a Str (PRINT's rendered string).
        let err = run_one_err(
            scope,
            parse_one("MATCH (m) -> :Number WITH (Some -> (PRINT \"x\") None -> (PRINT \"y\"))"),
        );
        assert!(
            matches!(&err.kind, KErrorKind::TypeMismatch { arg, .. } if arg == "<return>"),
            "expected <return> TypeMismatch from the arm result, got {err}",
        );
    }

    #[test]
    fn match_value_is_admissible_against_declared_return_slot() {
        // The arm result is re-tagged to the declared `:Number`, so a Number-typed
        // FN slot admits the whole MATCH expression.
        let bytes = run_program(
            "UNION Maybe = (Some :Number None :Null)\n\
             LET m = (Maybe (Some 7))\n\
             FN (ID n :Number) -> :Number = (n)\n\
             PRINT (ID (MATCH (m) -> :Number WITH (Some -> (it) None -> (0))))",
        );
        assert_eq!(bytes, b"7\n");
    }

    #[test]
    fn match_other_branch_runs_when_tag_matches() {
        let bytes = run_program(
            "UNION Maybe = (Some :Number None :Null)\n\
             LET m = (Maybe (None null))\n\
             MATCH (m) -> :Str WITH (Some -> (PRINT \"yes\") None -> (PRINT \"nothing\"))",
        );
        assert_eq!(bytes, b"nothing\n");
    }

    #[test]
    fn match_on_bool_true_takes_true_branch() {
        let bytes = run_program(
            "MATCH true -> :Str WITH (true -> (PRINT \"yes\") false -> (PRINT \"no\"))",
        );
        assert_eq!(bytes, b"yes\n");
    }

    #[test]
    fn match_on_bool_false_takes_false_branch() {
        let bytes = run_program(
            "MATCH false -> :Str WITH (true -> (PRINT \"yes\") false -> (PRINT \"no\"))",
        );
        assert_eq!(bytes, b"no\n");
    }

    #[test]
    fn recursive_tagged_match_no_uaf() {
        // Pins the `outer_frame` chain — per-call-arena-protocol.md
        // § MATCH frame lifetime under tail recursion.
        let bytes = run_program(
            "UNION Bit = (One :Null Zero :Null)\n\
             FN (HOP b :Any) -> Any = (MATCH (b) -> :Str WITH (\
                 One -> (HOP (Bit (Zero null)))\
                 Zero -> (PRINT \"done\")\
             ))\n\
             HOP (Bit (One null))",
        );
        assert_eq!(bytes, b"done\n");
    }

    #[test]
    fn match_on_bool_inexhaustive_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(
            scope,
            parse_one("MATCH true -> :Str WITH (false -> (PRINT \"x\"))"),
        );
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("inexhaustive") && msg.contains("`true`")),
            "expected inexhaustive ShapeError for missing true branch, got {err}",
        );
    }

    #[test]
    fn multi_statement_match_branch_returns_last_value() {
        let bytes = run_program(
            "UNION Maybe = (Some :Number None :Null)\n\
             LET m = (Maybe (Some 5))\n\
             MATCH (m) -> :Str WITH (\
                 Some -> ((PRINT \"got\") (PRINT it))\
                 None -> (PRINT \"no\")\
             )",
        );
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("got"), "missing 'got' in {s:?}");
        assert!(s.contains("5"), "missing 'it' value in {s:?}");
    }

    #[test]
    fn fn_recursion_with_multi_statement_body_via_match_terminates() {
        let bytes = run_program(
            "UNION Bit = (One :Null Zero :Null)\n\
             FN (HOP b :Any) -> Any = (\
                 (PRINT \"step\")\
                 (MATCH (b) -> :Str WITH (\
                     One -> (HOP (Bit (Zero null)))\
                     Zero -> (PRINT \"done\")\
                 ))\
             )\n\
             HOP (Bit (One null))",
        );
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("done"), "expected 'done' to print, got {s:?}");
    }
}
