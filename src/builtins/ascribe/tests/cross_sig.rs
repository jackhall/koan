//! Cross-SIG dispatch specificity: two distinct `SIG`-declared signature slots become
//! comparable when one structurally `sig_subtype`s the other. See
//! [design/typing/modules.md](../../../../design/typing/modules.md).

use crate::builtins::test_support::{TestRun, lookup_module};
use crate::machine::KErrorKind;
use crate::machine::model::KObject;
use crate::machine::{program_storage, run_root_storage};

/// `SIG Wide` requires everything `SIG Base` does, plus more (`Wide` strictly `sig_subtype`s
/// `Base`), so `Wide` is strictly more specific: a module satisfying both dispatches to the
/// `:Wide` overload, never `:Base`.
#[test]
fn strict_cross_sig_subtype_wins_dispatch() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Base = ((VAL x :Number))\n\
         SIG Wide = ((VAL x :Number) (VAL y :Str))",
    );
    test_run.run("FN (PICK m :Wide) -> Module = (MODULE generated = (LET tag = 1))");
    test_run.run("FN (PICK m :Base) -> Module = (MODULE generated = (LET tag = 2))");
    test_run.run("MODULE implementation = ((LET x = 1) (LET y = \"s\"))");
    test_run.run("LET arg = implementation");
    test_run.run("LET picked = (PICK arg)");

    let m = lookup_module(scope, "picked", test_run.registries());
    let tag = m.child_scope().lookup("tag");
    assert!(
        matches!(tag, Some(KObject::Number(n)) if *n == 1.0),
        "a module satisfying both Wide and Base must dispatch to the more-specific :Wide overload, got {:?}",
        tag.map(|o| o.ktype())
    );
}

/// Declaring `:Base` first must not let declaration order silently win: the strictness check
/// (`forward && !reverse`) is order-independent.
#[test]
fn strict_cross_sig_subtype_wins_regardless_of_declaration_order() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Base = ((VAL x :Number))\n\
         SIG Wide = ((VAL x :Number) (VAL y :Str))",
    );
    test_run.run("FN (PICK m :Base) -> Module = (MODULE generated = (LET tag = 2))");
    test_run.run("FN (PICK m :Wide) -> Module = (MODULE generated = (LET tag = 1))");
    test_run.run("MODULE implementation = ((LET x = 1) (LET y = \"s\"))");
    test_run.run("LET arg = implementation");
    test_run.run("LET picked = (PICK arg)");

    let m = lookup_module(scope, "picked", test_run.registries());
    let tag = m.child_scope().lookup("tag");
    assert!(
        matches!(tag, Some(KObject::Number(n)) if *n == 1.0),
        "declaring the less-specific :Base overload first must not flip the winner, got {:?}",
        tag.map(|o| o.ktype())
    );
}

/// Two incomparable distinct SIGs — `Alpha` requires `x`, `Beta` requires `y` — that a module
/// supplying both satisfies. Neither strictly `sig_subtype`s the other, so the overloads tie and
/// dispatch is ambiguous. This guards the `forward && !reverse` strictness: a one-way check would
/// let declaration order silently pick a winner instead of surfacing the tie. (Two *structurally
/// identical* SIGs are one type under content identity, so a tie can only arise from incomparable
/// interfaces like these, never from mutual satisfaction.)
#[test]
fn incomparable_distinct_sigs_are_ambiguous() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Alpha = ((VAL x :Number))\n\
         SIG Beta = ((VAL y :Number))",
    );
    test_run.run("FN (CHOOSE m :Alpha) -> Module = (MODULE generated = (LET tag = 1))");
    test_run.run("FN (CHOOSE m :Beta) -> Module = (MODULE generated = (LET tag = 2))");
    test_run.run("MODULE implementation = ((LET x = 1) (LET y = 2))");
    test_run.run("LET arg = implementation");

    let root = test_run.dispatch_watched_in(
        scope,
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            test_run.parse_one("CHOOSE arg"),
        ),
    );
    test_run
        .runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let error = test_run
        .runtime
        .edge_result_error(root)
        .expect_err("a module satisfying two mutually-satisfying distinct SIGs must be ambiguous");
    assert!(
        matches!(error.kind, KErrorKind::AmbiguousDispatch { .. }),
        "expected AmbiguousDispatch across mutually-satisfying distinct SIGs, got {error:?}",
    );
}

/// `WITH`-pinned variants of two distinct SIGs still compare by structural subtyping — the
/// pin folds into `of_sig` on both sides. `Wide` (with an extra `y` slot) beats `Base`, both
/// pinned to the same abstract `Elt = Number`.
#[test]
fn cross_sig_specificity_with_pinned_abstract_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Base = ((TYPE Elt) (VAL x :Number))\n\
         SIG Wide = ((TYPE Elt) (VAL x :Number) (VAL y :Str))",
    );
    test_run.run(
        "FN (PICKPIN m :(Wide WITH {Elt = Number})) -> Module = (MODULE generated = (LET tag = 1))",
    );
    test_run.run(
        "FN (PICKPIN m :(Base WITH {Elt = Number})) -> Module = (MODULE generated = (LET tag = 2))",
    );
    test_run.run("MODULE implementation = ((LET Elt = Number) (LET x = 1) (LET y = \"s\"))");
    test_run.run("LET arg = implementation");
    test_run.run("LET picked = (PICKPIN arg)");

    let m = lookup_module(scope, "picked", test_run.registries());
    let tag = m.child_scope().lookup("tag");
    assert!(
        matches!(tag, Some(KObject::Number(n)) if *n == 1.0),
        "a pinned :Wide must still beat a pinned :Base when it strictly refines it, got {:?}",
        tag.map(|o| o.ktype())
    );
}
