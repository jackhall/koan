//! In-flight named-type binder tracking. [`super::Bindings`] embeds a [`PendingTypes`] by
//! value and delegates the surface methods. A binder records itself here for its body's
//! duration so a consumer referencing an *earlier* still-finalizing type can find the
//! producer node to park on (the finalize gate in `resolve_type_expr`).
//!
//! MODULE does not participate — module bodies park on the outer scheduler,
//! not on type-name resolution inside elaboration.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;

use crate::machine::model::ast::KExpression;
use crate::machine::model::types::KKind;

use super::super::scope_id::ScopeId;

/// `schema_expr` is the unelaborated body; `kind` seeds the sealed `NominalMember`'s
/// surface family. The entry's presence marks the binder as in-flight so a consumer can
/// park on it.
pub struct PendingTypeEntry<'a> {
    pub kind: KKind,
    pub scope_id: ScopeId,
    pub schema_expr: KExpression<'a>,
}

pub struct PendingTypes<'a> {
    map: RefCell<HashMap<String, PendingTypeEntry<'a>>>,
}

impl<'a> PendingTypes<'a> {
    pub fn new() -> Self {
        Self {
            map: RefCell::new(HashMap::new()),
        }
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
    pub fn insert(&'a self, name: String, entry: PendingTypeEntry<'a>) -> PendingBinderGuard<'a> {
        let mut map = self.map.borrow_mut();
        if map.contains_key(&name) {
            panic!(
                "insert_pending_type = `{name}` already in flight — duplicate dispatch \
                 reached body-entry, which the placeholder install should have blocked",
            );
        }
        map.insert(name.clone(), entry);
        PendingBinderGuard {
            pending: self,
            name,
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
