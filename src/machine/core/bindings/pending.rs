//! SCC pre-registration / cycle-detection bookkeeping for in-flight named-type
//! binders. [`super::Bindings`] embeds a [`PendingTypes`] by value and delegates
//! the surface methods.
//!
//! MODULE does not participate — module bodies park on the outer scheduler,
//! not on type-name resolution inside elaboration.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;

use crate::machine::model::ast::KExpression;
use crate::machine::model::types::UserTypeKind;

use super::super::scope_id::ScopeId;

/// `schema_expr` is the unelaborated body the cycle-close sweep re-runs against
/// the post-pre-registration scope; `edges` is the adjacency list to other
/// in-flight binders this one has parked on.
pub struct PendingTypeEntry<'a> {
    pub kind: UserTypeKind<'a>,
    pub scope_id: ScopeId,
    pub schema_expr: KExpression<'a>,
    pub edges: Vec<String>,
}

pub struct PendingTypes<'a> {
    map: RefCell<HashMap<String, PendingTypeEntry<'a>>>,
}

impl<'a> PendingTypes<'a> {
    pub fn new() -> Self {
        Self { map: RefCell::new(HashMap::new()) }
    }

    pub fn get(&self) -> Ref<'_, HashMap<String, PendingTypeEntry<'a>>> {
        self.map.borrow()
    }

    /// Install a new in-flight binder entry and return an RAII guard whose Drop
    /// removes the entry.
    ///
    /// Panics on borrow conflict — pending-type writes happen at body-entry,
    /// outside the re-entrant `try_apply` hot path. Panics on duplicate name —
    /// placeholders should block a second dispatch from reaching body-entry.
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
    /// binder). Deduplicates so re-elaboration doesn't grow the list.
    pub fn record_edge(&self, from: &str, to: String) {
        let mut map = self.map.borrow_mut();
        if let Some(entry) = map.get_mut(from) {
            if !entry.edges.iter().any(|e| e == &to) {
                entry.edges.push(to);
            }
        }
    }

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
/// the matching entry; this is the *only* removal path outside `#[cfg(test)]`.
///
/// `try_borrow_mut` in Drop is defensive: no caller is expected to hold the
/// pending-types borrow when a guard drops. Silent skip is safe — the entry
/// persists until the next drain point, and no later code observes a stale
/// entry once the matching binder has finalized.
#[must_use = "PendingBinderGuard removes the pending-types entry on drop; \
              bind it for the elaboration's lifetime"]
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
