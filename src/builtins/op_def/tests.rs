//! `OP` surface tests: the declaration forms, what each registers, the shadowing/type-gating
//! semantics against the builtin operators, and the errors the surface rejects.

use crate::builtins::test_support::{binds_module, parse_one, TestRun};
use crate::machine::model::Held;
use crate::machine::model::KObject;
use crate::machine::model::TypeRegistry;
use crate::machine::run_root_storage;
use crate::machine::KErrorKind;

/// The numbers of a `KObject::List`, for the unary tests that collect a run into one list.
fn list_numbers(object: &KObject<'_>, types: &TypeRegistry) -> Vec<f64> {
    match object {
        KObject::List(items, _) => items
            .elements()
            .iter()
            .map(|item| match item {
                Held::Object(KObject::Number(n)) => *n,
                other => panic!("expected a Number element, got {}", other.summarize(types)),
            })
            .collect(),
        other => panic!("expected a list, got {}", other.ktype().name(types)),
    }
}

/// AC1: a module declares an operator, and a three-operand run inside the module body reduces
/// fold-left through the declared body — which sees its sibling module bindings (`bump`) because
/// the body captures its declaring scope.
#[test]
fn module_operator_run_reduces_fold_left_through_the_body() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run(
        "MODULE vectors = (\
           (LET bump = 10)\
           (OP #(⊕) OVER Number = ((left + right) + bump))\
           (LET total = (1 ⊕ 2 ⊕ 3)))",
    );
    // fold-left: (1 ⊕ 2) = 13, (13 ⊕ 3) = 26.
    assert!(
        matches!(test_run.run_one(parse_one("vectors.total")), KObject::Number(n) if *n == 26.0),
        "the run reduces fold-left through the operator body, which reads its sibling `bump`",
    );
}

/// AC3: the same symbol declared in two modules over distinct operand types resolves to each
/// module's own body — the ordinary innermost-wins scope walk, nothing operator-specific.
#[test]
fn same_symbol_in_two_modules_resolves_by_scope() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run(
        "MODULE takes_left = ((OP #(⊗) OVER Number = (left)) (LET result = (1 ⊗ 2 ⊗ 3)))\n\
         MODULE takes_right = ((OP #(⊗) OVER Str = (right)) (LET result = (\"x\" ⊗ \"y\" ⊗ \"z\")))",
    );
    assert!(
        matches!(test_run.run_one(parse_one("takes_left.result")), KObject::Number(n) if *n == 1.0),
    );
    assert!(
        matches!(test_run.run_one(parse_one("takes_right.result")), KObject::KString(s) if s == "z"),
    );
}

/// An operator declared over a `:(…)` operand type: the slot sub-dispatches, so the whole
/// declaration defers to a dep-finish before it registers.
#[test]
fn sigiled_operand_type_declares_over_lists() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let types = test_run.types.clone();
    test_run.run(
        "LET xs = [1 2]\n\
         LET ys = [3]\n\
         MODULE lists = ((OP #(&) OVER :(LIST OF Number) = (right)) (LET result = (xs & ys)))",
    );
    assert_eq!(
        list_numbers(test_run.run_one(parse_one("lists.result")), &types),
        vec![3.0],
    );
}

/// Builtin interplay: a module declares `+` over lists. Number operands still take the builtin
/// (the immutable root bucket is consulted first and type-gates), list operands fall through to
/// the module's body, and a three-operand list run resolves the module's own singleton group.
#[test]
fn declared_plus_over_lists_leaves_number_arithmetic_alone() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let types = test_run.types.clone();
    test_run.run(
        "LET xs = [1]\n\
         LET ys = [2]\n\
         LET zs = [3]\n\
         MODULE lists = (\
           (OP #(+) OVER :(LIST OF Number) = (right))\
           (LET numbers = (1 + 2))\
           (LET pair = (xs + ys))\
           (LET chained = (xs + ys + zs)))",
    );
    assert!(
        matches!(test_run.run_one(parse_one("lists.numbers")), KObject::Number(n) if *n == 3.0),
        "`1 + 2` still hits the builtin — the root bucket type-gates on Number operands",
    );
    assert_eq!(
        list_numbers(test_run.run_one(parse_one("lists.pair")), &types),
        vec![2.0],
        "list operands miss the builtin's strict gate and fall through to the module body",
    );
    assert_eq!(
        list_numbers(test_run.run_one(parse_one("lists.chained")), &types),
        vec![3.0],
        "the three-operand run resolves the module's singleton `+` group and folds left",
    );
}

/// `OP #(+) OVER Number` registers — builtin operators are shadowable — but the builtin still
/// wins for `Number` operands, because dispatch consults the immutable root bucket first.
#[test]
fn declaring_plus_over_number_registers_but_the_builtin_still_wins() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run("MODULE shadowed = ((OP #(+) OVER Number = (999)) (LET result = (1 + 2)))");
    assert!(
        matches!(test_run.run_one(parse_one("shadowed.result")), KObject::Number(n) if *n == 3.0),
        "the builtin `+` wins for the operand types it declares",
    );
}

/// A unary operator takes the whole run as one list. All four surfaces reach the one body: the
/// infix run (reduced to the keyword-first shape) over literals and over named operands, the
/// two-operand call (through the synthesized binary bridge), and the prefix form.
#[test]
fn unary_operator_collects_the_run_prefix_infix_and_pair() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let types = test_run.types.clone();
    test_run.run(
        "LET one = 1\n\
         LET two = 2\n\
         MODULE gather = (\
           (UNARY OP #(~) OVER Number -> :(LIST OF Number) = (operands))\
           (LET chained = (1 ~ 2 ~ 3))\
           (LET named = (one ~ two ~ 3))\
           (LET pair = (4 ~ 5))\
           (LET prefix = (~ [6 7 8])))",
    );
    assert_eq!(
        list_numbers(test_run.run_one(parse_one("gather.named")), &types),
        vec![1.0, 2.0, 3.0],
        "a named operand of a run is an element expression, not an interned symbol",
    );
    assert_eq!(
        list_numbers(test_run.run_one(parse_one("gather.chained")), &types),
        vec![1.0, 2.0, 3.0],
        "an infix run collects into `operands`",
    );
    assert_eq!(
        list_numbers(test_run.run_one(parse_one("gather.pair")), &types),
        vec![4.0, 5.0],
        "a two-operand call reaches the list body through the binary bridge",
    );
    assert_eq!(
        list_numbers(test_run.run_one(parse_one("gather.prefix")), &types),
        vec![6.0, 7.0, 8.0],
        "the prefix form is the same keyword-first shape a reduced run takes",
    );
}

/// The declaration surface itself: an unquoted symbol is not an `OP` shape at all — it keys no
/// `OP` bucket, so it cannot dispatch.
#[test]
fn unquoted_symbol_does_not_declare_an_operator() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let error = test_run.run_one_err(parse_one("OP + OVER Number = (left)"));
    assert!(
        matches!(&error.kind, KErrorKind::DispatchFailed { .. }),
        "an unquoted symbol keys no OP overload, got {error}",
    );
}

/// A quote holding more than one token names no operator.
#[test]
fn multi_token_quote_is_not_an_operator_symbol() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let error = test_run.run_one_err(parse_one("OP #(1 + 2) OVER Number = (left)"));
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg) if msg.contains("one quoted token")),
        "expected the quoted-symbol diagnostic, got {error}",
    );
}

/// The declaration surface's own keywords cannot be operator symbols.
#[test]
fn reserved_symbol_is_rejected() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let error = test_run.run_one_err(parse_one("OP #(OVER) OVER Number = (left)"));
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg) if msg.contains("reserved")),
        "expected the reserved-symbol diagnostic, got {error}",
    );
}

/// An all-caps alphabetic symbol is an ordinary keyword token, so it is a legal operator name.
#[test]
fn all_caps_symbol_is_a_legal_operator() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run
        .run("MODULE picks = ((OP #(MAX) OVER Number = (left)) (LET result = (1 MAX 2 MAX 3)))");
    assert!(matches!(test_run.run_one(parse_one("picks.result")), KObject::Number(n) if *n == 1.0),);
}

/// A heterogeneous binary member is admissible only inside a PAIRWISE group, where a combiner
/// folds the pair results. Outside one, the explicit `->` is an error that says so.
#[test]
fn heterogeneous_binary_operator_outside_a_pairwise_group_errors() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let error = test_run.run_one_err(parse_one("OP #(≺) OVER Number -> Bool = (true)"));
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg) if msg.contains("PAIRWISE")),
        "expected the PAIRWISE diagnostic, got {error}",
    );
}

/// A unary operator's result type cannot be defaulted from its operand type — it consumes a whole
/// list of them — so the `-> Result` segment is mandatory.
#[test]
fn unary_operator_without_a_result_type_errors() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let error = test_run.run_one_err(parse_one("UNARY OP #(~) OVER Number = (operands)"));
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg) if msg.contains("must declare its result type")),
        "expected the missing-result diagnostic, got {error}",
    );
}

/// Two declarations of one symbol in one scope must agree on the reduction mode: the registry
/// upsert is idempotent for an equal record and an error for a conflicting one.
#[test]
fn same_scope_mode_conflict_errors() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run("OP #(⊙) OVER Number = (left)");
    let error = test_run.run_one_err(parse_one(
        "UNARY OP #(⊙) OVER Str -> :(LIST OF Str) = (operands)",
    ));
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg) if msg.contains('⊙')),
        "expected the mode-conflict diagnostic naming the operator, got {error}",
    );
}

/// Two declarations of one symbol over different operand types are two bucket overloads and one
/// registry entry — the upsert absorbs the second, equal registration.
#[test]
fn two_operand_types_under_one_symbol_upsert_to_one_group() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run(
        "MODULE both = (\
           (OP #(⊚) OVER Number = (left))\
           (OP #(⊚) OVER Str = (right))\
           (LET numbers = (1 ⊚ 2 ⊚ 3))\
           (LET strings = (\"a\" ⊚ \"b\" ⊚ \"c\")))",
    );
    assert!(matches!(test_run.run_one(parse_one("both.numbers")), KObject::Number(n) if *n == 1.0),);
    assert!(matches!(test_run.run_one(parse_one("both.strings")), KObject::KString(s) if s == "c"),);
}

/// Parking: the declaration's operand type sub-dispatches, so the `OP` slot is still in flight
/// when the sibling run below it pops. The chain arm finds the declaration's pending-overload
/// entry and parks on it instead of surfacing an undeclared-operator miss.
#[test]
fn a_run_parks_on_a_still_finalizing_declaration() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let types = test_run.types.clone();
    test_run.run(
        "LET xs = [1]\n\
         LET ys = [2]\n\
         LET zs = [3]\n\
         MODULE deferred = (\
           (OP #(⊛) OVER :(LIST OF Number) = (left))\
           (LET result = (xs ⊛ ys ⊛ zs)))",
    );
    assert_eq!(
        list_numbers(test_run.run_one(parse_one("deferred.result")), &types),
        vec![1.0],
    );
}

/// Lexical cutoff: an operator declared *after* a run is invisible to it — the pending-overload
/// probe is visibility-gated exactly like every other name lookup, so the run errors rather than
/// parking on a declaration it cannot see. The erroring body statement short-circuits the module's
/// finalize, so the module never binds; the same two statements in declaration order do bind.
#[test]
fn a_run_above_the_declaration_does_not_see_it() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE early = ((LET result = (1 ⊘ 2 ⊘ 3)) (OP #(⊘) OVER Number = (left)))\n\
         MODULE ordered = ((OP #(⊘) OVER Number = (left)) (LET result = (1 ⊘ 2 ⊘ 3)))",
    );
    assert!(
        !binds_module(scope, "early"),
        "the run above the declaration must fail, which short-circuits the module's finalize",
    );
    assert!(
        binds_module(scope, "ordered"),
        "the same statements in declaration order resolve",
    );
    assert!(
        matches!(test_run.run_one(parse_one("ordered.result")), KObject::Number(n) if *n == 1.0),
    );
}

/// The declaration evaluates to the function it declares, as a bare `FN` does.
#[test]
fn declaration_evaluates_to_the_operator_function() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let value = test_run.run_one(parse_one("OP #(⊹) OVER Number = (left)"));
    assert!(
        matches!(value, KObject::KFunction(_)),
        "an OP statement evaluates to its synthesized function, got {}",
        value.ktype().name(&test_run.types),
    );
}
