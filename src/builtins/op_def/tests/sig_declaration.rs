//! The bodyless `OP` / `UNARY OP` heads: a SIG body's operator members. The head parses through
//! the definition form's own path and derives its shapes through the same `operator_shape`, so
//! these tests pin what the declaration *records* — the bucket keys, the `(params) -> ret` types
//! and the chaining record in the signature's stored schema — plus the guards that keep
//! declaration and definition in their own bodies.

use crate::builtins::test_support::{TestRun, binder_token, key_keyword, lookup_type};
use crate::machine::KErrorKind;
use crate::machine::model::{
    DeclaredGroup, KType, KeyElement, KeywordSymbol, Record, ReductionMode, SigSchema, TypeNode,
    UntypedKey,
};
use crate::machine::{program_storage, run_root_storage};

/// The stored schema of the signature `name` binds in `scope`.
fn sig_schema(
    scope: &crate::machine::Scope<'_>,
    types: &crate::machine::model::TypeRegistry,
    name: &str,
) -> SigSchema {
    let handle = lookup_type(scope, name).unwrap_or_else(|| panic!("{name} must bind a type"));
    match types.node(handle) {
        TypeNode::Signature { schema, .. } => schema,
        _ => panic!("{name} must bind a Signature KType"),
    }
}

/// A bucket key spelled out: `_` is an argument slot, anything else a fixed token.
fn key(spelling: &[&str]) -> UntypedKey {
    spelling
        .iter()
        .map(|part| match *part {
            "_" => KeyElement::Slot,
            token => key_keyword(token),
        })
        .collect()
}

/// The sole overload declared under `spelling`.
fn overload(schema: &SigSchema, spelling: &[&str]) -> KType {
    let overloads = schema
        .keyworded
        .get(&key(spelling))
        .unwrap_or_else(|| panic!("the signature declares the key {spelling:?}"));
    assert_eq!(overloads.len(), 1, "one head, one overload");
    overloads[0]
}

/// A record over the named glyphs at `mode`, spelled as the schema stores one.
fn record(members: &[&str], mode: ReductionMode) -> DeclaredGroup {
    let mut members: Vec<KeywordSymbol> = members
        .iter()
        .map(|text| KeywordSymbol::of(text).expect("a fixture glyph is keyword-class"))
        .collect();
    members.sort_unstable();
    DeclaredGroup { members, mode }
}

/// A bare head declares both halves of what the definition writes: the binary bucket under the
/// key a two-operand use computes, at the machine-fixed operand binders, and a singleton
/// fold-left record.
#[test]
fn a_bare_head_records_its_bucket_and_a_singleton_fold_left_record() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Addable = ((LET Carrier = Number) (OP #(⊕) OVER Carrier))");
    let schema = sig_schema(scope, test_run.types(), "Addable");
    let expected = test_run.types().function_type(
        Record::from_pairs([
            (binder_token("left"), KType::NUMBER),
            (binder_token("right"), KType::NUMBER),
        ]),
        KType::NUMBER,
    );
    assert_eq!(overload(&schema, &["_", "⊕", "_"]), expected);
    assert_eq!(
        schema.operators,
        vec![record(&["⊕"], ReductionMode::FoldLeft)]
    );
}

/// One head names the unary triple: both bucket keys a use site can compute — the list form a
/// reduced run takes and the binary bridge a two-operand run takes — and the `Unary` record.
#[test]
fn a_unary_head_records_the_whole_triple() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Coll = ((LET Carrier = Number) (UNARY OP #(~) OVER Carrier -> Str))");
    let schema = sig_schema(scope, test_run.types(), "Coll");
    let types = test_run.types();
    assert_eq!(
        overload(&schema, &["~", "_"]),
        types.function_type(
            Record::from_pairs([(binder_token("operands"), types.list(KType::NUMBER))]),
            KType::STR,
        ),
    );
    assert_eq!(
        overload(&schema, &["_", "~", "_"]),
        types.function_type(
            Record::from_pairs([
                (binder_token("left"), KType::NUMBER),
                (binder_token("right"), KType::NUMBER),
            ]),
            KType::STR,
        ),
    );
    assert_eq!(schema.operators, vec![record(&["~"], ReductionMode::Unary)]);
}

/// A head's type slots resolve against the SIG body's own scope, so a sibling type member types
/// the declared operand — the same read a `VAL` slot's type takes.
#[test]
fn a_head_resolves_a_sibling_type_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Addable = ((TYPE Carrier) (VAL zero :Carrier) (OP #(⊕) OVER Carrier))");
    let schema = sig_schema(scope, test_run.types(), "Addable");
    let carrier = schema.value_slots
        [&crate::builtins::test_support::value_name("zero", test_run.registries())];
    let expected = test_run.types().function_type(
        Record::from_pairs([
            (binder_token("left"), carrier),
            (binder_token("right"), carrier),
        ]),
        carrier,
    );
    assert_eq!(
        overload(&schema, &["_", "⊕", "_"]),
        expected,
        "the head's operand is the signature's own abstract member, as a VAL slot's type is",
    );
}

/// A type member named *after* the head is no more visible to it than to a `VAL` slot: a SIG body
/// resolves its type references in order, and the head takes the same pointed diagnostic its
/// sibling declarators take, naming its own slot.
#[test]
fn a_forward_type_reference_is_refused_like_a_val_slots() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let error = test_run.run_one_err(
        test_run.parse_one("SIG Bad = ((OP #(⊕) OVER Carrier) (LET Carrier = Number))"),
    );
    assert!(
        error
            .to_string()
            .contains("`Carrier` is used in OP operand type before being declared"),
        "got {error}",
    );
}

/// Both container-operand spellings reach the same declared type: the bare `(LIST OF …)` is
/// flipped to the sigiled form by the head's own type-slot mask, as the definition's is.
#[test]
fn both_container_operand_spellings_declare_one_type() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Bare = ((OP #(⊕) OVER (LIST OF Number)))\n\
         SIG Sigiled = ((OP #(⊕) OVER :(LIST OF Number)))",
    );
    let types = test_run.types();
    let bare = sig_schema(scope, types, "Bare");
    let sigiled = sig_schema(scope, types, "Sigiled");
    assert_eq!(
        overload(&bare, &["_", "⊕", "_"]),
        overload(&sigiled, &["_", "⊕", "_"]),
    );
}

/// Two heads over one symbol at different operand types are two overloads and **one** record: the
/// operator chains one way however many shapes it takes.
#[test]
fn two_heads_over_one_symbol_are_two_overloads_and_one_record() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Addable = ((OP #(⊕) OVER Number) (OP #(⊕) OVER Str))");
    let schema = sig_schema(scope, test_run.types(), "Addable");
    assert_eq!(schema.keyworded[&key(&["_", "⊕", "_"])].len(), 2);
    assert_eq!(
        schema.operators,
        vec![record(&["⊕"], ReductionMode::FoldLeft)]
    );
}

/// A signature names itself by its content, and an operator member renders as the head declaring
/// it rather than as the FN head its bucket key would otherwise spell.
#[test]
fn an_operator_member_renders_as_its_own_head() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Addable = ((LET Carrier = Number) (OP #(⊕) OVER Carrier))");
    let handle = lookup_type(scope, "Addable").expect("Addable binds");
    assert_eq!(
        handle.name(test_run.registries()),
        "SIG (Carrier: Number, OP #(⊕) OVER Number)",
    );
}

/// A `GROUP`'s heads render individually and the record follows them, so the rendering spells
/// every declarator the signature was written with.
#[test]
fn a_declared_group_renders_after_its_members() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Ring = ((LET Carrier = Number) \
         (GROUP FOLD RIGHT = ((OP #(⊕) OVER Carrier) (OP #(⊖) OVER Carrier))))",
    );
    let handle = lookup_type(scope, "Ring").expect("Ring binds");
    let rendered = handle.name(test_run.registries());
    assert!(
        rendered.contains("OP #(⊕) OVER Number") && rendered.contains("OP #(⊖) OVER Number"),
        "both members render as their own heads, got {rendered}",
    );
    assert!(
        rendered.contains("GROUP FOLD RIGHT {"),
        "the record renders after the members, got {rendered}",
    );
}

/// The unary triple renders as the one head that declares it: naming its two keys separately
/// would spell a surface no signature can be written with.
#[test]
fn a_unary_triple_renders_as_one_head() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Coll = ((UNARY OP #(~) OVER Number -> Str))");
    let handle = lookup_type(scope, "Coll").expect("Coll binds");
    assert_eq!(
        handle.name(test_run.registries()),
        "SIG (UNARY OP #(~) OVER Number -> Str)",
    );
}

/// An FN head over an operator key claims the bucket and no chaining, so it keeps the FN-head
/// rendering — the schema's operator channel is what makes a member an operator member.
#[test]
fn an_fn_head_over_an_operator_key_stays_an_fn_head() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Addable = ((FN (left :Number ⊕ right :Number) -> Number))");
    let handle = lookup_type(scope, "Addable").expect("Addable binds");
    assert_eq!(
        handle.name(test_run.registries()),
        "SIG ((left :Number ⊕ right :Number) -> Number)",
    );
    assert!(
        sig_schema(scope, test_run.types(), "Addable")
            .operators
            .is_empty()
    );
}

// ---------- the refusals ----------

/// Every refusal below is a `ShapeError` whose message names the spelling that would have been
/// right; the assertions pin the pointer, not the whole sentence.
fn refusal(source: &str) -> String {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let error = test_run.run_one_err(test_run.parse_one(source));
    match &error.kind {
        KErrorKind::ShapeError(message) => message.clone(),
        _ => panic!("expected a shape error, got {error}"),
    }
}

#[test]
fn a_head_outside_a_sig_body_points_at_the_definition() {
    let message = refusal("OP #(⊕) OVER Str");
    assert!(
        message.contains("only valid inside a SIG body")
            && message.contains("OP #(⊕) OVER <Operand> = (<body>)"),
        "got {message}",
    );
}

#[test]
fn a_definition_inside_a_sig_body_points_at_the_head() {
    let message = refusal("SIG Bad = ((OP #(⊕) OVER Number = ((left))))");
    assert!(
        message.contains("declared rather than defined")
            && message.contains("OP #(⊕) OVER <Operand>"),
        "got {message}",
    );
}

/// A fold member's result is its operand type, so an explicit `-> Result` only reads under a
/// pairwise mode — the same rule the definition applies, stated against the SIG group.
#[test]
fn a_heterogeneous_head_outside_a_pairwise_group_is_refused() {
    let message = refusal("SIG Bad = ((OP #(≺) OVER Number -> Bool))");
    assert!(
        message.contains("only a PAIRWISE group's members may do"),
        "got {message}",
    );
}

#[test]
fn a_unary_head_inside_a_group_is_refused() {
    let message =
        refusal("SIG Bad = ((GROUP FOLD LEFT = ((UNARY OP #(~) OVER Number -> Number))))");
    assert!(message.contains("chains with nothing"), "got {message}");
}

/// The result-less unary head is a reserved key: nothing registers there, so the shape reaches
/// the dispatch-miss diagnosis and takes its pointed message.
#[test]
fn a_result_less_unary_head_names_its_missing_result() {
    let message = refusal("SIG Bad = ((UNARY OP #(~) OVER Number))");
    assert!(
        message.contains("must declare its result type")
            && message.contains("UNARY OP #(~) OVER <Operand> -> <Result>"),
        "got {message}",
    );
}

/// One signature declares one chaining mode per operator, exactly as one scope does.
#[test]
fn a_symbol_in_two_records_is_a_chaining_conflict() {
    let message =
        refusal("SIG Bad = ((OP #(⊕) OVER Bool) (GROUP FOLD RIGHT = ((OP #(⊕) OVER Number))))");
    assert!(
        message.contains("one signature declares one chaining mode per operator"),
        "got {message}",
    );
}
