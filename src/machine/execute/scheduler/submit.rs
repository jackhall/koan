use std::rc::Rc;

use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::{KType, SignatureElement};
use crate::machine::{BindingIndex, FunctionLookup, KFunction, LexicalFrame, NodeId, Scope};

use super::super::dispatch::DispatchState;
use super::super::nodes::{work_park_producers, CallFrame, Node, NodeScope, NodeWork};
use super::super::CombineFinish;
use super::dep_graph::work_owned_edges;
use super::Scheduler;

/// Submission-time binder-install info — see [design/execution-model.md
/// § Dispatch-time name placeholders](../../../../design/execution-model.md#dispatch-time-name-placeholders)
/// for the per-bucket eager-slot mask rules.
struct BinderInstall {
    key: BinderKey,
    eager_slot_mask: Vec<bool>,
}

/// The two install channels a binder may use, mutually exclusive per binder.
enum BinderKey {
    Name(String),
    Bucket(crate::machine::model::types::UntypedKey),
}

fn extract_binder_install<'run, 'step>(
    expr: &KExpression<'run>,
    scope: &'step Scope<'step>,
) -> Option<BinderInstall> {
    let key = expr.untyped_key();
    // Visibility-unfiltered lookup: this runs before the dispatch's chain is
    // assembled, so `chain_cutoff = None`.
    for s in scope.ancestors() {
        let FunctionLookup { overloads, .. } = s.bindings().lookup_function(&key, None);
        if overloads.is_empty() {
            continue;
        }
        let bucket_fns = overloads;
        let picked: Option<(&KFunction<'step>, BinderKey)> = bucket_fns.iter().find_map(|f| {
            if let Some(name) = f.binder_name.and_then(|extractor| extractor(expr)) {
                Some((*f, BinderKey::Name(name)))
            } else {
                f.binder_bucket
                    .and_then(|extractor| extractor(expr))
                    .map(|bucket| (*f, BinderKey::Bucket(bucket)))
            }
        });
        let Some((picked_fn, install_key)) = picked else {
            continue;
        };
        // Eager mask: AND across every binder overload in the bucket — a
        // "binder overload" being any function declaring `binder_name` OR
        // `binder_bucket`.
        let mut mask: Vec<bool> = picked_fn
            .signature
            .elements
            .iter()
            .map(|el| match el {
                SignatureElement::Argument(arg) => arg.ktype != KType::KExpression,
                SignatureElement::Keyword(_) => false,
            })
            .collect();
        for other in bucket_fns.iter() {
            if other.binder_name.is_none() && other.binder_bucket.is_none() {
                continue;
            }
            for (i, el) in other.signature.elements.iter().enumerate() {
                if i >= mask.len() {
                    break;
                }
                if let SignatureElement::Argument(arg) = el {
                    if arg.ktype == KType::KExpression {
                        mask[i] = false;
                    }
                }
            }
        }
        return Some(BinderInstall {
            key: install_key,
            eager_slot_mask: mask,
        });
    }
    None
}

impl<'run> Scheduler<'run> {
    /// Submit an unresolved expression for the scheduler to dispatch + execute
    /// against `scope`. The only public way to add work.
    pub fn add_dispatch(&mut self, expr: KExpression<'run>, scope: &'run Scope<'run>) -> NodeId {
        self.add(NodeWork::dispatch(expr), scope)
    }

    /// Submit each `statement` as a fresh lexical block over `scope`. Inherent entry point for
    /// REPL-style / test submission of a block of top-level statements, so callers need not hold
    /// the [`SchedulerHandle`](super::super::SchedulerHandle) trait. Delegates to the trait method.
    pub fn enter_block(
        &mut self,
        scope_id: crate::machine::ScopeId,
        statements: Vec<KExpression<'run>>,
        scope: &'run Scope<'run>,
    ) -> Vec<NodeId> {
        <Self as super::super::SchedulerHandle>::enter_block(self, scope_id, statements, scope)
    }

    /// Schedule a `Combine` slot against an explicit `scope`. `owned_subs` are sub-Dispatches
    /// this Combine allocated (cascade-freed on success); `park_producers` are existing sibling
    /// slots it splices but does not own (kept alive past success via `Notify` edges). The finish
    /// closure sees results as `[park_producers..., owned_subs...]`. Test fixture entry point; the
    /// run path uses [`Scheduler::combine_here`] / `add_combine_in_frame`.
    #[cfg(test)]
    pub(in crate::machine::execute) fn add_combine(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        scope: &'run Scope<'run>,
        finish: CombineFinish<'run>,
    ) -> NodeId {
        let park_count = park_producers.len();
        let mut deps = park_producers;
        deps.extend(owned_subs);
        self.add(
            NodeWork::Combine {
                deps,
                park_count,
                finish,
            },
            scope,
        )
    }

    /// Inherit-from-ambient entry point. When there is no ambient chain
    /// (REPL-style submission, test fixtures), synthesize a detached chain so
    /// the visibility predicate treats every scope as "complete" — the
    /// submission then reads through to every previously-bound name.
    pub(in crate::machine::execute::scheduler) fn add(
        &mut self,
        work: NodeWork<'run>,
        scope: &'run Scope<'run>,
    ) -> NodeId {
        if self.active_chain.is_some() {
            self.add_with_chain(work, scope, None)
        } else {
            self.add_with_chain(work, scope, Some(LexicalFrame::detached()))
        }
    }

    /// Run-lifetime submission funnel. `explicit_chain` is `Some` for
    /// `enter_block`-routed submissions (top-level, MODULE / SIG body, TRY body
    /// success-as-block, FN body invoke), `None` otherwise (inherits
    /// `self.active_chain`). Decides the slot's [`NodeScope`] handle — `Yoked` when this
    /// runs inside the per-call frame whose own child is `scope` (re-projected from the
    /// cart), else `Root` — then hands off to [`Self::submit_node`].
    pub(super) fn add_with_chain(
        &mut self,
        work: NodeWork<'run>,
        scope: &'run Scope<'run>,
        explicit_chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        // Establish the run frame on the first run-lifetime submission (top-level run scope),
        // so every top-level slot carries a frame cart and `active_frame` is never `None` during
        // a top-level step. Adopts the passed scope without minting a child.
        if self.run_frame.is_none() {
            self.run_frame = Some(crate::machine::CallArena::adopting(scope));
        }
        // Single-cart storage: when this submission runs inside a per-call frame whose own
        // child is the very scope passed, store a payload-less `Yoked` and let the read
        // boundary re-project from the frame cart — no fabricated `&'run` persisted. Any other
        // scope (a run-root scope, or a frame sub-scope the frame does not directly back)
        // genuinely lives at `'run`, so it stays `Anchored`.
        let node_scope = match &self.active_frame {
            Some(f)
                if std::ptr::eq(
                    f.scope() as *const Scope<'_> as *const (),
                    scope as *const Scope<'_> as *const (),
                ) =>
            {
                NodeScope::Yoked
            }
            _ => NodeScope::Anchored(scope),
        };
        self.submit_node(work, scope, node_scope, explicit_chain)
    }

    /// Submit `work` against the executing slot's own [`NodeScope`] handle (`active_node_scope`):
    /// `Anchored(&'run)` re-uses the genuine run-lived borrow the slot already holds; `Yoked`
    /// re-projects from the active frame cart. The transient `scope` for binder-install is the
    /// same handle materialized. Backs the `*_here` methods — the honest
    /// re-dispatch-against-my-own-scope path.
    pub(super) fn submit_here(&mut self, work: NodeWork<'run>) -> NodeId {
        let node_scope = self
            .active_node_scope
            .expect("a slot step installs active_node_scope before the body submits");
        let explicit_chain = self.active_chain.is_none().then(LexicalFrame::detached);
        match node_scope {
            NodeScope::Anchored(scope) => {
                self.submit_node(work, scope, NodeScope::Anchored(scope), explicit_chain)
            }
            NodeScope::Yoked => {
                let frame = self
                    .active_frame
                    .clone()
                    .expect("a Yoked slot step has an active frame");
                let scope = frame.scope_for_bind();
                self.submit_node(work, scope, NodeScope::Yoked, explicit_chain)
            }
        }
    }

    /// Dispatch `expr` against the executing slot's own scope handle. Inherent sibling of
    /// the `SchedulerHandle::add_dispatch_here` trait method, callable from inherent
    /// scheduler code.
    pub(in crate::machine::execute) fn dispatch_here(&mut self, expr: KExpression<'run>) -> NodeId {
        self.submit_here(NodeWork::dispatch(expr))
    }

    /// Schedule a `Combine` against the executing slot's own scope handle. Inherent sibling
    /// of `SchedulerHandle::add_combine_here`.
    pub(in crate::machine::execute) fn combine_here(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        finish: CombineFinish<'run>,
    ) -> NodeId {
        let park_count = park_producers.len();
        let mut deps = park_producers;
        deps.extend(owned_subs);
        self.submit_here(NodeWork::Combine {
            deps,
            park_count,
            finish,
        })
    }

    /// Node-creation core, shared by the run-lifetime [`Self::add_with_chain`] and the framed
    /// [`Self::add_dispatch_with_chain_in_frame`]. `scope` is read only transiently
    /// (binder-install, placeholder install, `pre_subs` recursion) and never retained — the
    /// node keeps a `NodeScope<'run>` handle, not this borrow — so it is clamped to a `'step`
    /// read: a run scope and a `scope_for_bind` re-projection both shorten into it.
    /// `node_scope` is the pre-decided slot handle the caller built (`Root` at `'run` for a run
    /// scope, `Yoked` for a framed one).
    pub(super) fn submit_node<'step>(
        &mut self,
        work: NodeWork<'run>,
        scope: &'step Scope<'step>,
        node_scope: NodeScope<'run>,
        explicit_chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        // Compute the chain FIRST so recursive sub-submissions inherit the
        // parent's lexical chain (and therefore its visibility index).
        let chain = explicit_chain.or_else(|| self.active_chain.clone()).expect(
            "every dispatched node has a chain — submission outside enter_block / \
                 ambient active_chain is a bug",
        );
        // Nested binder pre-submission — see [design/execution-model.md
        // § Submission-time binder install and recursive sub-Dispatch](../../../../design/execution-model.md#submission-time-binder-install-and-recursive-sub-dispatch).
        let placeholder_install: Option<BinderInstall> = match &work {
            NodeWork::Dispatch { expr, .. } => extract_binder_install(expr, scope),
            _ => None,
        };
        let pre_subs: Vec<(usize, NodeId)> =
            if let (Some(install), NodeWork::Dispatch { expr, .. }) =
                (placeholder_install.as_ref(), &work)
            {
                let mut subs = Vec::new();
                for (i, part) in expr.parts.iter().enumerate() {
                    if !install.eager_slot_mask.get(i).copied().unwrap_or(false) {
                        continue;
                    }
                    let ExpressionPart::Expression(boxed) = &part.value else {
                        continue;
                    };
                    let sub_expr = (**boxed).clone();
                    // Pass the parent's chain explicitly: without it, a top-level
                    // submission with no ambient `active_chain` would assign a
                    // detached chain, bypassing index-gated visibility and letting
                    // a forward sibling resolve.
                    let sub_id = self.submit_node(
                        NodeWork::dispatch(sub_expr),
                        scope,
                        node_scope,
                        Some(chain.clone()),
                    );
                    subs.push((i, sub_id));
                }
                subs
            } else {
                Vec::new()
            };
        // Rewrite Dispatch work to carry the pre-submitted sub-NodeIds.
        // `pre_subs` are not read-deps of this Dispatch (they become owned-deps
        // of the Bind Phase 4 spawns), so `work_owned_edges` is unaffected.
        let work = match work {
            NodeWork::Dispatch {
                expr,
                state: prior_state,
            } => {
                let prior_pre_subs = match prior_state {
                    DispatchState::Initialized(i) => i.pre_subs,
                    _ => unreachable!("add_with_chain only receives Dispatch in Initialized state"),
                };
                debug_assert!(
                    prior_pre_subs.is_empty(),
                    "add_with_chain only receives Dispatch with empty pre_subs",
                );
                let _ = prior_pre_subs;
                NodeWork::Dispatch {
                    expr,
                    state: DispatchState::initialized(pre_subs),
                }
            }
            other => other,
        };
        let owned_edges = work_owned_edges(&work);
        let no_owned = owned_edges.is_empty();
        // Top-level submissions (no active frame) fall back to the run frame, so every slot
        // carries a cart and `active_frame` is `Some` during its step. `run_frame` is
        // established by `add_with_chain` before the first submission, so the fallback is
        // always `Some` — the cart is non-optional node state.
        let cart = self.active_frame.clone().unwrap_or_else(|| {
            self.run_frame
                .clone()
                .expect("run_frame established by add_with_chain before any submission")
        });
        // Stamp the placeholder at the binder's lexical position — the SAME `BindingIndex`
        // the eventual `register_*` call at finalize installs.
        let bind_index_for_placeholder = placeholder_install
            .as_ref()
            .map(|_| BindingIndex::value(chain.index));
        let pending_owned: Vec<NodeId> = owned_edges
            .iter()
            .map(|e| e.node_id())
            .filter(|p| !self.is_result_ready(*p))
            .collect();
        let pending_park: Vec<NodeId> = work_park_producers(&work)
            .iter()
            .copied()
            .filter(|p| !self.is_result_ready(*p))
            .collect();
        let no_park = work_park_producers(&work).is_empty();
        let id = self.store.alloc_slot(Node {
            work,
            scope: node_scope,
            frame: CallFrame {
                cart,
                reserve: None,
                contract: None,
            },
            chain,
        });
        self.deps.install_for_slot(id, owned_edges, &pending_owned);
        for p in &pending_park {
            self.deps.add_park_edge(*p, id);
        }
        if let Some(install) = placeholder_install {
            let bind_index = bind_index_for_placeholder.unwrap_or(BindingIndex::BUILTIN);
            // Installs are best-effort: `install_placeholder` is lenient when
            // `data[name]` is already a KFunction or the same slot re-installs;
            // `install_pending_overload` appends alongside sibling installs for
            // the same bucket (each pending entry wakes consumers parked on it).
            match install.key {
                BinderKey::Name(name) => {
                    let _ = scope.install_placeholder(name, id, bind_index);
                }
                BinderKey::Bucket(bucket) => {
                    let _ = scope.install_pending_overload(bucket, id, bind_index);
                }
            }
        }
        if pending_owned.is_empty() && pending_park.is_empty() {
            if self.active_frame.is_none() && no_owned && no_park {
                self.queues.push_fresh(id.index());
            } else {
                self.queues.push_in_flight_submit(id.index());
            }
        }
        id
    }
}
