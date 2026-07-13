//! Cross-SIG dispatch specificity: two distinct `SIG`-declared signature slots become
//! comparable when one structurally `sig_subtype`s the other. See
//! [design/typing/modules.md](../../../../design/typing/modules.md).

use crate::builtins::test_support::{parse_one, run, run_root_silent};
use crate::machine::core::run_root_storage;
use crate::machine::execute::KoanRuntime;
use crate::machine::model::types::memo_reset;
use crate::machine::model::{KObject, KType};
use crate::machine::KErrorKind;

/// `SIG Wide` requires everything `SIG Base` does, plus more (`Wide` strictly `sig_subtype`s
/// `Base`), so `Wide` is strictly more specific: a module satisfying both dispatches to the
/// `:Wide` overload, never `:Base`.
#[test]
fn strict_cross_sig_subtype_wins_dispatch() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    memo_reset();
    run(
        scope,
        "SIG Base = ((VAL x :Number))\n\
         SIG Wide = ((VAL x :Number) (VAL y :Str))",
    );
    run(
        scope,
        "FN (PICK m :Wide) -> Module = (MODULE Generated = (LET tag = 1))",
    );
    run(
        scope,
        "FN (PICK m :Base) -> Module = (MODULE Generated = (LET tag = 2))",
    );
    run(scope, "MODULE Impl = ((LET x = 1) (LET y = \"s\"))");
    run(scope, "LET Arg = Impl");
    run(scope, "LET Picked = (PICK Arg)");

    let m = match scope.resolve_type("Picked") {
        Some(KType::Module { module: m }) => *m,
        _ => panic!("Picked should be a module identity in types"),
    };
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
    memo_reset();
    run(
        scope,
        "SIG Base = ((VAL x :Number))\n\
         SIG Wide = ((VAL x :Number) (VAL y :Str))",
    );
    run(
        scope,
        "FN (PICK m :Base) -> Module = (MODULE Generated = (LET tag = 2))",
    );
    run(
        scope,
        "FN (PICK m :Wide) -> Module = (MODULE Generated = (LET tag = 1))",
    );
    run(scope, "MODULE Impl = ((LET x = 1) (LET y = \"s\"))");
    run(scope, "LET Arg = Impl");
    run(scope, "LET Picked = (PICK Arg)");

    let m = match scope.resolve_type("Picked") {
        Some(KType::Module { module: m }) => *m,
        _ => panic!("Picked should be a module identity in types"),
    };
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

/// Two structurally-identical but distinct SIGs are mutually-satisfying under
/// `sig_subtype` — forward and reverse both hold, so neither strictly refines the other.
/// A module satisfying both admits both overloads with equal specificity, so dispatch is
/// ambiguous. This guards the `forward && !reverse` strictness: a one-way check would let
/// declaration order silently pick a winner instead of surfacing the tie.
#[test]
fn mutually_satisfying_distinct_sigs_are_ambiguous() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    memo_reset();
    run(
        scope,
        "SIG SigOne = ((VAL x :Number))\n\
         SIG SigTwo = ((VAL x :Number))",
    );
    run(
        scope,
        "FN (CHOOSE m :SigOne) -> Module = (MODULE Generated = (LET tag = 1))",
    );
    run(
        scope,
        "FN (CHOOSE m :SigTwo) -> Module = (MODULE Generated = (LET tag = 2))",
    );
    run(scope, "MODULE Impl = ((LET x = 1))");
    run(scope, "LET Arg = Impl");

    let mut runtime = KoanRuntime::new();
    let root = runtime.dispatch_in_scope(parse_one("CHOOSE Arg"), scope);
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
    memo_reset();
    run(
        scope,
        "SIG Base = ((TYPE Elt) (VAL x :Number))\n\
         SIG Wide = ((TYPE Elt) (VAL x :Number) (VAL y :Str))",
    );
    run(
        scope,
        "FN (PICKPIN m :(Wide WITH {Elt = Number})) -> Module = (MODULE Generated = (LET tag = 1))",
    );
    run(
        scope,
        "FN (PICKPIN m :(Base WITH {Elt = Number})) -> Module = (MODULE Generated = (LET tag = 2))",
    );
    run(
        scope,
        "MODULE Impl = ((LET Elt = Number) (LET x = 1) (LET y = \"s\"))",
    );
    run(scope, "LET Arg = Impl");
    run(scope, "LET Picked = (PICKPIN Arg)");

    let m = match scope.resolve_type("Picked") {
        Some(KType::Module { module: m }) => *m,
        _ => panic!("Picked should be a module identity in types"),
    };
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
