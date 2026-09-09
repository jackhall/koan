//! The bodyless `OP` / `UNARY OP` heads: a SIG body's operator members. The head parses through
//! the definition form's own path and derives its shapes through the same `operator_shape`, so
//! these tests pin what the declaration *records* — the bucket keys, the `(params) -> ret` types
//! and the chaining record in the signature's stored schema — plus the guards that keep
//! declaration and definition in their own bodies.

use crate::builtins::test_support::{TestRun, key_keyword, lookup_type};
use crate::machine::KErrorKind;
use crate::machine::model::{
    DeclaredGroup, KType, KeyElement, KeywordSymbol, ReductionMode, SigSchema, TypeNode, UntypedKey,
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

/// The schema members keying `spelling`, read back off each member's own shape.
fn members_keyed(
    schema: &SigSchema,
    types: &crate::machine::model::TypeRegistry,
    spelling: &[&str],
) -> Vec<KType> {
    let wanted = key(spelling);
    schema
        .keyworded
        .iter()
        .filter(|member| crate::machine::model::shape_key_is(**member, &wanted, types))
        .copied()
        .collect()
}

/// The sole member declared under `spelling`.
fn overload(
    schema: &SigSchema,
    types: &crate::machine::model::TypeRegistry,
    spelling: &[&str],
) -> KType {
    let members = members_keyed(schema, types, spelling);
    assert_eq!(members.len(), 1, "one head, one member");
    members[0]
}

/// An operator member's shape: the binary form's two operand positions around the glyph, or the
/// list form's single run position after it.
fn operator_shape(
    types: &crate::machine::model::TypeRegistry,
    spelling: &[&str],
    slots: &[KType],
    ret: KType,
) -> KType {
    use crate::machine::model::DispatchTokenElement;
    let mut next = slots.iter();
    let elements: Vec<DispatchTokenElement> = spelling
        .iter()
        .map(|part| match *part {
            "_" => DispatchTokenElement::Slot(*next.next().expect("one type per slot position")),
            token => DispatchTokenElement::Keyword(
                crate::builtins::test_support::key_keyword_symbol(token),
            ),
        })
        .collect();
    types.shape_type(&[], &elements, ret)
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
    let expected = operator_shape(
        test_run.types(),
        &["_", "⊕", "_"],
        &[KType::NUMBER, KType::NUMBER],
        KType::NUMBER,
    );
    assert_eq!(
        overload(&schema, test_run.types(), &["_", "⊕", "_"]),
        expected
    );
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
        overload(&schema, types, &["~", "_"]),
        operator_shape(types, &["~", "_"], &[types.list(KType::NUMBER)], KType::STR),
    );
    assert_eq!(
        overload(&schema, types, &["_", "~", "_"]),
        operator_shape(
            types,
            &["_", "~", "_"],
            &[KType::NUMBER, KType::NUMBER],
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
    let expected = operator_shape(
        test_run.types(),
        &["_", "⊕", "_"],
        &[carrier, carrier],
        carrier,
    );
    assert_eq!(
        overload(&schema, test_run.types(), &["_", "⊕", "_"]),
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
        overload(&bare, types, &["_", "⊕", "_"]),
        overload(&sigiled, types, &["_", "⊕", "_"]),
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
    assert_eq!(
        members_keyed(&schema, test_run.types(), &["_", "⊕", "_"]).len(),
        2
    );
    assert_eq!(
        schema.operators,
        vec![record(&["⊕"], ReductionMode::FoldLeft)]
    );
}

/// A signature names itself by its content, and an operator member renders as the head declaring
/// it rather than as the `EXPR` head its bucket key would otherwise spell.
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

/// A record whose mode a bare head does **not** imply renders its own `GROUP` head, singleton or
/// not. A bare head declares a fold-*left* singleton specifically, so a one-member
/// `GROUP FOLD RIGHT` is a different interface and must not print the same name — otherwise a
/// mode-mismatch diagnostic would name a signature spelled identically to the module it rejects.
#[test]
fn a_singleton_record_at_an_unimplied_mode_renders_its_group_head() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Alpha = ((OP #(⊕) OVER Number))\n\
         SIG Beta = ((GROUP FOLD RIGHT = ((OP #(⊕) OVER Number))))",
    );
    let alpha = lookup_type(scope, "Alpha").expect("Alpha binds");
    let beta = lookup_type(scope, "Beta").expect("Beta binds");
    assert_eq!(
        alpha.name(test_run.registries()),
        "SIG (OP #(⊕) OVER Number)",
        "a bare head's own singleton is fold-left, which the head already says",
    );
    assert_eq!(
        beta.name(test_run.registries()),
        "SIG (OP #(⊕) OVER Number, GROUP FOLD RIGHT {⊕})",
        "the fold-right singleton is a claim only the GROUP head makes",
    );
    assert_ne!(
        alpha.name(test_run.registries()),
        beta.name(test_run.registries()),
        "two signatures that differ must not share a name",
    );
}

/// The same rule for a one-member pairwise group: its mode carries a combiner and a direction,
/// none of which its member's head spells.
#[test]
fn a_singleton_pairwise_record_renders_its_group_head() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Cmp = ((OP #(BOTH) OVER Bool) \
         (GROUP PAIRWISE FOLD #(BOTH) LEFT = ((OP #(≺) OVER Number -> Bool))))",
    );
    let rendered = lookup_type(scope, "Cmp")
        .expect("Cmp binds")
        .name(test_run.registries());
    assert!(
        rendered.contains("GROUP PAIRWISE FOLD #(BOTH) LEFT {≺}"),
        "got {rendered}",
    );
}

/// A second overload in a declared operator's own bucket renders as an operator head over its
/// own operand type. A shape carries no argument names, so nothing tells a member declared by an
/// `FN` head apart from one declared by an `OP` head: both spell the same key at the same slots,
/// and the symbol's chaining record covers the whole bucket.
#[test]
fn a_second_overload_in_a_declared_operators_bucket_renders_as_an_operator_head() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Mixed = ((OP #(⊕) OVER Number) (EXPR (x :Str ⊕ y :Str) -> Str))");
    let rendered = lookup_type(scope, "Mixed")
        .expect("Mixed binds")
        .name(test_run.registries());
    assert!(
        rendered.contains("OP #(⊕) OVER Number"),
        "the operator member renders as its head, got {rendered}",
    );
    assert!(
        rendered.contains("OP #(⊕) OVER Str"),
        "the second overload renders over its own operand type, got {rendered}",
    );
}

/// An `EXPR` head over an operator key claims the bucket and no chaining, so it keeps the head
/// rendering — the schema's operator channel is what makes a member an operator member.
#[test]
fn an_fn_head_over_an_operator_key_stays_an_fn_head() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Addable = ((EXPR (left :Number ⊕ right :Number) -> Number))");
    let handle = lookup_type(scope, "Addable").expect("Addable binds");
    assert_eq!(
        handle.name(test_run.registries()),
        "SIG ((_ :Number ⊕ _ :Number) -> Number)",
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
