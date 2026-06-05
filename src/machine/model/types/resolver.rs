//! Scheduler-aware type-name elaboration. Walks a [`TypeName`] against a [`Scope`],
//! threads a "currently elaborating" set so recursive type definitions short-circuit to
//! the transient [`KType::RecursiveRef`] instead of deadlocking on their own placeholder,
//! and returns [`ElabResult::Park`] when a referenced type-binding placeholder hasn't
//! finalized so the caller can install dep edges and re-run the elaboration on wake.
//!
//! When a placeholder reference closes a strongly-connected component (detected via
//! [`detect_pending_cycle`]), [`seal_type_cycle`] packages the members into one shared
//! [`RecursiveSet`] and installs a [`KType::SetRef`] into each member's `bindings.types`
//! entry; the members fill their schemas as they finalize.
//!
//! Type-name bindings live in [`Scope::bindings`]'s `types` map; consumers go through
//! [`elaborate_type_expr`] when scope-aware lookup is needed or [`KType::from_type_expr`]
//! when only the builtin table matters.

use std::collections::HashSet;
use std::rc::Rc;

use crate::machine::core::{Resolution, Scope, ScopeId};
use crate::machine::model::ast::TypeName;
use crate::machine::NodeId;

use super::ktype::KType;
use super::recursive_set::{NominalKind, NominalMember, RecursiveSet};

#[cfg(test)]
mod tests;

/// Outcome of one elaboration walk over a `TypeName`.
#[derive(Debug)]
pub enum ElabResult<'a> {
    /// Fully elaborated. Self / forward-sibling references appear as transient
    /// `RecursiveRef(name)` leaves, sealed into `SetLocal` indices at the member's finalize.
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
/// - `current_decl_*`: SCC context. When set, the `Resolution::Placeholder` arm records
///   dependency edges into `pending_types` and runs DFS cycle detection from
///   `current_decl_name`. `None` for non-binder elaboration (FN signatures, LET RHS,
///   ascription) so those sites never touch `pending_types`.
pub struct Elaborator<'s, 'a> {
    pub scope: &'s Scope<'a>,
    pub threaded: HashSet<String>,
    pub current_decl_name: Option<String>,
    pub current_decl_kind: Option<NominalKind>,
    pub current_decl_scope_id: Option<ScopeId>,
}

impl<'s, 'a> Elaborator<'s, 'a> {
    pub fn new(scope: &'s Scope<'a>) -> Self {
        Self {
            scope,
            threaded: HashSet::new(),
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
    pub fn with_current_decl(mut self, name: String, kind: NominalKind, scope_id: ScopeId) -> Self {
        self.current_decl_name = Some(name);
        self.current_decl_kind = Some(kind);
        self.current_decl_scope_id = Some(scope_id);
        self
    }
}

/// Walk a `TypeName` against the elaborator's scope. Bare leaves route through the
/// threaded set first (recursive back-edge), then `Scope::resolve_type`, then
/// `Scope::resolve` for the placeholder path, and finally `KType::from_name` so
/// fixture scopes that skip builtin registration still resolve builtin names.
/// Parameterized shapes (`:(LIST OF X)`, `:(MAP K -> V)`) no longer reach this
/// walk — they sub-Dispatch through the standalone dispatcher.
pub fn elaborate_type_expr<'a>(el: &mut Elaborator<'_, 'a>, t: &TypeName) -> ElabResult<'a> {
    let name = t.as_str();
    if el.threaded.contains(name) {
        // Self / forward-sibling reference inside a type-definition body: a transient
        // `RecursiveRef`, sealed into a `SetLocal` index when the member finalizes.
        return ElabResult::Done(KType::RecursiveRef(name.to_string()));
    }
    if let Some(kt) = el.scope.resolve_type(name) {
        return ElabResult::Done(kt.clone());
    }
    match el.scope.resolve(name) {
        Resolution::Placeholder(id) => {
            // Record the edge unconditionally: the parked-on name may not be in
            // `pending_types` yet, but DFS sees the persistent edge list later
            // and seals the cycle when the second binder records its reciprocal
            // edge. Trivial self-cycles (`LET T = T`) are caught earlier by the
            // dispatch driver's eager-resolve pass.
            if let Some(decl) = el.current_decl_name.clone() {
                el.scope
                    .bindings()
                    .record_pending_edge(&decl, name.to_string());
                if let Some(members) = detect_pending_cycle(el.scope, &decl) {
                    seal_type_cycle(el.scope, &members);
                    // Resolving the parked-on name now hits the just-installed `SetRef`.
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

/// Seal an SCC into one shared [`RecursiveSet`] and pre-install a `SetRef` into each
/// member's `bindings.types` entry, so cross-member `resolve_type` lookups succeed on the
/// very next call. Members are `pending` (schema unfilled); each member's own finalize
/// fills its schema (converting transient `RecursiveRef(name)` leaves to `SetLocal(index)`)
/// against this same set, recovered from its installed `SetRef`.
///
/// `pending_types` entries are left in place on purpose: each member's finalize is the
/// sole remover of its own entry. The elaborator rebuilt inside each finalize sees
/// `bindings.types` populated and so never re-enters this function.
fn seal_type_cycle(scope: &Scope<'_>, members: &[String]) {
    // Snapshot under a single `pending_types` read borrow; release before calling into
    // Scope methods that take their own borrows.
    let infos: Vec<(String, NominalKind, ScopeId)> = {
        let pending = scope.bindings().pending_types();
        members
            .iter()
            .map(|n| {
                let entry = pending
                    .get(n)
                    .expect("cycle member must be in pending_types when seal fires");
                (n.clone(), entry.kind, entry.scope_id)
            })
            .collect()
    };
    // One `Rc` shared by every member; intra-set references resolve against it by index.
    let set = Rc::new(RecursiveSet::new(
        infos
            .iter()
            .map(|(n, kind, sid)| NominalMember::pending(n.clone(), *sid, *kind))
            .collect(),
    ));
    for (index, (name, _, _)) in infos.into_iter().enumerate() {
        let identity = KType::SetRef {
            set: Rc::clone(&set),
            index,
        };
        // Recover the cycle member's installed `BindingIndex` from its placeholder so
        // the identity installs at the lexical position the eventual finalize will use.
        // Lookup is visibility-unfiltered (seal runs outside any consumer's chain). The
        // `unwrap_or` fallback is defensive: a missing placeholder here is an upstream
        // programming error, not a soft-recovery point.
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

/// Outcome of [`finalize_nominal_member`].
pub enum SealOutcome<'a> {
    /// The member sealed (or was already sealed); the arena reference is its `SetRef`
    /// identity, ready to wrap in a `KObject::KTypeValue`.
    Sealed(&'a KType<'a>),
    /// A transient `RecursiveRef(name)` named no set member — a sealing bug surfaced as a
    /// shape error rather than a dangling reference.
    DanglingRef(String),
    /// The name already binds a different type (a redeclaration); the install raised
    /// `Rebind`, propagated to the binder.
    Rebind(crate::machine::core::KError),
}

/// Seal a nominal type's elaborated schema into its [`RecursiveSet`] member and install the
/// `SetRef` identity into `bindings.types[name]`. Three cases collapse here:
///
/// 1. **Recursive member** — `bindings.types[name]` already holds a `SetRef` (pre-installed
///    by [`seal_type_cycle`]); reuse that set + index.
/// 2. **Non-recursive type** — no pre-install; mint a *singleton* set of one `pending`
///    member at index 0 (a pure self-recursive type with no sibling still has a self-edge,
///    handled identically since its own name is in the singleton's `index_of`).
/// 3. **Already sealed** — the member's schema is filled (a parallel finalize ran first);
///    short-circuit and return the existing identity.
///
/// In every case the schema's transient `RecursiveRef(name)` leaves are sealed to
/// `SetLocal(index)` against the (singleton or SCC) set before the member is filled.
#[allow(clippy::result_large_err)]
pub fn finalize_nominal_member<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    scope_id: ScopeId,
    kind: NominalKind,
    build_schema: impl FnOnce(&Rc<RecursiveSet<'a>>) -> SchemaSealResult<'a>,
    bind_index: crate::machine::core::BindingIndex,
) -> SealOutcome<'a> {
    // Recover the seal's pre-install (if any), distinguishing it from a genuine prior type:
    // - An existing `SetRef` whose member is still *pending* (unfilled) is the seal's
    //   contribution for *this* declaration — reuse its set + index.
    // - An existing `SetRef` whose member is filled, with the same `(scope_id, kind)`, is a
    //   parallel finalize of *this* declaration — short-circuit on it (idempotent).
    // - Any other existing `SetRef` is a genuine prior type of this name; mint a fresh
    //   singleton so the install path below raises the `Rebind` a redeclaration deserves.
    let pre_installed = match scope.bindings().lookup_type(name, None) {
        Some(KType::SetRef { set, index }) if !set.member(*index).is_filled() => {
            Some((Rc::clone(set), *index))
        }
        Some(kt @ KType::SetRef { set, index }) => {
            let member = set.member(*index);
            if member.scope_id == scope_id && member.kind == kind {
                return SealOutcome::Sealed(kt);
            }
            None
        }
        _ => None,
    };
    let (set, index) = match pre_installed {
        Some(pair) => pair,
        None => {
            // Non-recursive (or a redeclaration): a singleton over this one member.
            let set = Rc::new(RecursiveSet::new(vec![NominalMember::pending(
                name.to_string(),
                scope_id,
                kind,
            )]));
            (set, 0)
        }
    };
    // Build + seal the schema (intra-set `RecursiveRef` / `SetRef` → `SetLocal`).
    let schema = match build_schema(&set) {
        SchemaSealResult::Ok(schema) => schema,
        SchemaSealResult::Dangling(missing) => return SealOutcome::DanglingRef(missing),
    };
    set.member(index).fill(schema);
    // Install the `SetRef` identity. A non-equal existing entry (a redeclaration) surfaces
    // as `Rebind`, propagated to the binder.
    let identity = KType::SetRef {
        set: Rc::clone(&set),
        index,
    };
    match scope.register_type_upsert(name.to_string(), identity, bind_index) {
        Ok(kt_ref) => SealOutcome::Sealed(kt_ref),
        Err(e) => SealOutcome::Rebind(e),
    }
}

/// Outcome of [`finalize_nominal_member`].
pub enum SchemaSealResult<'a> {
    /// The schema sealed cleanly.
    Ok(super::recursive_set::NominalSchema<'a>),
    /// A transient `RecursiveRef` named no set member — a sealing bug.
    Dangling(String),
}
