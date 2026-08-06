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
    test_run.run(
        "USING some_module SCOPE ((FN (SHOW c :Color) -> Str = (\"a color\")) \
         (SHOW (Color (Red null))))",
    );
    let result = test_run.run_one(parse_one(
        &program,
        "USING some_module SCOPE (SHOW (Color (Blue null)))",
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
    test_run.run("USING sealed SCOPE (FN (TAKES x :Elem) -> Str = (\"ok\"))");
    let err = test_run.run_one_err(parse_one(&program, "USING sealed SCOPE (TAKES 5)"));
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

/// The type-side collision guard, mirroring the value-side one: a block-local type declaration
/// reusing a surfaced member's name would forward to the call site and be silently shadowed by the
/// window's own entry, so the write op rejects it.
#[test]
fn block_type_declaration_colliding_with_a_member_errors() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Pointed = ((TYPE Elem) (VAL zero :Elem))\n\
         MODULE int_ord = ((LET Elem = Number) (LET zero = 0))\n\
         LET sealed = (int_ord :| Pointed)",
    );
    let err = test_run.run_one_err(parse_one(&program, "USING sealed SCOPE (LET Elem = Str)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("collides with a surfaced module type member") && msg.contains("`Elem`")),
        "expected the type-side collision ShapeError naming `Elem`, got {err}",
    );
}

/// The type channel under real per-statement chains (`enter_source`, not the detached-chain
/// `run`): a block-local type alias types a block-local `FN`'s slot, and a later statement of the
/// same block dispatches through it. The forwarded `types` entry carries its window position
/// ([`BindingIndex::window`](crate::machine::BindingIndex)), so the in-block reader gates by
/// block ordering exactly as the value channel does.
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

/// The guard's companion: a type declaration that collides with nothing forwards to the call site
/// like any other block bind, and is usable after the block ends.
#[test]
fn non_colliding_block_type_declaration_forwards_to_the_call_site() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Pointed = ((TYPE Elem) (VAL zero :Elem))\n\
         MODULE int_ord = ((LET Elem = Number) (LET zero = 0))\n\
         LET sealed = (int_ord :| Pointed)",
    );
    test_run.run("USING sealed SCOPE (LET Other = Str)");
    test_run.run("FN (WIDEN s :Other) -> Str = (s)");
    let result = test_run.run_one(parse_one(&program, "WIDEN \"after\""));
    assert!(matches!(result, KObject::KString(s) if *s == "after"));
}
