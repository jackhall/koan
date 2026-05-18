//! SCC pre-registration / cycle-detection bookkeeping for in-flight named-type
//! binders. Owns its own `RefCell<HashMap<...>>`; [`super::Bindings`] embeds a
//! [`PendingTypes`] by value and delegates the surface methods. The guard
//! references `PendingTypes` directly, so the parent façade does not need to
//! expose internal field access for cleanup.
//!
//! MODULE does not participate — module bodies park on the outer scheduler,
//! not on type-name resolution inside elaboration (see roadmap stage 3.2).

use std::cell::{Ref, RefCell};
use std::collections::HashMap;

use crate::machine::model::ast::KExpression;
use crate::machine::model::types::UserTypeKind;

use super::super::scope_id::ScopeId;

/// Per-binder state captured at the moment a STRUCT / named-UNION enters its
/// elaborator. `schema_expr` is the unelaborated body the cycle-close sweep
/// re-runs against the post-pre-registration scope; `kind` and `scope_id` are
/// the identity fields the cycle-close writes into `bindings.types` as
/// `KType::UserType { kind, scope_id, name }`; `edges` is the adjacency list
/// the elaborator appends to each time this binder parks on a fellow in-flight
/// binder's placeholder.
pub struct PendingTypeEntry<'a> {
    pub kind: UserTypeKind,
    pub scope_id: ScopeId,
    pub schema_expr: KExpression<'a>,
    pub edges: Vec<String>,
}

/// Owns the pending-type bookkeeping for one [`super::Bindings`]. The
/// `RefCell` is private to this module — all access goes through the
/// surface methods so the guard's `Drop` is the only path that touches
/// the map without going through the façade.
pub struct PendingTypes<'a> {
    map: RefCell<HashMap<String, PendingTypeEntry<'a>>>,
}

impl<'a> PendingTypes<'a> {
    pub fn new() -> Self {
        Self { map: RefCell::new(HashMap::new()) }
    }

    /// Read-only handle for the SCC pre-registration map. Same `Ref<'_, _>`
    /// semantics as the [`super::Bindings`] read accessors.
    pub fn get(&self) -> Ref<'_, HashMap<String, PendingTypeEntry<'a>>> {
        self.map.borrow()
    }

    /// Install a new in-flight binder entry and return an RAII guard whose Drop
    /// removes the entry. The caller binds the guard for the elaboration's
    /// lifetime; on the Park path the guard is moved into the `CombineFinish`
    /// closure so the entry survives the wait and is cleaned up when the
    /// closure returns.
    ///
    /// Panics on borrow conflict — pending-type writes happen at body-entry,
    /// outside the re-entrant `try_apply` hot path; a conflict here is a
    /// programming error. Panics on duplicate name — same scope cannot have
    /// two in-flight binders for one name (placeholders block the second
    /// dispatch from progressing this far).
    pub fn insert(
        &'a self,
        name: String,
        entry: PendingTypeEntry<'a>,
    ) -> PendingBinderGuard<'a> {
        let mut map = self.map.borrow_mut();
        if map.contains_key(&name) {
            panic!(
                "insert_pending_type = `{name}` already in flight — duplicate dispatch \
                 reached body-entry, which the placeholder install should have blocked",
            );
        }
        map.insert(name.clone(), entry);
        PendingBinderGuard { pending: self, name }
    }

    /// Append `to` to `from`'s adjacency list (no-op if `from` isn't a pending
    /// binder — the elaborator can be running under a non-binder context).
    /// Used by the elaborator's `Resolution::Placeholder` arm when the
    /// parked-on name is itself an in-flight binder.
    ///
    /// Panics on borrow conflict for the same reason as [`PendingTypes::insert`];
    /// deduplicates against existing edges so a re-elaboration that re-parks on
    /// the same name doesn't grow the list.
    pub fn record_edge(&self, from: &str, to: String) {
        let mut map = self.map.borrow_mut();
        if let Some(entry) = map.get_mut(from) {
            if !entry.edges.iter().any(|e| e == &to) {
                entry.edges.push(to);
            }
        }
    }

    /// Test helper: explicitly remove an entry to exercise the guard Drop's
    /// "tolerates absent entry" path.
    #[cfg(test)]
    pub fn remove(&self, name: &str) {
        self.map.borrow_mut().remove(name);
    }
}

impl<'a> Default for PendingTypes<'a> {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII handle returned by [`PendingTypes::insert`]. Dropping the guard removes
/// the matching entry. The guard is the *only* removal path on the happy and
/// error paths — struct_def / union body sites hold it for the duration of the
/// synchronous elaboration, and the Park path moves it into the `CombineFinish`
/// closure so the entry survives the wait and is cleaned up regardless of the
/// finish arm (Done, Err, second-Park).
///
/// `try_borrow_mut` in Drop is defensive: no caller is expected to hold the
/// pending-types borrow when a guard drops (the read-borrow in
/// `detect_pending_cycle` is released before any guard could drop, and
/// cycle-close holds only a short-lived read borrow). Silent skip is safe —
/// the entry persists until the next drain point, and no later code observes
/// a stale entry once the matching binder has finalized.
#[must_use = "PendingBinderGuard removes the pending-types entry on drop; \
              bind it for the elaboration's lifetime, or move it into the \
              CombineFinish closure on the Park path"]
pub struct PendingBinderGuard<'a> {
    pending: &'a PendingTypes<'a>,
    name: String,
}

impl<'a> Drop for PendingBinderGuard<'a> {
    fn drop(&mut self) {
        if let Ok(mut map) = self.pending.map.try_borrow_mut() {
            map.remove(&self.name);
        }
    }
}
