//! The `EXPR` surface: the keyworded callable whose arguments are positional and reached by
//! dispatch, against the lambda `FN :{…}` reached by name. Three spellings share one head parse —
//! the definition, the combined statement, and the bodyless head whose carrier is the head's
//! expression shape as a type value — so these tests pin what each one installs and where the
//! bodyless head additionally declares.

use crate::builtins::test_support::{TestRun, fn_is_registered, lookup_type};
use crate::machine::KErrorKind;
use crate::machine::model::{DispatchTokenElement, KObject, KType, SigSchema, TypeNode};
use crate::memory::{program_storage, run_root_storage};

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

/// A definition registers under its head's bucket key and is reached by dispatch.
#[test]
fn a_definition_registers_its_head_as_a_dispatch_bucket() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("EXPR (DOUBLE n :Number) -> Number = (n * 2)");

    assert!(fn_is_registered(scope, "DOUBLE"));
    let result = test_run.run_one(test_run.parse_one("DOUBLE 21"));
    assert!(matches!(result, KObject::Number(n) if *n == 42.0));
}

/// The combined statement spells `FN EXPR`: the value channel it binds is a lambda-typed name and
/// the bucket it registers is the shape, so one statement installs both.
#[test]
fn the_combined_statement_installs_name_and_bucket() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("LET tripler = FN EXPR (TRIPLE n :Number) -> Number = (n * 3)");

    assert!(fn_is_registered(scope, "TRIPLE"));
    let by_keyword = test_run.run_one(test_run.parse_one("TRIPLE 5"));
    assert!(matches!(by_keyword, KObject::Number(n) if *n == 15.0));
    let by_name = test_run.run_one(test_run.parse_one("tripler {n = 5}"));
    assert!(
        matches!(by_name, KObject::Number(n) if *n == 15.0),
        "the bound name reaches the same function through the call-by-name lane"
    );
}

/// Outside a SIG body the bodyless head is a type value and nothing else: it binds an alias and
/// declares into no signature. Its slots may be written `_`, since a shape's slots are positional.
#[test]
fn a_bodyless_head_outside_a_sig_body_binds_a_shape_alias() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("LET Doubling = :(EXPR (DOUBLE _ :Number) -> Number)");

    let alias = lookup_type(scope, "Doubling").expect("the alias binds a type");
    assert!(
        matches!(
            test_run.types().node(alias),
            TypeNode::ExpressionShape { .. }
        ),
        "the head's carrier is its expression shape, got {}",
        alias.display_name(test_run.registries()),
    );
    assert_eq!(
        alias.display_name(test_run.registries()).to_string(),
        ":(EXPR (DOUBLE _ :Number) -> Number)",
    );
}

/// Bare inside a SIG body the same head declares a keyworded member — and the shape it records is
/// the one an alias spells outside, so a declaration and the type value are one derivation.
#[test]
fn a_bodyless_head_in_a_sig_body_declares_a_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Doubler = ((EXPR (DOUBLE _ :Number) -> Number))\n\
         LET Doubling = :(EXPR (DOUBLE other :Number) -> Number)",
    );

    let schema = sig_schema(scope, test_run.types(), "Doubler");
    let alias = lookup_type(scope, "Doubling").expect("the alias binds a type");
    assert_eq!(
        schema.keyworded.as_slice(),
        [alias],
        "the declared member is the head's shape, which drops the slot names",
    );
}

/// Under the type sigil the head is a type value even inside a SIG body: `(VAL f :(EXPR …))`
/// declares a *value* slot typed by the shape, not a keyworded member.
#[test]
fn a_shape_under_the_type_sigil_declares_no_keyworded_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Holder = ((VAL f :(EXPR (DOUBLE _ :Number) -> Number)))");

    let schema = sig_schema(scope, test_run.types(), "Holder");
    assert!(
        schema.keyworded.is_empty(),
        "the shape typed a value slot rather than declaring a bucket member",
    );
    assert_eq!(schema.value_slots.len(), 1);
}

/// A module's definition satisfies a signature's declaration through the shape they share, and the
/// member stays callable through the opaque view the ascription mints.
#[test]
fn a_definition_satisfies_a_declaration_of_the_same_shape() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Doubler = ((EXPR (DOUBLE _ :Number) -> Number))\n\
         MODULE m = ((EXPR (DOUBLE n :Number) -> Number = (n * 2)))\n\
         LET view = (m :| Doubler)",
    );

    let result = test_run.run_one(test_run.parse_one("USING view SCOPE (DOUBLE 4)"));
    assert!(matches!(result, KObject::Number(n) if *n == 8.0));
}

/// Two heads differing only in what they name their slots project one shape: the type is the
/// keywords, the slot types and the return, and nothing else.
#[test]
fn heads_differing_only_in_slot_names_are_one_shape() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "LET Named = :(EXPR (PICK first :Number OF second :Str) -> Number)\n\
         LET Unnamed = :(EXPR (PICK _ :Number OF _ :Str) -> Number)",
    );

    let named: KType = lookup_type(scope, "Named").expect("Named binds a type");
    let unnamed: KType = lookup_type(scope, "Unnamed").expect("Unnamed binds a type");
    assert_eq!(named, unnamed);
}

/// `_` is a slot only where the slots are positional. A definition binds its arguments by name in
/// the body, so a wildcard there names nothing the body could read.
#[test]
fn a_wildcard_slot_is_refused_in_a_definition() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);

    let error = test_run.run_one_err(test_run.parse_one("EXPR (DOUBLE _ :Number) -> Number = (2)"));
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(message)
            if message.contains("`_` names no parameter")),
        "got {error}",
    );
}

/// A keyword-free head has no fixed token to dispatch on, so it is no shape at all — the callable
/// it describes is a lambda, and the diagnostic says so.
#[test]
fn a_keyword_free_head_is_refused() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);

    let error = test_run.run_one_err(test_run.parse_one("LET Bad = :(EXPR (x :Number) -> Number)"));
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(message)
            if message.contains("a shape has at least one keyword")),
        "got {error}",
    );
}

// ---------- the quantifier group ----------

/// `FOR ALL (<names>)` binds each name to the `Quantified(index)` leaf at its position, in a child
/// scope the head elaborates against — so a quantifier reaches a slot type and the return through
/// the ordinary type-name lookup, and the shape carries the group.
#[test]
fn a_quantified_head_binds_its_group_for_the_head_and_the_return() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("LET Identity = :(EXPR FOR ALL (Elt) (IDENT _ :Elt) -> Elt)");

    let shape = lookup_type(scope, "Identity").expect("the alias binds a type");
    let TypeNode::ExpressionShape {
        quantifiers,
        elements,
        ret,
    } = test_run.types().node(shape)
    else {
        panic!("a quantified head carries an expression shape");
    };
    assert_eq!(quantifiers.len(), 1);
    let quantified = test_run.types().quantified(0);
    assert_eq!(ret, quantified);
    assert_eq!(
        elements
            .iter()
            .filter_map(|element| match element {
                DispatchTokenElement::Slot(kt) => Some(*kt),
                DispatchTokenElement::Keyword(_) => None,
            })
            .collect::<Vec<_>>(),
        [quantified],
        "the slot lowered to the group's first name",
    );
}

/// The names are render-only: two groups differing only in what they spell intern one shape.
#[test]
fn quantified_shapes_are_equal_up_to_the_names() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "LET One = :(EXPR FOR ALL (Elt) (IDENT _ :Elt) -> Elt)\n\
         LET Two = :(EXPR FOR ALL (Other) (IDENT _ :Other) -> Other)",
    );

    let one = lookup_type(scope, "One").expect("One binds a type");
    let two = lookup_type(scope, "Two").expect("Two binds a type");
    assert_eq!(one, two);
}

/// Alpha-equivalence is by *position*, not by spelling order: swapping the group's names and the
/// uses together leaves one interned shape, because a name lowers to the index it stands at.
#[test]
fn a_swapped_group_with_swapped_uses_interns_once() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "LET One = :(EXPR FOR ALL (Elt Res) (STEP _ :Elt _ :Res) -> Res)\n\
         LET Two = :(EXPR FOR ALL (Res Elt) (STEP _ :Res _ :Elt) -> Elt)",
    );

    let one = lookup_type(scope, "One").expect("One binds a type");
    let two = lookup_type(scope, "Two").expect("Two binds a type");
    assert_eq!(one, two);
}

/// A quantified definition registers and runs; its group rides the callable, and the type it
/// reports on the value lane erases the quantified positions, which no lambda type can name.
#[test]
fn a_quantified_definition_dispatches() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("EXPR FOR ALL (Elt) (IDENT x :Elt) -> Elt = (x)");

    let result = test_run.run_one(test_run.parse_one("IDENT 7"));
    assert!(matches!(result, KObject::Number(n) if *n == 7.0));
}

/// The quantified bodyless head declares in a SIG body exactly as its unquantified twin does.
#[test]
fn a_quantified_bodyless_head_declares_in_a_sig_body() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Pure = ((EXPR FOR ALL (Elt) (PURE _ :Elt) -> Elt))\n\
         LET Shape = :(EXPR FOR ALL (Elt) (PURE _ :Elt) -> Elt)",
    );

    let schema = sig_schema(scope, test_run.types(), "Pure");
    let shape = lookup_type(scope, "Shape").expect("the alias binds a type");
    assert_eq!(schema.keyworded.as_slice(), [shape]);
}

/// A quantifier is a name the call solves, so the group admits Type tokens and nothing else, names
/// each once, and names at least one.
#[test]
fn a_malformed_quantifier_group_is_refused() {
    for (group, fragment) in [
        ("()", "names at least one quantifier"),
        ("(Elt Elt)", "twice"),
        ("(x)", "is not one"),
    ] {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let source = format!("LET Bad = :(EXPR FOR ALL {group} (PURE _ :Number) -> Number)");
        let error = test_run.run_one_err(test_run.parse_one(&source));
        assert!(
            matches!(&error.kind, KErrorKind::ShapeError(message)
                if message.contains(fragment)),
            "`FOR ALL {group}` must be refused with `{fragment}`, got {error}",
        );
    }
}

// ---------- the shape as a slot type ----------

/// A slot typed by a shape is filled by a callable, not by a type: it admits one whose registered
/// shape satisfies the declared one, and refuses a lambda and a callable registered under another
/// key. The two channels never cross — a lambda slot reads a callable's parameter record by name,
/// a shape slot reads its element run positionally.
#[test]
fn a_shape_typed_slot_admits_only_a_callable_of_that_shape() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "LET doubler = FN EXPR (DOUBLE n :Number) -> Number = (n * 2)\n\
         LET tripler = FN EXPR (TRIPLE n :Number) -> Number = (n * 3)\n\
         LET plain = (FN :{n :Number} -> Number = (n))\n\
         EXPR (APPLY g :(EXPR (DOUBLE _ :Number) -> Number)) -> Number = (7)",
    );

    let matched = test_run.run_one(test_run.parse_one("APPLY doubler"));
    assert!(matches!(matched, KObject::Number(n) if *n == 7.0));

    for refused in ["APPLY plain", "APPLY tripler"] {
        let error = test_run.run_one_err(test_run.parse_one(refused));
        assert!(
            matches!(&error.kind, KErrorKind::DispatchFailed { .. }),
            "`{refused}` must reach no overload, got {error}",
        );
    }
}

/// Two definitions differing only in what they name their slots satisfy one declaration: the names
/// are binder-side, so both project the shape the SIG declared.
#[test]
fn definitions_differing_only_in_slot_names_satisfy_one_declaration() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Doubler = ((EXPR (DOUBLE _ :Number) -> Number))\n\
         MODULE one = ((EXPR (DOUBLE n :Number) -> Number = (n * 2)))\n\
         MODULE two = ((EXPR (DOUBLE value :Number) -> Number = (value * 2)))\n\
         LET first = (one :| Doubler)\n\
         LET second = (two :| Doubler)",
    );

    for view in [
        "USING first SCOPE (DOUBLE 4)",
        "USING second SCOPE (DOUBLE 4)",
    ] {
        let result = test_run.run_one(test_run.parse_one(view));
        assert!(matches!(result, KObject::Number(n) if *n == 8.0), "{view}");
    }
}

/// The retired `FN` spellings are gone outright rather than diagnosed: nothing registers under a
/// `FN` key with a `(<head>)` operand, so the head evaluates where it stands and the statement
/// reports whatever the generic path reports. No message names the respelling.
#[test]
fn the_retired_fn_spellings_no_longer_resolve() {
    for source in [
        "LET Bad = :(FN (x :Number) -> Bool)",
        "SIG Pure = ((FN (PURE x :Number) -> Number))",
        "FN (PICK n :Number) -> Number = (n)",
    ] {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let error = test_run.run_one_err(test_run.parse_one(source));
        assert!(
            !format!("{error}").contains("EXPR"),
            "a retired spelling earns no migration diagnostic, got {error}",
        );
    }
}
