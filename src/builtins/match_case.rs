use crate::machine::WriteGate;

use crate::machine::model::KKind;

use crate::machine::model::KType;
use crate::machine::{KError, KErrorKind, Scope};

use super::branch_walk::{find_branch_body_by_member, find_branch_body_by_type};
use super::{arg, kw, sig};
use crate::machine::model::RunRegistries;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { branches, return_type, union, value } }

/// `MATCH <value:Any> -> :<T> WITH <branches:KExpression>` — branch by type test.
///
/// Any value can be matched. Each arm head resolves to a `KType` **through the scope**; the arms
/// whose type admits `value` ([`KType::matches_value`]) compete in the most-specific-wins
/// tournament (ruling F1), and the winner runs with `it` bound to the scrutinee unchanged (ruling
/// F3); a boolean head binds `Null`. Reading heads as type names is a property of this form's
/// syntax, not of the runtime scrutinee — naming a union's variants and unwrapping the matched one
/// is [`body_over`]'s job. `-> :T` is the mandatory declared return
/// type every arm must agree on; the selected arm's result is checked against it (and
/// re-tagged to it) when the arm's tail completes, via the
/// [`ReturnContract::Arm`](crate::machine::ReturnContract) carried
/// on the tail. `branches` is the parens-wrapped body of repeated `<head> -> <body>`
/// triples; the winning arm is dispatched as a tail expression with `it` bound in a
/// per-MATCH overlay scope, a block-local child no later call-site statement reaches on
/// its ancestor walk (so the binding can't leak). No admitting arm → `ShapeError`
/// naming the scrutinee's runtime type; an F1 ambiguity or malformed shape → `ShapeError`.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use super::branch_walk::{arm_tail, payload_envelope, resolve_arm_contract};
    use crate::machine::{Action, require_kexpression};

    // Selection needs only a borrow of the scrutinee — it never stores the reference — so no
    // upfront copy is made.
    let value = match ctx.args.object(&SLOTS.value) {
        Some(v) => v,
        None => {
            return Action::done(Err(KError::new(KErrorKind::MissingArg(
                "value".to_string(),
            ))));
        }
    };
    let contract = crate::try_action!(resolve_arm_contract(ctx, "MATCH"));
    let branches_expr = crate::try_action!(require_kexpression(ctx.args, "MATCH", &SLOTS.branches));
    let selected = match find_branch_body_by_type(
        &branches_expr,
        value,
        ctx.scope,
        ctx.chain.clone(),
        ctx.registries,
        ctx.scratch,
    ) {
        Ok(Some(arm)) => arm,
        Ok(None) => {
            return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                "inexhaustive match = no branch for value of type `{}`",
                value.ktype().display_name(ctx.registries)
            )))));
        }
        Err(msg) => return Action::done(Err(KError::new(KErrorKind::ShapeError(msg)))),
    };
    // The scrutinee reaches its `it` binding through the same carrier door TRY's success arm uses:
    // the envelope's retained host pins the producer until the single bind-time copy, and the
    // projection (ruling F3) picks the scrutinee itself or its wrapped payload. A boolean arm over
    // a `Bool` scrutinee binds `Null` (a boolean carries no payload). A region-pure scrutinee (e.g.
    // a plain Number) has no carrier of its own, so it is placed into this scope's region and
    // enveloped here — the arm binds from one tier either way.
    let scrutinee =
        if !selected.binds_payload && matches!(value, crate::machine::model::KObject::Bool(_)) {
            crate::try_action!(
                ctx.scope
                    .deliver_pure_value(&crate::machine::model::KObject::Null)
            )
        } else {
            match ctx.args.carrier(&SLOTS.value) {
                Some(carrier) => carrier.duplicate(),
                None => crate::try_action!(ctx.scope.deliver_pure_value(value)),
            }
        };
    // A variant/tag arm binds the payload, so the envelope narrows to the payload's own parted
    // cell; a general arm binds the scrutinee whole.
    let it_carrier = match selected.binds_payload {
        true => payload_envelope(&scrutinee),
        false => scrutinee,
    };
    arm_tail(
        ctx.scope,
        it_carrier,
        selected.body,
        contract,
        ctx.registries,
    )
}

/// `MATCH <value:Any> OVER <union:ProperType> -> :<T> WITH <branches:KExpression>` — branch by
/// **union member**.
///
/// `OVER` names the union the arm heads are read against, so every head is a member name looked up
/// in that union's member list rather than a scope name — the head-reading regime is fixed by the
/// form's syntax, never by what the scrutinee turns out to be at runtime. The operand is any
/// union-noded type: a `UNION` binder, a `LET`-bound alias of one, or an inline `:(A | B)`. The arm
/// set must name every member exactly once, so the form is exhaustive by construction; selection
/// then runs the same F1 tournament the type-test form runs, and the winning arm binds `it` to the
/// matched member's payload (a non-wrapping member binds the value itself). See
/// [`find_branch_body_by_member`] for the full rule, and [`body`] for everything the two forms
/// share — the `-> :T` contract, the overlay scope, and the arm tail.
pub fn body_over<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use super::branch_walk::{arm_tail, payload_envelope, resolve_arm_contract, resolve_type_slot};
    use crate::machine::{Action, require_kexpression};

    let value = match ctx.args.object(&SLOTS.value) {
        Some(v) => v,
        None => {
            return Action::done(Err(KError::new(KErrorKind::MissingArg(
                "value".to_string(),
            ))));
        }
    };
    // The spelling the operand slot carried, kept for the diagnostics: a user `UNION` binds an
    // anonymous union, so the handle alone names nothing the source wrote.
    let spelling = ctx.args.unresolved_type(&SLOTS.union);
    let union = crate::try_action!(resolve_type_slot(
        ctx,
        &SLOTS.union,
        "MATCH",
        "OVER operand"
    ));
    let contract = crate::try_action!(resolve_arm_contract(ctx, "MATCH"));
    let branches_expr = crate::try_action!(require_kexpression(ctx.args, "MATCH", &SLOTS.branches));
    let selected = match find_branch_body_by_member(
        &branches_expr,
        union,
        spelling,
        value,
        ctx.scope,
        ctx.registries,
        ctx.scratch,
    ) {
        Ok(arm) => arm,
        Err(msg) => return Action::done(Err(KError::new(KErrorKind::ShapeError(msg)))),
    };
    // The scrutinee reaches `it` through the same carrier door the type-test form uses; a
    // region-pure value (no carrier of its own) is enveloped here so the arm binds from one tier.
    let scrutinee = match ctx.args.carrier(&SLOTS.value) {
        Some(carrier) => carrier.duplicate(),
        None => crate::try_action!(ctx.scope.deliver_pure_value(value)),
    };
    let it_carrier = match selected.binds_payload {
        true => payload_envelope(&scrutinee),
        false => scrutinee,
    };
    arm_tail(
        ctx.scope,
        it_carrier,
        selected.body,
        contract,
        ctx.registries,
    )
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let signature = sig(
        KType::ANY,
        vec![
            kw(registries, "MATCH"),
            arg(registries, &SLOTS.value, KType::ANY),
            kw(registries, "->"),
            arg(
                registries,
                &SLOTS.return_type,
                KType::of_kind(KKind::ProperType),
            ),
            kw(registries, "WITH"),
            arg(registries, &SLOTS.branches, KType::KEXPRESSION),
        ],
    );
    crate::builtins::register_builtin(scope, signature, body, registries, gate);

    // The `OVER` overload sits in its own keyword bucket (`MATCH OVER -> WITH`), so the two forms
    // are told apart by the spelling of the call and never compete in a tournament.
    let over = sig(
        KType::ANY,
        vec![
            kw(registries, "MATCH"),
            arg(registries, &SLOTS.value, KType::ANY),
            kw(registries, "OVER"),
            arg(registries, &SLOTS.union, KType::of_kind(KKind::ProperType)),
            kw(registries, "->"),
            arg(
                registries,
                &SLOTS.return_type,
                KType::of_kind(KKind::ProperType),
            ),
            kw(registries, "WITH"),
            arg(registries, &SLOTS.branches, KType::KEXPRESSION),
        ],
    );
    crate::builtins::register_builtin(scope, over, body_over, registries, gate);
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::TestRun;
    use crate::machine::KErrorKind;
    use crate::machine::model::KObject;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;

    fn run_program(source: &str) -> Vec<u8> {
        let program = program_storage();
        let region = run_root_storage();
        let (mut test_run, captured) = TestRun::with_buf(&program, &region);
        test_run.run(source);

        captured.borrow().clone()
    }

    #[test]
    fn match_dispatches_branch_for_matching_tag() {
        let bytes = run_program(
            "UNION Maybe = (Some :Number None :Null)\n\
             LET m = (Maybe.Some 42)\n\
             MATCH (m) OVER Maybe -> :Str WITH (Some -> (PRINT \"got\") None -> (PRINT \"no\"))",
        );
        assert_eq!(bytes, b"got\n");
    }

    #[test]
    fn match_binds_inner_value_to_it() {
        let bytes = run_program(
            "UNION Outcome = (Ok :Str Err :Str)\n\
             LET r = (Outcome.Ok \"all good\")\n\
             MATCH (r) OVER Outcome -> :Str WITH (Ok -> (PRINT it) Err -> (PRINT \"failed\"))",
        );
        assert_eq!(bytes, b"all good\n");
    }

    #[test]
    fn match_does_not_run_unmatched_branches() {
        let bytes = run_program(
            "UNION Maybe = (Some :Number None :Null)\n\
             LET m = (Maybe.Some 1)\n\
             MATCH (m) OVER Maybe -> :Str WITH (Some -> (PRINT \"yes\") None -> (PRINT \"NO_SHOULD_NOT_APPEAR\"))",
        );
        assert_eq!(bytes, b"yes\n");
    }

    #[test]
    fn match_inexhaustive_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("UNION Maybe = (Some :Number None :Null)\nLET m = (Maybe.None null)");
        let err = test_run.run_one_err(
            test_run.parse_one("MATCH (m) OVER Maybe -> :Str WITH (Some -> (PRINT \"yes\"))"),
        );
        // `OVER` is exhaustive-only: the uncovered member is named at the form, before any value
        // is looked at, so the error fires whichever variant `m` happens to hold.
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("inexhaustive") && msg.contains("None")),
            "expected inexhaustive ShapeError naming the runtime type, got {err}",
        );
    }

    #[test]
    fn match_arm_violating_declared_return_type_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("UNION Maybe = (Some :Number None :Null)\nLET m = (Maybe.Some 1)");
        // Declared `:Number`, but the taken arm returns a Str (PRINT's rendered string).
        let err = test_run.run_one_err(test_run.parse_one(
            "MATCH (m) OVER Maybe -> :Number WITH (Some -> (PRINT \"x\") None -> (PRINT \"y\"))",
        ));
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
             LET m = (Maybe.Some 7)\n\
             FN (ID n :Number) -> :Number = (n)\n\
             PRINT (ID (MATCH (m) OVER Maybe -> :Number WITH (Some -> (it) None -> (0))))",
        );
        assert_eq!(bytes, b"7\n");
    }

    #[test]
    fn match_other_branch_runs_when_tag_matches() {
        let bytes = run_program(
            "UNION Maybe = (Some :Number None :Null)\n\
             LET m = (Maybe.None null)\n\
             MATCH (m) OVER Maybe -> :Str WITH (Some -> (PRINT \"yes\") None -> (PRINT \"nothing\"))",
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
    fn match_on_bool_inexhaustive_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run
            .run_one_err(test_run.parse_one("MATCH true -> :Str WITH (false -> (PRINT \"x\"))"));
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
             LET m = (Maybe.Some 5)\n\
             MATCH (m) OVER Maybe -> :Str WITH (\
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
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run.run_one_err(test_run.parse_one(
            "MATCH (42) -> :Str WITH (Number -> (PRINT \"a\") Number -> (PRINT \"b\"))",
        ));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("ambiguous") && msg.contains("`Number`")),
            "expected ambiguity ShapeError naming the tied arms, got {err}",
        );
    }

    #[test]
    fn match_over_rejects_a_head_naming_no_member() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("UNION Maybe = (Some :Number None :Null)\nLET m = (Maybe.Some 1)");
        // Head validation runs against the `OVER` union's member list before any selection, so a
        // name that is no member of it is an error at the form — never a silent non-match.
        let err = test_run.run_one_err(
            test_run.parse_one("MATCH (m) OVER Maybe -> :Str WITH (Bogus -> (PRINT \"x\"))"),
        );
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg == "`Bogus` is not a member of `Maybe` (members: Some, None)"),
            "expected the unknown-member message listing the union's members, got {err}",
        );
    }

    /// The `-> :T` slot is a `ProperType` slot on a non-binder overload, so a bare user name
    /// reaches the body as the bind seam's unlowered-name carrier and `resolve_arm_contract`
    /// resolves it by scope walk. An unbound one keeps the not-a-known-type diagnostic.
    #[test]
    fn match_unresolved_return_type_name_reports_not_a_known_type() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run
            .run_one_err(test_run.parse_one("MATCH (42) -> :Bogus WITH (Number -> (PRINT \"x\"))"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg == "MATCH return type `Bogus` is not a known type"),
            "expected the unresolved return-type message, got {err}",
        );
    }

    /// The same slot with a *bound* user type name resolves through the carrier and runs, so the
    /// carrier is a real resolution path, not just an error channel.
    #[test]
    fn match_return_type_resolves_a_user_bound_name_through_the_carrier() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Tag = Number");
        let result =
            test_run.run_one(test_run.parse_one("MATCH (42) -> :Tag WITH (Number -> (Tag (7)))"));
        assert!(matches!(result, KObject::Wrapped { .. }));
    }

    #[test]
    fn match_bogus_head_over_non_variant_scrutinee_stays_short() {
        // A non-variant scrutinee (a plain Number) resolves heads through the scope; a bogus head
        // keeps the short unresolved-type message with no variants hint.
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run
            .run_one_err(test_run.parse_one("MATCH (42) -> :Str WITH (Bogus -> (PRINT \"x\"))"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg == "match arm type `Bogus` is not a known type"),
            "expected the short unresolved-type message with no hint, got {err}",
        );
    }

    #[test]
    fn match_on_bool_falls_through_to_typed_arm() {
        // No boolean literal admits the `false` scrutinee, so selection falls through to the
        // typed `Bool` arm.
        let bytes = run_program(
            "MATCH false -> :Str WITH (true -> (PRINT \"yes\") Bool -> (PRINT \"boolean\"))",
        );
        assert_eq!(bytes, b"boolean\n");
    }

    #[test]
    fn match_on_bool_two_admitting_literal_heads_are_ambiguous() {
        // Two `true ->` heads both admit the `true` scrutinee as exact matches with no strict
        // winner → ambiguity.
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run
            .run_one_err(test_run.parse_one(
                "MATCH true -> :Str WITH (true -> (PRINT \"a\") true -> (PRINT \"b\"))",
            ));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("ambiguous") && msg.contains("`true`")),
            "expected ambiguity ShapeError naming the tied exact heads, got {err}",
        );
    }

    #[test]
    fn fn_recursion_with_multi_statement_body_via_match_terminates() {
        let bytes = run_program(
            "UNION Bit = (One :Null Zero :Null)\n\
             FN (HOP b :Any) -> Any = (\
                 (PRINT \"step\")\
                 (MATCH (b) OVER Bit -> :Str WITH (\
                     One -> (HOP (Bit.Zero null))\
                     Zero -> (PRINT \"done\")\
                 ))\
             )\n\
             HOP (Bit.One null)",
        );
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("done"), "expected 'done' to print, got {s:?}");
    }

    /// A non-wrapping member binds the value itself: an inline `:(Number | Str)` holds structural
    /// members, which wrap nothing, so the winning arm's `it` is the scrutinee unchanged (F3).
    #[test]
    fn match_over_a_structural_member_binds_the_value_unchanged() {
        let bytes = run_program(
            "LET NumStr = :(Number | Str)\n\
             MATCH (5) OVER NumStr -> :Str WITH (Number -> (PRINT it) Str -> (PRINT \"str\"))",
        );
        assert_eq!(bytes, b"5\n");
    }

    /// The `OVER` operand must be union-noded. A proper type that is no union names no members to
    /// read the heads against, so the form errors before any arm is looked at.
    #[test]
    fn match_over_a_non_union_operand_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run.run_one_err(
            test_run.parse_one("MATCH (1) OVER Number -> :Str WITH (Number -> (PRINT it))"),
        );
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg == "`MATCH … OVER` operand must resolve to a union type; `Number` is not one"),
            "expected the non-union operand error, got {err}",
        );
    }

    /// Naming one member twice is an arm-set error, not a first-wins selection: validation runs
    /// over the whole slate on every execution.
    #[test]
    fn match_over_rejects_a_duplicate_head() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("UNION Maybe = (Some :Number None :Null)\nLET m = (Maybe.Some 1)");
        let err = test_run.run_one_err(test_run.parse_one(
            "MATCH (m) OVER Maybe -> :Str WITH (Some -> (PRINT \"a\") Some -> (PRINT \"b\") None -> (PRINT \"n\"))",
        ));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg == "`MATCH … OVER` names member `Some` twice"),
            "expected the duplicate-member error, got {err}",
        );
    }

    /// A scrutinee that inhabits no member of the `OVER` union is an error naming both: the arm set
    /// is exhaustive over `U`, so the miss is the value's, not the slate's.
    #[test]
    fn match_over_a_value_outside_the_union_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("UNION Maybe = (Some :Number None :Null)");
        let err = test_run.run_one_err(test_run.parse_one(
            "MATCH (\"x\") OVER Maybe -> :Str WITH (Some -> (PRINT \"s\") None -> (PRINT \"n\"))",
        ));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg == "value of type `Str` inhabits no member of `Maybe`"),
            "expected the inhabits-no-member error, got {err}",
        );
    }

    /// A `_` arm is the default: it stands in for every member no named arm claims, so an arm set
    /// carrying one may leave members uncovered and still be a complete match.
    #[test]
    fn match_over_wildcard_defaults_the_uncovered_members() {
        let bytes = run_program(
            "UNION Maybe = (Some :Number None :Null)\n\
             LET m = (Maybe.None null)\n\
             MATCH (m) OVER Maybe -> :Str WITH (Some -> (PRINT \"s\") _ -> (PRINT \"default\"))",
        );
        assert_eq!(bytes, b"default\n");
    }

    /// `_` binds `it` the way a named arm does — to the payload, when the value wraps a member of
    /// the walked union.
    #[test]
    fn match_over_wildcard_binds_the_payload() {
        let bytes = run_program(
            "UNION Maybe = (Some :Number None :Null)\n\
             LET m = (Maybe.Some 7)\n\
             MATCH (m) OVER Maybe -> :Str WITH (None -> (PRINT \"n\") _ -> (PRINT it))",
        );
        assert_eq!(bytes, b"7\n");
    }

    /// A named arm wins over `_` whatever the source order — the default arm is the fallback, not
    /// a competitor in the tournament.
    #[test]
    fn match_over_a_named_arm_wins_over_the_wildcard() {
        let bytes = run_program(
            "UNION Maybe = (Some :Number None :Null)\n\
             LET m = (Maybe.Some 1)\n\
             MATCH (m) OVER Maybe -> :Str WITH (_ -> (PRINT \"default\") Some -> (PRINT \"named\"))",
        );
        assert_eq!(bytes, b"named\n");
    }

    /// Without a `_` arm the coverage rule is unchanged: an uncovered member is an error at the
    /// form, and the diagnostic points at the arm that would fix it.
    #[test]
    fn match_over_without_a_wildcard_still_errors_on_an_uncovered_member() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("UNION Maybe = (Some :Number None :Null)\nLET m = (Maybe.Some 1)");
        let err = test_run.run_one_err(
            test_run.parse_one("MATCH (m) OVER Maybe -> :Str WITH (Some -> (PRINT \"s\"))"),
        );
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("no arm for None") && msg.contains("`_` arm")),
            "expected the inexhaustive error suggesting a `_` arm, got {err}",
        );
    }

    /// `_` covers the members no named arm claims — never a value outside the union, which is the
    /// value's miss and stays an error with a default arm present.
    #[test]
    fn match_over_wildcard_does_not_cover_a_value_outside_the_union() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("UNION Maybe = (Some :Number None :Null)");
        let err = test_run.run_one_err(
            test_run.parse_one("MATCH (\"x\") OVER Maybe -> :Str WITH (_ -> (PRINT \"default\"))"),
        );
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg == "value of type `Str` inhabits no member of `Maybe`"),
            "expected the inhabits-no-member error, got {err}",
        );
    }

    /// One `_` arm at most: a second is an arm-set error, the same shape naming a member twice is.
    #[test]
    fn match_over_rejects_a_second_wildcard_arm() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("UNION Maybe = (Some :Number None :Null)\nLET m = (Maybe.Some 1)");
        let err = test_run.run_one_err(test_run.parse_one(
            "MATCH (m) OVER Maybe -> :Str WITH (_ -> (PRINT \"a\") _ -> (PRINT \"b\"))",
        ));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg == "`MATCH … OVER` has more than one `_` arm"),
            "expected the duplicate-wildcard error, got {err}",
        );
    }

    /// A record-repr variant binds its record payload to `it`, the same narrowing a scalar payload
    /// takes — the binding reads the wrap, not the payload's shape.
    #[test]
    fn match_over_binds_a_record_repr_variants_payload() {
        let bytes = run_program(
            "UNION Shape = (Circle :{r :Number} Square :{side :Number})\n\
             LET c = (Shape.Circle {r = 2})\n\
             MATCH (c) OVER Shape -> :Str WITH (Circle -> (PRINT it) Square -> (PRINT \"sq\"))",
        );
        assert_eq!(bytes, b"{r = 2}\n");
    }

    /// The head-reading regime is a property of the form's syntax, never of the runtime scrutinee:
    /// without `OVER`, a variant name in head position is read as a type test and resolves through
    /// the scope — where a member never binds — so it is unbound rather than a silent tag match.
    #[test]
    fn a_variant_name_in_an_over_less_head_is_read_as_a_type_test() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("UNION Maybe = (Some :Number None :Null)\nLET m = (Maybe.Some 1)");
        let err = test_run
            .run_one_err(test_run.parse_one("MATCH (m) -> :Str WITH (Some -> (PRINT \"x\"))"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg == "match arm type `Some` is not a known type"),
            "expected the unresolved-head error from the type-test regime, got {err}",
        );
    }

    /// The type-test form still reads a *variant* head through a sigil, because a projection is an
    /// ordinary type expression: the arm competes on specificity like any other type arm.
    #[test]
    fn an_over_less_match_type_tests_a_projected_member() {
        let bytes = run_program(
            "UNION Maybe = (Some :Number None :Null)\n\
             LET Just = Maybe.Some\n\
             MATCH ((Maybe.Some 1)) -> :Str WITH (Just -> (PRINT \"some\") Any -> (PRINT \"other\"))",
        );
        assert_eq!(bytes, b"some\n");
    }

    /// Closure escape from an arm overlay. The arm binds `it` into an overlay child of the call
    /// site's own scope, so a closure defined in the arm captures that overlay; the escaped closure
    /// must still read `it` after the MATCH ends. Run-root churn after the escape exercises drop
    /// discipline (a dangling reference into a reclaimed scope surfaces under Miri).
    #[test]
    fn match_arm_closure_capturing_it_escapes_soundly() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("UNION Maybe = (Some :Number None :Null)\nLET m = (Maybe.Some 7)");
        test_run.run(
            "LET add = (MATCH (m) OVER Maybe -> :Any WITH (\
                 Some -> (FN :{n :Number} -> Number = (it + n))\
                 None -> (FN :{n :Number} -> Number = (n))\
             ))",
        );
        test_run.run("FN (NOOP) -> Number = (1)");
        for _ in 0..10 {
            test_run.run_one(test_run.parse_one("NOOP"));
        }
        let result = test_run.run_one(test_run.parse_one("add {n = 100}"));
        assert!(
            matches!(result, KObject::Number(n) if *n == 107.0),
            "the escaped closure must still read the arm-bound `it` after churn",
        );
    }
}
