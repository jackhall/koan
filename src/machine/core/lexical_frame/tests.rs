//! Unit tests for [`LexicalFrame`]. Higher-level scheduler-integration tests live in
//! `src/machine/execute/run_loop/tests/`.

use std::rc::Rc;

use super::{LexicalFrame, assemble_body_chain};
use crate::builtins::test_support::run_root_bare;
use crate::machine::core::arena::FrameStorage;
use crate::machine::core::{Scope, ScopeId, run_root_storage};

#[test]
fn root_has_no_parent() {
    let scope = ScopeId::next();
    let frame = LexicalFrame::root(scope, 0);
    assert!(frame.parent.is_none());
    assert_eq!(frame.scope_id, scope);
    assert_eq!(frame.index, 0);
    assert_eq!(frame.depth(), 1);
}

#[test]
fn push_prepends_and_links_parent() {
    let outer_scope = ScopeId::next();
    let inner_scope = ScopeId::next();
    let outer = LexicalFrame::root(outer_scope, 3);
    let inner = LexicalFrame::push(Some(outer.clone()), inner_scope, 0);
    assert_eq!(inner.scope_id, inner_scope);
    assert_eq!(inner.index, 0);
    assert!(inner.parent.is_some());
    let parent_ref = inner.parent.as_ref().expect("parent set");
    assert!(Rc::ptr_eq(parent_ref, &outer));
    assert_eq!(inner.depth(), 2);
}

#[test]
fn index_for_finds_nearest_match() {
    let outer_scope = ScopeId::next();
    let inner_scope = ScopeId::next();
    let outer = LexicalFrame::root(outer_scope, 5);
    let inner = LexicalFrame::push(Some(outer), inner_scope, 2);
    assert_eq!(inner.index_for(inner_scope), Some(2));
    assert_eq!(inner.index_for(outer_scope), Some(5));
    let unknown_scope = ScopeId::next();
    assert_eq!(inner.index_for(unknown_scope), None);
}

#[test]
fn sibling_frames_share_parent_rc() {
    let outer_scope = ScopeId::next();
    let inner_scope = ScopeId::next();
    let outer = LexicalFrame::root(outer_scope, 0);
    let sibling_a = LexicalFrame::push(Some(outer.clone()), inner_scope, 0);
    let sibling_b = LexicalFrame::push(Some(outer.clone()), inner_scope, 1);
    let pa = sibling_a.parent.as_ref().expect("parent set");
    let pb = sibling_b.parent.as_ref().expect("parent set");
    assert!(Rc::ptr_eq(pa, pb), "siblings must share parent Rc");
    assert_ne!(sibling_a.index, sibling_b.index);
}

#[test]
fn index_for_returns_innermost_match_when_scope_reappears() {
    // A FN whose body's scope_id matches an outer one is pathological but covered: the
    // walk returns the head-most hit, so an inner re-entry shadows the outer index.
    let shared_scope = ScopeId::next();
    let outer_scope = ScopeId::next();
    let outer = LexicalFrame::root(shared_scope, 7);
    let middle = LexicalFrame::push(Some(outer), outer_scope, 1);
    let inner = LexicalFrame::push(Some(middle), shared_scope, 2);
    assert_eq!(inner.index_for(shared_scope), Some(2));
}

/// A nested-scope fixture and the chain naming each of its scopes, innermost first — the shape
/// [`assemble_body_chain`] reads its ancestors' indices out of.
struct Nest<'a> {
    outer: &'a Scope<'a>,
    middle: &'a Scope<'a>,
    inner: &'a Scope<'a>,
}

fn nest<'a>(storage: &'a Rc<FrameStorage>) -> Nest<'a> {
    let outer = run_root_bare(storage);
    let middle = outer.alloc_child_under();
    let inner = middle.alloc_child_under();
    Nest {
        outer,
        middle,
        inner,
    }
}

fn shape(chain: &LexicalFrame) -> Vec<(ScopeId, usize)> {
    chain.iter().map(|f| (f.scope_id, f.index)).collect()
}

#[test]
fn assembled_chain_names_each_call_site_ancestor() {
    let storage = run_root_storage();
    let n = nest(&storage);
    // The call-site chain's head names a scope off the body's lexical walk, so the assembly
    // has to find each ancestor's index by walking rather than reading the head.
    let call_site = LexicalFrame::push(
        Some(LexicalFrame::push(
            Some(LexicalFrame::push(
                Some(LexicalFrame::root(n.outer.id, 4)),
                n.middle.id,
                2,
            )),
            n.inner.id,
            0,
        )),
        ScopeId::next(),
        7,
    );
    let body = n.inner.alloc_child_under();

    let chain = assemble_body_chain(body, call_site, 1);

    assert_eq!(
        shape(&chain),
        vec![
            (body.id, 1),
            (n.inner.id, 0),
            (n.middle.id, 2),
            (n.outer.id, 4)
        ],
        "the head is the body's own scope; below it, every ancestor the call-site chain names, \
         carrying that chain's index",
    );
}

#[test]
fn ancestors_the_call_site_chain_never_names_contribute_no_frame() {
    let storage = run_root_storage();
    let n = nest(&storage);
    // `middle` is on no chain, so it drops out while `outer` below it still stands.
    let call_site = LexicalFrame::root(n.outer.id, 4);
    let chain = assemble_body_chain(n.inner, call_site, 1);
    assert_eq!(shape(&chain), vec![(n.inner.id, 1), (n.outer.id, 4)]);
}

#[test]
fn matching_suffix_is_shared_by_rc_not_rebuilt() {
    let storage = run_root_storage();
    let n = nest(&storage);
    // The call-site chain is exactly the parent chain the assembly would build for a body in
    // `inner` — head `inner`, then `middle`, then `outer` — so the whole suffix is reusable.
    let expected_suffix = LexicalFrame::push(
        Some(LexicalFrame::push(
            Some(LexicalFrame::root(n.outer.id, 4)),
            n.middle.id,
            2,
        )),
        n.inner.id,
        3,
    );
    let body = n.inner.alloc_child_under();

    let chain = assemble_body_chain(body, Rc::clone(&expected_suffix), 1);

    let parent = chain
        .parent
        .as_ref()
        .expect("body head links to its ancestors");
    assert!(
        Rc::ptr_eq(parent, &expected_suffix),
        "a suffix the call-site chain already spells is shared, not re-minted",
    );
}

#[test]
fn interposed_frame_defeats_sharing_but_not_correctness() {
    let storage = run_root_storage();
    let n = nest(&storage);
    let interposed = ScopeId::next();
    // `interposed` sits between `outer` and `middle` on the call-site chain but on no lexical
    // walk, so the standing `middle` sub-chain's parent is not the chain assembly builds.
    let standing_middle = LexicalFrame::push(
        Some(LexicalFrame::push(
            Some(LexicalFrame::root(n.outer.id, 4)),
            interposed,
            9,
        )),
        n.middle.id,
        2,
    );
    let call_site = LexicalFrame::push(Some(Rc::clone(&standing_middle)), n.inner.id, 3);

    let chain = assemble_body_chain(n.inner, call_site, 1);

    assert_eq!(
        shape(&chain),
        vec![(n.inner.id, 1), (n.middle.id, 2), (n.outer.id, 4)],
        "the interposed frame is not a lexical ancestor, so it stays off the assembled chain",
    );
    let middle_frame = chain.parent.as_ref().expect("ancestors assembled");
    assert!(
        !Rc::ptr_eq(middle_frame, &standing_middle),
        "a standing sub-chain whose parent differs is re-minted rather than shared",
    );
}
