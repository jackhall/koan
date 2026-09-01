//! Scheduler-aware type-name elaboration. Walks a type token's [`TypeSymbol`] against a [`Scope`],
//! gating each bare leaf against a [`LexicalFrame`] so a type declared lexically later is invisible
//! — a forward type reference is a position error, not a silent success.
//!
//! A name the ambient declaration window announces is the one exception, and what it resolves to
//! depends on who is asking ([`TypeResolutionMode`]). For the member's own **declarator** it
//! short-circuits to that member's relative [`TypeNode::Sibling`] handle, which the window's seal
//! rewrites absolute; a binder name (a `UNION`'s, which names no single member) resolves to the
//! union of the members it owns. For a **consumer** it parks until the window seals and then reads
//! the absolute handle off the sealed window — never the relative one, which is meaningless outside
//! the window that minted it. A reference to an *earlier* type still finalizing returns
//! [`TypeResolution::Park`] so the caller re-runs the elaboration on wake.
//!
//! Type-name bindings live in [`Scope::bindings`]'s `types` map; consumers go through
//! [`elaborate_type_identifier`] when scope-aware lookup is needed or [`KType::from_symbol`]
//! when only the builtin table matters.

use std::collections::HashSet;
use std::rc::Rc;

use crate::machine::ProducerId;
use crate::machine::core::bindings::{TypeWritePolicy, WriteOp};
use crate::machine::core::{DeclarationSite, LexicalFrame, NameLookup, Scope};
use crate::machine::model::RunRegistries;
use crate::machine::model::labels::TypeSymbol;

use super::declaration_window::{DeclWindow, WindowView};
use super::kkind::KKind;
use super::ktype::{KType, render_label};
use super::node::TypeNode;
use super::recursive_group_window::RecursiveGroupWindow;

#[cfg(test)]
mod tests;

/// Outcome of resolving a type name to a `T`, shared across layers: both model and execute
/// use `TypeResolution<KType>` now that `KType` is a `Copy` handle. `Park` carries the binder
/// [`ProducerId`]s a still-finalizing referent waits on; `Unbound` the **name that missed** — the
/// symbol the lookup already held, so the miss costs no rendering and the spelling is read back
/// only where a diagnostic quotes it, through [`unknown_type_name`]. The payload-free
/// arms let a layer lift `Done` through [`Self::and_then_done`] and forward the rest unchanged.
#[derive(Debug)]
pub enum TypeResolution<T> {
    Done(T),
    Park(Vec<ProducerId>),
    Unbound(TypeSymbol),
}

impl<T> TypeResolution<T> {
    /// Transform the `Done` payload, which may itself resolve to a `Park` / `Unbound` (the execute
    /// layer's finalize gate turns a `Done` into a `Park` when a referenced type is still in
    /// flight). `Park` / `Unbound` forward unchanged.
    pub fn and_then_done<U>(self, f: impl FnOnce(T) -> TypeResolution<U>) -> TypeResolution<U> {
        match self {
            TypeResolution::Done(payload) => f(payload),
            TypeResolution::Park(sources) => TypeResolution::Park(sources),
            TypeResolution::Unbound(name) => TypeResolution::Unbound(name),
        }
    }
}

/// Who is asking the elaborator for a co-declared name, which decides what a still-open window may
/// hand back.
///
/// A **declarator** is building the schema a member's own identity is computed from: it takes the
/// relative [`TypeNode::Sibling`] back-edge, which the seal rewrites absolute. Parking it until the
/// window sealed would deadlock the group on its own producer.
///
/// A **consumer** — an FN signature, a LET ascription, any ordinary type position — must never
/// observe a relative handle: it is meaningless outside the window that minted it and would silently
/// never match at dispatch. A consumer waits for the seal and then reads the absolute handle.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TypeResolutionMode {
    Consumer,
    Declarator,
}

/// Per-elaboration-walk state.
///
/// - `threaded`: names currently being elaborated against an open window, so a reference to one
///   resolves through the window instead of parking on its placeholder. A declarator seeds its own
///   binder name plus every name its window announces, which is what lets a sub-dispatched sigil
///   body carry co-declared references as pre-resolved cells.
/// - `window`: the declaration window this walk elaborates against, when the caller is a
///   declarator. Setting it is what puts the walk in [`TypeResolutionMode::Declarator`]; every
///   other walk consults the ambient window the scope chain carries, as a consumer.
/// - `chain`: the lexical position the bare-leaf resolution is gated against.
pub struct Elaborator<'b, 'a> {
    pub scope: &'b Scope<'a>,
    pub threaded: HashSet<TypeSymbol>,
    window: Option<WindowView<'b, 'a>>,
    mode: TypeResolutionMode,
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
            mode: TypeResolutionMode::Consumer,
            chain: None,
        }
    }

    pub fn with_threaded<I: IntoIterator<Item = TypeSymbol>>(mut self, names: I) -> Self {
        self.threaded.extend(names);
        self
    }

    /// Elaborate against `window` as its **declarator**: the walk builds a member's own schema, so
    /// a co-declared name lowers to its relative sibling handle. Takes precedence over the ambient
    /// window the scope chain carries, so a nested same-named declaration cannot hijack an
    /// announced slot.
    pub fn with_window(mut self, window: WindowView<'b, 'a>) -> Self {
        self.window = Some(window);
        self.mode = TypeResolutionMode::Declarator;
        self
    }

    /// The window a co-declared name resolves against: this walk's own, else the ambient one the
    /// scope chain carries (a module body's announced group).
    pub fn window(&self) -> Option<WindowView<'_, 'a>> {
        self.window.or_else(|| {
            self.scope
                .nearest_declaration_window()
                .map(|(_, window)| WindowView::Announced(window))
        })
    }

    /// Gate bare-leaf resolution against `chain`: a type binding lexically later than
    /// this position is invisible, so a forward type reference misses instead of
    /// resolving across source order.
    pub fn with_chain(mut self, chain: Option<Rc<LexicalFrame>>) -> Self {
        self.chain = chain;
        self
    }
}

/// A consumer's wait for a still-open window: park on the producer of every member that has not
/// filled, so the single wake lands after the seal. Parking on the full set is what makes one wake
/// enough — a second park after wake is a protocol error, not a longer wait.
///
/// A variant carries no placeholder of its own: the `UNION` statement's binder stamps it, so a
/// variant parks on its owner. An unfilled member whose producer is gone is a declaration that
/// died; that is a typed miss, never a park that would never wake.
fn park_until_seal(
    el: &Elaborator<'_, '_>,
    name: TypeSymbol,
    view: WindowView<'_, '_>,
) -> TypeResolution<KType> {
    let Some((owner, _)) = el.scope.nearest_declaration_window() else {
        // Co-declared outside the body that announced it: the name resolves to nothing here, which
        // is the same miss any unbound name is.
        return TypeResolution::Unbound(name);
    };
    let mut producers: Vec<ProducerId> = Vec::new();
    for (member, member_owner) in view.unfilled_members() {
        let declarer = member_owner.unwrap_or(member);
        match owner.bindings().type_placeholder_producer(declarer) {
            Some(node_id) => {
                if !producers.contains(&node_id) {
                    producers.push(node_id);
                }
            }
            // A declaration that died leaves its member unfilled and its placeholder gone, so the
            // name being resolved against the window names nothing.
            None => return TypeResolution::Unbound(name),
        }
    }
    TypeResolution::Park(producers)
}

/// Walk a type name against the elaborator's scope. Bare leaves route through the ambient
/// declaration window first (the co-declared back-edge), then `resolve_type_with_chain` for bound
/// names and the placeholder path, and finally the builtin-table fallback via
/// [`KType::from_symbol`] so fixture scopes that skip builtin registration still
/// resolve builtin names. Parameterized shapes sub-Dispatch through the standalone dispatcher,
/// not this walk.
///
/// The window is consulted **before** the binding tables, sealed or not: a co-declared name takes
/// the window's answer rather than any outer binding of the same name, and a sealed window answers
/// with the member's absolute handle directly — the group's own `types` writes all carry the
/// sealing statement's position, which a consumer earlier in the body would not see through the
/// lexical chain.
pub fn elaborate_type_identifier(
    el: &mut Elaborator<'_, '_>,
    name: TypeSymbol,
    registries: &RunRegistries,
) -> TypeResolution<KType> {
    let types = &registries.types;
    let classified = name;
    if let Some(view) = el.window() {
        // A bare leaf naming a standalone member of the window is a co-declared sibling (or a
        // self-reference). A `UNION`'s variants are *not* standalone types: a bare `Node :Leaf` is
        // an unknown-type error, and a sibling variant is reached only through its binder
        // (`:Tree`) or the member projection `:(Tree.Leaf)` (handled in `typed_field_list`).
        if let Some(index) = view.member_index(classified) {
            return match (view.sealed_member(index), el.mode) {
                (Some(kt), _) => TypeResolution::Done(kt),
                (None, TypeResolutionMode::Declarator) => {
                    TypeResolution::Done(types.intern(TypeNode::Sibling(index)))
                }
                (None, TypeResolutionMode::Consumer) => park_until_seal(el, classified, view),
            };
        }
        // A binder names no single member — a `UNION`'s name denotes the union of every variant it
        // declares (`Node :Tree` inside `UNION Tree = (…)`).
        if view.binds(classified) {
            return match (view.sealed_binder(classified), el.mode) {
                (Some(kt), _) => TypeResolution::Done(kt),
                (None, TypeResolutionMode::Declarator) => {
                    match view.binder_union(classified, types) {
                        Some(kt) => TypeResolution::Done(kt),
                        None => TypeResolution::Unbound(name),
                    }
                }
                (None, TypeResolutionMode::Consumer) => park_until_seal(el, classified, view),
            };
        }
        // A threaded name the window has not announced yet: a forward reference inside a declarator
        // that discovers its members as it walks its own schema. Announcing it here keeps the
        // relative index stable, and the declarator's finalize reports any member left unfilled as
        // a reference to a type the declaration never made.
        if !view.is_sealed()
            && el.threaded.contains(&classified)
            && let Some(kt) = view.sibling(classified, KKind::NewType, types)
        {
            return TypeResolution::Done(kt);
        }
    }
    match el.scope.resolve_type_with_chain(name, el.chain.as_deref()) {
        Some(NameLookup::Bound(kt)) => return TypeResolution::Done(kt),
        // A visible placeholder is an earlier-declared type still finalizing: park on its
        // producer and re-elaborate when it terminalizes. A forward reference is filtered by the
        // chain before reaching here — a position error, not a park. Mutual recursion across the
        // cut co-declares the types in one module body, answered by the window above.
        Some(NameLookup::Parked(edge)) => return TypeResolution::Park(vec![edge]),
        None => {}
    }
    // Not a type binding, and there is no value side to consult: the token-class partition commits
    // a Type token to the type universe — `data` keys by `ValueSymbol`, which no Type token mints
    // — so a name reaching here can hold no value to layer a sharper miss over. What
    // remains is the builtin table — tried last so a fixture scope that skips builtin registration
    // still resolves builtin names — and then an unknown-name failure.
    match KType::from_symbol(name) {
        Some(kt) => TypeResolution::Done(kt),
        None => TypeResolution::Unbound(name),
    }
}

/// The miss diagnostic a [`TypeResolution::Unbound`] renders to — the one wording every unbound
/// arm shares, built where a diagnostic quotes the name and nowhere else. The spelling goes
/// straight into the message's own buffer, so naming the miss costs that buffer alone.
pub fn unknown_type_name(name: TypeSymbol, registries: &RunRegistries) -> String {
    format!(
        "unknown type name `{}`",
        crate::machine::model::types::display_label(name.symbol(), registries)
    )
}

/// Outcome of [`finalize_nominal_member`].
pub enum SealOutcome<'a> {
    /// The member sealed (or was already sealed): the `Copy` handle is its interned member handle,
    /// ready to wrap in a `Carried::Type`, beside the `types` writes installing every name the seal
    /// settles. The writes ride the step outcome — a redeclaration surfaces as the binder's error
    /// terminal when the run loop applies them.
    Sealed { kt: KType, writes: Vec<WriteOp<'a>> },
    /// The member's schema filled, but its window still holds unfilled members, so no member has
    /// an identity yet. Only a member of an announced module group reaches this: the fill that
    /// closes the group is the seal barrier, and it installs every member at once.
    Deferred,
    /// A reference named no member of the window — a sealing bug surfaced as a shape error rather
    /// than a dangling reference.
    DanglingRef(String),
}

/// Every `types` write a sealed window installs, in one run: one per standalone member and one per
/// binder, all at the sealing statement's `site`. Variants get none — a variant is reached through
/// its binder's union node, never by name — which is also why they never reach `Module::type_members`.
///
/// [`TypeWritePolicy::UpsertEqual`] is what makes a re-entrant finalize of the same declaration
/// idempotent: it recognizes the re-entry by its installing
/// [`Installer`](crate::machine::core::Installer) matching the stored entry's, while a genuine
/// redeclaration installs under a different statement and surfaces as `Rebind`.
pub fn seal_writes<'a>(view: WindowView<'_, 'a>, site: DeclarationSite) -> Vec<WriteOp<'a>> {
    view.installable()
        .into_iter()
        .map(|(name, kt)| WriteOp::Type {
            name,
            kt,
            site,
            policy: TypeWritePolicy::UpsertEqual,
            builtin_shadow_guard: true,
        })
        .collect()
}

/// Fill a nominal type's elaborated representation into its window member and, once the window
/// seals, install every name the seal settles. Three cases collapse here:
///
/// 1. **Announced member** — the enclosing module body's ambient window already announces `name`;
///    fill that slot. Unless this fill is the group's last, the window stays open and the outcome
///    is [`SealOutcome::Deferred`] — no member has an identity until every member's content is
///    known, because identity is computed over the whole reference structure.
/// 2. **Standalone declaration** — no window announces `name`, so `window` is this declarator's
///    own one-member window: filling its only member seals it, and a self-reference was already
///    interned as `Sibling(0)`.
/// 3. **Already sealed** — a parallel finalize of this same declaration ran first; the window
///    hands back the same handles and the upsert is idempotent.
pub fn finalize_nominal_member<'a>(
    window: &DeclWindow<'a>,
    name: TypeSymbol,
    build_repr: impl FnOnce(WindowView<'_, 'a>) -> KType,
    site: DeclarationSite,
    brand: crate::machine::core::RegionBrand<'a>,
    registries: &RunRegistries,
) -> SealOutcome<'a> {
    let types = &registries.types;
    let index = match window.view().member_index(name) {
        Some(index) => index,
        // The declarator handed a window that does not announce its own binder — a wiring bug, not
        // a user error, but reported as a dangling reference rather than a panic.
        None => return SealOutcome::DanglingRef(render_label(name.symbol(), registries)),
    };
    let repr = build_repr(window.view());
    if !window.fill(index, repr, brand, types) {
        return SealOutcome::Deferred;
    }
    let view = window.view();
    let kt = match view.sealed_member(index) {
        Some(kt) => kt,
        None => return SealOutcome::DanglingRef(render_label(name.symbol(), registries)),
    };
    SealOutcome::Sealed {
        kt,
        writes: seal_writes(view, site),
    }
}

/// The window a declarator named `name` elaborates and seals against: the ambient window carried by
/// **this very scope** when it announces the name, else a fresh one-member window this declaration
/// owns outright.
///
/// Self-only, not a chain walk: a same-named declaration nested deeper in the body opens its own
/// singleton rather than hijacking the announced slot.
pub fn declarator_window<'a>(scope: &'a Scope<'a>, name: TypeSymbol) -> DeclWindow<'a> {
    match scope.own_declaration_window() {
        Some(window) if window.member_index(name).is_some() => DeclWindow::Ambient(window),
        _ => DeclWindow::Owned(RecursiveGroupWindow::new(vec![(name, KKind::NewType)])),
    }
}
