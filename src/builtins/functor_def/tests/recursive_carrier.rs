//! `KFunctor` admits as a return-type carrier via the recursive arm: an outer
//! FUNCTOR whose return slot denotes another functor.

use crate::builtins::test_support::{lookup_fn, run, run_root_silent};
use crate::machine::model::KType;
use crate::machine::KoanRegion;

#[test]
fn functor_return_slot_curried_functor_admits() {
    let region = KoanRegion::new();
    let scope = run_root_silent(&region);
    run(
        scope,
        // Inner FUNCTOR uses the builtin `:Signature` rather than the sibling
        // `OrderedSig` to dodge forward resolution across sub-Dispatches.
        "SIG OrderedSig = (VAL compare :Number)\n\
         FUNCTOR (CURRIED Er :OrderedSig) -> :(FUNCTOR (Ty :Signature) -> Module) = \
            (FUNCTOR (INNER y :OrderedSig) -> Module = (MODULE Generated = (LET inner = 1)))",
    );
    let f = lookup_fn(scope, "CURRIED");
    assert!(
        f.is_functor,
        "outer functor with curried-functor return slot must register",
    );
    use crate::machine::model::types::ReturnType;
    match &f.signature.return_type {
        ReturnType::Resolved(KType::KFunctor { .. }) => {}
        ReturnType::Resolved(other) => {
            panic!("expected Resolved(KFunctor), got Resolved({:?})", other)
        }
        ReturnType::Deferred(_) => panic!("return type should be statically Resolved"),
    }
}
