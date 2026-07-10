use crate::machine::model::types::KKind;

use crate::machine::model::KType;
use crate::machine::{KError, KErrorKind, Scope};

use super::branch_walk::find_branch_body_by_type;
use super::{arg, kw, sig};

/// `MATCH <value:Any> -> :<T> WITH <branches:KExpression>` — branch by type.
///
/// Any value can be matched. Each arm head resolves to a `KType`; the arms whose type
/// admits `value` ([`KType::matches_value`]) compete in the most-specific-wins
/// tournament (ruling F1), and the winner runs. A variant head over a tagged-union value
/// binds the wrapped payload to `it`; a general type head binds the scrutinee unchanged
/// (ruling F3); a boolean head binds `Null`. `-> :T` is the mandatory declared return
/// type every arm must agree on; the selected arm's result is checked against it (and
/// re-tagged to it) when its value lifts, via the
/// [`ReturnContract::Arm`](crate::machine::core::kfunction::body::ReturnContract) carried
/// on the tail. `branches` is the parens-wrapped body of repeated `<head> -> <body>`
/// triples; the winning arm is dispatched as a tail expression with `it` bound in a
/// per-MATCH child scope (so the binding can't leak). No admitting arm → `ShapeError`
/// naming the scrutinee's runtime type; an F1 ambiguity or malformed shape → `ShapeError`.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use super::branch_walk::{arm_tail, resolve_arm_contract};
    use crate::machine::core::kfunction::action::{arg_object, require_kexpression, Action};

    let value = match arg_object(ctx.args, "value") {
        Some(v) => v.deep_clone(),
        None => {
            return Action::Done(Err(KError::new(KErrorKind::MissingArg(
                "value".to_string(),
            ))))
        }
    };
    let contract = crate::try_action!(resolve_arm_contract(ctx, "MATCH"));
    let branches_expr = crate::try_action!(require_kexpression(ctx.args, "MATCH", "branches"));
    let selected =
        match find_branch_body_by_type(&branches_expr, &value, ctx.scope, ctx.chain.clone()) {
            Ok(Some(arm)) => arm,
            Ok(None) => {
                return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "inexhaustive match = no branch for value of type `{}`",
                    value.ktype().name()
                )))))
            }
            Err(msg) => return Action::Done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        };
    // The scrutinee's envelope travels to the `it` binding: `it` is a clone of the matched value
    // (or its payload), so it reaches every region the scrutinee does, and the envelope's retained
    // host is the pin the copy's reach mints under. A region-pure scrutinee has no carrier → empty
    // reach.
    let scrutinee = ctx.arg_carrier("value").map(|carrier| carrier.duplicate());
    arm_tail(
        ctx.scope,
        ctx.frame.map(|f| f.storage_rc()),
        super::branch_walk::ItSource::Value {
            value: selected.it_value,
            delivered: scrutinee,
        },
        selected.body,
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
    use crate::machine::core::run_root_storage;
    use crate::machine::KErrorKind;

    fn run_program(source: &str) -> Vec<u8> {
        let region = run_root_storage();
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
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(
            scope,
            "UNION Maybe = (Some :Number None :Null)\nLET m = (Maybe (None null))",
        );
        let err = run_one_err(
            scope,
            parse_one("MATCH (m) -> :Str WITH (Some -> (PRINT \"yes\"))"),
        );
        // The no-arm error names the scrutinee's runtime type — a `None` value is a per-variant
        // newtype, so it reports the member name `None`.
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("inexhaustive") && msg.contains("None")),
            "expected inexhaustive ShapeError naming the runtime type, got {err}",
        );
    }

    #[test]
    fn match_arm_violating_declared_return_type_errors() {
        let region = run_root_storage();
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
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let err = run_one_err(
            scope,
            parse_one("MATCH true -> :Str WITH (false -> (PRINT \"x\"))"),
        );
        // No `true` arm admits the `true` scrutinee; the error names its runtime type `Bool`.
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("inexhaustive") && msg.contains("Bool")),
            "expected inexhaustive ShapeError naming the runtime type, got {err}",
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
    fn match_over_union_producer_selects_number_arm() {
        // A `:(Number | Str)`-returning FN yields a Number here; the `Number` arm selects.
        let bytes = run_program(
            "FN (PICK n :Number) -> :(Number | Str) = (n)\n\
             MATCH (PICK 7) -> :Str WITH (Number -> (PRINT \"num\") Str -> (PRINT \"str\"))",
        );
        assert_eq!(bytes, b"num\n");
    }

    #[test]
    fn match_over_union_producer_selects_str_arm() {
        let bytes = run_program(
            "FN (PICK s :Str) -> :(Number | Str) = (s)\n\
             MATCH (PICK \"hi\") -> :Str WITH (Number -> (PRINT \"num\") Str -> (PRINT \"str\"))",
        );
        assert_eq!(bytes, b"str\n");
    }

    #[test]
    fn match_general_type_arm_on_untagged_scrutinee() {
        // `MATCH (42) ...` — an untagged Number scrutinee picks the `Number` arm; `it` binds
        // the scrutinee unchanged (F3).
        let bytes =
            run_program("MATCH (42) -> :Str WITH (Number -> (PRINT it) Str -> (PRINT \"str\"))");
        assert_eq!(bytes, b"42\n");
    }

    #[test]
    fn match_f1_specific_arm_wins_over_broad_arm() {
        // `Number` is strictly more specific than `Any`; the specific arm wins whatever the
        // source order.
        let specific_first = run_program(
            "MATCH (42) -> :Str WITH (Number -> (PRINT \"num\") Any -> (PRINT \"any\"))",
        );
        assert_eq!(specific_first, b"num\n");
        let broad_first = run_program(
            "MATCH (42) -> :Str WITH (Any -> (PRINT \"any\") Number -> (PRINT \"num\"))",
        );
        assert_eq!(broad_first, b"num\n");
    }

    #[test]
    fn match_f1_ambiguous_arms_error_naming_both() {
        // Two `Number` arms both admit a Number with no strict specificity winner → ambiguity.
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let err = run_one_err(
            scope,
            parse_one("MATCH (42) -> :Str WITH (Number -> (PRINT \"a\") Number -> (PRINT \"b\"))"),
        );
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("ambiguous") && msg.contains("`Number`")),
            "expected ambiguity ShapeError naming the tied arms, got {err}",
        );
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
