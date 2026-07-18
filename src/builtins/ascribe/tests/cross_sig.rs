//! Cross-SIG dispatch specificity: two distinct `SIG`-declared signature slots become
//! comparable when one structurally `sig_subtype`s the other. See
//! [design/typing/modules.md](../../../../design/typing/modules.md).

use crate::builtins::test_support::{lookup_module, parse_one, run, run_root_silent};
use crate::machine::model::KObject;
use crate::machine::run_root_storage;
use crate::machine::KErrorKind;
use crate::machine::KoanRuntime;

/// `SIG Wide` requires everything `SIG Base` does, plus more (`Wide` strictly `sig_subtype`s
/// `Base`), so `Wide` is strictly more specific: a module satisfying both dispatches to the
/// `:Wide` overload, never `:Base`.
#[test]
fn strict_cross_sig_subtype_wins_dispatch() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Base = ((VAL x :Number))\n\
         SIG Wide = ((VAL x :Number) (VAL y :Str))",
    );
    run(
        scope,
        "FN (PICK m :Wide) -> Module = (MODULE generated = (LET tag = 1))",
    );
    run(
        scope,
        "FN (PICK m :Base) -> Module = (MODULE generated = (LET tag = 2))",
    );
    run(
        scope,
        "MODULE implementation = ((LET x = 1) (LET y = \"s\"))",
    );
    run(scope, "LET arg = implementation");
    run(scope, "LET picked = (PICK arg)");

    let m = lookup_module(scope, "picked");
    let tag = m
        .child_scope()
        .bindings()
        .data()
        .get("tag")
        .map(|(o, _, _)| *o);
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
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Base = ((VAL x :Number))\n\
         SIG Wide = ((VAL x :Number) (VAL y :Str))",
    );
    run(
        scope,
        "FN (PICK m :Base) -> Module = (MODULE generated = (LET tag = 2))",
    );
    run(
        scope,
        "FN (PICK m :Wide) -> Module = (MODULE generated = (LET tag = 1))",
    );
    run(
        scope,
        "MODULE implementation = ((LET x = 1) (LET y = \"s\"))",
    );
    run(scope, "LET arg = implementation");
    run(scope, "LET picked = (PICK arg)");

    let m = lookup_module(scope, "picked");
    let tag = m
        .child_scope()
        .bindings()
        .data()
        .get("tag")
        .map(|(o, _, _)| *o);
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
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Alpha = ((VAL x :Number))\n\
         SIG Beta = ((VAL y :Number))",
    );
    run(
        scope,
        "FN (CHOOSE m :Alpha) -> Module = (MODULE generated = (LET tag = 1))",
    );
    run(
        scope,
        "FN (CHOOSE m :Beta) -> Module = (MODULE generated = (LET tag = 2))",
    );
    run(scope, "MODULE implementation = ((LET x = 1) (LET y = 2))");
    run(scope, "LET arg = implementation");

    let mut runtime = KoanRuntime::new();
    let root = runtime.dispatch_in_scope(parse_one("CHOOSE arg"), scope);
    runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let error = runtime
        .result_error(root)
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
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Base = ((TYPE Elt) (VAL x :Number))\n\
         SIG Wide = ((TYPE Elt) (VAL x :Number) (VAL y :Str))",
    );
    run(
        scope,
        "FN (PICKPIN m :(Wide WITH {Elt = Number})) -> Module = (MODULE generated = (LET tag = 1))",
    );
    run(
        scope,
        "FN (PICKPIN m :(Base WITH {Elt = Number})) -> Module = (MODULE generated = (LET tag = 2))",
    );
    run(
        scope,
        "MODULE implementation = ((LET Elt = Number) (LET x = 1) (LET y = \"s\"))",
    );
    run(scope, "LET arg = implementation");
    run(scope, "LET picked = (PICKPIN arg)");

    let m = lookup_module(scope, "picked");
    let tag = m
        .child_scope()
        .bindings()
        .data()
        .get("tag")
        .map(|(o, _, _)| *o);
    assert!(
        matches!(tag, Some(KObject::Number(n)) if *n == 1.0),
        "a pinned :Wide must still beat a pinned :Base when it strictly refines it, got {:?}",
        tag.map(|o| o.ktype())
    );
}
