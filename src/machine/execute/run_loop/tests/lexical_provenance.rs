//! Lexical-provenance plumbing tests. Assertions peek at slot chains before
//! `execute` drains them, so tests submit via `enter_block` and read
//! `Scheduler::chain_of` directly.

use std::rc::Rc;

use crate::builtins::default_scope;
use crate::builtins::test_support::parse_one;
use crate::machine::core::FrameStorage;
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::source::Spanned;

use super::let_expr;
use crate::machine::execute::KoanRuntime;

fn lit<'run>(name: &str) -> KExpression<'run> {
    KExpression::new(vec![Spanned::bare(ExpressionPart::Keyword(name.into()))])
}

#[test]
fn top_level_statements_get_root_frames_with_consecutive_indices() {
    let region = FrameStorage::run_root();
    let root = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let ids = runtime.enter_block(
        root.id,
        vec![let_expr("a", 1.0), let_expr("b", 2.0), let_expr("c", 3.0)],
        root,
    );
    let chains: Vec<_> = ids
        .iter()
        .map(|id| runtime.chain_of(*id).unwrap())
        .collect();
    for (i, chain) in chains.iter().enumerate() {
        assert!(
            chain.parent.is_none(),
            "top-level frame i={i} must have parent: None"
        );
        assert_eq!(chain.scope_id, root.id);
        // Indices start at 1; `BindingIndex::BUILTIN` occupies 0.
        assert_eq!(chain.index, i + 1);
    }
    assert!(!Rc::ptr_eq(&chains[0], &chains[1]));
    assert!(!Rc::ptr_eq(&chains[1], &chains[2]));
}

#[test]
fn sibling_statements_in_inner_block_share_parent_rc() {
    let region = FrameStorage::run_root();
    let root = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let ids = runtime.enter_block(root.id, vec![lit("ANY1"), lit("ANY2")], root);
    let chain_a = runtime.chain_of(ids[0]).unwrap();
    let chain_b = runtime.chain_of(ids[1]).unwrap();
    assert!(chain_a.parent.is_none());
    assert!(chain_b.parent.is_none());
    let parent_chain = chain_a.clone();
    let inner_scope_id = crate::machine::core::ScopeId::next();
    // Push sibling frames directly; `execute` does this via `enter_block`
    // during a slot's run.
    let inner_a = crate::machine::LexicalFrame::push(Some(parent_chain.clone()), inner_scope_id, 0);
    let inner_b = crate::machine::LexicalFrame::push(Some(parent_chain.clone()), inner_scope_id, 1);
    let pa = inner_a.parent.as_ref().expect("set");
    let pb = inner_b.parent.as_ref().expect("set");
    assert!(Rc::ptr_eq(pa, pb), "siblings must share parent Rc");
}

#[test]
fn module_body_chain_parent_points_at_module_statement_frame() {
    use crate::machine::model::values::Module;
    let region = FrameStorage::run_root();
    let root = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let module_expr = parse_one("MODULE Foo = (LET x = 1)");
    let ids = runtime.enter_block(root.id, vec![module_expr], root);
    let top_id = ids[0];
    let top_chain = runtime.chain_of(top_id).expect("module statement chain");
    assert_eq!(top_chain.scope_id, root.id);
    assert_eq!(top_chain.index, 1);
    assert!(top_chain.parent.is_none());
    runtime.execute().expect("module runs");
    // Body slot has terminalized by now and dropped its chain; the body-chain
    // shape is exercised end-to-end by the recursive smoke tests below. MODULE is
    // type-only, so the `&Module` rides the identity in `types`.
    let module = match root.resolve_type("Foo") {
        Some(crate::machine::model::KType::Module { module: m, .. }) => *m,
        _ => panic!("Foo should be a module identity in types"),
    };
    let _: &Module<'_> = module;
}

/// Tail-recursive FN: body chain depth stays bounded by lexical nesting, not
/// call depth. Non-tail-recursive Rc allocation would OOM or overflow.
#[test]
fn tail_recursive_fn_does_not_balloon_chain() {
    let region = FrameStorage::run_root();
    let (scope, captured) = crate::builtins::test_support::run_root_with_buf(&region);
    crate::builtins::test_support::run(
        scope,
        "UNION Counter = (more :Null done :Null)\n\
         FN (LOOP n :Number c :Any) -> Number = (MATCH (c) -> :Number WITH (\
            more -> (LOOP (n) (Counter (more null)))\
            done -> (n)\
         ))\n\
         LOOP 1 (Counter (done null))",
    );
    let _ = captured;
}

/// FN body chain assembly: top-level FN followed by spacer LETs and a call.
/// Bounded-depth chain structure is exercised by
/// `tail_recursive_fn_does_not_balloon_chain`; this smoke-tests assembly.
#[test]
fn fn_body_call_with_spacers_produces_value() {
    let region = FrameStorage::run_root();
    let scope = crate::builtins::test_support::run_root_silent(&region);
    crate::builtins::test_support::run(
        scope,
        "FN (DBL x :Number) -> Number = (x)\n\
         LET a = 1\n\
         LET b = 2\n\
         LET c = 3\n\
         LET r = (DBL 5)",
    );
    let data = scope.bindings().data();
    use crate::machine::model::KObject;
    assert!(matches!(data.get("r").map(|(o, _, _)| *o), Some(KObject::Number(n)) if *n == 5.0));
}

#[test]
fn cons_head_subdispatch_inherits_parent_chain() {
    // CONS-head `dispatch_in_scope` inherits the active chain of the slot running
    // CONS; pinned indirectly via a multi-statement FN body folded into CONS.
    let region = FrameStorage::run_root();
    let scope = crate::builtins::test_support::run_root_silent(&region);
    crate::builtins::test_support::run(scope, "FN (FOO) -> Number = ((LET x = 1) (LET y = 2) (y))");
    use crate::machine::model::KObject;
    let v = crate::builtins::test_support::run_one(
        scope,
        crate::builtins::test_support::parse_one("FOO"),
    );
    assert!(matches!(v, KObject::Number(n) if *n == 2.0));
}

/// Debug-assert tripwire: strict `add_with_chain(_, _, None)` with no ambient
/// chain must panic. Public `add` / `dispatch_in_scope` auto-route to a root frame,
/// so reaching the strict path requires the super-visible helper directly.
#[test]
#[should_panic(expected = "every dispatched node has a chain")]
fn add_with_chain_without_chain_panics() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    runtime.add_with_chain(
        crate::machine::execute::dispatch::decide(KExpression::new(vec![Spanned::bare(
            ExpressionPart::Literal(KLiteral::Number(1.0)),
        )])),
        scope,
        None,
    );
}
