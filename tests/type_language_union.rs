//! End-to-end tests for the anonymous-union surface `:(A | B)` — the `|` unary-mode
//! operator group and its union-type constructor builtin (Phase 2 of anonymous-unions).
//!
//! Covers: a union FN parameter admits every member and rejects a non-member (AC1);
//! order-blind, idempotent member sets end-to-end (AC2); a union as an FN / MATCH / TRY
//! declared return type (AC4); a union-return value keeping its runtime type through
//! dispatch (AC5 / fork F4); the three-member run building in one pass (AC7); the known
//! surface asymmetry pins (§ 4.3); and value-context behavior.
//!
//! Companion design: [design/typing/type-language-via-dispatch.md].

use std::rc::Rc;

use koan::builtins::test_support::TestRun;
use koan::machine::{run_root_storage, FrameStorage};
use koan::parse::parse;

/// Run `src` to completion, returning everything it PRINTed.
fn run_capture(region: &Rc<FrameStorage>, src: &str) -> String {
    let (mut test_run, captured) = TestRun::with_buf(region);
    let scope = test_run.scope;
    let exprs = parse(src).expect("parse should succeed");
    for e in exprs {
        test_run.runtime.dispatch_in_scope(e, scope);
    }
    test_run
        .runtime
        .execute()
        .expect("scheduler should run to completion");
    let bytes = captured.borrow().clone();
    String::from_utf8(bytes).expect("PRINT output is UTF-8")
}

/// Run `src`, expecting the last top-level slot to be a slot-terminal error; returns its text.
fn run_expect_err(region: &Rc<FrameStorage>, src: &str) -> String {
    let mut test_run = TestRun::silent(region);
    let scope = test_run.scope;
    let exprs = parse(src).expect("parse should succeed");
    let ids: Vec<_> = exprs
        .into_iter()
        .map(|e| test_run.runtime.dispatch_in_scope(e, scope))
        .collect();
    test_run
        .runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let last = *ids.last().expect("at least one expression");
    match test_run.runtime.result_error(last) {
        Ok(()) => panic!("expected a slot error, got success"),
        Err(e) => e.to_string(),
    }
}

/// AC1: a union FN parameter `:(Number | Str)` admits a `Number` and a `Str` argument.
#[test]
fn union_param_admits_each_member() {
    let region = run_root_storage();
    let out = run_capture(
        &region,
        "FN (ACCEPT v :(Number | Str)) -> Str = (\"ok\")\n\
         PRINT (ACCEPT 5)\n\
         PRINT (ACCEPT \"hi\")",
    );
    assert_eq!(
        out.matches("ok").count(),
        2,
        "both members admit; got {out:?}"
    );
}

/// AC1: a union FN parameter `:(Number | Str)` rejects a `Bool` — no member admits it, so
/// dispatch falls through with no matching overload.
#[test]
fn union_param_rejects_non_member() {
    let region = run_root_storage();
    let err = run_expect_err(
        &region,
        "FN (ACCEPT v :(Number | Str)) -> Str = (\"ok\")\n\
         ACCEPT true",
    );
    assert!(
        err.contains("dispatch failed") || err.contains("no matching function"),
        "a non-member argument must fail dispatch; got {err}",
    );
}

/// AC2: member order is invisible at the surface — a `:(Str | Number)` parameter admits the
/// same arguments a `:(Number | Str)` one does.
#[test]
fn union_param_order_blind() {
    let region = run_root_storage();
    let out = run_capture(
        &region,
        "FN (ACCEPT v :(Str | Number)) -> Str = (\"ok\")\n\
         PRINT (ACCEPT 5)\n\
         PRINT (ACCEPT \"hi\")",
    );
    assert_eq!(
        out.matches("ok").count(),
        2,
        "order-blind admission; got {out:?}"
    );
}

/// AC2: `:(Number | Number)` collapses to `:Number` — the parameter admits a `Number` and
/// still rejects a `Str`, exactly as a bare `:Number` parameter would.
#[test]
fn duplicate_member_behaves_as_single() {
    let region = run_root_storage();
    let out = run_capture(
        &region,
        "FN (ONLY_NUM v :(Number | Number)) -> Str = (\"num\")\n\
         PRINT (ONLY_NUM 5)",
    );
    assert!(
        out.contains("num"),
        "collapsed union admits Number; got {out:?}"
    );

    let region = run_root_storage();
    let err = run_expect_err(
        &region,
        "FN (ONLY_NUM v :(Number | Number)) -> Str = (\"num\")\n\
         ONLY_NUM \"hi\"",
    );
    assert!(
        err.contains("dispatch failed") || err.contains("no matching function"),
        "collapsed `:(Number | Number)` rejects a Str like a bare `:Number`; got {err}",
    );
}

/// AC4 + AC5 (fork F4): an FN whose declared return type is `:(Number | Str)` validates its
/// result but never re-tags it — a returned `Number` keeps its runtime type, so a downstream
/// type-dispatched function lands on the `:Number` arm, and a returned `Str` lands on `:Str`.
#[test]
fn union_return_keeps_runtime_type_for_dispatch() {
    let region = run_root_storage();
    let out = run_capture(
        &region,
        "FN (WIDEN_NUM n :Number) -> :(Number | Str) = (n)\n\
         FN (WIDEN_STR s :Str) -> :(Number | Str) = (s)\n\
         FN (CLASSIFY x :Number) -> Str = (\"num\")\n\
         FN (CLASSIFY x :Str) -> Str = (\"str\")\n\
         PRINT (CLASSIFY (WIDEN_NUM 5))\n\
         PRINT (CLASSIFY (WIDEN_STR \"hi\"))",
    );
    assert!(
        out.contains("num") && out.contains("str"),
        "each union-return value keeps its runtime type for downstream dispatch; got {out:?}",
    );
}

/// AC4: a union as a MATCH `-> :T` return type. Both `Bool` arms return distinct member-typed
/// values checked against `:(Number | Str)`, and the value keeps its runtime type (F4), so a
/// downstream type-dispatched function classifies it.
#[test]
fn match_union_return_type() {
    let region = run_root_storage();
    let out = run_capture(
        &region,
        "FN (CLASSIFY x :Number) -> Str = (\"num\")\n\
         FN (CLASSIFY x :Str) -> Str = (\"str\")\n\
         FN (PICK flag :Bool) -> :(Number | Str) = \
           (MATCH (flag) -> :(Number | Str) WITH (true -> (5) false -> (\"hi\")))\n\
         PRINT (CLASSIFY (PICK true))\n\
         PRINT (CLASSIFY (PICK false))",
    );
    assert!(
        out.contains("num") && out.contains("str"),
        "MATCH arms typed against a union return keep runtime types; got {out:?}",
    );
}

/// AC4: a union as a TRY `-> :T` return type. The happy path returns a `Number` checked
/// against `:(Number | Str)` without re-tagging.
#[test]
fn try_union_return_type() {
    let region = run_root_storage();
    let out = run_capture(
        &region,
        "FN (CLASSIFY x :Number) -> Str = (\"num\")\n\
         FN (CLASSIFY x :Str) -> Str = (\"str\")\n\
         FN (RUN n :Number) -> :(Number | Str) = \
           (TRY (n) -> :(Number | Str) WITH (Ok -> (it)))\n\
         PRINT (CLASSIFY (RUN 7))",
    );
    assert!(
        out.contains("num"),
        "a TRY happy-path Number keeps its runtime type under a union return; got {out:?}",
    );
}

/// AC7: a three-member run `:(Number | Str | Bool)` builds the whole union in one pass —
/// a function parameter typed with it admits all three members.
#[test]
fn three_member_union_admits_all() {
    let region = run_root_storage();
    let out = run_capture(
        &region,
        "FN (ANY v :(Number | Str | Bool)) -> Str = (\"ok\")\n\
         PRINT (ANY 5)\n\
         PRINT (ANY \"hi\")\n\
         PRINT (ANY true)",
    );
    assert_eq!(
        out.matches("ok").count(),
        3,
        "all three members of a three-member run admit; got {out:?}",
    );
}

/// § 4.3 pin (failure shape): `:(LIST OF Number | Str)` does not chain — the run is not
/// slot/keyword-alternating, so it falls to keyworded dispatch and fails. Users parenthesize.
#[test]
fn unparenthesized_compound_member_fails() {
    let region = run_root_storage();
    let err = run_expect_err(&region, "LET Ty = :(LIST OF Number | Str)");
    assert!(
        err.contains("dispatch failed") || err.contains("no matching function"),
        "an unparenthesized compound member must fail dispatch; got {err}",
    );
}

/// § 4.3 pin (parenthesized success): `:((LIST OF Number) | Str)` resolves — an FN parameter
/// typed with it admits a matching list and a Str, and rejects a bare Number.
#[test]
fn parenthesized_compound_member_succeeds() {
    let region = run_root_storage();
    let out = run_capture(
        &region,
        "FN (TAKE v :((LIST OF Number) | Str)) -> Str = (\"ok\")\n\
         PRINT (TAKE [1 2 3])\n\
         PRINT (TAKE \"hi\")",
    );
    assert_eq!(
        out.matches("ok").count(),
        2,
        "the parenthesized compound union admits a list and a Str; got {out:?}",
    );

    let region = run_root_storage();
    let err = run_expect_err(
        &region,
        "FN (TAKE v :((LIST OF Number) | Str)) -> Str = (\"ok\")\n\
         TAKE 5",
    );
    assert!(
        err.contains("dispatch failed") || err.contains("no matching function"),
        "a bare Number is not a member of `(LIST OF Number) | Str`; got {err}",
    );
}

/// Value-context pin: `(Number | Str)` outside a sigil evaluates to a first-class union *type
/// value* (the `|` builtin runs in value context too). Binding it with LET succeeds; the pin
/// records that the surface accepts the expression rather than erroring.
#[test]
fn value_context_union_builds_a_type_value() {
    let region = run_root_storage();
    // No PRINT — just assert the program runs to completion binding the type value.
    let _ = run_capture(&region, "LET number_or_string = (Number | Str)");
}

// -- Phase 3: a union schema field typed as a sibling variant via `:(Tree Leaf)` -----------

/// AC bullet 5: a field can be typed as a sibling variant of the union being sealed via the
/// qualified sigil `:(Tree Leaf)` — `Tree` is the binder under seal, `Leaf` one of its
/// variants. The union constructs, a `Node` wrapping a nested `Leaf` matches its `Node` arm,
/// and the whole value projects (`PRINT` renders it structurally).
#[test]
fn sibling_variant_sigil_types_a_field() {
    let region = run_root_storage();
    let out = run_capture(
        &region,
        "UNION Tree = (Leaf :Number Node :(Tree Leaf))\n\
         LET tree = (Tree (Node (Tree (Leaf 1))))\n\
         MATCH (tree) -> :Str WITH (Node -> (PRINT \"node\") Leaf -> (PRINT \"leaf\"))",
    );
    assert_eq!(
        out, "node\n",
        "the outer value is a `Node`, so its MATCH arm fires; got {out:?}"
    );

    let region = run_root_storage();
    let projected = run_capture(
        &region,
        "UNION Tree = (Leaf :Number Node :(Tree Leaf))\n\
         LET tree = (Tree (Node (Tree (Leaf 1))))\n\
         PRINT tree",
    );
    assert_eq!(
        projected, "Node(1)\n",
        "the constructed sibling-typed value projects structurally; got {projected:?}"
    );
}

/// A misspelled sibling variant `:(Tree Bogus)` seals against the union's member set via
/// `index_of` and surfaces the standard unsealed-reference error naming the bad tag.
#[test]
fn sibling_variant_typo_references_unsealed_type() {
    let region = run_root_storage();
    let err = run_expect_err(&region, "UNION Tree = (Leaf :Number Node :(Tree Bogus))");
    assert!(
        err.contains("UNION `Tree` schema references unsealed type `Bogus`"),
        "a misspelled sibling variant names the bad tag; got {err}",
    );
}

/// AC bullet 5 (rejection half): a *bare* sibling tag `Node :Leaf` is NOT the qualified sigil
/// and stays an unknown-type error — only `:(Tree Leaf)` reaches a sibling variant.
#[test]
fn bare_sibling_tag_stays_unknown_type_error() {
    let region = run_root_storage();
    let err = run_expect_err(&region, "UNION Tree = (Leaf :Number Node :Leaf)");
    assert!(
        err.contains("unknown type name `Leaf` in UNION schema for `Node`"),
        "a bare sibling tag stays an unknown-type error; got {err}",
    );
}
