//! Unit tests for [`LexicalFrame`]. Higher-level scheduler-integration tests live in
//! `src/machine/execute/scheduler/tests/`.

use std::rc::Rc;

use super::LexicalFrame;
use crate::machine::core::ScopeId;

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
