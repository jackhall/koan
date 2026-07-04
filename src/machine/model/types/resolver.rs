//! Scheduler-aware type-name elaboration. Walks a [`TypeIdentifier`] against a [`Scope`], gating
//! each bare leaf against a [`LexicalFrame`] so a type declared lexically later is invisible
//! — a forward type reference is a position error, not a silent success. A self-reference
//! short-circuits to the transient [`KType::RecursiveRef`] via the threaded binder name, and
//! a co-declared `RECURSIVE TYPES` member via the scope's shared set; both seal to a
//! [`KType::SetLocal`] index at finalize. A reference to an *earlier* type still finalizing
//! returns [`TypeResolution::Park`] so the caller re-runs the elaboration on wake.
//!
//! Type-name bindings live in [`Scope::bindings`]'s `types` map; consumers go through
//! [`elaborate_type_identifier`] when scope-aware lookup is needed or [`KType::from_type_identifier`]
//! when only the builtin table matters.

use std::collections::HashSet;
use std::rc::Rc;

use crate::machine::core::{FrameSet, LexicalFrame, NameLookup, Scope, ScopeId};
use crate::machine::model::ast::TypeIdentifier;
use crate::machine::NodeId;

use super::kkind::KKind;
use super::ktype::KType;
use super::recursive_set::{NominalMember, RecursiveSet};

#[cfg(test)]
mod tests;

/// Outcome of resolving a `TypeIdentifier` to a `T`, shared across layers: model uses
/// `TypeResolution<KType>`, execute uses `TypeResolution<TypeHit>`. `Park` carries the producer
/// `NodeId`s a still-finalizing referent waits on; `Unbound` the miss diagnostic. The payload-free
/// arms let a layer lift `Done` through [`Self::and_then_done`] and forward the rest unchanged.
#[derive(Debug)]
pub enum TypeResolution<T> {
    Done(T),
    Park(Vec<NodeId>),
    Unbound(String),
}

impl<T> TypeResolution<T> {
    /// Transform the `Done` payload, which may itself resolve to a `Park` / `Unbound` (the execute
    /// layer's finalize gate turns a `Done` into a `Park` when a referenced type is still in
    /// flight). `Park` / `Unbound` forward unchanged.
    pub fn and_then_done<U>(self, f: impl FnOnce(T) -> TypeResolution<U>) -> TypeResolution<U> {
        match self {
            TypeResolution::Done(payload) => f(payload),
            TypeResolution::Park(producers) => TypeResolution::Park(producers),
            TypeResolution::Unbound(message) => TypeResolution::Unbound(message),
        }
    }
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
/// threaded set first (recursive back-edge), then `resolve_type_with_chain`, then
/// `resolve_with_chain` for the placeholder path, and finally the builtin-table fallback via
/// [`KType::from_type_identifier`] so fixture scopes that skip builtin registration still
/// resolve builtin names. Parameterized shapes sub-Dispatch through the standalone dispatcher,
/// not this walk.
pub fn elaborate_type_identifier<'a>(
    el: &mut Elaborator<'_, 'a>,
    t: &TypeIdentifier,
) -> TypeResolution<KType<'a>> {
    let name = t.as_str();
    if el.threaded.contains(name) {
        // Self / forward-sibling reference inside a type-definition body: a transient
        // `RecursiveRef`, sealed into a `SetLocal` index when the member finalizes.
        return TypeResolution::Done(KType::RecursiveRef(name.to_string()));
    }
    if let Some(set) = el.scope.nearest_recursive_set() {
        // A bare leaf naming an enclosing `RECURSIVE TYPES` member is a co-declared sibling
        // (or self): the block's threading makes cross-references resolve independent of source
        // order. Checked before `resolve_type_with_chain` so a member lowers to the back-edge
        // rather than the set's pre-installed external `SetRef`.
        if set.index_of(name).is_some() {
            return TypeResolution::Done(KType::RecursiveRef(name.to_string()));
        }
    }
    match el.scope.resolve_type_with_chain(name, el.chain.as_deref()) {
        Some(NameLookup::Bound(kt)) => return TypeResolution::Done(kt.clone()),
        // A visible placeholder is an earlier-declared type still finalizing: park on its
        // producer and re-elaborate when it terminalizes. A forward reference is filtered by the
        // chain before reaching here — a position error, not a park. Mutual recursion across the
        // cut uses a `RECURSIVE TYPES` block, threaded above.
        Some(NameLookup::Parked(id)) => return TypeResolution::Park(vec![id]),
        None => {}
    }
    // Not a type. Consult the value side only to sharpen the miss message: a name bound (or
    // binding) in the value language gets the layering diagnostic, an unknown name the
    // unknown-name failure. The builtin-table fallback via `from_type_identifier` is tried in
    // both arms so fixture scopes that skip builtin registration still resolve builtin names.
    match el.scope.resolve_with_chain(name, el.chain.as_deref()) {
        Some(NameLookup::Bound(_)) | Some(NameLookup::Parked(_)) => {
            match KType::<'a>::from_type_identifier(t) {
                Ok(kt) => TypeResolution::Done(kt),
                Err(_) => TypeResolution::Unbound(format!(
                    "`{name}` is value-language only — a type slot needs a type-language \
                     binder (a builtin type, a `LET {name} = <type>` alias, or a module/signature)"
                )),
            }
        }
        None => match KType::<'a>::from_type_identifier(t) {
            Ok(kt) => TypeResolution::Done(kt),
            Err(msg) => TypeResolution::Unbound(msg),
        },
    }
}

/// Outcome of [`finalize_nominal_member`].
pub enum SealOutcome<'a> {
    /// The member sealed (or was already sealed); the region reference is its `SetRef`
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
    // - `SetRef` with a pending (unfilled) member: the seal's contribution for this
    //   declaration — reuse its set + index.
    // - `SetRef` with a filled member and matching `(scope_id, kind)`: a parallel finalize of
    //   this declaration — short-circuit (idempotent).
    // - any other `SetRef`: a genuine prior type of this name; mint a fresh singleton so the
    //   install path below raises the `Rebind` a redeclaration deserves.
    let pre_installed = match scope
        .bindings()
        .lookup_type(name, None)
        .and_then(NameLookup::bound)
    {
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
    match scope.register_type_upsert(name.to_string(), identity, bind_index, FrameSet::empty()) {
        Ok(kt_ref) => SealOutcome::Sealed(kt_ref),
        Err(e) => SealOutcome::Rebind(e),
    }
}

/// Outcome of the `build_schema` closure passed to [`finalize_nominal_member`].
pub enum SchemaSealResult<'a> {
    /// The schema sealed cleanly.
    Ok(super::recursive_set::NominalSchema<'a>),
    /// A transient `RecursiveRef` named no set member — a sealing bug.
    Dangling(String),
}
