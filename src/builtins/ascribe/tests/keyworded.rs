//! The keyworded surface across the ascription barrier: a signature declares dispatch-bucket
//! members with bodyless `FN` heads, and a view publishes exactly those, resolved the way dispatch
//! resolves them.
//!
//! A keyworded member is reached by dispatch rather than by name, so every call here runs inside a
//! `USING <view> SCOPE` window — the window borrows the view scope's binding table, buckets
//! included. What these pin is one property per axis: the selected overload is born coerced (so the
//! call reports view types and refuses source-typed arguments), the selection is dispatch's own
//! most-specific pick, an undeclared bucket is absent, and the whole surface rides the view's
//! self-sig — through `WITH` pins, a nested signature slot, and `:Sig` dispatch admission.

use crate::builtins::test_support::{TestRun, lookup_module};
use crate::machine::KErrorKind;
use crate::machine::model::KObject;
use crate::machine::{program_storage, run_root_storage};

/// The running fixture: one abstract member, the *same* function offered on both lanes — a `VAL
/// pure` slot and a `(PURE _)` bucket member — and a `(HIDE _)` bucket the signature never names.
/// `TAKEELT` accepts the opaque view's own `Elt`, `TAKECARRIER` the source's `Carrier`, so a call's
/// result names which side of the barrier it came out on.
fn box_program() -> &'static str {
    "NEWTYPE Carrier = Number\n\
     SIG Box = ((TYPE Elt) (VAL zero :Elt) (VAL pure :(FN :{x :Elt} -> Elt)) \
     (FN (PURE x :Elt) -> Elt))\n\
     MODULE bx = ((LET Elt = Carrier) (LET zero = (Carrier 0)) \
     (LET pure = FN (PURE x :Carrier) -> Carrier = (x)) \
     (FN (HIDE x :Number) -> Number = (x)))\n\
     LET view = (bx :| Box)\n\
     LET tview = (bx :! Box)\n\
     FN (TAKEELT x :(view.Elt)) -> Number = (3)\n\
     FN (TAKECARRIER x :Carrier) -> Number = (4)"
}

// ---------- The boundary ----------

/// **AC 3.** A declared keyworded member called through an opaque view coerces exactly as a
/// function-typed VAL slot does: the result carries the view's own `Elt`, and a source-typed
/// argument does not cross inward — the wrapper's parameter is the view's type, so the call is a
/// dispatch miss rather than a silent reach into the source's representation.
#[test]
fn a_keyworded_call_through_an_opaque_view_reports_the_views_types() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(box_program());

    let result =
        test_run.run_one(test_run.parse_one("USING view SCOPE (TAKEELT (PURE (view.zero)))"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 3.0),
        "the call's result must satisfy the view's own `Elt`, got {}",
        result.summary(test_run.registries()),
    );
    let error = test_run.run_one_err(test_run.parse_one("USING view SCOPE (PURE (Carrier 1))"));
    assert!(
        matches!(&error.kind, KErrorKind::DispatchFailed { .. }),
        "a source-built argument must not cross the barrier inward, got {error}",
    );
}

/// **AC 3, the two lanes agree.** One function offered as both a `VAL` slot and a bucket member
/// coerces identically on each: the value-lane call and the dispatched call report the same view
/// type, because both are the same coercion plan applied at the same boundary.
#[test]
fn the_value_lane_and_the_bucket_coerce_alike() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(box_program());

    for expression in [
        "TAKEELT (view.pure {x = (view.zero)})",
        "USING view SCOPE (TAKEELT (PURE (view.zero)))",
    ] {
        let result = test_run.run_one(test_run.parse_one(expression));
        assert!(
            matches!(result, KObject::Number(n) if *n == 3.0),
            "`{expression}` must read at the view's `Elt`, got {}",
            result.summary(test_run.registries()),
        );
    }
}

/// **AC 4, the keyworded channel.** A bucket the signature never declares is *absent* from the
/// view: the window over it resolves nothing, while the same window over the source module still
/// dispatches. Pruning shapes the view, not the module the view was taken of.
#[test]
fn an_undeclared_bucket_is_absent_from_the_view() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(box_program());

    for view in ["view", "tview"] {
        let error =
            test_run.run_one_err(test_run.parse_one(&format!("USING {view} SCOPE (HIDE 5)")));
        assert!(
            matches!(&error.kind, KErrorKind::DispatchFailed { .. }),
            "an undeclared bucket must not resolve through `{view}`, got {error}",
        );
    }
    let hidden = test_run.run_one(test_run.parse_one("USING bx SCOPE (HIDE 5)"));
    assert!(
        matches!(hidden, KObject::Number(n) if *n == 5.0),
        "the source module still dispatches the bucket the view drops",
    );
}

/// **AC 4, the transparent mode.** `:!` seeds the signature's abstract members at the source's own
/// bindings, so the coercion plan is the identity and the declared bucket entry replays verbatim:
/// the call reaches the underlying function at concrete types, unwrapped.
#[test]
fn a_keyworded_call_through_a_transparent_view_stays_concrete() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(box_program());

    let result =
        test_run.run_one(test_run.parse_one("USING tview SCOPE (TAKECARRIER (PURE (Carrier 1)))"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 4.0),
        "a transparent view's keyworded call must stay at the source's `Carrier`, got {}",
        result.summary(test_run.registries()),
    );
}

// ---------- Overload sets ----------

/// A signature may declare several members under one key. Each is satisfied and installed
/// independently, so both are callable through the view at their declared types.
#[test]
fn two_declared_overloads_under_one_key_are_both_callable() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Two = ((FN (PURE x :Number) -> Number) (FN (PURE x :Str) -> Str))\n\
         MODULE m2 = ((FN (PURE x :Number) -> Number = (1)) (FN (PURE x :Str) -> Str = (\"s\")))\n\
         LET v2 = (m2 :| Two)",
    );

    let number = test_run.run_one(test_run.parse_one("USING v2 SCOPE (PURE 7)"));
    assert!(
        matches!(number, KObject::Number(n) if *n == 1.0),
        "the `Number` overload must be callable through the view",
    );
    let text = test_run.run_one(test_run.parse_one("USING v2 SCOPE (PURE \"a\")"));
    assert!(
        matches!(text, KObject::KString(s) if *s == "s"),
        "the `Str` overload must be callable through the view",
    );
}

/// **The selection is dispatch's.** Two module overloads satisfy the declared member; the view
/// installs the most specific one and nothing else, so the wider overload — reachable on the source
/// module — is not reachable through the view.
#[test]
fn only_the_most_specific_satisfier_is_installed() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Wide = ((FN (PICK x :Number) -> Number))\n\
         MODULE both = ((FN (PICK x :Number) -> Number = (1)) (FN (PICK x :Any) -> Number = (2)))\n\
         LET wv = (both :| Wide)",
    );

    let picked = test_run.run_one(test_run.parse_one("USING wv SCOPE (PICK 7)"));
    assert!(
        matches!(picked, KObject::Number(n) if *n == 1.0),
        "the `Number` overload is strictly more specific and must be the one selected",
    );
    let error = test_run.run_one_err(test_run.parse_one("USING wv SCOPE (PICK \"s\")"));
    assert!(
        matches!(&error.kind, KErrorKind::DispatchFailed { .. }),
        "the unselected `Any` overload must not ride through the view, got {error}",
    );
}

/// Two satisfiers with no most specific one is the keyworded reading of a dispatch ambiguity, and
/// it fails the ascription — naming the declared head and both candidates.
#[test]
fn incomparable_satisfiers_fail_the_ascription() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Amb = ((FN (COMBINE a :Number AND b :Number) -> Number))\n\
         MODULE amb = ((FN (COMBINE a :Any AND b :Number) -> Number = (1)) \
         (FN (COMBINE a :Number AND b :Any) -> Number = (2)))",
    );
    let error = test_run.run_one_err(test_run.parse_one("(amb :| Amb)"));
    let rendered = error.to_string();
    assert!(
        rendered.contains("with no most specific one")
            && rendered.contains("(COMBINE a :Number AND b :Number) -> Number"),
        "expected an ambiguity naming the declared head, got {error}",
    );
}

/// A bucket that exists but holds nothing satisfying the declared member, and a bucket that is not
/// there at all, are distinct diagnostics — both naming the head the module failed to supply.
#[test]
fn an_unsatisfied_keyworded_member_names_the_head_it_wanted() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Need = ((FN (PURE x :Number) -> Number))\n\
         MODULE wrong = ((FN (PURE x :Str) -> Str = (\"z\")))\n\
         MODULE absent = (LET val = 1)",
    );

    let mismatch = test_run.run_one_err(test_run.parse_one("(wrong :| Need)"));
    assert!(
        mismatch
            .to_string()
            .contains("no overload satisfies keyworded member `(PURE x :Number) -> Number`"),
        "expected the rejected overloads to be reported, got {mismatch}",
    );
    let missing = test_run.run_one_err(test_run.parse_one("(absent :| Need)"));
    assert!(
        missing
            .to_string()
            .contains("missing keyworded member `(PURE x :Number) -> Number`"),
        "expected a missing-member diagnostic, got {missing}",
    );
}

// ---------- Identity: satisfaction, dispatch, self-sigs ----------

/// The keyworded surface is part of signature content, so a `:Sig` parameter slot admits an
/// *unascribed* module by it, and a signature that declares one is strictly more specific than the
/// same signature without it.
#[test]
fn a_keyworded_slot_admits_a_module_and_orders_by_specificity() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Bare = ((VAL tag :Number))\n\
         SIG Need = ((VAL tag :Number) (FN (PURE x :Number) -> Number))\n\
         MODULE has = ((LET tag = 1) (FN (PURE x :Number) -> Number = (1)))\n\
         MODULE lacks = ((LET tag = 2))\n\
         FN (CLASSIFY er :Need) -> Number = (10)\n\
         FN (CLASSIFY er :Bare) -> Number = (20)",
    );

    let specific = test_run.run_one(test_run.parse_one("CLASSIFY has"));
    assert!(
        matches!(specific, KObject::Number(n) if *n == 10.0),
        "a module supplying the keyworded member must pick the signature that declares it",
    );
    let general = test_run.run_one(test_run.parse_one("CLASSIFY lacks"));
    assert!(
        matches!(general, KObject::Number(n) if *n == 20.0),
        "a module without it must fall through to the signature that does not declare it",
    );
}

/// A view's self-sig carries the keyworded surface — re-expressed in the view's own bindings, so
/// the view structurally satisfies the signature it was ascribed to and can be ascribed again.
#[test]
fn a_views_self_sig_carries_its_keyworded_surface() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(box_program());

    let view = lookup_module(scope, "view", test_run.registries());
    let schema = view.self_sig(test_run.types());
    assert_eq!(
        schema.keyworded.len(),
        1,
        "the view publishes exactly the one bucket its signature declares",
    );
    let rendered = view.ktype().name(test_run.registries());
    assert!(
        rendered.contains("(PURE x :"),
        "the rendered signature must show the keyworded member, got {rendered}",
    );

    let again = test_run.run_one(test_run.parse_one("(view :| Box)"));
    assert!(
        matches!(again, KObject::Module(_)),
        "a view must satisfy the very signature it was ascribed to",
    );
}

/// A `WITH` pin substitutes into a keyworded member's types like any other slot: the pinned
/// signature declares the member at the concrete type, so the ascription's coercion plan is the
/// identity there and the call reaches the source's own function.
#[test]
fn a_with_pin_folds_through_a_keyworded_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "NEWTYPE Carrier = Number\n\
         SIG Box = ((TYPE Elt) (FN (PURE x :Elt) -> Elt))\n\
         MODULE bx = ((LET Elt = Carrier) (FN (PURE x :Carrier) -> Carrier = (x)))\n\
         LET pinned = (bx :| (Box WITH {Elt = Carrier}))\n\
         FN (TAKECARRIER x :Carrier) -> Number = (4)",
    );

    let result =
        test_run.run_one(test_run.parse_one("USING pinned SCOPE (TAKECARRIER (PURE (Carrier 3)))"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 4.0),
        "a pinned member crosses no barrier, got {}",
        result.summary(test_run.registries()),
    );
}

/// A nested signature's keyworded member coerces at the *outer* view's bindings: the nested module
/// is born as a coerced view of itself, so its dispatched call reports the outer view's `Elt` on
/// the same terms every other slot shape does — and the nested view is pruned too.
#[test]
fn a_nested_keyworded_member_reports_the_outer_views_types() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "NEWTYPE Carrier = Number\n\
         SIG Inner = ((TYPE Item) (VAL one :Item) (FN (ONE x :Item) -> Item))\n\
         SIG Outer = ((TYPE Elt) (VAL sub :{inner :(Inner WITH {Item = Elt})}))\n\
         MODULE carrier_inner = ((LET Item = Carrier) (LET one = (Carrier 1)) \
         (FN (ONE x :Carrier) -> Carrier = (x)) (FN (EXTRA x :Number) -> Number = (x)))\n\
         MODULE outer_m = ((LET Elt = Carrier) (LET sub = {inner = carrier_inner}))\n\
         LET view = (outer_m :| Outer)\n\
         FN (TAKEELT x :(view.Elt)) -> Number = (3)",
    );

    let result =
        test_run.run_one(test_run.parse_one("USING (view.sub.inner) SCOPE (TAKEELT (ONE (one)))"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 3.0),
        "the nested view's keyworded call must report the outer view's `Elt`, got {}",
        result.summary(test_run.registries()),
    );
    let error = test_run.run_one_err(test_run.parse_one("USING (view.sub.inner) SCOPE (EXTRA 5)"));
    assert!(
        matches!(&error.kind, KErrorKind::DispatchFailed { .. }),
        "the nested signature prunes its own undeclared bucket, got {error}",
    );
}

// ---------- Residence ----------

/// An opaque view minted **inside a per-call frame** and returned from it, with its keyworded
/// member called after that frame is gone.
///
/// A declared keyworded member that coerces is installed as a `coerce_function` wrapper — a
/// callable born in the *source* module's region, sealed into the view scope's dispatch bucket,
/// which the view's own region only points at. It is the same borrow leaf a coerced function-typed
/// VAL slot is, reached on the dispatch lane instead of by name: `Scope::coerce_delivered` claims
/// the pin, so the source region rides in the view region's union bundle and the returned view
/// carries that closure out of its minting frame.
///
/// A Miri audit-slate test: a release claim that dropped either region would free the callable this
/// call walks into, which only tree borrows observes — a normal build reads the freed bytes back
/// intact.
#[test]
fn a_returned_views_keyworded_member_stays_callable() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "NEWTYPE Carrier = Number\n\
         SIG Box = ((TYPE Elt) (VAL zero :Elt) (FN (PURE x :Elt) -> Elt))\n\
         MODULE bx = ((LET Elt = Carrier) (LET zero = (Carrier 0)) \
         (FN (PURE x :Carrier) -> Carrier = (x)))\n\
         FN (MAKEVIEW er :Box) -> Module = (er :| Box)\n\
         LET made = (MAKEVIEW bx)\n\
         FN (TAKEELT x :(made.Elt)) -> Number = (3)",
    );

    let result =
        test_run.run_one(test_run.parse_one("USING made SCOPE (TAKEELT (PURE (made.zero)))"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 3.0),
        "the wrapper sealed into the view's bucket must still run after its minting frame is gone, \
         got {}",
        result.summary(test_run.registries()),
    );
}
