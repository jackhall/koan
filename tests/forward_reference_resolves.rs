//! Integration tests for forward references under the index-gated resolution rule. A binding at
//! lexical index `i` is visible to a consumer at cutoff `c` iff `i < c`, so a later sibling is
//! invisible and no surface parks through to it. Mutual recursion has one spelling — co-declaring
//! the names in a `MODULE` body, whose announced window answers cross-order — and that is the only
//! mechanism that reads a name across source order.
//!
//! Three shapes are pinned here: the value channel's `UnboundName` for a forward `LET` / `FN`
//! reference, the type channel's `ForwardReference` across every type-expression surface
//! (`forward_type_reference_is_uniform_across_surfaces` below), and the backward-reference
//! companion for each, where the placeholder-and-park machinery does resolve.

use std::rc::Rc;

use koan::builtins::test_support::{TestRun, lookup_binding, lookup_type};
use koan::machine::model::KObject;
use koan::machine::{FrameStorage, ProgramStorage, program_storage, run_root_storage};

/// Scaffolding: spin up a fresh run inside `region`, run `source` end-to-end through the
/// scheduler, and hand back the whole run so tests can assert on the root scope's bindings
/// post-run and render type names against the run's own registry.
fn run<'a>(program: &'a ProgramStorage, region: &'a Rc<FrameStorage>, source: &str) -> TestRun<'a> {
    let mut test_run = TestRun::silent(program, region);
    let scope = test_run.scope;
    test_run.enter_source_in(scope, source);
    let _ = test_run.runtime.execute();
    test_run
}

/// Run `source`, returning the first errored top-level slot's error (or `None` if every
/// slot succeeded). Pairs with the `UnboundName`-surfacing tests below.
fn run_collecting_first_err(source: &str) -> Option<koan::machine::KError> {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let ids = test_run.enter_source_watched_in(scope, source);
    if let Err(e) = test_run.runtime.execute() {
        return Some(e);
    }
    for id in ids {
        if let Err(e) = test_run.runtime.edge_result_error(id) {
            return Some(e.clone());
        }
    }
    None
}

/// Forward LET at the same lexical level is `UnboundName`: a value LET's binding sits at
/// its statement's index, which is strictly greater than any earlier consumer's cutoff,
/// and LET is not a nominal-binder carve-out. The eager wrap-resolve / short-circuit
/// passes both surface `UnboundName(z)` from the perspective of `LET y = z`.
#[test]
fn forward_value_let_at_same_level_is_unbound() {
    use koan::machine::KErrorKind;
    let err = run_collecting_first_err("LET y = z\nLET z = 1")
        .expect("forward value LET reference should surface UnboundName");
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(n) if n == "z"),
        "expected UnboundName('z'), got {err}",
    );
}

/// Backward-ref companion: swapping the order so the producer precedes the consumer
/// re-enables the placeholder + park machinery. The bind at the earlier index satisfies
/// `b.idx < c` for the consumer's cutoff, the consumer's wrap-resolve either resolves
/// directly or parks on the live placeholder, and the slot wakes when `LET z` finalizes.
#[test]
fn backward_value_let_at_same_level_resolves() {
    let program = program_storage();
    let region = run_root_storage();
    let scope = run(&program, &region, "LET z = 1\nLET y = z").scope;
    assert!(matches!(lookup_binding(scope, "y"), Some(KObject::Number(n)) if *n == 1.0));
}

/// A backward reference sharing its reduced node with an eager sub-expression — the shape whose
/// speculative pick used to reach the splice walk with a still-finalizing `z` and drop the staged
/// sub on the park. Dispatch now parks on `z`'s producer before any pick, so the eager operand
/// stages exactly once, after the wake.
#[test]
fn backward_reference_beside_eager_operand_resolves() {
    let program = program_storage();
    let region = run_root_storage();
    let scope = run(&program, &region, "LET z = 1\nLET y = (z + (1 + 2))").scope;
    assert!(matches!(lookup_binding(scope, "y"), Some(KObject::Number(n)) if *n == 4.0));
}

/// MODULE body: forward LET reference under the body's child scope is `UnboundName` for
/// the same reason as the top-level case — `LET y = x` sits at body-scope index `i`,
/// `LET x = 1` at index `i+1`, and the gate hides the producer from the consumer.
#[test]
fn module_body_forward_value_reference_is_unbound() {
    use koan::machine::KErrorKind;
    let err = run_collecting_first_err("MODULE some_module = ((LET y = x) (LET x = 1))")
        .expect("forward value LET in module body should surface UnboundName");
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(n) if n == "x"),
        "expected UnboundName('x'), got {err}",
    );
}

/// Module-body backward-ref companion: with the producer ahead of the consumer the
/// resolution succeeds normally.
#[test]
fn module_body_backward_value_reference_resolves() {
    let program = program_storage();
    let region = run_root_storage();
    let scope = run(
        &program,
        &region,
        "MODULE some_module = ((LET x = 1) (LET y = x))",
    )
    .scope;
    // A module is a value — the `&Module` rides the Object-arm value in `data`.
    let m = match lookup_binding(scope, "some_module") {
        Some(KObject::Module(m)) => *m,
        _ => panic!("some_module should bind a module value"),
    };
    let y = lookup_binding(m.child_scope(), "y");
    assert!(matches!(y, Some(KObject::Number(n)) if *n == 1.0));
}

/// Multi-name in one expression: a single value-slot expression whose RHS references two
/// not-yet-bound names is `UnboundName` (the first unbound argument wins on the error
/// surface). Pinned to confirm the wrap-slot pass propagates the gate uniformly across
/// every bare-name part rather than only the first.
#[test]
fn multi_name_forward_reference_is_unbound() {
    use koan::machine::KErrorKind;
    let err = run_collecting_first_err(
        "FN (ADD a :Number BY b :Number) -> Number = (b)\n\
         LET out = (ADD aa BY bb)\n\
         LET aa = 1\n\
         LET bb = 2",
    )
    .expect("forward refs in FN call should surface UnboundName");
    assert!(
        matches!(
            &err.kind,
            KErrorKind::UnboundName(_) | KErrorKind::DispatchFailed { .. }
        ),
        "expected UnboundName or DispatchFailed, got {err}",
    );
}

/// Backward-ref companion: ordering the LET binders ahead of the consumer makes both
/// references visible under the gate and the call resolves normally.
#[test]
fn multi_name_backward_reference_resolves() {
    let program = program_storage();
    let region = run_root_storage();
    let scope = run(
        &program,
        &region,
        "FN (ADD a :Number BY b :Number) -> Number = (b)\n\
         LET aa = 1\n\
         LET bb = 2\n\
         LET out = (ADD aa BY bb)",
    )
    .scope;
    assert!(matches!(lookup_binding(scope, "out"), Some(KObject::Number(n)) if *n == 2.0));
}

/// Forward function-name reference (call_by_name): a later-sibling `FN DOUBLE` is
/// value-style gated (FN is not a nominal-binder carve-out), so `LET out = (DOUBLE 5)`
/// at an earlier index cannot see the binder. The dispatch's per-scope
/// `Bindings::lookup_function` filters by the same visibility predicate and
/// surfaces `DispatchFailed` (or `UnboundName` depending on how far the dispatch
/// reaches before the gate fires).
#[test]
fn forward_call_by_name_is_dispatch_failure() {
    use koan::machine::KErrorKind;
    let err = run_collecting_first_err(
        "LET out = (DOUBLE 5)\n\
         FN (DOUBLE x :Number) -> Number = (x)",
    )
    .expect("forward FN call should surface a dispatch error");
    assert!(
        matches!(
            &err.kind,
            KErrorKind::DispatchFailed { .. } | KErrorKind::UnboundName(_)
        ),
        "expected DispatchFailed or UnboundName, got {err}",
    );
}

/// Forward struct-name reference (ATTR `s.x`): the value LET `p` is invisible to the
/// earlier consumer under the gate even though `STRUCT Pt` itself is a nominal-binder
/// carve-out. The carve-out covers the type identity, not the value-side LET that
/// constructs an instance.
#[test]
fn forward_attr_lookup_through_value_let_is_unbound() {
    use koan::machine::KErrorKind;
    let err = run_collecting_first_err(
        "LET v = p.x\n\
         NEWTYPE Pt = :{x :Number, y :Number}\n\
         LET p = (Pt {x = 7, y = 9})",
    )
    .expect("forward ATTR on value LET should surface UnboundName");
    assert!(
        matches!(
            &err.kind,
            KErrorKind::UnboundName(_) | KErrorKind::DispatchFailed { .. }
        ),
        "expected UnboundName or DispatchFailed, got {err}",
    );
}

/// Backward-ref companion: the value LET precedes its consumer. The STRUCT identity is
/// already a nominal binder visible to siblings; the value-side carrier `p` sits at an
/// earlier index than `v`, so both references resolve.
#[test]
fn backward_attr_lookup_resolves_after_struct_binding() {
    let program = program_storage();
    let region = run_root_storage();
    let scope = run(
        &program,
        &region,
        "NEWTYPE Pt = :{x :Number, y :Number}\n\
         LET p = (Pt {x = 7, y = 9})\n\
         LET v = p.x",
    )
    .scope;
    assert!(matches!(lookup_binding(scope, "v"), Some(KObject::Number(n)) if *n == 7.0));
}

/// LET-as-type-alias is gated exactly as the value side is: `LET Ty = Un` followed by
/// `LET Un = Number` hides `Un` from `Ty`'s consumer under the strict cutoff, so the elaborator
/// refuses rather than resolving forward through the placeholder. The refusal is the position
/// error every type-expression surface reports — the name exists, one statement further down.
#[test]
fn forward_let_type_alias_is_a_forward_reference() {
    use koan::machine::KErrorKind;
    let err = run_collecting_first_err("LET Ty = Un\nLET Un = Number")
        .expect("forward LET type alias should surface ForwardReference");
    assert!(
        matches!(&err.kind, KErrorKind::ForwardReference { name, .. } if name == "Un"),
        "expected ForwardReference naming Un, got {err}",
    );
}

/// Backward-ref companion for the type-alias case: ordering the alias's target first
/// makes the LET-Ty resolve to `Number` via the gated `resolve_type_with_chain` walk.
#[test]
fn backward_let_type_alias_resolves_to_number() {
    use koan::machine::model::KType;
    let program = program_storage();
    let region = run_root_storage();
    let scope = run(&program, &region, "LET Un = Number\nLET Ty = Un").scope;
    assert!(
        lookup_type(scope, "Ty") == Some(KType::NUMBER),
        "expected Ty to resolve to Number, got {:?}",
        lookup_type(scope, "Ty"),
    );
}

/// Module-qualified type name in LET-RHS position. `LET MyT = mo.Ty` where `mo` is a
/// module exporting `Ty = Number`. MODULE is a nominal-binder carve-out, so `mo` is
/// visible to its sibling consumer regardless of source order; the inner module-body
/// LET runs once `mo` finalizes, the ATTR walker reads its `bindings.types`, and the
/// LET-Type-LHS overload routes the carrier through `register_type` on the parent
/// scope.
#[test]
fn let_alias_via_module_qualified_type_resolves() {
    use koan::machine::model::KType;
    let program = program_storage();
    let region = run_root_storage();
    let test_run = run(
        &program,
        &region,
        "MODULE mo = ((LET Ty = Number))\nLET MyT = mo.Ty",
    );
    let scope = test_run.scope;
    assert!(
        lookup_type(scope, "MyT") == Some(KType::NUMBER),
        "expected MyT to resolve to Number via mo.Ty, got {:?}",
        lookup_type(scope, "MyT").map(|t| t.name(test_run.registries())),
    );
}

/// Module-qualified type name in a `:(LIST OF)`-style type frame. `:(LIST OF mo.Ty)` rides
/// the existing `Deferred` path in `resolve_dispatch`. MODULE mo is a nominal-binder
/// carve-out so it's visible to the sibling `LET MyList`.
#[test]
fn type_frame_with_module_qualified_element_resolves() {
    let program = program_storage();
    let region = run_root_storage();
    let scope = run(
        &program,
        &region,
        "MODULE mo = ((LET Ty = Number))\n\
         LET MyList = :(LIST OF mo.Ty)",
    )
    .scope;
    assert!(
        lookup_type(scope, "MyList").is_some(),
        "expected MyList to bind via :(LIST OF mo.Ty)",
    );
}

/// Chained module-qualified type name `outer.inner.T`. Both modules are nominal-binder
/// carve-outs and visible regardless of source order.
#[test]
fn chained_module_qualified_type_resolves() {
    use koan::machine::model::KType;
    let program = program_storage();
    let region = run_root_storage();
    let scope = run(
        &program,
        &region,
        "MODULE outer = ((MODULE inner = ((LET Ty = Number))))\n\
         LET MyT = outer.inner.Ty",
    )
    .scope;
    assert!(
        lookup_type(scope, "MyT") == Some(KType::NUMBER),
        "expected MyT to resolve to Number via outer.inner.Ty, got {:?}",
        lookup_type(scope, "MyT"),
    );
}

/// Producer-error propagation: when a consumer references a still-pending producer (now
/// only possible across nominal-binder carve-outs or backward references), an error at
/// the producer propagates through. With value LETs gated, the simplest backward shape
/// — `LET x = (UNDEFINED_FN)` first, then `LET y = (x)` — keeps the visibility test
/// satisfied while exercising the error-propagation rails.
#[test]
fn producer_error_propagates_to_parked_consumer() {
    use koan::machine::KErrorKind;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let ids = test_run.dispatch_source_in(
        scope,
        "LET x = (UNDEFINED_FN)\n\
         LET y = (x)",
    );
    test_run
        .runtime
        .execute()
        .expect("a producer error routes into the slot, not a fatal execute abort");
    let err = test_run
        .runtime
        .edge_result_error(ids[0])
        .expect_err("execute should surface UNDEFINED_FN's dispatch failure");
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed for UNDEFINED_FN, got {err}",
    );
    assert!(
        test_run.runtime.edge_result_error(ids[1]).is_err(),
        "y must inherit its dependency's error",
    );
}

/// Self-referential binding guard: `LET x = x` is the degenerate "same-lexical-index"
/// case — the producer's `x` placeholder sits at index `i`, the RHS consumer reads at
/// cutoff `i`, and the strict `b.idx < c` predicate makes the binding invisible. The
/// consumer surfaces `UnboundName`. (The pre-gate path emitted `SchedulerDeadlock` via
/// `would_create_cycle`; the gate now intercepts the self-reference earlier.)
#[test]
fn self_referential_binding_is_unbound() {
    use koan::machine::KErrorKind;
    use koan::machine::interpret_with_writer;
    let err = interpret_with_writer("LET x = x", Box::new(std::io::sink()))
        .expect_err("self-reference should error");
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(name) if name == "x"),
        "expected UnboundName('x'), got {err}",
    );
}

// ---------- one forward reference, every type-expression surface ----------

/// The name each surface's program declares one statement below its consumer.
fn forward_reference_name(source: &str) -> String {
    use koan::machine::KErrorKind;
    let err = run_collecting_first_err(source)
        .unwrap_or_else(|| panic!("a forward type reference must error:\n{source}"));
    match &err.kind {
        KErrorKind::ForwardReference { name, .. } => name.clone(),
        _ => panic!("expected ForwardReference, got {err}\nfor:\n{source}"),
    }
}

/// One program shape — a consumer naming a type declared on the next line — walked across every
/// sigiled type surface, each answering with the same error kind. The two `NEWTYPE` reprs are the
/// pair that reaches the raiser by two routes, the dispatch lane's role-labeled slot and the
/// body's own resolve-or-await; both land on this one kind.
#[test]
fn forward_type_reference_is_uniform_across_surfaces() {
    for (surface, source) in [
        (
            "bare leaf annotation",
            "LET t = :Ordered\nNEWTYPE Ordered = Number",
        ),
        (
            "LIST OF element",
            "LET t = :(LIST OF Ordered)\nNEWTYPE Ordered = Number",
        ),
        (
            "MAP value",
            "LET t = :(MAP Str -> Ordered)\nNEWTYPE Ordered = Number",
        ),
        (
            "record-type field",
            "LET t = :{Ty :Ordered}\nNEWTYPE Ordered = Number",
        ),
        (
            "FN parameter list",
            "LET t = :(FN :{x :Ordered} -> Number)\nNEWTYPE Ordered = Number",
        ),
    ] {
        assert_eq!(
            forward_reference_name(source),
            "Ordered",
            "{surface} must name the late declaration",
        );
    }
    for (surface, source) in [
        (
            "NEWTYPE record repr",
            "NEWTYPE Box = :{v :Later}\nNEWTYPE Later = Number",
        ),
        (
            "NEWTYPE bare-leaf repr",
            "NEWTYPE Box = Later\nNEWTYPE Later = Number",
        ),
    ] {
        assert_eq!(
            forward_reference_name(source),
            "Later",
            "{surface} must name the late declaration",
        );
    }
}

/// The contexted surfaces frame the position with the slot the writer wrote, so one kind still
/// points at one place. `NEWTYPE Box = Later` is the row that pins the bare-leaf repr to a single
/// voice: its render is the lane's role noun, with no separate wording from the body.
#[test]
fn forward_reference_renders_its_surface_context() {
    for (source, expected) in [
        (
            "LET t = :{Ty :Ordered}\nNEWTYPE Ordered = Number",
            "`Ordered` is used in record fields for `Ty` before being declared",
        ),
        (
            "NEWTYPE Box = :{v :Later}\nNEWTYPE Later = Number",
            "`Later` is used in NEWTYPE record repr for `v` before being declared",
        ),
        (
            "NEWTYPE Box = Later\nNEWTYPE Later = Number",
            "`Later` is used in NEWTYPE repr before being declared",
        ),
        (
            "LET t = :Ordered\nNEWTYPE Ordered = Number",
            "`Ordered` is used before being declared",
        ),
    ] {
        let err = run_collecting_first_err(source).expect("a forward type reference must error");
        let rendered = format!("{err}");
        assert!(
            rendered.starts_with(expected),
            "expected a render opening `{expected}`, got {rendered}",
        );
        assert!(
            rendered.contains("group mutually recursive types in a MODULE body"),
            "the fix hint names the co-declaration spelling, got {rendered}",
        );
    }
}

/// The split the diagnostic exists to draw: a name declared nowhere is *not* a position error. A
/// contexted surface renders the unified never-declared wording; a context-free one keeps
/// `UnboundName`, symmetric with the value channel.
#[test]
fn a_never_declared_type_name_is_not_a_forward_reference() {
    use koan::machine::KErrorKind;
    let contexted =
        run_collecting_first_err("LET t = :{Ty :Nope}").expect("an unknown field type must error");
    assert!(
        matches!(&contexted.kind, KErrorKind::ShapeError(msg)
            if msg == "unknown type name `Nope` in record fields for `Ty`"),
        "expected the never-declared render, got {contexted}",
    );
    let bare = run_collecting_first_err("LET t = :Nope").expect("an unknown leaf type must error");
    assert!(
        matches!(&bare.kind, KErrorKind::UnboundName(name) if name == "Nope"),
        "expected UnboundName, got {bare}",
    );
}

/// A self-reference is neither: `LET Ty = Ty` names the binding its own statement declares, so the
/// declared-later hint would name a fix it does not have. The classifier separates the two by
/// re-probing one statement forward — the same-statement claim answers there, a later sibling's
/// does not.
#[test]
fn a_self_referential_type_alias_is_not_a_forward_reference() {
    use koan::machine::KErrorKind;
    let err = run_collecting_first_err("LET Ty = Ty").expect("a self-reference must error");
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(name) if name == "Ty"),
        "expected UnboundName for a self-reference, got {err}",
    );
}
