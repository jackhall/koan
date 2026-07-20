//! Scheduler-aware type-name elaboration. Walks a [`TypeIdentifier`] against a [`Scope`], gating
//! each bare leaf against a [`LexicalFrame`] so a type declared lexically later is invisible
//! — a forward type reference is a position error, not a silent success. A name announced by the
//! ambient [`RecursiveGroupWindow`] short-circuits to that member's relative
//! [`TypeNode::Sibling`] handle, which the window's seal rewrites to an absolute member handle; a
//! window's own binder name (a `UNION`'s, which names no single member) resolves to the union of
//! every announced member. A reference to an *earlier* type still finalizing returns
//! [`TypeResolution::Park`] so the caller re-runs the elaboration on wake.
//!
//! Type-name bindings live in [`Scope::bindings`]'s `types` map; consumers go through
//! [`elaborate_type_identifier`] when scope-aware lookup is needed or [`KType::from_type_identifier`]
//! when only the builtin table matters.

use std::collections::HashSet;
use std::rc::Rc;

use crate::machine::core::{LexicalFrame, NameLookup, Scope};
use crate::machine::model::ast::TypeIdentifier;
use crate::machine::NodeId;

use super::kkind::KKind;
use super::ktype::KType;
use super::node::TypeNode;
use super::recursive_group_window::{RecursiveGroupWindow, RelativeSchema};
use super::registry::TypeRegistry;

#[cfg(test)]
mod tests;

/// Outcome of resolving a `TypeIdentifier` to a `T`, shared across layers: model uses
/// `TypeResolution<KType>`, execute uses `TypeResolution<&KType>`. `Park` carries the producer
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
/// - `threaded`: binder names currently being elaborated, so a self-reference resolves through the
///   ambient window instead of parking on its own placeholder.
/// - `window`: the declarator's own open window, when it owns one. A `RECURSIVE TYPES` block's
///   window rides the scope chain instead, because it spans several separately dispatched
///   declarations; this field carries the window of a declarator that opens and seals one within a
///   single elaboration.
/// - `chain`: the lexical position the bare-leaf resolution is gated against.
pub struct Elaborator<'b, 'a> {
    pub scope: &'b Scope<'a>,
    pub threaded: HashSet<String>,
    pub window: Option<Rc<RecursiveGroupWindow>>,
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
            window: None,
            chain: None,
        }
    }

    pub fn with_threaded<I: IntoIterator<Item = String>>(mut self, names: I) -> Self {
        self.threaded.extend(names);
        self
    }

    /// Elaborate against `window` — the declarator's own, taking precedence over any window the
    /// scope chain carries.
    pub fn with_window(mut self, window: Rc<RecursiveGroupWindow>) -> Self {
        self.window = Some(window);
        self
    }

    /// The window a co-declared name resolves against: this walk's own, else the nearest one on
    /// the scope chain (a `RECURSIVE TYPES` block's).
    pub fn window(&self) -> Option<Rc<RecursiveGroupWindow>> {
        self.window
            .clone()
            .or_else(|| self.scope.nearest_recursive_window())
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
pub fn elaborate_type_identifier(
    el: &mut Elaborator<'_, '_>,
    t: &TypeIdentifier,
    types: &TypeRegistry,
) -> TypeResolution<KType> {
    let name = t.as_str();
    // The relative-`Sibling` back-edge applies only while the window is open. Once it seals, its
    // members are bound to their absolute handles, and a member name resolves through the binding
    // below — returning the sealed identity, not a window-scoped relative index.
    if let Some(window) = el.window().filter(|w| !w.is_sealed()) {
        // A bare leaf naming a member of the ambient window is a co-declared sibling (or a
        // self-reference): it lowers to the relative `Sibling` handle, which the window's seal
        // rewrites to the member's absolute handle. Checked before `resolve_type_with_chain` so a
        // co-declared name takes the back-edge rather than any outer binding of the same name —
        // this is the one cross-order type-name resolution that survives strict lexical lookup.
        //
        // Only a binder-less window (a `RECURSIVE TYPES` block or a self-recursive newtype, whose
        // members are standalone types) resolves a bare member name this way. A `UNION`'s members
        // are *variants*, not standalone types: a bare `Node :Leaf` is an unknown-type error, and a
        // sibling variant is reached only through the binder (`:Tree`) or the qualified sigil
        // `:(Tree Leaf)` (handled in `typed_field_list`).
        if window.binder().is_none() {
            if let Some(index) = window.index_of(name) {
                return TypeResolution::Done(types.intern(TypeNode::Sibling(index)));
            }
        }
        // The window's own binder names no single member — a `UNION`'s name denotes the union of
        // every variant it declares (`Node :Tree` inside `UNION Tree = (…)`).
        if window.binder().as_deref() == Some(name) {
            return TypeResolution::Done(window.binder_union(types));
        }
        // A threaded binder the window has not announced yet: a forward reference inside a
        // declarator that discovers its members as it walks its own schema. Announcing it here
        // keeps the relative index stable, and the declarator's finalize reports any member left
        // unfilled as a reference to a type the declaration never made.
        if el.threaded.contains(name) {
            return TypeResolution::Done(window.sibling(name, KKind::NewType, types));
        }
    }
    match el.scope.resolve_type_with_chain(name, el.chain.as_deref()) {
        Some(NameLookup::Bound(kt)) => return TypeResolution::Done(*kt),
        // A visible placeholder is an earlier-declared type still finalizing: park on its
        // producer and re-elaborate when it terminalizes. A forward reference is filtered by the
        // chain before reaching here — a position error, not a park. Mutual recursion across the
        // cut uses a `RECURSIVE TYPES` block, threaded above.
        Some(NameLookup::Parked(id)) => return TypeResolution::Park(vec![id]),
        None => {}
    }
    // Not a type binding, and there is no value side to consult: the token-class partition
    // ([`Bindings::partition_guard`](crate::machine::core::Bindings)) commits a Type token to the
    // type universe, so a name reaching here can hold no value to layer a sharper miss over. What
    // remains is the builtin table — tried last so a fixture scope that skips builtin registration
    // still resolves builtin names — and then an unknown-name failure.
    match KType::from_type_identifier(t, types) {
        Ok(kt) => TypeResolution::Done(kt),
        Err(msg) => TypeResolution::Unbound(msg),
    }
}

/// Outcome of [`finalize_nominal_member`].
pub enum SealOutcome<'a> {
    /// The member sealed (or was already sealed); the region reference is its interned member
    /// handle, ready to wrap in a `Carried::Type`.
    Sealed(&'a KType),
    /// The member's schema filled, but its window still holds unfilled members, so no member has
    /// an identity yet. Only a `RECURSIVE TYPES` block reaches this: the block's own finish is the
    /// seal barrier, and it binds every member once the last one fills.
    Deferred,
    /// A reference named no member of the window — a sealing bug surfaced as a shape error rather
    /// than a dangling reference.
    DanglingRef(String),
    /// The name already binds a different type (a redeclaration); the install raised
    /// `Rebind`, propagated to the binder.
    Rebind(crate::machine::core::KError),
}

/// Fill a nominal type's elaborated schema into its window member and, once the window seals,
/// install the member's interned handle into `bindings.types[name]`. Three cases collapse here:
///
/// 1. **Block member** — the ambient `RECURSIVE TYPES` window already announces `name`; fill that
///    slot. Unless this fill is the block's last, the window stays open and the outcome is
///    [`SealOutcome::Deferred`] — no member has an identity until every member's content is known,
///    because identity is computed over the whole reference structure.
/// 2. **Standalone declaration** — no window announces `name`, so `window` is this declarator's
///    own one-member window (or a fresh one for a declarator that needs no elaboration): filling
///    its only member seals it, and a self-reference was already interned as `Sibling(0)`.
/// 3. **Already sealed** — a parallel finalize of this same declaration ran first; the window
///    hands back the same handles and the upsert is idempotent.
#[allow(clippy::result_large_err)]
pub fn finalize_nominal_member<'a>(
    scope: &Scope<'a>,
    window: &Rc<RecursiveGroupWindow>,
    name: &str,
    build_schema: impl FnOnce(&Rc<RecursiveGroupWindow>) -> RelativeSchema,
    bind_index: crate::machine::core::BindingIndex,
    types: &TypeRegistry,
) -> SealOutcome<'a> {
    let index = match window.index_of(name) {
        Some(index) => index,
        // The declarator handed a window that does not announce its own binder — a wiring bug, not
        // a user error, but reported as a dangling reference rather than a panic.
        None => return SealOutcome::DanglingRef(name.to_string()),
    };
    let schema = build_schema(window);
    let sealed = match window.fill_member(index, schema, types) {
        Some(sealed) => sealed,
        None => return SealOutcome::Deferred,
    };
    // A non-equal existing entry (a redeclaration) surfaces as `Rebind`, propagated to the binder.
    match scope.register_nominal_upsert(name.to_string(), sealed.members[index], bind_index) {
        Ok(kt_ref) => SealOutcome::Sealed(kt_ref),
        Err(e) => SealOutcome::Rebind(e),
    }
}

/// The window a declarator named `name` (of family `kind`) elaborates and seals against: the
/// ambient `RECURSIVE TYPES` window when it announces the name, else a fresh one-member window
/// this declaration owns outright.
pub fn declarator_window(scope: &Scope<'_>, name: &str, kind: KKind) -> Rc<RecursiveGroupWindow> {
    match scope.nearest_recursive_window() {
        Some(window) if window.holds(name) => window,
        _ => RecursiveGroupWindow::new(vec![(name.to_string(), kind)], None),
    }
}
