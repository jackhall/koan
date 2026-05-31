//! End-to-end smoke test for the `FUNCTOR` binder, mirroring the `MakeSet`
//! shape from [design/typing/functors.md](../design/typing/functors.md).
//!
//! The test exercises the full shipped FUNCTOR pipeline:
//! 1. **Define** — `FUNCTOR (MAKESET Er :OrderedSig) -> SetSig = (MODULE Result
//!    = ...)` registers a KFunction with `is_functor: true`.
//! 2. **Apply** — `LET IntSet = (MAKESET IntOrd)` invokes the functor with a
//!    signature-typed module argument; per-call type-side install registers
//!    `Er`'s type-language identity into the body's child scope.
//! 3. **Produce** — the body's `MODULE Result = (...)` returns a module value
//!    that the LET RHS binds as `IntSet`. The Stage-5 allowlist routes the
//!    Module carrier through `derive_nominal_identity` so `IntSet` lands both
//!    in `bindings.types` and `bindings.data`.
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
use koan::machine::{KFunction, RuntimeArena, Scheduler, Scope};
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

fn run<'a>(arena: &'a RuntimeArena, src: &str) -> &'a Scope<'a> {
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = default_scope(arena, Box::new(SharedBuf(captured)));
    let exprs = parse(src).expect("parse should succeed");
    let mut sched = Scheduler::new();
    for e in exprs {
        sched.add_dispatch(e, scope);
    }
    sched.execute().expect("scheduler should run to completion");
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

/// End-to-end MakeSet smoke. The functor takes an `OrderedSig`-satisfying
/// module (`IntOrd`), produces a module value carrying a value-side `tag`
/// member, and the LET assigns the result to `IntSet`. The shape pulls
/// every Stage 0-6 piece of the FUNCTOR work into a single Scheduler run:
///
/// - Stage 0/2: `KType::KFunctor` projection on the functor carrier.
/// - Stage 3: FUNCTOR binder admits the `OrderedSig → Module` shape.
/// - Stage 4: cross-arm wall is dormant here (no FN/FUNCTOR slot mix); the
///   test pins the happy-path so the wall isn't exercised.
/// - Stage 5: LET allowlist admits both the Module-valued `IntOrd` ascription
///   and the produced `IntSet` module.
#[test]
fn functor_binder_e2e_makeset_produces_module() {
    let arena = RuntimeArena::new();
    // The natural FUNCTOR application form: `(MAKESET IntOrd)` works directly
    // when `IntOrd`'s carrier carries the declared signature in its
    // `compatible_sigs` set. The LET partition guard
    // (design/typing/elaboration.md § Binding-map partition) forces the
    // ascription rebind to use a Type-classified identifier
    // (`LET IntOrd = (IntOrdBase :! OrderedSig)`) so the module/signature
    // carrier never rides a value-classified alias; the dispatch admission then
    // consults `compatible_sigs` at the signature-typed slot, so no parens-wrap
    // or ascription-view workaround is required at the call site.
    let scope = run(
        &arena,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrdBase = ((LET compare = 7))\n\
         LET IntOrd = (IntOrdBase :! OrderedSig)\n\
         FUNCTOR (MAKESET Er :OrderedSig) -> Module = \
            (MODULE Result = ((LET tag = 0)))\n\
         LET IntSet = (MAKESET IntOrd)",
    );
    // `MAKESET` registered as a FUNCTOR-flagged KFunction in the dispatch
    // table (FN / FUNCTOR write to `functions`, not `data`).
    let makeset = lookup_fn(scope, "MAKESET");
    assert!(
        makeset.is_functor,
        "MAKESET must carry is_functor: true (Stage-2 / Stage-3 plumbing)",
    );
    // `IntSet` landed as a Module — bound type-only (the LET allowlist routes Module
    // identities through `register_type`), so the `&Module` rides the `KType::Module`
    // identity in `bindings.types`, with no value-side carrier in `bindings.data`.
    assert!(
        scope.lookup("IntSet").is_none(),
        "IntSet must be type-only — no value-side carrier in data",
    );
    let m = match scope.resolve_type("IntSet") {
        Some(KType::Module { module, .. }) => *module,
        other => panic!("IntSet should be a Module identity in types, got {other:?}"),
    };
    // The functor body's `(LET tag = 0)` lifted into the result module's
    // child scope — verifies the per-call body actually ran and the
    // produced module carries its declared member.
    let tag = m.child_scope().lookup("tag");
    assert!(
        matches!(tag, Some(KObject::Number(n)) if *n == 0.0),
        "IntSet's `tag` member should be 0, got {:?}",
        tag.map(|o| o.ktype()),
    );
    // Type-side: `IntSet` is reachable as a type via `Scope::resolve_type`
    // (the nominal binder installs both the type identity and the carrier).
    let int_set_type = scope
        .resolve_type("IntSet")
        .expect("IntSet should be reachable via resolve_type");
    assert!(
        matches!(int_set_type, KType::Module { .. }),
        "IntSet's type entry should be a Module carrier",
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
/// (`:(Functor (OrderedSig) -> Module)`) routed through the parser's
/// `Functor`-special-cased `Function`-arrow fold. With the
/// type-language-via-dispatch move the parser does no folding and the
/// PascalCase `Functor` head has no registered overload — the equivalent
/// surface is the all-uppercase `FUNCTOR` keyword. `:Signature` substitutes
/// for `OrderedSig` because the inner sigil sub-Dispatch may race the outer
/// SIG declaration; using the always-bound builtin meta-type keeps the test
/// focused on the disjoint-surface check rather than scheduling.
#[test]
fn functor_binder_and_sigil_coexist() {
    let arena = RuntimeArena::new();
    let scope = run(
        &arena,
        "SIG OrderedSig = (VAL compare :Number)\n\
         FUNCTOR (MAKEINNER Er :OrderedSig) -> Module = \
            (MODULE Res = ((LET inner = 1)))\n\
         FUNCTOR (MAKEOUTER Er :OrderedSig) -> :(FUNCTOR (Ty :Signature) -> Module) = \
            (FUNCTOR (INNER Fr :OrderedSig) -> Module = (MODULE Res = ((LET v = 2))))",
    );
    let outer = lookup_fn(scope, "MAKEOUTER");
    assert!(outer.is_functor, "outer FUNCTOR carries is_functor");
    use koan::machine::model::ReturnType;
    match &outer.signature.return_type {
        ReturnType::Resolved(KType::KFunctor { .. }) => {}
        ReturnType::Resolved(other) => {
            panic!("outer return type should elaborate to KFunctor, got {}", other.name())
        }
        ReturnType::Deferred(_) => {
            panic!("outer return type should be statically Resolved (no param ref)")
        }
    }
}
