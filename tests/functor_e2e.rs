//! End-to-end smoke test for a functor — a module-returning function — mirroring the
//! `MakeSet` shape from [design/typing/functors.md](../design/typing/functors.md).
//!
//! The test exercises the full pipeline:
//! 1. **Define** — `FN (MAKESET er :Ordered) -> Module = (MODULE generated = ...)` registers
//!    an ordinary keyworded FN in the dispatch table.
//! 2. **Apply** — `LET int_set = (MAKESET int_ord)` invokes it with a signature-typed module
//!    argument; the per-call type-side install registers `er`'s type-language identity into
//!    the body's child scope.
//! 3. **Produce** — the body's `MODULE generated = (...)` returns a module value that the LET
//!    RHS binds as `int_set`, on the value channel.
//!
//! Mirror of the dispatch/type-checking already covered by the smaller-scope tests in
//! `src/builtins/fn_def/tests/functor/`; this is the cross-pipeline check that FN,
//! MODULE-returning bodies, and LET compose under a single Scheduler run.

use std::cell::RefCell;
use std::rc::Rc;

use koan::builtins::test_support::{SharedBuf, TestRun};
use koan::machine::model::{KObject, SignatureElement, TypeNode};
use koan::machine::{run_root_storage, FrameStorage, KFunction, Scope};
use koan::parse::parse;

/// Run `src` to completion and hand back the whole run — the seeded scope the assertions
/// read bindings from, plus the run frame's registry type names render against.
fn run<'a>(region: &'a Rc<FrameStorage>, src: &str) -> TestRun<'a> {
    let mut test_run = TestRun::silent(region);
    let scope = test_run.scope;
    let exprs = parse(src).expect("parse should succeed");
    for e in exprs {
        test_run.runtime.dispatch_in_scope(e, scope);
    }
    test_run
        .runtime
        .execute()
        .expect("scheduler should run to completion");
    test_run
}

/// Walk the dispatch table for an FN overload whose first keyword matches `keyword`.
/// Inline copy of `builtins::test_support::lookup_fn` (which is `#[cfg(test)]`-gated); the
/// integration crate sees neither the helper nor the raw `Bindings::functions` view
/// (gated `#[cfg(test)]`), so we go through the public `Bindings::iter_functions`
/// value-yielding iterator.
fn lookup_fn<'a>(scope: &'a Scope<'a>, keyword: &str) -> &'a KFunction<'a> {
    for (_, bucket) in scope.bindings().iter_functions() {
        for f in bucket {
            let first_kw = f.signature.elements.iter().find_map(|e| match e {
                SignatureElement::Keyword(s) => Some(s.as_str()),
                _ => None,
            });
            if first_kw == Some(keyword) {
                return f;
            }
        }
    }
    panic!("no FN overload registered under `{keyword}`");
}

/// End-to-end MakeSet smoke. The FN takes an `Ordered`-satisfying module (`int_ord`),
/// produces a module value carrying a value-side `tag` member, and the LET assigns the
/// result to `int_set`.
#[test]
fn functor_e2e_makeset_produces_module() {
    let region = run_root_storage();
    // `(MAKESET int_ord)` works directly when `int_ord`'s carrier carries the declared
    // signature in its `compatible_sigs` set. The LET partition guard
    // (design/typing/elaboration.md § Binding-map partition) forces the ascription rebind to
    // use a Type-classified identifier (`LET int_ord = (int_ord_base :! Ordered)`) so the
    // module/signature carrier never rides a value-classified alias; the dispatch admission
    // then consults `compatible_sigs` at the signature-typed slot, so no parens-wrap or
    // ascription-view workaround is required at the call site.
    let test_run = run(
        &region,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord_base = ((LET compare = 7))\n\
         LET int_ord = (int_ord_base :! Ordered)\n\
         FN (MAKESET er :Ordered) -> Module = \
            (MODULE generated = ((LET tag = 0)))\n\
         LET int_set = (MAKESET int_ord)",
    );
    let scope = test_run.scope;
    // `MAKESET` registered as a KFunction in the dispatch table (FN writes to `functions`,
    // not `data`), and its `ktype()` is an ordinary function type.
    let makeset = lookup_fn(scope, "MAKESET");
    assert!(
        matches!(
            test_run.types.node(KObject::KFunction(makeset).ktype()),
            TypeNode::KFunction { .. }
        ),
        "a module-returning FN types as a function type",
    );
    // `int_set` landed as a module value: a module is a value, so LET binds it on the value
    // channel (`bindings.data`) under its Type-token name and nothing lands in `types`.
    assert!(
        scope.resolve_type("int_set").is_none(),
        "a module is a value — nothing lands in `types`",
    );
    let m = match scope.lookup("int_set") {
        Some(KObject::Module(module)) => *module,
        _ => panic!("int_set should bind a module value in data"),
    };
    // The body's `(LET tag = 0)` lifted into the result module's child scope — verifies the
    // per-call body actually ran and the produced module carries its declared member.
    let tag = m.child_scope().lookup("tag");
    assert!(
        matches!(tag, Some(KObject::Number(n)) if *n == 0.0),
        "int_set's `tag` member should be 0, got {:?}",
        tag.map(|o| o.ktype()),
    );
    // The module value's `ktype()` is its principal signature. Ruling 12: a signature renders
    // structurally (`SIG (tag: Number)`), not by the module name — the type a `:Signature` slot
    // matches it against.
    assert_eq!(
        KObject::Module(m).ktype().name(&test_run.types),
        "SIG (tag: Number)",
        "a module value is typed by its self-sig",
    );
}

/// A `LET`-bound module-returning FN is applied through the ordinary value-side call: the
/// one-record-literal named-args form `(apply_it {x = 5})` fills the parameter by name and
/// yields the module the body produces. The name binds value-side; nothing lands in
/// `bindings.types`.
#[test]
fn let_bound_fn_applied_by_named_args_yields_module() {
    let region = run_root_storage();
    let test_run = run(
        &region,
        "LET apply_it = (FN (APPLYIT x :Number) -> Module = \
            (MODULE inner = ((LET tag = x))))\n\
         LET got = (apply_it {x = 5})",
    );
    let scope = test_run.scope;
    assert!(
        matches!(scope.lookup("apply_it"), Some(KObject::KFunction(_))),
        "apply_it binds value-side as a KFunction",
    );
    assert!(
        scope.resolve_type("apply_it").is_none(),
        "`bindings.types` holds no callable value",
    );
    // Applying it produced a module that the outer LET bound as `got`.
    let m = match scope.lookup("got") {
        Some(KObject::Module(module)) => *module,
        _ => panic!("got should be the module value produced by applying apply_it"),
    };
    let tag = m.child_scope().lookup("tag");
    assert!(
        matches!(tag, Some(KObject::Number(n)) if *n == 5.0),
        "the applied body should set `tag = 5` from the named arg, got {:?}",
        tag.map(|o| o.ktype()),
    );
}

/// Run `src` through the real interpreter entry point (which reads each top-level
/// node's result, so a LET-RHS bind error surfaces), expecting an error.
///
/// The `run` helper above uses `dispatch_in_scope` + `execute()` to inspect scope
/// bindings; that path stores a node's error without returning it from `execute()`,
/// so it can't witness a bind-time `TypeMismatch`. `interpret_with_writer` mirrors
/// the CLI: `enter_block` the top level, then propagate the first node error.
fn run_expect_err(src: &str) -> String {
    let sink: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    match koan::machine::interpret_with_writer(src, Box::new(SharedBuf(sink))) {
        Ok(()) => panic!("expected an error, got success"),
        Err(e) => e.to_string(),
    }
}

/// A `:Signature`-typed parameter, filled by name with a *satisfying* module, applies through
/// the named-argument surface.
///
/// `int_ord` is a module whose `compatible_sigs` carries `Ordered` (installed by the
/// `:! Ordered` ascription). The named-arg call `(make_set {base = int_ord})` reconstructs
/// the positional call `[MKSET, int_ord]`; the post-pick tail resolves the bare-name `base`
/// slot by sub-Dispatch to its module carrier, so `bind`'s `accepts_part` consults
/// `compatible_sigs` — the same satisfaction check the keyword-led `(MAKESET int_ord)` form
/// uses — and admits it. The body's `(LET tag = 0)` then runs, producing the module bound as
/// `got`.
#[test]
fn signature_param_satisfied_via_named_args() {
    let region = run_root_storage();
    let test_run = run(
        &region,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord_base = ((LET compare = 7))\n\
         LET int_ord = (int_ord_base :! Ordered)\n\
         LET make_set = (FN (MKSET base :Ordered) -> Module = \
            (MODULE inner = ((LET tag = 0))))\n\
         LET got = (make_set {base = int_ord})",
    );
    let scope = test_run.scope;
    let m = match scope.lookup("got") {
        Some(KObject::Module(module)) => *module,
        _ => panic!("got should be the module value produced by applying make_set"),
    };
    let tag = m.child_scope().lookup("tag");
    assert!(
        matches!(tag, Some(KObject::Number(n)) if *n == 0.0),
        "the applied body should set `tag = 0`, got {:?}",
        tag.map(|o| o.ktype()),
    );
}

/// Dual of the test above: a module that does *not* satisfy the slot signature, passed by
/// name, is a terminal `TypeMismatch`. The head uniquely picks `make_set` (no overload bucket
/// to fall through to), so a non-satisfying arg is a hard error rather than a dispatch
/// non-match. Pins that the named-arg path runs the real satisfaction check — it does not
/// blanket-admit any module into a `:Signature` slot.
#[test]
fn signature_param_unsatisfied_via_named_args_errors() {
    let err = run_expect_err(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE plain = ((LET other = 1))\n\
         LET make_set = (FN (MKSET base :Ordered) -> Module = \
            (MODULE inner = ((LET tag = 0))))\n\
         LET got = (make_set {base = plain})",
    );
    // Ruling 12: the slot signature renders structurally, so the mismatch names
    // `SIG (compare: Number)` rather than "Ordered".
    assert!(
        err.contains("type mismatch") && err.contains("SIG (compare: Number)"),
        "non-satisfying module by name should be a TypeMismatch against the signature, got: {err}",
    );
}
