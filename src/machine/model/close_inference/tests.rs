//! Inference-walk tests: the spec⟺registration consistency pin (every recognized form names a live
//! builtin bucket) and the free-identifier walk itself, read off parsed blocks.

use super::{DynamicNameForm, FORM_SPECS, FormRule, infer_close_captures};
use crate::builtins::test_support::TestRun;
use crate::machine::core::{ProgramStorage, program_storage, run_root_storage};
use crate::machine::model::key_spec::{KeyElementSpec, key_matches_untyped, render_key};
use crate::machine::model::{UntypedKey, render_label};

// ---------- spec ⟺ registration ----------

/// Every bucket key a builtin is registered under, anywhere on a seeded chain.
fn live_keys() -> Vec<UntypedKey> {
    let program = program_storage();
    let storage = run_root_storage();
    let run = TestRun::silent(&program, &storage);
    let mut keys = Vec::new();
    for scope in run.scope.ancestors() {
        for (key, _) in scope.bindings().functions().iter() {
            keys.push(key.to_vec());
        }
    }
    keys
}

/// Recognition is only sound because a matched key can resolve to nothing but that builtin's
/// overloads, so an entry naming no live bucket — a builtin renamed, re-shaped, or dropped — is a
/// rule the walk would apply to user code.
#[test]
fn every_form_spec_names_a_live_builtin_bucket() {
    let live = live_keys();
    for spec in FORM_SPECS {
        assert!(
            live.iter().any(|key| key_matches_untyped(spec.key, key)),
            "form spec {:?} names no live builtin bucket",
            render_key(spec.key)
        );
    }
}

/// One rule per key: two entries matching the same run would make the walk's reading depend on
/// table order.
#[test]
fn no_two_form_specs_share_a_key() {
    for (index, spec) in FORM_SPECS.iter().enumerate() {
        for other in &FORM_SPECS[index + 1..] {
            assert!(
                !(spec.key.len() == other.key.len()
                    && render_key(spec.key) == render_key(other.key)),
                "two form specs share the key {:?}",
                render_key(spec.key)
            );
        }
    }
}

/// Every slot a rule claims is a slot position of its own key. A rule pointing past its run, or at
/// one of its keywords, would read the wrong part.
#[test]
fn every_claimed_slot_names_a_slot_position() {
    for spec in FORM_SPECS {
        let claimed: Vec<usize> = match spec.rule {
            FormRule::Signature { signature, body } => {
                std::iter::once(signature).chain(body).collect()
            }
            FormRule::Operator { body, .. } => vec![body],
            FormRule::Arms { arms } => vec![arms],
            FormRule::ModuleBody { body } => vec![body],
            FormRule::Attribute { field } => vec![field],
            FormRule::Projection { fields } => vec![fields],
            FormRule::ExplicitClose { captures, body } => vec![captures, body],
            FormRule::InferredClose { body } => vec![body],
            FormRule::Dynamic(_) => vec![],
        };
        for index in claimed {
            assert!(
                index < spec.key.len(),
                "form spec {:?} claims slot {index} past its run",
                render_key(spec.key)
            );
            assert!(
                matches!(spec.key[index], KeyElementSpec::Slot),
                "form spec {:?} claims its keyword position {index}",
                render_key(spec.key)
            );
        }
    }
}

// ---------- the walk ----------

/// What `block` — the source a `CLOSE` body slot would hold — infers: the free value names, the
/// free type names, and the dynamic-name conflict if it hit one.
struct Inferred {
    values: Vec<String>,
    types: Vec<String>,
    conflict: Option<DynamicNameForm>,
}

fn infer(block: &str) -> Inferred {
    let program: ProgramStorage = program_storage();
    let storage = run_root_storage();
    let run = TestRun::silent(&program, &storage);
    let expression = run.parse_one(block);
    let registries = run.registries();
    let inference = infer_close_captures(&expression, run.brand().allocator(), registries);
    Inferred {
        values: inference
            .values
            .iter()
            .map(|name| render_label(name.symbol(), registries))
            .collect(),
        types: inference
            .types
            .iter()
            .map(|name| render_label(name.symbol(), registries))
            .collect(),
        conflict: inference.conflict.map(|conflict| conflict.form),
    }
}

/// The base case: every identifier a flat expression spells is free.
#[test]
fn a_flat_expression_frees_every_name_it_spells() {
    let inferred = infer("(x + y)");
    assert_eq!(inferred.values, ["x", "y"]);
    assert!(inferred.types.is_empty());
}

/// A name is reported once however often the block spells it.
#[test]
fn a_repeated_name_is_reported_once() {
    assert_eq!(infer("((x) (x + x))").values, ["x"]);
}

/// A block's own binder shadows the outer name — for the statements that can see it.
#[test]
fn a_local_binding_is_not_free() {
    assert_eq!(infer("((LET a = 1) (a + b))").values, ["b"]);
}

/// The positional rule: a binding at statement `j` is visible only from statement `j + 1` on, so a
/// read *before* the declaration resolves outward and is captured.
#[test]
fn a_read_before_its_declaration_is_free() {
    assert_eq!(infer("((PRINT x) (LET x = 5) (PRINT x))").values, ["x"]);
}

/// The cutoff is strict, so a binder does not bind its own right-hand side.
#[test]
fn a_binders_own_right_hand_side_resolves_outward() {
    assert_eq!(infer("((LET x = (x + 1)) (x))").values, ["x"]);
}

/// A nested scope restarts the positional count, and its own binders do not leak outward.
#[test]
fn a_nested_block_binds_only_within_itself() {
    assert_eq!(infer("(((LET a = 1) (a)) (a))").values, ["a"]);
}

// ---------- declaration forms ----------

/// An `FN` parameter binds its body; the annotations and the return type are uses in the enclosing
/// scope.
#[test]
fn fn_parameters_bind_the_body_and_annotations_are_uses() {
    let inferred = infer("((LET f = FN (GO a :Number) -> Number = (a + b)) (f))");
    assert_eq!(inferred.values, ["b"]);
    assert_eq!(inferred.types, ["Number"]);
}

/// An annotation is walked in the *enclosing* scope, so a value name it spells is free.
#[test]
fn a_signature_annotation_frees_the_names_it_spells() {
    let inferred = infer("(FN (GO a :(TYPE OF t)) -> Number = (a))");
    assert_eq!(inferred.values, ["t"]);
    assert_eq!(inferred.types, ["Number"]);
}

/// The anonymous `FN :{…}` form's record schema declares its parameters the same way.
#[test]
fn an_anonymous_fn_schema_declares_its_parameters() {
    let inferred = infer("(FN :{a :Number} -> Number = (a + b))");
    assert_eq!(inferred.values, ["b"]);
    assert_eq!(inferred.types, ["Number"]);
}

/// The type-language `FN <signature> -> <return>` declarator has no body to seed, and its parameter
/// names are still labels.
#[test]
fn an_fn_declarator_frees_only_its_annotations() {
    let inferred = infer("(FN (SCALE s :Shape f :Number) -> Shape)");
    assert!(inferred.values.is_empty());
    assert_eq!(inferred.types, ["Shape", "Number"]);
}

/// An `OP` body's operands are named by the surface, not spelled in it, so they bind.
#[test]
fn operator_operands_bind_the_body() {
    let inferred = infer("(OP #(<+>) OVER Number = (left + right + c))");
    assert_eq!(inferred.values, ["c"]);
    assert_eq!(inferred.types, ["Number"]);
}

/// The unary surface binds one whole-run operand instead.
#[test]
fn a_unary_operator_binds_its_operand_run() {
    let inferred = infer("(UNARY OP #(NEG) OVER Number -> Number = (operands + c))");
    assert_eq!(inferred.values, ["c"]);
}

/// A `VAL` declaration names a slot, not a value it reads.
#[test]
fn a_val_declaration_names_a_label() {
    let inferred = infer("(SIG Shape = ((VAL area :Number)))");
    assert!(inferred.values.is_empty());
    assert_eq!(inferred.types, ["Number"]);
}

// ---------- arms ----------

/// Each arm body runs with the scrutinee bound, and the heads are type-channel uses.
#[test]
fn match_arms_bind_the_scrutinee_binder() {
    let inferred = infer("(MATCH v -> Number WITH (Some -> (it) None -> (q)))");
    assert_eq!(inferred.values, ["v", "q"]);
    assert_eq!(inferred.types, ["Number", "Some", "None"]);
}

/// `it` outside any arm is an ordinary free name.
#[test]
fn the_arm_binder_outside_an_arm_is_free() {
    assert_eq!(infer("(it + 1)").values, ["it"]);
}

// ---------- label positions ----------

/// `m.x` reads `m`; `x` is a member label the record resolves, not a name the scope does.
#[test]
fn an_attribute_field_is_a_label() {
    assert_eq!(infer("(m.x)").values, ["m"]);
}

/// A projection's field list is labels only.
#[test]
fn a_projections_field_list_is_labels() {
    assert_eq!(infer("((a b) FROM r)").values, ["r"]);
}

/// A record literal's keys are the record's own field names.
#[test]
fn record_literal_keys_are_labels() {
    assert_eq!(infer("({x = 1, y = q})").values, ["q"]);
}

// ---------- the type-channel windows ----------

/// A nominal declaration's own name is visible inside its representation, so a self-recursive type
/// infers nothing — even though the positional rule alone would call the name free.
#[test]
fn a_self_recursive_nominal_declares_its_own_name() {
    let inferred = infer("((NEWTYPE Tree = :{left :Tree}) (1))");
    assert!(inferred.types.is_empty(), "{:?}", inferred.types);
    assert!(inferred.values.is_empty());
}

/// A standalone `UNION`'s variant tags are declared names too.
#[test]
fn a_unions_variant_tags_are_declared_names() {
    let inferred = infer("((UNION Shape = (Circle :Number, Square :Number)) (1))");
    assert_eq!(inferred.types, ["Number"]);
}

/// A module body pre-announces its type declarations, so a mutually recursive pair infers neither
/// name — in either source order.
#[test]
fn a_module_body_announces_its_declarations_body_wide() {
    for body in [
        "((NEWTYPE Aa = :{b :Bb}) (NEWTYPE Bb = :{a :Aa}))",
        "((NEWTYPE Bb = :{a :Aa}) (NEWTYPE Aa = :{b :Bb}))",
    ] {
        let inferred = infer(&format!("((MODULE pair = {body}) (pair))"));
        assert!(inferred.types.is_empty(), "{body}: {:?}", inferred.types);
        assert!(inferred.values.is_empty(), "{body}: {:?}", inferred.values);
    }
}

// ---------- frontiers ----------

/// A nested `CLOSE OVER` names its captures against this chain and severs its body.
#[test]
fn a_nested_explicit_close_contributes_its_capture_list_only() {
    assert_eq!(infer("(CLOSE OVER (a) (b))").values, ["a"]);
}

/// A nested `CLOSE` contributes the free set of *its* block.
#[test]
fn a_nested_inferred_close_contributes_its_own_free_set() {
    assert_eq!(infer("(CLOSE ((LET a = 1) (a + b)))").values, ["b"]);
}

/// A quote in an eager position is data — nothing in it resolves.
#[test]
fn a_quote_in_an_eager_position_is_data() {
    let inferred = infer("(#(x + 1))");
    assert!(inferred.values.is_empty());
}

// ---------- dynamic-name conflicts ----------

/// `$(…)` resolves its names at evaluation, so the block has no inferable capture list.
#[test]
fn an_eval_form_is_a_conflict() {
    assert_eq!(infer("($(e))").conflict, Some(DynamicNameForm::Eval));
}

/// The spelled-out `EVAL` is the same form and reports the same conflict.
#[test]
fn the_spelled_eval_form_is_the_same_conflict() {
    assert_eq!(infer("(EVAL e)").conflict, Some(DynamicNameForm::Eval));
}

/// The domain reaches into nested bodies: a conflict anywhere the block would evaluate counts.
#[test]
fn a_conflict_inside_a_nested_body_is_found() {
    assert_eq!(
        infer("(FN :{} -> Any = ($(e)))").conflict,
        Some(DynamicNameForm::Eval)
    );
}

/// `USING … SCOPE` surfaces module members dynamically.
#[test]
fn a_using_window_is_a_conflict() {
    assert_eq!(
        infer("(USING m SCOPE (x))").conflict,
        Some(DynamicNameForm::Using)
    );
}

/// A nested `CLOSE OVER` is severed, so what its body spells is its own business — the ruled escape
/// hatch for both dynamic forms.
#[test]
fn a_severed_body_does_not_raise_the_enclosing_forms_conflict() {
    assert!(infer("(CLOSE OVER (a) ($(e)))").conflict.is_none());
}

/// A nested `CLOSE` raises its own conflict when it evaluates, so it does not propagate one here.
#[test]
fn a_nested_inferred_close_keeps_its_own_conflict() {
    assert!(infer("(CLOSE ($(e)))").conflict.is_none());
}
