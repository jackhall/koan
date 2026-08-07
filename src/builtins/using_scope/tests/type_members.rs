//! Type members surfaced by a `USING … SCOPE` window: a module's types resolve by bare name in
//! type positions inside the block, exactly as its values resolve in value positions.
//!
//! Module names carry a lowercase letter (the token classifier reads all-uppercase names as
//! keywords); dispatch keywords (`SHOW`, `PAINT`, `TAKES`, `NAMEOF`) stay all-uppercase, and never
//! a lone capital — a single uppercase letter classifies as neither keyword nor type name.

use crate::builtins::test_support::{extract_terminal, parse_one, TestRun};
use crate::machine::model::{Carried, KObject};
use crate::machine::KErrorKind;
use crate::machine::{program_storage, run_root_storage};

/// A plain module's `UNION` member types a dispatch slot inside the block: the window borrows the
/// module child scope's `types` table whole, so `:Color` resolves there.
#[test]
fn plain_module_type_member_types_a_dispatch_slot() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("MODULE some_module = ((UNION Color = (Red :Null Blue :Null)))");
    let result = test_run.run_one(parse_one(
        &program,
        "USING some_module SCOPE ((FN (SHOW c :Color) -> Str = (\"a color\")) \
         (SHOW (Color (Blue null))))",
    ));
    assert!(matches!(result, KObject::KString(s) if *s == "a color"));
}

/// The same member in the other type position — a `FN`'s declared return type — so both halves of
/// the sigil type language are covered, not just the slot.
#[test]
fn plain_module_type_member_types_a_return_slot() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("MODULE some_module = ((UNION Color = (Red :Null Blue :Null)))");
    let result = test_run.run_one(parse_one(
        &program,
        "USING some_module SCOPE ((FN (PAINT c :Color) -> Color = (c)) \
         (PAINT (Color (Blue null))))",
    ));
    assert!(
        matches!(result, KObject::Tagged { tag, .. } if *tag == "Blue"),
        "the module's `Color` must type both the slot and the return, got {:?}",
        result.ktype(),
    );
}

/// The item's payoff: an opaque view's abstract member resolves by bare name inside the block. The
/// window reads the view scope's `types` table, seeded at ascription with the per-call mint, so
/// `:Elem` is the view's `AbstractType` and the `ATTR`-tagged member satisfies it.
#[test]
fn opaque_view_surfaces_its_abstract_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Pointed = ((TYPE Elem) (VAL zero :Elem))\n\
         MODULE int_ord = ((LET Elem = Number) (LET zero = 0))\n\
         LET sealed = (int_ord :| Pointed)",
    );
    let result = test_run.run_one(parse_one(
        &program,
        "USING sealed SCOPE ((FN (TAKES x :Elem) -> Str = (\"ok\")) (TAKES (sealed.zero)))",
    ));
    assert!(matches!(result, KObject::KString(s) if *s == "ok"));
}

/// The opacity half of the same seeding: the view scope holds the mint, never the representation,
/// so `Number` does not satisfy the block's `:Elem` and the call finds no overload. `Number` is
/// reachable in the block as itself — this asserts it is not reachable *as* `Elem`.
#[test]
fn opaque_view_hides_the_representation_inside_the_block() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Pointed = ((TYPE Elem) (VAL zero :Elem))\n\
         MODULE int_ord = ((LET Elem = Number) (LET zero = 0))\n\
         LET sealed = (int_ord :| Pointed)",
    );
    let err = test_run.run_one_err(parse_one(
        &program,
        "USING sealed SCOPE ((FN (TAKES x :Elem) -> Str = (\"ok\")) (TAKES 5))",
    ));
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
        "a raw Number must not satisfy the view's abstract `Elem`, got {err}",
    );
}

/// A signature's manifest member surfaces at its fixed identity, unhidden — the other half of what
/// the ascription seeds into the view scope.
#[test]
fn opaque_view_surfaces_its_manifest_member_concretely() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Boxed = ((TYPE Elem) (LET Tag = Str) (VAL zero :Elem) (VAL label :Tag))\n\
         MODULE int_ord = ((LET Elem = Number) (LET Tag = Str) (LET zero = 0) (LET label = \"n\"))\n\
         LET sealed = (int_ord :| Boxed)",
    );
    let result = test_run.run_one(parse_one(
        &program,
        "USING sealed SCOPE ((FN (NAMEOF t :Tag) -> Str = (t)) (NAMEOF \"plain\"))",
    ));
    assert!(
        matches!(result, KObject::KString(s) if *s == "plain"),
        "a manifest `Tag = Str` admits a bare Str inside the block, got {:?}",
        result.ktype(),
    );
}

/// A transparent view reuses the source module's child scope, so the block reads the source's
/// concrete identities — transparent is transparent, and the seeding above does not touch it.
#[test]
fn transparent_view_surfaces_the_concrete_type() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Pointed = ((TYPE Elem) (VAL zero :Elem))\n\
         MODULE int_ord = ((LET Elem = Number) (LET zero = 0))\n\
         LET opened = (int_ord :! Pointed)",
    );
    let result = test_run.run_one(parse_one(
        &program,
        "USING opened SCOPE ((FN (TAKES x :Elem) -> Str = (\"ok\")) (TAKES 5))",
    ));
    assert!(matches!(result, KObject::KString(s) if *s == "ok"));
}

/// Type-side shadowing, mirroring the value-side rule: a block-local type declaration reusing a
/// surfaced member's name is ordinary inner-scope shadowing — the block layer answers before the
/// window from the next statement on, so the view's abstract `Elem` gives way to the local `Str`
/// and a bare `Str` satisfies the slot.
#[test]
fn block_type_declaration_shadows_a_surfaced_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Pointed = ((TYPE Elem) (VAL zero :Elem))\n\
         MODULE int_ord = ((LET Elem = Number) (LET zero = 0))\n\
         LET sealed = (int_ord :| Pointed)",
    );
    let result = test_run.run_one(parse_one(
        &program,
        "USING sealed SCOPE ((LET Elem = Str) (FN (TAKES x :Elem) -> Str = (x)) \
         (TAKES \"shadowed\"))",
    ));
    assert!(
        matches!(result, KObject::KString(s) if *s == "shadowed"),
        "the block's own `Elem` must win over the view's abstract member, got {:?}",
        result.ktype(),
    );
}

/// The type channel under real per-statement chains (`enter_source`, not the detached-chain
/// `run`): a block-local type alias types a block-local `FN`'s slot, and a later statement of the
/// same block dispatches through it. The `types` entry lives in the block's own scope at its plain
/// statement index, so the in-block reader gates by block ordering exactly as the value channel
/// does.
#[test]
fn block_type_alias_types_a_later_statement_of_the_same_block() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let ids = test_run.enter_source(
        "MODULE some_module = ((UNION Color = (Red :Null Blue :Null)))\n\
         USING some_module SCOPE (\n  \
         LET Alias = Color\n  \
         FN (SHOW c :Alias) -> Str = (\"a color\")\n  \
         (SHOW (Alias (Blue null)))\n\
         )",
    );
    test_run
        .runtime
        .execute()
        .expect("scheduler should succeed");
    let tail = extract_terminal(
        &test_run.runtime,
        test_run.scope,
        *ids.last().expect("two top-level statements"),
    );
    assert!(
        matches!(tail, Carried::Object(KObject::KString(s)) if *s == "a color"),
        "a block statement dispatches through its earlier siblings' alias and FN",
    );
}

/// The type channel's locality half: a block-local type declaration dies with the block exactly as
/// a value bind does, so a `FN` declared after the block cannot name it.
#[test]
fn block_type_declaration_dies_with_the_block() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Pointed = ((TYPE Elem) (VAL zero :Elem))\n\
         MODULE int_ord = ((LET Elem = Number) (LET zero = 0))\n\
         LET sealed = (int_ord :| Pointed)",
    );
    test_run.run("USING sealed SCOPE (LET Other = Str)");
    let err = test_run.run_one_err(parse_one(&program, "FN (WIDEN s :Other) -> Str = (s)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("unknown type name `Other`")),
        "expected the block-local `Other` to be unknown after the block, got {err}",
    );
}
