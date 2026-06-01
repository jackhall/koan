//! `LexicalFrame` — immutable cactus-chain frame attached to every dispatched node.
//!
//! Each frame names one position in one lexical block: `(scope_id, index)`. Frames
//! link bottom-up through `parent`; head is innermost, `parent: None` at the tail
//! marks a top-level statement. Siblings share their parent `Rc` (cactus sharing).
//!
//! Chain depth equals lexical scope-nesting depth, not call depth: tail-recursive FN
//! invocations rebuild the new body's chain from the function's lexical `outer` walk,
//! so a long tail-recursive loop produces an equal-depth chain each iteration rather
//! than ballooning.
//!
//! [`LexicalFrame::index_for`] backs the index-gated visibility predicate: a binding
//! at index `i` is visible to a consumer at cutoff `c` iff `i < c`. `None` from
//! `index_for` means "no frame on this chain mentions that scope" and is read as
//! "scope complete — every entry visible".

use std::rc::Rc;

use super::{Scope, ScopeId};

#[cfg(test)]
mod tests;

#[derive(Debug)]
pub struct LexicalFrame {
    pub scope_id: ScopeId,
    pub index: usize,
    pub parent: Option<Rc<LexicalFrame>>,
}

impl LexicalFrame {
    pub fn root(scope_id: ScopeId, index: usize) -> Rc<Self> {
        Rc::new(LexicalFrame {
            scope_id,
            index,
            parent: None,
        })
    }

    pub fn push(parent: Option<Rc<Self>>, scope_id: ScopeId, index: usize) -> Rc<Self> {
        Rc::new(LexicalFrame {
            scope_id,
            index,
            parent,
        })
    }

    /// A chain mentioning no real scope: `index_for` returns `None` for every
    /// `ScopeId`, so the visibility predicate sees every scope as complete and every
    /// binding in it as visible. Used for ambient-chain-less submission (REPL, test
    /// fixtures) against an existing scope so previously-bound names resolve.
    pub fn detached() -> Rc<Self> {
        Rc::new(LexicalFrame {
            scope_id: ScopeId::DETACHED,
            index: 0,
            parent: None,
        })
    }

    /// First frame's `index` whose `scope_id` matches, walking head-first. `None`
    /// means no frame on this chain mentions that scope; the visibility predicate
    /// reads `None` as "scope complete, every entry visible".
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

    #[cfg(test)]
    pub fn iter(&self) -> impl Iterator<Item = &LexicalFrame> {
        std::iter::successors(Some(self), |f| f.parent.as_deref())
    }

    #[cfg(test)]
    pub fn depth(&self) -> usize {
        self.iter().count()
    }
}

/// Body chain for a user-fn invoke. Walks `body_scope`'s lexical `outer` chain,
/// stacks one frame per scope that also appears on the call-site chain, then
/// prepends `(body_scope.id, body_index)` as the head.
///
/// Depth is bounded by source-level nesting, not call depth — tail-recursion
/// through the same FN produces a structurally equal chain each iteration.
pub fn assemble_body_chain<'a>(
    body_scope: &Scope<'a>,
    call_site_chain: Rc<LexicalFrame>,
    body_index: usize,
) -> Rc<LexicalFrame> {
    let mut hits: Vec<(ScopeId, usize)> = Vec::new();
    let mut current = body_scope.outer;
    while let Some(s) = current {
        if let Some(index) = call_site_chain.index_for(s.id) {
            hits.push((s.id, index));
        }
        current = s.outer;
    }
    // Reverse so the outermost scope ends up at the tail (`parent: None`).
    hits.reverse();
    let mut chain: Option<Rc<LexicalFrame>> = None;
    for (sid, idx) in hits {
        chain = Some(LexicalFrame::push(chain, sid, idx));
    }
    // `body_index = 0` is single-statement: the lone body statement sees only its
    // own parameters. Multi-statement bodies pass `N` for the last statement so
    // siblings at `idx < N` are visible.
    LexicalFrame::push(chain, body_scope.id, body_index)
}
