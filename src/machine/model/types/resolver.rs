//! Scheduler-aware type-name elaboration. Walks a [`TypeExpr`] against a [`Scope`],
//! threads a "currently elaborating" set so recursive type definitions short-circuit to
//! [`KType::RecursiveRef`] instead of deadlocking on their own placeholder, and returns
//! [`ElabResult::Park`] when a referenced type-binding placeholder hasn't finalized so
//! the caller can install dep edges and re-run the elaboration on wake.
//!
//! Type-name bindings live in [`Scope::bindings`]'s `types` map; consumers go through
//! [`elaborate_type_expr`] when scope-aware lookup is needed or [`KType::from_type_expr`]
//! when only the builtin table matters.

use std::collections::HashSet;

use crate::machine::core::{Resolution, Scope, ScopeId};
use crate::machine::model::ast::{TypeExpr, TypeParams};
use crate::machine::NodeId;

use super::ktype::{KType, UserTypeKind};

#[cfg(test)]
mod tests;

/// Outcome of one elaboration walk over a `TypeExpr`.
#[derive(Debug)]
pub enum ElabResult<'a> {
    /// Fully elaborated. Whether to `Mu`-wrap rides on the elaborator's
    /// `fired_self_ref_for` set, not on this variant.
    Done(KType<'a>),
    /// Referenced type-binding placeholders haven't finalized. Caller installs park
    /// edges on every producer and re-runs the elaboration when they terminalize.
    Park(Vec<NodeId>),
    /// Bare leaf didn't resolve and isn't a builtin.
    Unbound(String),
}

impl<'a> ElabResult<'a> {
    /// Reduce sub-elaboration results with precedence **Unbound > Park > Done**.
    /// `Ok` preserves input order; `Err` carries the first `Unbound` or merged `Park`
    /// producers.
    fn collect<I: IntoIterator<Item = ElabResult<'a>>>(
        results: I,
    ) -> Result<Vec<KType<'a>>, ElabResult<'a>> {
        let iter = results.into_iter();
        let (lower, _) = iter.size_hint();
        let mut dones: Vec<KType<'a>> = Vec::with_capacity(lower);
        let mut parks: Vec<NodeId> = Vec::new();
        let mut unbound: Option<String> = None;
        for r in iter {
            match r {
                ElabResult::Done(kt) => dones.push(kt),
                ElabResult::Park(ps) => parks.extend(ps),
                ElabResult::Unbound(m) if unbound.is_none() => unbound = Some(m),
                ElabResult::Unbound(_) => {}
            }
        }
        if let Some(m) = unbound {
            Err(ElabResult::Unbound(m))
        } else if !parks.is_empty() {
            Err(ElabResult::Park(parks))
        } else {
            Ok(dones)
        }
    }
}

/// Per-elaboration-walk state.
///
/// - `threaded`: binder names currently being elaborated, so a self-reference becomes
///   `RecursiveRef` instead of parking on its own placeholder.
/// - `fired_self_ref_for`: which threaded names actually fired a back-reference;
///   drives the caller's `KType::Mu` wrap decision.
/// - `current_decl_*`: SCC context. When set, the `Resolution::Placeholder` arm records
///   dependency edges into `pending_types` and runs DFS cycle detection from
///   `current_decl_name`. `None` for non-binder elaboration (FN signatures, LET RHS,
///   ascription) so those sites never touch `pending_types`.
pub struct Elaborator<'s, 'a> {
    pub scope: &'s Scope<'a>,
    pub threaded: HashSet<String>,
    pub fired_self_ref_for: HashSet<String>,
    pub current_decl_name: Option<String>,
    pub current_decl_kind: Option<UserTypeKind<'a>>,
    pub current_decl_scope_id: Option<ScopeId>,
}

impl<'s, 'a> Elaborator<'s, 'a> {
    pub fn new(scope: &'s Scope<'a>) -> Self {
        Self {
            scope,
            threaded: HashSet::new(),
            fired_self_ref_for: HashSet::new(),
            current_decl_name: None,
            current_decl_kind: None,
            current_decl_scope_id: None,
        }
    }

    pub fn with_threaded<I: IntoIterator<Item = String>>(mut self, names: I) -> Self {
        self.threaded.extend(names);
        self
    }

    /// Seed SCC context: the `Resolution::Placeholder` arm will record dependency
    /// edges into `pending_types` and run cycle detection from `name`. The matching
    /// `PendingTypeEntry` must already be installed before the walk starts.
    pub fn with_current_decl(
        mut self,
        name: String,
        kind: UserTypeKind<'a>,
        scope_id: ScopeId,
    ) -> Self {
        self.current_decl_name = Some(name);
        self.current_decl_kind = Some(kind);
        self.current_decl_scope_id = Some(scope_id);
        self
    }
}

/// Walk a `TypeExpr` against the elaborator's scope. Bare leaves route through the
/// threaded set first (recursive back-edge), then `Scope::resolve_type`, then
/// `Scope::resolve` for the placeholder path, and finally `KType::from_name` so
/// fixture scopes that skip builtin registration still resolve builtin names.
/// Parameterized shapes (`:(LIST OF X)`, `:(MAP K -> V)`) no longer reach this
/// walk — they sub-Dispatch through the standalone dispatcher.
pub fn elaborate_type_expr<'a>(el: &mut Elaborator<'_, 'a>, t: &TypeExpr) -> ElabResult<'a> {
    match (&t.name, &t.params) {
        (name, TypeParams::None) => {
            if el.threaded.contains(name) {
                el.fired_self_ref_for.insert(name.clone());
                return ElabResult::Done(KType::RecursiveRef(name.clone()));
            }
            if let Some(kt) = el.scope.resolve_type(name) {
                return ElabResult::Done(kt.clone());
            }
            match el.scope.resolve(name) {
                Resolution::Placeholder(id) => {
                    // Record the edge unconditionally: the parked-on name may not be in
                    // `pending_types` yet, but DFS sees the persistent edge list later
                    // and closes the cycle when the second binder records its reciprocal
                    // edge. Trivial self-cycles (`LET T = T`) are caught earlier by the
                    // dispatch driver's eager-resolve pass.
                    if let Some(decl) = el.current_decl_name.clone() {
                        el.scope.bindings().record_pending_edge(&decl, name.clone());
                        if let Some(members) = detect_pending_cycle(el.scope, &decl) {
                            close_type_cycle(el.scope, &members);
                            if let Some(kt) = el.scope.resolve_type(name) {
                                return ElabResult::Done(kt.clone());
                            }
                        }
                    }
                    ElabResult::Park(vec![id])
                }
                // `from_name` is tried first in both arms so fixture scopes that skip
                // builtin registration still resolve builtin names. The split only
                // affects the miss message: a `Value` resolution means the name *is*
                // bound, just in the value language, so the diagnostic must name the
                // type-language / value-language layering rather than read as an
                // unknown-name failure (see design/typing/functors.md).
                Resolution::Value(_) => match KType::<'a>::from_name(name) {
                    Some(kt) => ElabResult::Done(kt),
                    None => ElabResult::Unbound(format!(
                        "`{name}` is value-language only — a type slot needs a type-language \
                         binder (a builtin type, a `LET {name} = <type>` alias, or a module/signature)"
                    )),
                },
                Resolution::UnboundName => match KType::<'a>::from_name(name) {
                    Some(kt) => ElabResult::Done(kt),
                    None => ElabResult::Unbound(format!("unknown type name `{name}`")),
                },
            }
        }
    }
}

/// DFS over `pending_types`' adjacency lists from `start`. Returns the cycle's
/// members (discovery order, root-first) if any path leads back to `start`.
///
/// Names referenced by edges but not themselves in `pending_types` (a binder not yet
/// dispatched, or a non-binder placeholder) are leaf-terminated: their out-edges
/// aren't recorded yet, so there's nothing further to follow.
pub(crate) fn detect_pending_cycle(scope: &Scope<'_>, start: &str) -> Option<Vec<String>> {
    let pending = scope.bindings().pending_types();
    if !pending.contains_key(start) {
        return None;
    }
    let mut stack: Vec<(String, usize)> = vec![(start.to_string(), 0)];
    let mut on_path: HashSet<String> = HashSet::new();
    on_path.insert(start.to_string());

    while let Some((node, idx)) = stack.last().cloned() {
        let edges = pending
            .get(&node)
            .map(|e| e.edges.clone())
            .unwrap_or_default();
        if idx >= edges.len() {
            stack.pop();
            on_path.remove(&node);
            continue;
        }
        let next = edges[idx].clone();
        stack.last_mut().unwrap().1 = idx + 1;
        if next == start {
            // Closed back to the origin; the live stack is the cycle, root-first.
            return Some(stack.iter().map(|(n, _)| n.clone()).collect());
        }
        if on_path.contains(&next) || !pending.contains_key(&next) {
            // Inner cycle not involving `start`, or a leaf. The outer cycle from
            // `start` may still exist via another edge.
            continue;
        }
        on_path.insert(next.clone());
        stack.push((next, 0));
    }
    None
}

/// Synchronous SCC cycle-close. Installs each member's payload-empty identity
/// (`KType::UserType { kind, scope_id, name }`, schema not yet elaborated) into
/// `bindings.types` so cross-member `resolve_type` lookups succeed on the very next
/// call. Each member's own finalize replaces this sentinel with the schema-bearing
/// identity via `Scope::register_type_upsert`.
///
/// `pending_types` entries are left in place on purpose: each member's finalize is the
/// sole remover of its own entry. The elaborator rebuilt inside each finalize sees
/// `bindings.types` populated and so never re-enters this function.
fn close_type_cycle(scope: &Scope<'_>, members: &[String]) {
    // Snapshot under a single `pending_types` read borrow; release before calling into
    // Scope methods that take their own borrows.
    let identities: Vec<(String, UserTypeKind, ScopeId)> = {
        let pending = scope.bindings().pending_types();
        members
            .iter()
            .map(|n| {
                let entry = pending
                    .get(n)
                    .expect("cycle member must be in pending_types when cycle-close fires");
                (n.clone(), entry.kind.clone(), entry.scope_id)
            })
            .collect()
    };
    for (name, kind, scope_id) in identities {
        let identity = KType::UserType {
            kind,
            scope_id,
            name: name.clone(),
        };
        // Recover the cycle member's installed `BindingIndex` from its placeholder so
        // the identity installs at the lexical position the eventual finalize will use.
        // Lookup is visibility-unfiltered (cycle-close runs outside any consumer's
        // chain). The `unwrap_or` fallback is defensive: missing placeholder here is
        // an upstream programming error, not a soft-recovery point.
        let bind_index = scope
            .ancestors()
            .find_map(|s| {
                if !matches!(
                    s.bindings().lookup_value(&name, None),
                    Some(crate::machine::core::Resolution::Placeholder(_))
                ) {
                    return None;
                }
                s.bindings().placeholder_index(&name)
            })
            .unwrap_or(crate::machine::core::BindingIndex {
                idx: 0,
                nominal_binder: true,
            });
        scope.cycle_close_install_identity(name, identity, bind_index);
    }
}
