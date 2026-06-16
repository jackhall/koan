//! Dispatch-layer submission: the one entry point that turns a `KExpression` into a submitted
//! dispatch slot. It owns the AST-shaped work the scheduler must not — binder-install (which
//! name/overload a binder-shaped call declares), the recursive pre-submission of eager argument
//! slots, and the submission-time placeholder install that makes forward references park. The
//! scheduler exposes only [`Scheduler::submit_node`] (a generic slot allocator) and the
//! `Scope::install_*` primitives; this function orchestrates them.
//!
//! Binders can appear as arbitrary nested subexpressions, so this runs on *every* dispatch
//! submission, not just block statements. See
//! [design/execution-model.md § Submission-time binder install and recursive
//! sub-Dispatch](../../../../design/execution-model.md#submission-time-binder-install-and-recursive-sub-dispatch).

use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::UntypedKey;
use crate::machine::model::{KType, SignatureElement};
use crate::machine::{BindingIndex, FunctionLookup, KFunction, LexicalFrame, NodeId, Scope};

use super::super::nodes::NodeScope;
use super::super::runtime::KoanRuntime;

/// Submission-time binder-install info — see the module docs for the per-bucket eager-slot mask
/// rules.
struct BinderInstall {
    key: BinderKey,
    eager_slot_mask: Vec<bool>,
}

/// The two install channels a binder may use, mutually exclusive per binder.
enum BinderKey {
    Name(String),
    Bucket(UntypedKey),
}

/// Find the binder overload (if any) the dispatching scope's chain declares for `expr`, and the
/// eager-slot mask its bucket admits. Pure dispatch semantics: it reads only the function table
/// and signatures, never scheduler state.
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

impl<'run> KoanRuntime<'run> {
    /// Submit `expr` as a dispatch slot against `scope` (with handle `node_scope` and
    /// `explicit_chain`, resolved by the calling submission wrapper). Computes binder-install,
    /// pre-submits the eager argument slots as sub-dispatches (so a nested binder's placeholder
    /// installs at the same outermost step), allocates the slot via [`Scheduler::submit_node`], then
    /// stamps the binder's placeholder on the scope — before the slot is ever popped, so a later
    /// sibling parks rather than surfacing `UnboundName` / `DispatchFailed`.
    pub(in crate::machine::execute) fn submit_dispatch<'step>(
        &mut self,
        expr: KExpression<'run>,
        scope: &'step Scope<'step>,
        node_scope: NodeScope,
        explicit_chain: Option<std::rc::Rc<LexicalFrame>>,
    ) -> NodeId {
        // Resolve the chain once so the recursive pre-subs inherit the parent's lexical chain (and
        // therefore its visibility index); pass it back to `submit_node` explicitly so it does not
        // re-derive a detached one.
        let chain = explicit_chain
        .or_else(|| self.sched.active_chain_clone())
        .expect("every dispatched node has a chain — submission outside enter_block / ambient active_chain is a bug");
        let install = extract_binder_install(&expr, scope);
        let pre_subs: Vec<(usize, NodeId)> = match &install {
            Some(install) => {
                let mut subs = Vec::new();
                for (i, part) in expr.parts.iter().enumerate() {
                    if !install.eager_slot_mask.get(i).copied().unwrap_or(false) {
                        continue;
                    }
                    let ExpressionPart::Expression(boxed) = &part.value else {
                        continue;
                    };
                    let sub_expr = (**boxed).clone();
                    let sub_id =
                        self.submit_dispatch(sub_expr, scope, node_scope, Some(chain.clone()));
                    subs.push((i, sub_id));
                }
                subs
            }
            None => Vec::new(),
        };
        let id = self.sched.submit_node(
            super::decide_with_presubs(expr, pre_subs),
            node_scope,
            Some(chain.clone()),
        );
        if let Some(install) = install {
            // Stamp the placeholder at the binder's lexical position — the SAME `BindingIndex` the
            // eventual `register_*` call at finalize installs. Installs are best-effort: lenient when
            // `data[name]` is already a KFunction or the same slot re-installs.
            let bind_index = BindingIndex::value(chain.index);
            match install.key {
                BinderKey::Name(name) => {
                    let _ = scope.install_placeholder(name, id, bind_index);
                }
                BinderKey::Bucket(bucket) => {
                    let _ = scope.install_pending_overload(bucket, id, bind_index);
                }
            }
        }
        id
    }
}
