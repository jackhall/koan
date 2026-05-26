//! `KFunctor` admits as a return-type carrier via the recursive arm. Covers
//! the curried-multi-module-functor shape: an outer FUNCTOR whose return slot
//! denotes another functor.

use crate::builtins::test_support::{lookup_fn, run, run_root_silent};
use crate::machine::model::KType;
use crate::machine::RuntimeArena;

/// `-> :(Functor (...) -> Module)` — an outer functor that returns an inner
/// functor. The Stage-1 sigil lowers this to `KType::KFunctor { ret: KFunctor
/// { ret: AnyModule }}`; the FUNCTOR validator's recursive `KFunctor` arm
/// walks the outer ret and admits via the inner `AnyModule`.
#[test]
fn functor_return_slot_curried_functor_admits() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         FUNCTOR (CURRIED Er :OrderedSig) -> :(Functor (OrderedSig) -> Module) = \
            (FUNCTOR (INNER y :OrderedSig) -> Module = (MODULE Result = (LET inner = 1)))",
    );
    let f = lookup_fn(scope, "CURRIED");
    assert!(
        f.is_functor,
        "outer functor with curried-functor return slot must register",
    );
    // The signature's return type must elaborate to a `KFunctor` (no Resolved
    // → KFunction collapse). The Stage-1 sigil arm produces `KFunctor`
    // directly; this test pins that the validator doesn't reject the shape.
    use crate::machine::model::types::ReturnType;
    match &f.signature.return_type {
        ReturnType::Resolved(KType::KFunctor { .. }) => {}
        ReturnType::Resolved(other) => {
            panic!("expected Resolved(KFunctor), got Resolved({:?})", other)
        }
        ReturnType::Deferred(_) => panic!("return type should be statically Resolved"),
    }
}
