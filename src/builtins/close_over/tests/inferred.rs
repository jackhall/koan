//! `CLOSE (block)` acceptance — the short form that derives its capture list from the block's free
//! identifiers. Every case here is stated against observable behaviour: what the escaped closure
//! answers once its producer frames are dead, how many regions the escape pins, and which
//! diagnostics the form raises. The walk's own decisions — which positions are name *uses* and
//! which are labels or declarations — are pinned name-by-name in
//! [`close_inference::tests`](crate::machine::model::close_inference), because a capture the walk
//! adds needlessly is usually invisible from the outside: a redundant capture copies a value the
//! block could already see and pins nothing extra. What *is* observable is a capture the walk
//! **misses** (the block reports the name unbound) and a value-channel name it invents (the form
//! reports it unbound at the `CLOSE` statement), so those are the discriminators used throughout.

use super::{HELPER_THUNK, THUNK, declaring_chain, error_of, held_and_released, output};
use crate::machine::KErrorKind;
use crate::machine::model::DynamicNameForm;

// ---------- equivalence with the explicit form ----------

/// The inferred list severs exactly what the spelled-out one does: held while `esc` is alive, the
/// run keeps the run root and the block's own region and nothing else, however deep the producer
/// chain the closure was built inside.
#[test]
fn an_inferred_closure_pins_only_its_block_region() {
    for depth in [0usize, 1, 3, 5] {
        assert_eq!(
            held_and_released(&super::producer_chain(
                depth,
                &format!("CLOSE ({THUNK})"),
                ""
            )),
            (2, 0),
            "an inferred closure over a {depth}-deep producer chain must pin the block region alone",
        );
    }
}

/// Severance is not amnesia here either: the free `n` inside the block's nested function is
/// inferred, so the escaped closure still answers after every frame it was built in has died.
#[test]
fn an_inferred_closure_still_answers_after_its_producers_die() {
    assert_eq!(
        output(&super::producer_chain(
            3,
            &format!("CLOSE ({THUNK})"),
            "PRINT (esc {})\n"
        )),
        "7\n"
    );
}

/// Both spellings of the same block agree, value by value.
#[test]
fn the_two_forms_agree_on_the_same_block() {
    let source = |form: &str| {
        format!(
            "LET mk = (FN :{{n :Number}} -> Any = (\
                 (LET m = (n * 2))\
                 ({form})))\n\
             LET esc = (mk {{n = 7}})\n\
             PRINT (esc {{}})\n"
        )
    };
    let explicit = output(&source(
        "CLOSE OVER (n m) ((LET g = (FN :{} -> Number = (n + m))) (g))",
    ));
    assert_eq!(explicit, "21\n");
    assert_eq!(
        output(&source(
            "CLOSE ((LET g = (FN :{} -> Number = (n + m))) (g))"
        )),
        explicit,
        "the inferred list must produce what naming the same two bindings produces",
    );
}

/// `CLOSE OVER ()` keeps meaning "capture nothing", and is a different form from `CLOSE`: the same
/// block that runs under the short form reports its free name unbound under the empty list.
#[test]
fn the_empty_capture_list_stays_distinct_from_the_short_form() {
    assert_eq!(output("LET a = (7)\nLET x = (CLOSE (a))\nPRINT x\n"), "7\n");
    let error = error_of(
        "LET mk = (FN :{n :Number} -> Any = (CLOSE OVER () ((LET g = (FN :{} -> Number = (n))) (g))))\n\
         LET esc = (mk {n = 7})\n",
        "esc {}",
    );
    assert!(
        matches!(&error.kind, KErrorKind::UnboundName(name) if name == "n"),
        "an empty capture list must still capture nothing, got {error}",
    );
}

// ---------- position-aware freeness ----------

/// A block-local binding is not a capture, and it is not required to resolve outside either: `a` is
/// bound nowhere but the block, so a walk that called it free would raise `UnboundName` here.
#[test]
fn a_block_local_binding_is_not_inferred() {
    assert_eq!(
        output("LET b = (2)\nLET x = (CLOSE ((LET a = (1)) (a + b)))\nPRINT x\n"),
        "3\n"
    );
}

/// The strict `idx < cutoff` rule the interpreter resolves by: a read *above* a block-local `LET`
/// of the same name resolves outward and is captured, while the read below it takes the local.
#[test]
fn a_read_above_a_local_declaration_captures_the_outer_one() {
    assert_eq!(
        output("LET x = (1)\nLET y = (CLOSE ((PRINT x) (LET x = (5)) (PRINT x)))\n"),
        "1\n5\n"
    );
}

/// A binder never binds its own right-hand side, so the `x` feeding the block's own `LET x` is the
/// outer one — the same exclusive cutoff a plain block reads under.
#[test]
fn a_declarations_own_right_hand_side_reads_outward() {
    assert_eq!(
        output("LET x = (1)\nLET y = (CLOSE ((LET x = (x + 1)) (x)))\nPRINT y\n"),
        "2\n"
    );
}

/// A nested body restarts its own scope: the inner `LET` shadows only inside the function, so the
/// block's own read of the same name still resolves outward and is captured.
#[test]
fn a_nested_body_shadows_only_within_itself() {
    assert_eq!(
        output(
            "LET x = (1)\n\
             LET y = (CLOSE ((LET g = (FN :{} -> Number = ((LET x = (5)) (x)))) ((g {}) + x)))\n\
             PRINT y\n"
        ),
        "6\n"
    );
}

// ---------- seeded scopes: parameters, operands, arms ----------

/// A nested function's parameters are its own; the free name in its *body* is the capture. If `p`
/// were inferred the form would raise `UnboundName`, and if `n` were missed the escaped call would.
#[test]
fn a_nested_functions_parameter_is_not_inferred_and_its_body_is() {
    assert_eq!(
        output(&super::producer_chain(
            2,
            "CLOSE ((LET g = (FN :{p :Number} -> Number = (p + n))) (g {p = 1}))",
            "PRINT esc\n"
        )),
        "8\n"
    );
}

/// A signature *annotation* is a use position in the enclosing scope, so a per-call name read there
/// is captured — the block still elaborates the parameter type after its producer is gone.
#[test]
fn a_signature_annotation_captures_the_name_it_reads() {
    assert_eq!(
        output(
            "LET mk = (FN :{n :Number} -> Any = (\
                 CLOSE ((LET h = (FN :{b :(TYPE OF n)} -> Number = (b + 1))) (h {b = 4}))))\n\
             LET esc = (mk {n = 1})\n\
             PRINT esc\n"
        ),
        "5\n"
    );
}

/// A `MATCH` arm binds `it` in its own scope. Used only inside an arm it is not a capture — a walk
/// that called it free would report it unbound at the `CLOSE` statement.
#[test]
fn a_match_arms_it_is_not_inferred() {
    assert_eq!(
        output(
            "LET v = (3)\n\
             LET x = (CLOSE (MATCH v -> Number WITH (Number -> (it + 1) Any -> (0))))\n\
             PRINT x\n"
        ),
        "4\n"
    );
}

/// The same name read *outside* any arm of the block is an ordinary free identifier: `it` bound by
/// an enclosing `MATCH` is captured through the chain like any other per-call binding.
#[test]
fn an_enclosing_arms_it_is_captured() {
    assert_eq!(
        output(
            "LET v = (3)\n\
             LET x = (MATCH v -> Any WITH (Number -> (CLOSE (it + 1)) Any -> (0)))\n\
             PRINT x\n"
        ),
        "4\n"
    );
}

/// An `OP` body binds `left` and `right`, a `UNARY OP` body `operands`; none of the three is a free
/// identifier of the block that declares the operator.
#[test]
fn operator_operands_are_not_inferred() {
    assert_eq!(
        output("LET x = (CLOSE ((OP #(⊗) OVER Number = (left * right)) (2 ⊗ 3)))\nPRINT x\n"),
        "6\n"
    );
    assert_eq!(
        output(
            "LET x = (CLOSE (\
                 (UNARY OP #(~) OVER Number -> :(LIST OF Number) = (operands))\
                 (1 ~ 2)))\n\
             PRINT x\n"
        ),
        "[1, 2]\n"
    );
}

// ---------- label positions ----------

/// `m.x` names a member, not a binding: the block reads the module `m` and nothing called `x`.
#[test]
fn an_attribute_label_is_not_inferred() {
    assert_eq!(
        output("MODULE m = (LET v = 5)\nLET x = (CLOSE (m.v))\nPRINT x\n"),
        "5\n"
    );
}

/// A projection's field list is labels too: `a` and `b` are bound nowhere, so a walk that read them
/// as uses would report the first one unbound at the form. `FROM` narrows the record's *carried
/// type* while the substrate stays whole, so what prints is the record it was handed.
#[test]
fn a_projection_field_list_is_not_inferred() {
    assert_eq!(
        output("LET r = ({a = 1 b = 2 c = 3})\nLET p = (CLOSE ((a b) FROM r))\nPRINT p\n"),
        "{a = 1, b = 2, c = 3}\n"
    );
}

/// A record literal's field names are labels; its values are ordinary uses.
#[test]
fn a_record_literals_keys_are_not_inferred() {
    assert_eq!(
        output("LET v = (2)\nLET r = (CLOSE ({first = v}))\nPRINT r\n"),
        "{first = 2}\n"
    );
}

// ---------- declaration windows in the type channel ----------

/// A nominal declaration binds the name it declares inside its own representation, so a
/// self-recursive type needs no capture — the block declares and uses `Tree` on its own.
#[test]
fn a_self_recursive_declaration_needs_no_capture() {
    assert_eq!(
        output("LET x = (CLOSE ((NEWTYPE Tree = :{left :Tree}) (1)))\nPRINT x\n"),
        "1\n"
    );
}

/// A module body announces its nominal declarations body-wide, so a mutually recursive pair inside
/// the block resolves in either source order without either name becoming a capture.
#[test]
fn a_mutually_recursive_module_body_needs_no_capture() {
    let source = |first: &str, second: &str| {
        format!(
            "LET x = (CLOSE (\
                 (MODULE pair = (({first}) ({second})))\
                 (5)))\nPRINT x\n"
        )
    };
    let forward = "NEWTYPE Aa = :{b :Bb}";
    let backward = "NEWTYPE Bb = :{a :Number}";
    assert_eq!(output(&source(forward, backward)), "5\n");
    assert_eq!(output(&source(backward, forward)), "5\n");
}

/// A union's variant tags are declared names as well: named in the declaration and again in the
/// construction, they are never free identifiers of the block. A member is not bare-name-resolvable
/// at all, so a walk that read either occurrence as a use would name something nothing can bind —
/// and, since the type loop now raises on a no-hit, would raise rather than pass silently.
#[test]
fn union_variant_tags_are_not_inferred() {
    assert_eq!(
        output(
            "LET x = (CLOSE (\
                 (UNION Maybe = (Some :Number None :Null))\
                 (Maybe.Some 2)))\n\
             PRINT x\n"
        ),
        "Some(2)\n"
    );
}

/// Ruling 2, the type channel: a type declared in a producer frame is captured by inference, so the
/// escaped block still names it — the twin of the explicit form's `CLOSE OVER (Meters n)`.
#[test]
fn a_per_call_type_is_inferred_like_a_value() {
    let source = |form: &str| {
        format!(
            "LET mk = (FN :{{n :Number}} -> Any = (\
                 (NEWTYPE Meters = Number)\
                 ({form})))\n\
             LET esc = (mk {{n = 3}})\n\
             PRINT (esc {{}})\n"
        )
    };
    let explicit = output(&source(
        "CLOSE OVER (Meters n) ((LET g = (FN :{} -> Any = (Meters n))) (g))",
    ));
    assert_eq!(explicit, "Meters(3)\n");
    assert_eq!(
        output(&source(
            "CLOSE ((LET g = (FN :{} -> Any = (Meters n))) (g))"
        )),
        explicit,
        "the type channel must infer what naming the type explicitly captures",
    );
}

// ---------- frontiers ----------

/// A nested `CLOSE OVER` contributes the names in its *capture list* — they resolve against the
/// outer chain at the inner form's build, so the outer inference has to have brought them in.
#[test]
fn a_nested_explicit_capture_list_is_a_use() {
    assert_eq!(
        output(&super::producer_chain(
            2,
            &format!("CLOSE (CLOSE OVER (n) ({THUNK}))"),
            "PRINT (esc {})\n"
        )),
        "7\n"
    );
}

/// A nested `CLOSE` is walked through: the inner block's free names become the outer form's
/// captures, so the doubly severed closure still answers.
#[test]
fn a_nested_inferred_block_contributes_its_free_names() {
    assert_eq!(
        output(&super::producer_chain(
            2,
            &format!("CLOSE (CLOSE ({THUNK}))"),
            "PRINT (esc {})\n"
        )),
        "7\n"
    );
}

/// A quote in an eager position is data: nothing inside it is a use, so a name bound nowhere is no
/// error, and the block's value is the quote itself.
#[test]
fn an_eager_quote_contributes_nothing() {
    assert_eq!(
        output("LET q = (CLOSE (#(nowhere + 1)))\nPRINT q\n"),
        "nowhere + 1\n"
    );
}

/// A quote *is* the body when it sits in a lazy code slot, so the name inside it is a capture — the
/// escaped block reads it after its producer is gone.
#[test]
fn a_quote_in_a_code_slot_is_walked_as_the_body() {
    assert_eq!(
        output(&super::producer_chain(2, "CLOSE #(n + 1)", "PRINT esc\n")),
        "8\n"
    );
}

// ---------- what inference does not have to name ----------

/// A per-call dispatch registration closes implicitly, exactly as under the explicit form: the
/// inferred list is empty and the escaped body still reaches the registration's home frame alone.
#[test]
fn a_registration_still_closes_implicitly() {
    let inferred = HELPER_THUNK.replace("CLOSE OVER ()", "CLOSE");
    for depth in [0usize, 1, 3] {
        assert_eq!(
            held_and_released(&declaring_chain(depth, &inferred, "")),
            (3, 0),
            "an inferred block pins the registration's home frame alone at depth {depth}",
        );
    }
    assert_eq!(
        output(&declaring_chain(3, &inferred, "PRINT (esc {})\n")),
        "8\n"
    );
}

/// A name that resolves only in the eternal tier stays visible through the block's outer link, so
/// it is skipped rather than copied — and the escape stays flat in the producer depth either way.
#[test]
fn an_eternal_name_is_not_captured() {
    let source = super::producer_chain(
        3,
        "CLOSE ((LET g = (FN :{} -> Number = ((TOPLEVEL 4) + eternal + n))) (g))",
        "PRINT (esc {})\n",
    );
    let prelude = "FN (TOPLEVEL x :Number) -> Number = (x * 2)\nLET eternal = (1)\n";
    assert_eq!(output(&format!("{prelude}{source}")), "16\n");
    assert_eq!(
        held_and_released(&format!(
            "{prelude}{}",
            super::producer_chain(
                3,
                "CLOSE ((LET g = (FN :{} -> Number = (eternal + n))) (g))",
                ""
            )
        )),
        (2, 0),
        "capturing nothing eternal leaves the escape pinning its block region alone",
    );
}

/// A free name that resolves nowhere is this statement's own `UnboundName`, raised at the `CLOSE`
/// form exactly as the explicit list's unresolvable capture is.
#[test]
fn an_unresolvable_free_name_is_unbound_at_the_form() {
    let error = error_of("", "CLOSE (nope + 1)");
    assert!(
        matches!(&error.kind, KErrorKind::UnboundName(name) if name == "nope"),
        "expected UnboundName for an unresolvable free name, got {error}",
    );
}

/// An inferred name whose producer is still in flight parks the form, so what the block captures
/// never depends on the drain order.
#[test]
fn an_in_flight_producer_parks_the_form() {
    assert_eq!(
        output(
            "LET slow = (FN :{q :Number} -> Number = (q + 1))\n\
             LET a = (slow {q = 6})\n\
             LET x = (CLOSE (a))\n\
             PRINT x\n"
        ),
        "7\n"
    );
}

/// A `USING` window *enclosing* the form is part of the walked chain: a surfaced value member read
/// in the block is an ordinary per-call hit and is captured.
#[test]
fn a_surfaced_member_is_captured_through_an_enclosing_window() {
    assert_eq!(
        output(
            "LET mk = (FN :{n :Number} -> Any = (\
                 ((MODULE inner = (LET v = 2)))\
                 (USING inner SCOPE (CLOSE ((LET g = (FN :{} -> Number = (v))) (g))))))\n\
             LET esc = (mk {n = 1})\n\
             PRINT (esc {})\n"
        ),
        "2\n"
    );
}

// ---------- dynamic names in the inference domain ----------

/// Assert `probe` raises the inferred-close conflict for `form`, and that the message names the
/// offending surface.
fn assert_conflict(probe: &str, form: DynamicNameForm) {
    let error = error_of("LET a = (7)\nMODULE m = (LET v = 1)\n", probe);
    match &error.kind {
        KErrorKind::DynamicNamesUnderInferredClose { form: raised, .. } => {
            assert_eq!(*raised, form, "wrong dynamic form reported for {probe}");
        }
        _ => panic!("expected the inferred-close conflict for {probe}, got {error}"),
    }
    let rendered = error.to_string();
    assert!(
        rendered.contains(form.surface()) && rendered.contains("CLOSE OVER"),
        "the diagnostic must name the surface and the remedy, got {rendered}",
    );
}

/// `$(…)` resolves names dynamically, so a block containing one has no inferable capture list.
#[test]
fn an_eval_sigil_in_the_block_is_refused() {
    assert_conflict("CLOSE ($(#(a)))", DynamicNameForm::Eval);
}

/// The spelled-out head is the same form and the same refusal.
#[test]
fn a_spelled_eval_in_the_block_is_refused() {
    assert_conflict("CLOSE (EVAL #(a))", DynamicNameForm::Eval);
}

/// The ban covers the whole inference domain, not just the block's top level.
#[test]
fn an_eval_inside_a_nested_function_is_refused() {
    assert_conflict(
        "CLOSE ((LET g = (FN :{} -> Number = ($(#(a))))) (g))",
        DynamicNameForm::Eval,
    );
}

/// `USING … SCOPE` surfaces module members dynamically, so it is refused on the same ground.
#[test]
fn a_using_window_in_the_block_is_refused() {
    assert_conflict("CLOSE (USING m SCOPE (v))", DynamicNameForm::Using);
}

/// Nested just as far: a window inside a function inside the block.
#[test]
fn a_using_window_inside_a_nested_function_is_refused() {
    assert_conflict(
        "CLOSE ((LET g = (FN :{} -> Number = (USING m SCOPE (v)))) (g))",
        DynamicNameForm::Using,
    );
}

/// Ruling 3's escape hatch: a nested `CLOSE OVER` is severed, so its body is outside the inference
/// domain and both dynamic forms run there untouched.
#[test]
fn a_nested_explicit_block_admits_both_dynamic_forms() {
    assert_eq!(
        output("LET a = (7)\nLET x = (CLOSE (CLOSE OVER (a) ($(#(a)))))\nPRINT x\n"),
        "7\n"
    );
    assert_eq!(
        output(
            "MODULE m = (LET v = 1)\n\
             LET x = (CLOSE (CLOSE OVER () (USING m SCOPE (v))))\n\
             PRINT x\n"
        ),
        "1\n"
    );
}

/// A nested `CLOSE` raises its own conflict when *it* evaluates rather than propagating one
/// outward, so the outer form is free to infer over a block that merely contains one.
#[test]
fn a_nested_inferred_blocks_conflict_is_its_own() {
    assert_conflict("CLOSE (CLOSE ($(#(a))))", DynamicNameForm::Eval);
}

/// A type-channel free identifier that resolves nowhere is unbound **at the form**, the same
/// verdict the value channel gives. This is what lets the walk's type reports be trusted: every
/// surface that spells a `Type` token which is no scope name is one the walk skips.
#[test]
fn an_unresolvable_type_name_is_unbound_at_the_form() {
    let error = error_of("", "LET x = (CLOSE (FN (MAKE n :Ghost) -> Any = (n)))");
    assert!(
        matches!(&error.kind, KErrorKind::UnboundName(name) if name == "Ghost"),
        "expected `Ghost` unbound at the CLOSE, got {error}",
    );
}

/// A builtin type is eternal-homed, so it is visible through the block's own outer link and costs
/// no capture — the no-hit raise above must not catch it.
#[test]
fn a_builtin_type_name_is_not_reported_unbound() {
    assert_eq!(
        output("LET x = (CLOSE (FN (MAKE n :Number) -> Any = (n)))\nPRINT (x {n = 4})\n"),
        "4\n"
    );
}

/// `MATCH … OVER` arm heads are member names, not scope names, so the walk skips them: the block
/// infers its scrutinee and its union operand and nothing else.
#[test]
fn match_over_arm_heads_are_not_inferred() {
    assert_eq!(
        output(
            "UNION Maybe = (Some :Number None :Null)\n\
             LET m = (Maybe.Some 3)\n\
             LET x = (CLOSE (MATCH (m) OVER Maybe -> :Number WITH (Some -> (it) None -> (0))))\n\
             PRINT x\n"
        ),
        "3\n"
    );
}

/// `TRY` arm heads are error kinds, read the same way — a name the scope never binds.
#[test]
fn try_arm_heads_are_not_inferred() {
    assert_eq!(
        output(
            "UNION Maybe = (Some :Number None :Null)\n\
             LET m = (Maybe.Some 1)\n\
             LET x = (CLOSE (TRY (MATCH (m) OVER Maybe -> :Number WITH (None -> (0)))\
                 -> :Str WITH (ShapeError -> (\"caught\"))))\n\
             PRINT x\n"
        ),
        "caught\n"
    );
}

/// `Maybe.Some` contributes the lhs only: ATTR's field slot is a member label the union resolves,
/// never a name the scope does.
#[test]
fn a_member_projection_infers_its_union_only() {
    assert_eq!(
        output(
            "LET mk = (FN :{n :Number} -> Any = (\
                 (UNION Maybe = (Some :Number None :Null))\
                 (CLOSE (Maybe.Some n))))\n\
             LET esc = (mk {n = 5})\n\
             PRINT esc\n"
        ),
        "Some(5)\n"
    );
}
