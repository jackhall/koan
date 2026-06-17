use std::rc::Rc;

use super::runtime::KoanWorkload;
use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::core::{ScopeId, ScopePtr};
use crate::machine::model::Carried;
use crate::machine::{CallArena, KError, LexicalFrame, NodeId};

/// The generic per-node state lives in [`crate::scheduler::nodes`]; re-exported here so the Koan
/// execute tree has a single `nodes` surface combining them with the Koan-side [`NodeStep`] /
/// [`NodePayload`] / [`NodeScope`].
pub(super) use crate::scheduler::nodes::{CallFrame, Node, NodeWork};

/// Outcome of a node's run. `Replace` is the tail-call path: rewrite the slot's work and
/// re-enqueue the same index so it runs again with no fresh slot allocated, giving constant
/// memory across tail-call sequences. When `frame` is `Some`, its `scope()` becomes the
/// slot's scope and its `arena()` owns per-call allocations; `None` keeps the existing
/// frame and scope. `function`, when set, names the user-fn whose body the replacement is
/// entering — any error landing on this slot gets a `TraceFrame` appended for the trace.
///
/// `block_entry` annotates lexical-block entry. `None` keeps the slot's current
/// `LexicalFrame` chain unchanged. `Some(scope_id)` enters a new lexical block: when
/// `function` is `None` the reinstall site prepends `(scope_id, 0)` to the chain; when
/// `function` is `Some(_)` the chain is rebuilt via `assemble_body_chain` (the FN-body
/// rule that keeps chain depth = lexical nesting depth, NOT call depth).
// `Replace` is intrinsically the large variant (it carries `NodeWork` plus the
// frame/function/chain tail-call payload); `Done` only grows with the cached
// `KExpression` it indirectly holds. Boxing a short-lived return value's hot tail-call
// path to balance the variants is the wrong trade — the imbalance is inherent.
#[allow(clippy::large_enum_variant)]
pub(super) enum NodeStep<'run> {
    Done(Result<Carried<'run>, KError>),
    Replace {
        work: NodeWork<KoanWorkload>,
        frame: Option<Rc<CallArena>>,
        function: Option<ReturnContract<'run>>,
        block_entry: Option<ScopeId>,
        /// Body-scope chain index for FN-body / MATCH-arm / TRY-arm tail-replace
        /// (mirrors [`Outcome::Continue::body_index`]).
        /// Positions the freshly-pushed block frame at index `N` for multi-statement
        /// tail-into-last; `0` is the single-statement case.
        body_index: usize,
    },
    /// The slot is spliced out as an alias of `producer` (a bare-name forward whose producer was not
    /// yet ready). The slot's consumers have already been moved onto `producer`'s notify list; this
    /// just marks the slot so `read_result` follows through to `producer`. See [`Outcome::Forward`].
    Alias(NodeId),
}

/// Slot-stored scope handle, carrying no lifetime so the node it sits on does not pin `'run`
/// through its scope. `Anchored` holds an erased [`ScopePtr`] to a genuinely run-lived scope (a
/// fresh child a binder body allocated in a real arena; NOT the builtins-only
/// [`ScopeKind::Root`](crate::machine::core::ScopeKind)), re-attached at read with a borrow bounded
/// by the reader (`reattach_bounded`) and a free content lifetime — sound because the pointee lives
/// for all of `'run`. A per-call frame scope instead stores `Yoked` — no pointer at all — and is
/// re-projected from the slot's own [`Node::frame`](crate::scheduler::nodes::Node) cart at read time
/// (single-cart: the frame `Rc` already on the slot is the sole liveness witness, so there is no
/// second `Rc` clone and no contention with `try_reset_for_tail`'s uniqueness check). Storing an
/// erased handle rather than a live `&'run` borrow keeps the borrow honest across a TCO
/// `try_reset_for_tail` (nothing persisted points into the reset arena; the live frame is re-read
/// each step) and keeps the slot from naming `'run` in its node-stored scope state.
///
/// `Copy` because both arms are trivially copyable ([`ScopePtr`] is `Copy` / a unit) and submission
/// threads the handle through `pre_subs` recursion without re-deriving it.
#[derive(Clone, Copy)]
pub(super) enum NodeScope {
    Anchored(ScopePtr<'static>),
    Yoked,
}

/// The opaque per-node workload payload: the Koan name-resolution state the scheduler stores on a
/// slot, threads through a step, and hands back, but does not own as scheduler machinery — the
/// slot's [`NodeScope`] handle and its lexical [`chain`](Self::chain). Lifetime-free (the scope is
/// an erased `NodeScope`, the chain an `Rc`), so the node it sits on pins no `'run` through it. This
/// is the concrete Koan stand-in for the generic workload payload the scheduler is parametric
/// over (`KoanWorkload::Payload`). Cheap-`Clone`: `NodeScope` is `Copy`, the chain
/// is an `Rc`.
#[derive(Clone)]
pub(super) struct NodePayload {
    pub(super) scope: NodeScope,
    /// Immutable cactus-chain naming this node's lexical position. Head frame is the
    /// innermost enclosing block; tail (`parent: None`) is top-level. See
    /// `core/lexical_frame.rs`.
    pub(super) chain: Rc<LexicalFrame>,
}

#[cfg(test)]
mod tests {
    //! Miri coverage for the `NodeScope::Anchored` lifetime fabrication: each test pins the
    //! erase→reattach shape under tree borrows; logical assertions are minimal — these fail when
    //! Miri reports UB, not on values.

    use super::*;
    use crate::builtins::default_scope;
    use crate::machine::core::RuntimeArena;
    use crate::machine::model::KObject;
    use crate::machine::{BindingIndex, Scope};

    /// A `NodeScope::Anchored` erases a genuinely run-lived scope to a lifetime-free `ScopePtr`
    /// (`erase_static`) and reattaches it (`reattach_bounded`) at read — the fabrication the
    /// scheduler performs each step for an `Anchored` slot. Mirrors the erase→reattach pair plus a
    /// subsequent arena mutation through a sibling pointer; fails on UB, not values.
    #[test]
    fn node_scope_anchored_erase_reattach_roundtrip() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let v = arena.alloc_object(KObject::Number(7.0));
        scope
            .bind_value("k".to_string(), v, BindingIndex::BUILTIN)
            .unwrap();

        let ns = NodeScope::Anchored(ScopePtr::erase_static(scope));
        let NodeScope::Anchored(ptr) = &ns else {
            unreachable!("constructed Anchored")
        };
        // Reattach with a borrow bounded by `&ns`; read a binding back, then mutate the arena
        // through a sibling pointer while the reattached scope is still live.
        let reattached: &Scope<'_> = unsafe { ptr.reattach_bounded() };
        assert!(matches!(reattached.lookup("k"), Some(KObject::Number(n)) if *n == 7.0));
        let _other = arena.alloc_object(KObject::Number(8.0));
        assert!(reattached.lookup("k").is_some());
    }
}
