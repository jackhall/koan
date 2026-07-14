//! End-to-end smoke test for the `FUNCTOR` binder, mirroring the `MakeSet`
//! shape from [design/typing/functors.md](../design/typing/functors.md).
//!
//! The test exercises the full shipped FUNCTOR pipeline:
//! 1. **Define** — `FUNCTOR (MAKESET er :Ordered) -> Set = (MODULE generated
//!    = ...)` registers a KFunction with `is_functor: true`.
//! 2. **Apply** — `LET int_set = (MAKESET int_ord)` invokes the functor with a
//!    signature-typed module argument; per-call type-side install registers
//!    `er`'s type-language identity into the body's child scope.
//! 3. **Produce** — the body's `MODULE generated = (...)` returns a module value
//!    that the LET RHS binds as `int_set`. The Stage-5 allowlist routes the
//!    `KTypeValue(Module)` carrier to a single type-side `register_type` install,
//!    so `int_set` lands only in `bindings.types`.
//!
//! Mirror of the dispatch/type-checking already covered by the smaller-scope
//! tests in `src/builtins/fn_def/tests/functor/` and
//! `src/builtins/functor_def/tests/`; this is the cross-pipeline check that
//! FUNCTOR, FN-via-MODULE bodies, and the LET allowlist compose under a single
//! Scheduler run.

use std::cell::RefCell;
use std::rc::Rc;

use koan::builtins::default_scope;
use koan::machine::model::{KObject, KType, SignatureElement};
use koan::machine::{run_root_storage, FrameStorage, KFunction, KoanRuntime, Scope};
use koan::parse::parse;

/// Shared `Write` adapter — every test here drops PRINT output (the smoke
/// asserts on bindings, not stdout). Local copy avoids depending on the
/// `koan::builtins::test_support` module, which is `pub(crate)`.
struct SharedBuf(Rc<RefCell<Vec<u8>>>);
impl std::io::Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn run<'a>(region: &'a Rc<FrameStorage>, src: &str) -> &'a Scope<'a> {
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = default_scope(region, Box::new(SharedBuf(captured)));
    let exprs = parse(src).expect("parse should succeed");
    let mut runtime = KoanRuntime::new();
    for e in exprs {
        runtime.dispatch_in_scope(e, scope);
    }
    runtime
        .execute()
        .expect("scheduler should run to completion");
    scope
}

/// Walk the dispatch table for an FN / FUNCTOR overload whose first keyword
/// matches `keyword`. Inline copy of `builtins::test_support::lookup_fn`
/// (which is `pub(crate)`); the integration crate sees neither the helper
/// nor the raw `Bindings::functions` view (gated `#[cfg(test)]`), so we go
/// through the public `Bindings::iter_functions` value-yielding iterator.
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
    panic!("no FN/FUNCTOR overload registered under `{keyword}`");
}

/// End-to-end MakeSet smoke. The functor takes an `Ordered`-satisfying
/// module (`int_ord`), produces a module value carrying a value-side `tag`
/// member, and the LET assigns the result to `int_set`. The shape pulls
/// every Stage 0-6 piece of the FUNCTOR work into a single Scheduler run:
///
/// - Stage 0/2: `KType::KFunctor` projection on the functor carrier.
/// - Stage 3: FUNCTOR binder admits the `Ordered → Module` shape.
/// - Stage 4: cross-arm wall is dormant here (no FN/FUNCTOR slot mix); the
///   test pins the happy-path so the wall isn't exercised.
/// - Stage 5: LET allowlist admits both the Module-valued `int_ord` ascription
///   and the produced `int_set` module.
#[test]
fn functor_binder_e2e_makeset_produces_module() {
    let region = run_root_storage();
    // The natural FUNCTOR application form: `(MAKESET int_ord)` works directly
    // when `int_ord`'s carrier carries the declared signature in its
    // `compatible_sigs` set. The LET partition guard
    // (design/typing/elaboration.md § Binding-map partition) forces the
    // ascription rebind to use a Type-classified identifier
    // (`LET int_ord = (int_ord_base :! Ordered)`) so the module/signature
    // carrier never rides a value-classified alias; the dispatch admission then
    // consults `compatible_sigs` at the signature-typed slot, so no parens-wrap
    // or ascription-view workaround is required at the call site.
    let scope = run(
        &region,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord_base = ((LET compare = 7))\n\
         LET int_ord = (int_ord_base :! Ordered)\n\
         FUNCTOR (MAKESET er :Ordered) -> Module = \
            (MODULE generated = ((LET tag = 0)))\n\
         LET int_set = (MAKESET int_ord)",
    );
    // `MAKESET` registered as a FUNCTOR-flagged KFunction in the dispatch
    // table (FN / FUNCTOR write to `functions`, not `data`).
    let makeset = lookup_fn(scope, "MAKESET");
    assert!(
        makeset.is_functor,
        "MAKESET must carry is_functor: true (Stage-2 / Stage-3 plumbing)",
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
    // The functor body's `(LET tag = 0)` lifted into the result module's
    // child scope — verifies the per-call body actually ran and the
    // produced module carries its declared member.
    let tag = m.child_scope().lookup("tag");
    assert!(
        matches!(tag, Some(KObject::Number(n)) if *n == 0.0),
        "int_set's `tag` member should be 0, got {:?}",
        tag.map(|o| o.ktype()),
    );
    // The module value's `ktype()` is its principal signature, whose name renders as the
    // module path — the type a `:Signature` slot matches it against.
    assert_eq!(
        KObject::Module(m).ktype().name(),
        m.path,
        "a module value is typed by its self-sig",
    );
}

/// Caveat-2 closer: a `LET`-bound functor name is applied through the
/// `:(MyFunctor {…})` sigil surface and yields a module — end-to-end.
///
/// `LET ApplyIt = (FUNCTOR (APPLYIT x :Number) -> Module = …)` binds the functor
/// *type-side* (`bindings.types[ApplyIt] = KType::KFunctor { body: Some }`, nothing
/// in `bindings.data`). The single-part `:(ApplyIt {x = 5})` sigil routes through
/// the `SigiledTypeExpr` fast lane → a `Type`-head `TypeCall` of `ApplyIt {x = 5}`.
/// `resolve_type_with_chain(ApplyIt)` returns the body-bearing functor type, so the
/// `Function` arm calls it and the body's `MODULE inner = …` produces a module the
/// outer `LET got = …` binds.
///
/// The named-arg surface keys on the functor's param name, which must be a bare
/// lowercase identifier to fill a record-literal field — hence a `Number` param
/// `x`. Satisfying a `:Ordered`-typed param through this named-arg path is
/// pinned by `functor_signature_param_satisfied_via_named_sigil` below.
#[test]
fn let_bound_functor_applied_via_sigil_yields_module() {
    let region = run_root_storage();
    let scope = run(
        &region,
        "LET ApplyIt = (FUNCTOR (APPLYIT x :Number) -> Module = \
            (MODULE inner = ((LET tag = x))))\n\
         LET got = :(ApplyIt {x = 5})",
    );
    // `ApplyIt` is type-bound (a functor name lands in `bindings.types`), never in
    // `bindings.data`, and carries its callable body.
    assert!(
        scope.lookup("ApplyIt").is_none(),
        "ApplyIt must NOT be value-bound — a functor name registers type-side",
    );
    assert!(
        matches!(
            scope.resolve_type("ApplyIt"),
            Some(KType::KFunctor { body: Some(_), .. })
        ),
        "ApplyIt should resolve type-side to a body-bearing KFunctor",
    );
    // Applying the functor produced a module that the outer LET bound as `got`.
    let m = match scope.lookup("got") {
        Some(KObject::Module(module)) => *module,
        _ => panic!("got should be the module value produced by applying ApplyIt"),
    };
    let tag = m.child_scope().lookup("tag");
    assert!(
        matches!(tag, Some(KObject::Number(n)) if *n == 5.0),
        "applied functor's body should set `tag = 5` from the named arg, got {:?}",
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

/// Closes `roadmap/named-arg-signature-satisfaction.md`: a `:Signature`-typed
/// functor param, filled by name with a *satisfying* module, applies through the
/// named-argument sigil surface.
///
/// `int_ord` is a module whose `compatible_sigs` carries `Ordered` (installed by
/// the `:! Ordered` ascription). The named-arg call `:(MakeSet {base = int_ord})`
/// reconstructs the positional call `[MKSET, int_ord]`; the post-pick tail resolves
/// the bare-name `base` slot by sub-Dispatch to its module carrier, so `bind`'s
/// `accepts_part` consults `compatible_sigs` — the same satisfaction check the
/// keyword-led `(MAKESET int_ord)` form uses — and admits it. The functor body's
/// `(LET tag = 0)` then runs, producing the module bound as `got`.
#[test]
fn functor_signature_param_satisfied_via_named_sigil() {
    let region = run_root_storage();
    let scope = run(
        &region,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord_base = ((LET compare = 7))\n\
         LET int_ord = (int_ord_base :! Ordered)\n\
         LET MakeSet = (FUNCTOR (MKSET base :Ordered) -> Module = \
            (MODULE inner = ((LET tag = 0))))\n\
         LET got = :(MakeSet {base = int_ord})",
    );
    let m = match scope.lookup("got") {
        Some(KObject::Module(module)) => *module,
        _ => panic!("got should be the module value produced by applying MakeSet"),
    };
    let tag = m.child_scope().lookup("tag");
    assert!(
        matches!(tag, Some(KObject::Number(n)) if *n == 0.0),
        "applied functor's body should set `tag = 0`, got {:?}",
        tag.map(|o| o.ktype()),
    );
}

/// Dual of the test above: a module that does *not* satisfy the slot signature,
/// passed by name, is a terminal `TypeMismatch`. The head uniquely picks `MakeSet`
/// (no overload bucket to fall through to), so a non-satisfying arg is a hard error
/// rather than a dispatch non-match. Pins that the named-arg path runs the real
/// satisfaction check — it does not blanket-admit any module into a `:Signature` slot.
#[test]
fn functor_signature_param_unsatisfied_via_named_sigil_errors() {
    let err = run_expect_err(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE plain = ((LET other = 1))\n\
         LET MakeSet = (FUNCTOR (MKSET base :Ordered) -> Module = \
            (MODULE inner = ((LET tag = 0))))\n\
         LET got = :(MakeSet {base = plain})",
    );
    assert!(
        err.contains("type mismatch") && err.contains("Ordered"),
        "non-satisfying module by name should be a TypeMismatch against Ordered, got: {err}",
    );
}

/// Surface-disjoint check: `FUNCTOR` at value-position binder and the new
/// keyworded `FUNCTOR` at type-position sigil both work in the same run
/// without collision. The all-uppercase `FUNCTOR` keyword classifies as a
/// Keyword in both positions; the dispatcher routes value-side `FUNCTOR <Name>
/// ...` to the binder overload and sigiled `:(FUNCTOR (T :S) -> Module)` to the
/// type-constructor overload registered in
/// [`crate::builtins::type_constructors`].
///
/// Pre-type-language-via-dispatch this test used the PascalCase `Functor` head
/// (`:(Functor (Ordered) -> Module)`) routed through the parser's
/// `Functor`-special-cased `Function`-arrow fold. With the
/// type-language-via-dispatch move the parser does no folding and the
/// PascalCase `Functor` head has no registered overload — the equivalent
/// surface is the all-uppercase `FUNCTOR` keyword. `:Signature` substitutes
/// for `Ordered` because the inner sigil sub-Dispatch may race the outer
/// SIG declaration; using the always-bound builtin meta-type keeps the test
/// focused on the disjoint-surface check rather than scheduling.
#[test]
fn functor_binder_and_sigil_coexist() {
    let region = run_root_storage();
    let scope = run(
        &region,
        "SIG Ordered = (VAL compare :Number)\n\
         FUNCTOR (MAKEINNER er :Ordered) -> Module = \
            (MODULE res = ((LET inner = 1)))\n\
         FUNCTOR (MAKEOUTER er :Ordered) -> :(FUNCTOR (Ty :Signature) -> Module) = \
            (FUNCTOR (INNER fr :Ordered) -> Module = (MODULE res = ((LET v = 2))))",
    );
    let outer = lookup_fn(scope, "MAKEOUTER");
    assert!(outer.is_functor, "outer FUNCTOR carries is_functor");
    use koan::machine::model::ReturnType;
    match &outer.signature.return_type {
        ReturnType::Resolved(KType::KFunctor { .. }) => {}
        ReturnType::Resolved(other) => {
            panic!(
                "outer return type should elaborate to KFunctor, got {}",
                other.name()
            )
        }
        ReturnType::Deferred(_) => {
            panic!("outer return type should be statically Resolved (no param ref)")
        }
    }
}
