//! `LexicalFrame` — immutable cactus-chain frame attached to every dispatched node.
//!
//! Each frame names one position in one lexical block: `(scope_id, index)` where
//! `scope_id` is the [`ScopeId`] of the enclosing block's lexical scope and `index` is
//! the statement's position inside that block (assigned at submission time by
//! `Scheduler::enter_block`).
//!
//! Frames link bottom-up through `parent: Option<Rc<LexicalFrame>>`: the head is the
//! innermost enclosing block, the chain walks outward through every enclosing lexical
//! block, and `parent: None` at the tail marks a top-level statement. Sibling
//! statements at the same block share their parent `Rc` (cactus sharing).
//!
//! Chain depth equals lexical scope-nesting depth, not call depth. Tail-recursive
//! and mutually tail-recursive FN invocations rebuild the new body's chain from the
//! function's lexical `outer` walk (see `assemble_body_chain` in
//! `kfunction/invoke.rs`), so a long tail-recursive loop produces an equal-depth
//! chain each iteration rather than ballooning.
//!
//! The lookup helper [`LexicalFrame::index_for`] backs the index-gated resolution
//! gate: [`crate::machine::core::scope::visible`] reads it to decide whether a
//! binding at index `i` is visible to a consumer at cutoff `c` (the `b.idx < c`
//! predicate), and the overload-bucket pre-filter in
//! [`crate::machine::core::resolve_dispatch`] applies the same predicate
//! per-overload.

use std::rc::Rc;

use super::{Scope, ScopeId};

#[cfg(test)]
mod tests;

/// One node's lexical position. Immutable; only [`Self::push`] / [`Self::root`] create
/// frames, and the parent `Rc` is shared across sibling statements.
#[derive(Debug)]
pub struct LexicalFrame {
    pub scope_id: ScopeId,
    pub index: usize,
    pub parent: Option<Rc<LexicalFrame>>,
}

impl LexicalFrame {
    /// Build a root frame (`parent: None`), used for top-level statements.
    pub fn root(scope_id: ScopeId, index: usize) -> Rc<Self> {
        Rc::new(LexicalFrame { scope_id, index, parent: None })
    }

    /// Prepend a frame onto `parent`. `parent: None` is equivalent to [`Self::root`];
    /// `Some(_)` marks this as one level deeper in the lexical block nesting.
    pub fn push(parent: Option<Rc<Self>>, scope_id: ScopeId, index: usize) -> Rc<Self> {
        Rc::new(LexicalFrame { scope_id, index, parent })
    }

    /// A chain that mentions no real scope — `index_for` returns `None` for every
    /// `ScopeId`, so the visibility predicate sees every scope as "complete" and
    /// every binding in it as visible. Used by [`crate::machine::execute::Scheduler::add`]'s
    /// auto-root branch (no ambient chain) so a REPL / test-fixture submission
    /// against an existing scope reads through to previously-bound names.
    pub fn detached() -> Rc<Self> {
        Rc::new(LexicalFrame { scope_id: ScopeId::DETACHED, index: 0, parent: None })
    }

    /// Walk this chain (head first) and return the first frame's `index` whose
    /// `scope_id` matches. `None` means "no frame on this chain mentions that
    /// scope" — the index-gated resolution gate reads `None` as "this scope is
    /// complete from this chain's perspective" (every statement in it preceded
    /// this one in source order), so every entry there is visible.
    /// [`crate::machine::core::scope::visible`] is the predicate.
    pub fn index_for(&self, scope_id: ScopeId) -> Option<usize> {
        let mut current: Option<&LexicalFrame> = Some(self);
        while let Some(frame) = current {
            if frame.scope_id == scope_id {
                return Some(frame.index);
            }
            current = frame.parent.as_deref();
        }
        None
    }

    /// Walk the chain head-to-tail. Test-only; production consumers read positions
    /// through [`Self::index_for`].
    #[cfg(test)]
    pub fn iter(&self) -> impl Iterator<Item = &LexicalFrame> {
        std::iter::successors(Some(self), |f| f.parent.as_deref())
    }

    /// Chain depth — head frame is depth 1, each parent adds one. Test-only helper
    /// for assertions about lexical-nesting-bounded depth (tail recursion must not
    /// grow this number).
    #[cfg(test)]
    pub fn depth(&self) -> usize {
        self.iter().count()
    }
}

/// Build the body chain for a user-fn invoke. Walks `body_scope`'s lexical `outer`
/// chain (the FN's captured definition scope upward), looks each scope up in the
/// call-site chain, and stacks one frame per hit. The result is prepended with
/// `(body_scope.id, 0)` as the new head.
///
/// Result depth equals the number of enclosing lexical blocks still on the call-
/// site chain — bounded by source-level nesting, not call depth. Tail-recursion
/// through the same FN produces a structurally equal chain each iteration.
///
/// Pre-condition: `body_scope.outer` is the FN's captured definition scope, set
/// up by `CallArena::new(outer, _)` in `KFunction::invoke`.
pub fn assemble_body_chain<'a>(
    body_scope: &Scope<'a>,
    call_site_chain: Rc<LexicalFrame>,
) -> Rc<LexicalFrame> {
    let mut hits: Vec<(ScopeId, usize)> = Vec::new();
    let mut current = body_scope.outer;
    while let Some(s) = current {
        if let Some(index) = call_site_chain.index_for(s.id) {
            hits.push((s.id, index));
        }
        current = s.outer;
    }
    // `hits` is innermost-first (walked outward); reverse so the outermost lexical
    // scope's frame ends up as the chain's tail (parent: None for the root case).
    hits.reverse();
    let mut chain: Option<Rc<LexicalFrame>> = None;
    for (sid, idx) in hits {
        chain = Some(LexicalFrame::push(chain, sid, idx));
    }
    LexicalFrame::push(chain, body_scope.id, 0)
}
