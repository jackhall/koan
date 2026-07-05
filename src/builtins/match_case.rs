use crate::machine::model::types::KKind;
use std::rc::Rc;

use crate::machine::model::{KObject, KType};
use crate::machine::{KError, KErrorKind, Scope};

use super::branch_walk::find_branch_body;
use super::{arg, kw, sig};

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
pub fn body<'a>(
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
        None => {
            return Action::Done(Err(KError::new(KErrorKind::MissingArg(
                "value".to_string(),
            ))))
        }
    };
    let contract = crate::try_action!(resolve_arm_contract(ctx, "MATCH"));
    let branches_expr = crate::try_action!(require_kexpression(ctx.args, "MATCH", "branches"));
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
    // The scrutinee's reach travels to the `it` binding: `it` is a clone of the matched value, so it
    // reaches every region the scrutinee does. A region-pure scrutinee has no carrier → empty reach.
    let scrutinee_witness = ctx
        .arg_carrier("value")
        .map(|carrier| carrier.witness().clone())
        .unwrap_or_default();
    arm_tail(
        ctx.scope,
        ctx.frame.map(|f| f.storage_rc()),
        super::branch_walk::ItSource::Value {
            value: value.deep_clone(),
            reach: scrutinee_witness,
        },
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
            arg("return_type", KType::OfKind(KKind::ProperType)),
            kw("WITH"),
            arg("branches", KType::KExpression),
        ],
    );
    crate::builtins::register_builtin(scope, "MATCH", signature, body);
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{
        parse_one, run, run_one_err, run_root_silent, run_root_with_buf,
    };
    use crate::machine::core::FrameStorage;
    use crate::machine::KErrorKind;

    fn run_program(source: &str) -> Vec<u8> {
        let region = FrameStorage::run_root();
        let (scope, captured) = run_root_with_buf(&region);
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
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
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
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
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
        // Pins the `FrameStorage.outer` chain — per-call-region/README.md
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
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
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
