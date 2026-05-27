use std::rc::Rc;

use crate::machine::{
    BindingIndex, CatchFinish, CombineFinish, FunctionLookup, KFunction, LexicalFrame, NodeId,
    Scope,
};
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::{KType, SignatureElement};

use super::super::nodes::{Node, NodeWork, work_park_producers};
use super::dep_graph::work_owned_edges;
use super::Scheduler;

/// Submission-time binder-install info. Walks `scope` and its outer chain looking
/// for a function in `functions[expr.untyped_key()]` whose `binder_name` OR
/// `binder_bucket` extractor returns `Some` for `expr`; the first matching
/// extractor wins. The picked function's `is_nominal_binder` flag rides through
/// too so the install at `add_with_chain` can stamp the matching D7 visibility
/// carve-out on the [`BindingIndex`]. Submission-time install lets a later
/// sibling park on the placeholder before the producer slot is popped from the
/// FIFO.
///
/// `eager_slot_mask` records which argument slot indices are *unanimously* eager
/// (non-`KExpression`) across every binder overload in the bucket that matched
/// this expression. The recursive-submission walk in
/// [`Scheduler::add_with_chain`] pre-submits exactly those slots; any slot a
/// single overload tags `KExpression` (lazy) cannot be pre-submitted because the
/// eventual dispatch may resolve to that overload. See
/// `roadmap/dispatch_fix/nested-binder-submission.md`.
struct BinderInstall {
    key: BinderKey,
    is_nominal_binder: bool,
    eager_slot_mask: Vec<bool>,
}

/// The two install channels a binder may use. Mutually exclusive at the
/// binder-definition level — `LET`/`STRUCT`/`UNION`/`SIG`/`MODULE` bind exactly
/// one name (`Name`); `FN`/`FUNCTOR` register a function via inner-call bucket
/// key (`Bucket`). The previous shape (two `Option<_>` fields with only the
/// `(Some, None)` and `(None, Some)` combinations legal) is gone — the dichotomy
/// rides in the type.
enum BinderKey {
    /// Name-keyed binder: installs `placeholders[name]` at submission time so a
    /// later sibling's bare-name reference parks on this slot.
    Name(String),
    /// Bucket-keyed binder: installs a `pending_overloads[bucket]` entry at
    /// submission time so a sibling bare-arg call form (`(MAKESET IntOrd)`)
    /// parks on this slot even though `functions[bucket]` won't be live until
    /// the body finalizes.
    Bucket(crate::machine::model::types::UntypedKey),
}

fn extract_binder_install<'a>(
    expr: &KExpression<'a>,
    scope: &'a Scope<'a>,
) -> Option<BinderInstall> {
    let key = expr.untyped_key();
    // Submission-time extraction runs before any chain is assembled for this
    // dispatch (the caller is `add_with_chain`, which only computes the chain
    // after this returns), so the lookup walk is visibility-unfiltered —
    // `chain_cutoff = None` at every scope.
    for s in scope.ancestors() {
        let bucket_fns = match s.bindings().lookup_function(&key, None) {
            FunctionLookup::Bucket(b) => b,
            FunctionLookup::Pending(_) | FunctionLookup::None => continue,
        };
        // Find the first overload whose `binder_name` OR `binder_bucket`
        // extractor matches. Either marks the call as binder-shaped; the picked
        // overload's `is_nominal_binder` and `signature` shape drive the eager
        // mask intersection across the rest of the bucket. Name-keyed binders
        // (LET / STRUCT / UNION / SIG / MODULE) and bucket-keyed binders
        // (FN / FUNCTOR) are mutually exclusive at the binder-definition level —
        // each overload supplies exactly one extractor — so the chosen
        // `BinderKey` is unambiguous per pick.
        let picked: Option<(&KFunction<'a>, BinderKey)> =
            bucket_fns.iter().find_map(|f| {
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
        // Build the eager mask: start from the picked overload's signature
        // shape, then AND in every other binder overload in the bucket. "Binder
        // overload" here means any function declaring `binder_name` OR
        // `binder_bucket` — both classes pre-submit their eager parts.
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
            is_nominal_binder: picked_fn.is_nominal_binder,
            eager_slot_mask: mask,
        });
    }
    None
}

impl<'a> Scheduler<'a> {
    /// Submit an unresolved expression for the scheduler to dispatch + execute against
    /// `scope`. The only public way to add work; `Bind`/`Combine` are internal scaffolding
    /// spawned during a `Dispatch` node's run.
    ///
    /// Routes through [`Self::add`]; the ambient-vs-root chain decision lives there.
    pub fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        self.add(NodeWork::dispatch(expr), scope)
    }

    /// Schedule a `Combine` slot. `owned_subs` are sub-Dispatches this Combine
    /// allocated itself (cascade-freed on success); `park_producers` are
    /// existing sibling slots whose values it splices but does not own (kept
    /// alive past the Combine's success via `Notify` edges). The Combine's
    /// finish closure sees results in `[park_producers..., owned_subs...]`
    /// order. See `SchedulerHandle::add_combine`.
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
        self.add(NodeWork::Combine { deps, park_count, finish }, scope)
    }

    /// Schedule a `Catch` slot. See `SchedulerHandle::add_catch`.
    pub fn add_catch(
        &mut self,
        from: NodeId,
        scope: &'a Scope<'a>,
        finish: CatchFinish<'a>,
    ) -> NodeId {
        self.add(NodeWork::Catch { from, finish }, scope)
    }

    /// Inherit-from-ambient entry point. Sub-dispatches inside a builtin body (CONS-
    /// head, FN signature subs, list/dict literal items, ...) route here so they
    /// pick up the executing slot's `active_chain` without each call site naming
    /// it. Block-entry sites that compute their own chain call
    /// [`Self::add_with_chain`] directly.
    ///
    /// When there is no ambient chain (REPL-style submission, test fixtures
    /// poking at scheduler internals), synthesize a detached chain so the visibility
    /// predicate treats every scope as "complete" and the submission reads through
    /// to every previously-bound name. This is what lets a fixture call
    /// `run(scope, "(LET x = 1)")` then `run_one(scope, parse_one("x"))` and have
    /// the second submission see `x`.
    pub(super) fn add(&mut self, work: NodeWork<'a>, scope: &'a Scope<'a>) -> NodeId {
        if self.active_chain.is_some() {
            self.add_with_chain(work, scope, None)
        } else {
            self.add_with_chain(work, scope, Some(LexicalFrame::detached()))
        }
    }

    /// Single funnel for node creation. `explicit_chain` is `Some` for
    /// `enter_block`-routed submissions (top-level, MODULE / SIG body, TRY body
    /// success-as-block, FN body invoke), `None` for everything else (which then
    /// inherits `self.active_chain`).
    ///
    /// Debug-asserts that *some* chain is in scope — top-level routes through
    /// `enter_block(root.id, ...)` so the only path that hits this with both
    /// `explicit_chain: None` and `active_chain: None` is a bug.
    pub(super) fn add_with_chain(
        &mut self,
        work: NodeWork<'a>,
        scope: &'a Scope<'a>,
        explicit_chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        // Compute the chain FIRST so recursive sub-submissions inherit the
        // parent's lexical chain (and therefore its visibility index). Reading
        // `self.active_chain` after the recursive `self.add(...)` call would be
        // unreliable since sub-submissions don't write `active_chain`.
        let chain = explicit_chain
            .or_else(|| self.active_chain.clone())
            .expect(
                "every dispatched node has a chain — submission outside enter_block / \
                 ambient active_chain is a bug",
            );
        // Decide up front whether this submission is a binder-shaped Dispatch.
        // If so: (a) extract the install info as today, and (b) recursively submit
        // each eager Expression-shaped argument slot as a sub-Dispatch, so any
        // nested binder's own placeholder installs at this outermost submission
        // point. The collected `pre_subs` rides through into the parent's
        // `NodeWork::Dispatch { pre_subs }` so Phase 4 reuses them instead of
        // allocating fresh sub-Dispatches. See
        // `roadmap/dispatch_fix/nested-binder-submission.md`.
        let placeholder_install: Option<BinderInstall> = match &work {
            NodeWork::Dispatch { expr, .. } => extract_binder_install(expr, scope),
            _ => None,
        };
        let pre_subs: Vec<(usize, NodeId)> = if let (
            Some(install),
            NodeWork::Dispatch { expr, .. },
        ) = (placeholder_install.as_ref(), &work)
        {
            let mut subs = Vec::new();
            for (i, part) in expr.parts.iter().enumerate() {
                // `eager_slot_mask[i]` is true only for slots EVERY binder
                // overload in the bucket tags as non-`KExpression`. Lazy slots
                // (e.g. FN's signature / return-type-KExpression overload / body,
                // FUNCTOR / MODULE bodies) dispatch in the callee's scope at
                // body-invoke time — not here.
                if !install.eager_slot_mask.get(i).copied().unwrap_or(false) {
                    continue;
                }
                let ExpressionPart::Expression(boxed) = &part.value else { continue; };
                let sub_expr = (**boxed).clone();
                // Inherit the parent's chain so the sub-Dispatch's lexical index
                // matches the parent's — the same one its eventual Phase 4 Bind
                // dep would have used. Without this, a top-level submission with
                // no ambient `active_chain` would assign a detached chain (which
                // bypasses index-gated visibility entirely and lets a forward
                // sibling resolve).
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
        // Rewrite the work so the parent's Dispatch carries the pre-submitted
        // sub-NodeIds. Non-Dispatch variants pass through untouched. The pre-sub
        // re-bundling MUST happen before `work_owned_edges` reads the work shape,
        // but since `pre_subs` are not read-deps of the Dispatch (they become
        // owned-deps of the Bind that Phase 4 spawns), `work_owned_edges` returns
        // the same edges either way.
        let work = match work {
            NodeWork::Dispatch { expr, pre_subs: prior } => {
                // The submission entry points (`add_dispatch`, `add_with_chain` via
                // `add_dispatch_with_chain`, `literal`/`finish` re-Dispatches) all
                // construct `Dispatch` with empty `pre_subs`. A non-empty `prior`
                // would indicate a re-submission of an already-prepared Dispatch,
                // which the current callers never do.
                debug_assert!(
                    prior.is_empty(),
                    "add_with_chain only receives Dispatch with empty pre_subs",
                );
                let _ = prior;
                NodeWork::Dispatch { expr, pre_subs }
            }
            other => other,
        };
        let owned_edges = work_owned_edges(&work);
        let no_owned = owned_edges.is_empty();
        let frame = self.active_frame.clone();
        // Stamp the placeholder with the SAME `BindingIndex` the eventual `register_*`
        // call at finalize will install — `idx` is this slot's lexical position; the
        // D7 nominal-binder carve-out (true for STRUCT / named UNION / SIG / FUNCTOR /
        // MODULE) rides on the picked binder's `is_nominal_binder` flag.
        let bind_index_for_placeholder = placeholder_install.as_ref().map(|p| BindingIndex {
            idx: chain.index,
            nominal_binder: p.is_nominal_binder,
        });
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
        let id = self
            .store
            .alloc_slot(Node { work, scope, frame, function: None, chain });
        self.deps.install_for_slot(id, owned_edges, &pending_owned);
        for p in &pending_park {
            self.deps.add_park_edge(*p, id);
        }
        if let Some(install) = placeholder_install {
            // `bind_index_for_placeholder` is `Some` whenever `placeholder_install` is.
            let bind_index = bind_index_for_placeholder.unwrap_or(BindingIndex::BUILTIN);
            // Each binder uses exactly one install channel — `BinderKey` carries
            // the discriminant. Name-keyed (LET / STRUCT / UNION / SIG / MODULE)
            // installs `placeholders[name]`; bucket-keyed (FN / FUNCTOR) installs
            // a `pending_overloads[bucket]` entry. Each install is best-effort:
            // install_placeholder is lenient when `data[name]` is already a
            // KFunction or the same slot re-installs; install_pending_overload
            // appends an entry alongside any sibling installs for the same
            // bucket (each sibling's pending entry is a wake source for
            // consumers parked on the bucket).
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
