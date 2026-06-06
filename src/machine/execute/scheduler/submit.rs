use std::rc::Rc;

use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::{KType, SignatureElement};
use crate::machine::{
    BindingIndex, CatchFinish, CombineFinish, FunctionLookup, KFunction, LexicalFrame, NodeId,
    Scope,
};

use super::super::dispatch::DispatchState;
use super::super::nodes::{work_park_producers, Node, NodeWork};
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

fn extract_binder_install<'a>(
    expr: &KExpression<'a>,
    scope: &'a Scope<'a>,
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
        let picked: Option<(&KFunction<'a>, BinderKey)> = bucket_fns.iter().find_map(|f| {
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

impl<'a> Scheduler<'a> {
    /// Submit an unresolved expression for the scheduler to dispatch + execute
    /// against `scope`. The only public way to add work.
    pub fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        self.add(NodeWork::dispatch(expr), scope)
    }

    /// Schedule a `Combine` slot. `owned_subs` are sub-Dispatches this Combine
    /// allocated (cascade-freed on success); `park_producers` are existing
    /// sibling slots it splices but does not own (kept alive past success via
    /// `Notify` edges). The finish closure sees results as
    /// `[park_producers..., owned_subs...]`.
    pub fn add_combine(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        scope: &'a Scope<'a>,
        finish: CombineFinish<'a>,
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

    /// Schedule a `Catch` slot.
    pub fn add_catch(
        &mut self,
        from: NodeId,
        scope: &'a Scope<'a>,
        finish: CatchFinish<'a>,
    ) -> NodeId {
        self.add(NodeWork::Catch { from, finish }, scope)
    }

    /// Inherit-from-ambient entry point. When there is no ambient chain
    /// (REPL-style submission, test fixtures), synthesize a detached chain so
    /// the visibility predicate treats every scope as "complete" — the
    /// submission then reads through to every previously-bound name.
    pub(in crate::machine::execute::scheduler) fn add(
        &mut self,
        work: NodeWork<'a>,
        scope: &'a Scope<'a>,
    ) -> NodeId {
        if self.active_chain.is_some() {
            self.add_with_chain(work, scope, None)
        } else {
            self.add_with_chain(work, scope, Some(LexicalFrame::detached()))
        }
    }

    /// Single funnel for node creation. `explicit_chain` is `Some` for
    /// `enter_block`-routed submissions (top-level, MODULE / SIG body, TRY body
    /// success-as-block, FN body invoke), `None` otherwise (inherits
    /// `self.active_chain`).
    pub(super) fn add_with_chain(
        &mut self,
        work: NodeWork<'a>,
        scope: &'a Scope<'a>,
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
                    let sub_id = self.add_with_chain(
                        NodeWork::dispatch(sub_expr),
                        scope,
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
        let frame = self.active_frame.clone();
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
            scope,
            frame,
            reserve_frame: None,
            function: None,
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
