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

    /// The same chain with every position advanced one statement, so a binding the strict
    /// `i < c` gate hides *because it is the consumer's own statement that declares it* becomes
    /// visible, while a later sibling's stays hidden. That is the one probe separating a
    /// self-reference (`LET Ty = Ty`) from a genuine forward reference, and a diagnostic is the
    /// only caller: it runs on the error path, over a chain as deep as the lexical nesting.
    pub fn including_own_statement(frame: &LexicalFrame) -> Rc<Self> {
        LexicalFrame::push(
            frame.parent.as_deref().map(Self::including_own_statement),
            frame.scope_id,
            frame.index + 1,
        )
    }

    /// First frame's `index` whose `scope_id` matches, walking head-first. `None`
    /// (no frame mentions that scope) reads as "scope complete, every entry visible".
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

/// First frame on `chain` whose `scope_id` matches, as the sub-chain `Rc` — the frame and
/// everything below it. Same head-first traversal as [`LexicalFrame::index_for`], returning
/// the frame instead of its index so a caller can share the standing sub-chain outright.
fn frame_for(chain: &Rc<LexicalFrame>, scope_id: ScopeId) -> Option<&Rc<LexicalFrame>> {
    let mut current = chain;
    loop {
        if current.scope_id == scope_id {
            return Some(current);
        }
        current = current.parent.as_ref()?;
    }
}

fn same_chain(a: Option<&Rc<LexicalFrame>>, b: Option<&Rc<LexicalFrame>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}

/// Parent chain for the scopes from `scope` outward: one frame per lexical ancestor the
/// call-site chain names, outermost at the tail. Built on the recursion's unwind so the
/// outermost frame is made first; when the call-site chain already holds a sub-chain whose
/// head is this scope's hit and whose parent is the chain just built, that sub-chain is
/// shared instead of re-minted — in a steady tail loop the whole suffix is one shared clone.
/// Depth is source-level scope nesting, so the recursion stands in for the container a heap
/// walk would need.
fn assemble_parents(
    scope: Option<&Scope<'_>>,
    call_site_chain: &Rc<LexicalFrame>,
) -> Option<Rc<LexicalFrame>> {
    let scope = scope?;
    let parent = assemble_parents(scope.outer(), call_site_chain);
    // A scope the call-site chain never names contributes no frame; the ancestors already
    // assembled below it still stand.
    let Some(standing) = frame_for(call_site_chain, scope.id) else {
        return parent;
    };
    if same_chain(standing.parent.as_ref(), parent.as_ref()) {
        return Some(Rc::clone(standing));
    }
    Some(LexicalFrame::push(parent, scope.id, standing.index))
}

/// Body chain for a user-fn invoke. Walks `body_scope`'s lexical `outer` chain, stacks one
/// frame per scope that also appears on the call-site chain, then prepends
/// `(body_scope.id, body_index)` as the head. Depth is bounded by source-level nesting, not
/// call depth (see module header), and any suffix the call-site chain already spells the same
/// way is shared by `Rc` rather than rebuilt.
pub fn assemble_body_chain<'a>(
    body_scope: &Scope<'a>,
    call_site_chain: Rc<LexicalFrame>,
    body_index: usize,
) -> Rc<LexicalFrame> {
    let chain = assemble_parents(body_scope.outer(), &call_site_chain);
    // `body_index = 1` is single-statement: the lone body statement sits above the
    // `idx 0` parameters / `it`, so the strict `idx < cutoff` predicate admits them.
    // Multi-statement bodies pass `N` for the last statement so siblings at
    // `idx < N` (params and earlier statements) are visible.
    LexicalFrame::push(chain, body_scope.id, body_index)
}
