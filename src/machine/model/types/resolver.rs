//! Scheduler-aware type-name elaboration. Walks a [`TypeIdentifier`] against a [`Scope`], gating
//! each bare leaf against a [`LexicalFrame`] so a type declared lexically later is invisible
//! — a forward type reference is a position error, not a silent success. A self-reference
//! short-circuits to the transient [`KType::RecursiveRef`] via the threaded binder name, and
//! a co-declared `RECURSIVE TYPES` member via the scope's shared set; both seal to a
//! [`KType::SetLocal`] index at finalize. A reference to an *earlier* type still finalizing
//! returns [`ElabResult::Park`] so the caller re-runs the elaboration on wake.
//!
//! Type-name bindings live in [`Scope::bindings`]'s `types` map; consumers go through
//! [`elaborate_type_identifier`] when scope-aware lookup is needed or [`KType::from_type_identifier`]
//! when only the builtin table matters.

use std::collections::HashSet;
use std::rc::Rc;

use crate::machine::core::{LexicalFrame, Resolution, Scope, ScopeId};
use crate::machine::model::ast::TypeIdentifier;
use crate::machine::NodeId;

use super::kkind::KKind;
use super::ktype::KType;
use super::recursive_set::{NominalMember, RecursiveSet};

#[cfg(test)]
mod tests;

/// Outcome of one elaboration walk over a `TypeIdentifier`.
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

/// Per-elaboration-walk state.
///
/// - `threaded`: binder names currently being elaborated, so a self-reference becomes
///   `RecursiveRef` instead of parking on its own placeholder.
/// - `chain`: the lexical position the bare-leaf resolution is gated against.
pub struct Elaborator<'b, 'a> {
    pub scope: &'b Scope<'a>,
    pub threaded: HashSet<String>,
    /// Lexical chain the bare-leaf resolution is gated against, so a type declared
    /// lexically later than this elaboration's position is invisible. `None` is the
    /// unfiltered mode (test/builtin scopes with no chain).
    pub chain: Option<Rc<LexicalFrame>>,
}

impl<'b, 'a> Elaborator<'b, 'a> {
    pub fn new(scope: &'b Scope<'a>) -> Self {
        Self {
            scope,
            threaded: HashSet::new(),
            chain: None,
        }
    }

    pub fn with_threaded<I: IntoIterator<Item = String>>(mut self, names: I) -> Self {
        self.threaded.extend(names);
        self
    }

    /// Gate bare-leaf resolution against `chain`: a type binding lexically later than
    /// this position is invisible, so a forward type reference misses instead of
    /// resolving across source order.
    pub fn with_chain(mut self, chain: Option<Rc<LexicalFrame>>) -> Self {
        self.chain = chain;
        self
    }
}

/// Walk a `TypeIdentifier` against the elaborator's scope. Bare leaves route through the
/// threaded set first (recursive back-edge), then `Scope::resolve_type`, then
/// `Scope::resolve` for the placeholder path, and finally `KType::from_name` so
/// fixture scopes that skip builtin registration still resolve builtin names.
/// Parameterized shapes (`:(LIST OF X)`, `:(MAP K -> V)`) no longer reach this
/// walk — they sub-Dispatch through the standalone dispatcher.
pub fn elaborate_type_identifier<'a>(el: &mut Elaborator<'_, 'a>, t: &TypeIdentifier) -> ElabResult<'a> {
    let name = t.as_str();
    if el.threaded.contains(name) {
        // Self / forward-sibling reference inside a type-definition body: a transient
        // `RecursiveRef`, sealed into a `SetLocal` index when the member finalizes.
        return ElabResult::Done(KType::RecursiveRef(name.to_string()));
    }
    if let Some(set) = el.scope.nearest_recursive_set() {
        // A bare leaf naming a member of the enclosing `RECURSIVE TYPES` block is a
        // co-declared sibling (or self): lower to the same transient `RecursiveRef`
        // back-edge, sealed to a `SetLocal` index at the member's finalize. This is the
        // block's threading — it makes cross-references resolve independent of source
        // order, with no forward placeholder. Checked before `resolve_type` so a member
        // lowers to the back-edge rather than the set's pre-installed external `SetRef`.
        if set.index_of(name).is_some() {
            return ElabResult::Done(KType::RecursiveRef(name.to_string()));
        }
    }
    if let Some(kt) = el.scope.resolve_type_with_chain(name, el.chain.as_deref()) {
        return ElabResult::Done(kt.clone());
    }
    match el.scope.resolve_with_chain(name, el.chain.as_deref()) {
        // A *visible* placeholder is an earlier-declared type still finalizing: park on its
        // producer and re-elaborate when it terminalizes. A forward reference (a later
        // sibling) is filtered out by the chain before reaching here, so it falls through
        // to `UnboundName` — a position error, not a park. Mutual recursion across the cut
        // is expressed with a `RECURSIVE TYPES` block, threaded above.
        Resolution::Placeholder(id) => ElabResult::Park(vec![id]),
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

/// Outcome of [`finalize_nominal_member`].
pub enum SealOutcome<'a> {
    /// The member sealed (or was already sealed); the arena reference is its `SetRef`
    /// identity, ready to wrap in a `Carried::Type`.
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
/// 1. **Block member** — `bindings.types[name]` already holds a `SetRef` (pre-installed by
///    the `RECURSIVE TYPES` block over its shared set); reuse that set + index.
/// 2. **Non-recursive / self-recursive type** — no pre-install; mint a *singleton* set of
///    one `pending` member at index 0 (a self-recursive type's own name is in the
///    singleton's `index_of`, so its self-reference seals to `SetLocal(0)`).
/// 3. **Already sealed** — the member's schema is filled (a parallel finalize ran first);
///    short-circuit and return the existing identity.
///
/// In every case the schema's transient `RecursiveRef(name)` leaves are sealed to
/// `SetLocal(index)` against the (singleton or shared) set before the member is filled.
#[allow(clippy::result_large_err)]
pub fn finalize_nominal_member<'a>(
    scope: &Scope<'a>,
    name: &str,
    scope_id: ScopeId,
    kind: KKind,
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
