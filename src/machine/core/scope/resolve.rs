//! Name-resolution ladders on [`Scope`]: value / type / operator-group lookup, the shared
//! `walk_chain` / `resolve_builtin_first` traversals, the visibility `binding_cutoff`, and the
//! builtin-shadow consults. Split out of the parent `scope` module; the `Scope` struct,
//! its constructors, and small accessors live there.

use super::Scope;
use crate::machine::core::bindings::{Bindings, NameLookup};
use crate::machine::core::{KFunction, LexicalFrame};
use crate::machine::model::{CarriedFamily, KObject, KType, OperatorGroup};
use crate::machine::{CarrierWitness, DeliveredCarried};
use crate::witnessed::Witnessed;

impl<'a> Scope<'a> {
    /// True iff `name` is a builtin type. The builtins live once in the immutable
    /// run-global root, so a user type declaration colliding with one is a `Rebind` at
    /// any depth — the consult hits the root directly rather than each layer of the
    /// `outer` chain. TraceFrame-local bindings (FN parameters, MATCH/TRY `it`) live below
    /// the root, so ordinary user-vs-user cross-scope shadowing is unaffected.
    pub(crate) fn shadows_builtin_type(&self, name: &str) -> bool {
        self.root_scope().bindings().has_builtin_type(name)
    }

    /// True iff `key` names a builtin dispatch bucket — a finalized overload lives
    /// under it in the run-global root. Builtins are immutable and unshadowable, so a
    /// user FN whose untyped signature key collides with a builtin is a
    /// `Rebind`; it must never merge into the builtin bucket. The consult reads the
    /// root directly.
    pub(crate) fn shadows_builtin_function(&self, key: &crate::machine::model::UntypedKey) -> bool {
        self.root_scope().bindings().has_builtin_function(key)
    }

    /// Nearest value binding of `name` up the `outer` chain. Collapses a `Parked`
    /// producer and a miss to `None`. Visibility unfiltered — use
    /// [`Self::lookup_with_chain`] from a dispatch-driven path.
    pub fn lookup(&self, name: &str) -> Option<&'a KObject<'a>> {
        self.lookup_with_chain(name, None)
    }

    /// Chain-gated companion to [`Self::lookup`]. Filter consults `chain` per
    /// [`visible`].
    pub fn lookup_with_chain(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<&'a KObject<'a>> {
        self.resolve_with_chain(name, chain)
            .and_then(NameLookup::bound)
    }

    /// Resolve `name` against this scope and the `outer` chain. Stops at the first
    /// per-scope hit, checking `data` then `placeholders` — an inner placeholder
    /// shadows an outer value binding, because the inner producer hasn't finalized
    /// and the consumer must park rather than read through.
    ///
    /// Type-side bindings are not consulted — see [`Self::resolve_type`].
    /// Visibility unfiltered; dispatch-driven reads use [`Self::resolve_with_chain`].
    pub fn resolve(&self, name: &str) -> Option<NameLookup<&'a KObject<'a>>> {
        self.resolve_with_chain(name, None)
    }

    /// The chain-derived visibility cutoff for a per-scope `bindings` lookup, or `None` when this
    /// scope's bindings are all unconditionally visible. A transparent `USING` window
    /// ([`Self::child_transparent`]) surfaces a finalized module's members as imports available
    /// throughout the block — index-0 semantics, like builtins and bound parameters — so they
    /// carry no lexical-ordering relationship to the reading position and take no cutoff. Without
    /// this, a body statement dispatched into the window via `enter_block` (chain frame
    /// `(window, i)`) would filter the surfaced members by an unrelated index and miss them.
    pub(crate) fn binding_cutoff(&self, chain: Option<&LexicalFrame>) -> Option<usize> {
        if self.bindings.is_borrowed() {
            None
        } else {
            chain.and_then(|c| c.index_for(self.id))
        }
    }

    /// Walk `self` and its `outer` ancestors, returning the first scope's `probe` hit — the single
    /// ancestor-with-cutoff traversal every name-resolution ladder shares. Each ladder supplies the
    /// per-scope `probe`, which reads that scope's `bindings` gated by its
    /// [`binding_cutoff`](Self::binding_cutoff); the innermost visible hit wins.
    fn walk_chain<T>(&self, probe: impl Fn(&Scope<'a>) -> Option<T>) -> Option<T> {
        self.ancestors().find_map(probe)
    }

    /// Builtin-first resolution: a builtin entry is unshadowable and authoritative, so consult the
    /// immutable run-global root in one hop and return its hit; a non-builtin name finds nothing in
    /// the root and falls through to the innermost-wins [`Self::walk_chain`]. The `is_builtin` gate is
    /// the `idx == 0` [`Bindings::has_builtin_type`] / [`Bindings::has_builtin_function`] predicate,
    /// so a synthetic root-position user entry still resolves by the chain walk below.
    fn resolve_builtin_first<T>(
        &self,
        is_builtin: impl Fn(&Bindings<'a>) -> bool,
        root_hit: impl FnOnce(&Bindings<'a>) -> Option<T>,
        probe: impl Fn(&Scope<'a>) -> Option<T>,
    ) -> Option<T> {
        let root = self.root_scope().bindings();
        if is_builtin(root) {
            return root_hit(root);
        }
        self.walk_chain(probe)
    }

    /// Chain-gated companion to [`Self::resolve`]. Per-scope hits are filtered through the
    /// [`binding_cutoff`](Self::binding_cutoff), so hidden entries (later siblings, or value-style
    /// binders before their lexical position) are skipped and the walk continues outward.
    pub fn resolve_with_chain(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<NameLookup<&'a KObject<'a>>> {
        self.walk_chain(|scope| {
            scope
                .bindings()
                .lookup_value(name, scope.binding_cutoff(chain))
        })
    }

    /// Carrier-returning twin of [`Self::resolve_with_chain`]: resolve `name` to the bound value
    /// wrapped in a [`Witnessed`] carrier naming its reach, so an object-value read embeds a carrier
    /// by construction instead of reconstructing the reach from the value. Walks the same `outer`
    /// chain, but at the **binding** scope wraps the value via [`Self::resident_value_carrier`] — the
    /// witness is that scope's home frame, not the reading scope's. The non-`Bound` dispositions mirror
    /// [`Self::resolve_with_chain`].
    pub(crate) fn resolve_value_carrier(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<NameLookup<Witnessed<CarriedFamily, CarrierWitness>>> {
        self.walk_chain(|scope| {
            scope
                .bindings()
                .lookup_value_carrier(name, scope.binding_cutoff(chain))
                .map(|hit| hit.map(|value| scope.resident_value_carrier(value.obj, value.stored)))
        })
    }

    /// Resolve `name` down the outer chain like [`Self::lookup`], and seal the hit as a resident
    /// delivered operand of its declaring scope — the fold-operand form of a binding read, its
    /// stored reach and home bit riding the envelope. Walks the same chain as
    /// [`Self::resolve_with_chain`] (the value-side `lookup_value_carrier` twin of `lookup_value`,
    /// so shadowing agrees with `lookup`), wraps the hit at its **binding** scope via
    /// [`Self::resident_value_carrier`], then seals it into a [`DeliveredCarried`] envelope pinned by
    /// that scope's home frame. A still-finalizing placeholder collapses to `None`, exactly as
    /// [`Self::lookup`].
    pub(crate) fn lookup_value_delivered(&self, name: &str) -> Option<DeliveredCarried> {
        self.walk_chain(|scope| {
            scope
                .bindings()
                .lookup_value_carrier(name, scope.binding_cutoff(None))
                .map(|hit| {
                    hit.map(|value| {
                        scope.seal_resident_delivered(
                            scope.resident_value_carrier(value.obj, value.stored),
                        )
                    })
                })
        })
        .and_then(NameLookup::bound)
    }

    /// Resolve a *finalized* type, unfiltered. The `Option<&KType>` adapter over
    /// [`Self::resolve_type_with_chain`]: an in-flight [`NameLookup::Parked`]
    /// collapses to `None` here, so callers that must park on the producer use
    /// `resolve_type_with_chain` and match its `Parked` arm.
    pub fn resolve_type(&self, name: &str) -> Option<&'a crate::machine::model::KType> {
        self.resolve_type_with_chain(name, None)
            .and_then(NameLookup::bound)
    }

    /// Chain-gated type-side resolution — the type-language mirror of
    /// [`Self::resolve_with_chain`]. Per-scope `types` (and `BindKind::Type` placeholder)
    /// hits are filtered through [`visible`], so a type binding declared lexically later in
    /// the same block is invisible to an earlier sibling — a forward type reference is a
    /// position error. Surfaces a still-finalizing producer as [`NameLookup::Parked`]
    /// so a type consumer parks on it (rather than bootstrapping off the value-side lookup).
    pub fn resolve_type_with_chain(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<NameLookup<&'a KType>> {
        self.resolve_builtin_first(
            |root| root.has_builtin_type(name),
            |root| root.lookup_type(name, None),
            |scope| {
                scope
                    .bindings()
                    .lookup_type(name, scope.binding_cutoff(chain))
            },
        )
    }

    /// Resolve a chain's operator-group probe against this scope and the `outer` chain:
    /// per-scope `operators` hits are filtered through [`visible`], so the innermost
    /// visible registration wins and operator shadowing falls out of the walk. The
    /// builtin groups the run-global root seeds are found last, so they are defaults a
    /// declaring scope may override. Unlike the type and function ladders this walk is
    /// **not** builtin-first: a registry hit carries a member set and a mode but no
    /// operand types, so it cannot type-gate the way the root's function buckets do —
    /// the root's `+` still wins for `Number` operands through the strict bucket gate,
    /// while a scope that declares `+` over its own operand type reduces its own runs.
    /// `chain = None` is the test/builtin-registration unfiltered mode.
    pub fn resolve_operator_group_with_chain(
        &self,
        probe: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<&'a OperatorGroup> {
        self.walk_chain(|scope| {
            scope
                .bindings()
                .lookup_operator_group(probe, scope.binding_cutoff(chain))
        })
    }

    pub fn lookup_kfunction(&self, name: &str) -> Option<&'a KFunction<'a>> {
        match self.lookup(name)? {
            KObject::KFunction(f) => Some(*f),
            _ => None,
        }
    }
}
