//! Name-resolution ladders on [`Scope`]: value / type / operator-group lookup, the shared
//! `walk_chain` traversal, the visibility `binding_cutoff`, and the
//! builtin-shadow consults. Split out of the parent `scope` module; the `Scope` struct,
//! its constructors, and small accessors live there.

use std::rc::Rc;

use super::AdoptSeam;
use super::Scope;
use crate::machine::core::bindings::NameLookup;
use crate::machine::core::LexicalFrame;
use crate::machine::model::{KObject, KType, OperatorGroup};
use crate::machine::DeliveredCarried;

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

    /// Nearest value binding of `name` up the `outer` chain, **adopted** into this scope's own
    /// region ([`Self::adopt_carried`] mints the binding's reach here and retains it, so the returned
    /// reference outlives the read). Collapses a `Parked` producer and a miss to `None`. Visibility
    /// unfiltered.
    ///
    /// The adoption is the price of a bare `&KObject`: every production read that only *inspects* a
    /// binding takes [`Self::lookup_value_delivered`] instead and reads under the envelope's own
    /// pins, retaining nothing. This ladder survives for the assertion suites and for a consumer
    /// that genuinely needs the value to outlive the read.
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
    /// Visibility unfiltered; the adoption cost is [`Self::lookup`]'s.
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

    /// Chain-gated companion to [`Self::resolve`]. Per-scope hits are filtered through the
    /// [`binding_cutoff`](Self::binding_cutoff), so hidden entries (later siblings, or value-style
    /// binders before their lexical position) are skipped and the walk continues outward.
    pub fn resolve_with_chain(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<NameLookup<&'a KObject<'a>>> {
        self.resolve_value_delivered(name, chain).map(|hit| {
            hit.map(|delivered| {
                self.adopt_carried(&delivered, AdoptSeam::Retaining)
                    .object()
            })
        })
    }

    /// Resolve `name` down the outer chain and **lift** the hit into a delivery envelope pinned by
    /// its declaring scope's region owner — the read form of a binding, its exact reach upgraded
    /// `Weak → Rc` so the value's whole reach travels owned. Walks the same chain as every other
    /// value ladder, so shadowing agrees; the lift happens at the **binding** scope, whose arena
    /// hosts the description. The non-`Bound` dispositions mirror the bare resolution.
    pub(crate) fn resolve_value_delivered(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<NameLookup<DeliveredCarried>> {
        self.walk_chain(|scope| {
            scope
                .bindings()
                .lookup_value(name, scope.binding_cutoff(chain))
                .map(|hit| hit.map(|sealed| scope.lift_resident(sealed)))
        })
    }

    /// [`Self::resolve_value_delivered`] unfiltered, with a still-finalizing placeholder collapsed
    /// to `None` — the fold-operand form of a binding read.
    pub(crate) fn lookup_value_delivered(&self, name: &str) -> Option<DeliveredCarried> {
        self.resolve_value_delivered(name, None)
            .and_then(NameLookup::bound)
    }

    /// Test affordance: [`Self::lookup`], panicking when `name` is unbound — the assertion-suite
    /// form for a binding the test just wrote and expects to find.
    #[cfg(test)]
    pub(crate) fn expect_value(&self, name: &str) -> &'a KObject<'a> {
        self.lookup(name)
            .unwrap_or_else(|| panic!("expected `{name}` to be bound"))
    }

    /// Test affordance: probe **this scope's own** `data` at an explicit visibility `cutoff` and
    /// adopt the hit — the per-scope step [`Self::resolve_value_delivered`] composes into a chain
    /// walk, exposed for the suites that assert on cutoff gating directly rather than through a
    /// [`LexicalFrame`].
    #[cfg(test)]
    pub(crate) fn lookup_value_here_for_test(
        &self,
        name: &str,
        cutoff: Option<usize>,
    ) -> Option<NameLookup<&'a KObject<'a>>> {
        self.bindings().lookup_value(name, cutoff).map(|hit| {
            hit.map(|sealed| {
                let delivered = self.lift_resident(sealed);
                self.adopt_carried(&delivered, AdoptSeam::Retaining)
                    .object()
            })
        })
    }

    /// Resolve a *finalized* type, unfiltered. The `Option<KType>` adapter over
    /// [`Self::resolve_type_with_chain`]: an in-flight [`NameLookup::Parked`]
    /// collapses to `None` here, so callers that must park on the producer use
    /// `resolve_type_with_chain` and match its `Parked` arm.
    pub fn resolve_type(&self, name: &str) -> Option<crate::machine::model::KType> {
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
    ) -> Option<NameLookup<KType>> {
        // Builtin-first: a builtin type is unshadowable and authoritative, so the immutable
        // run-global root answers in one hop; a non-builtin name finds nothing there and falls
        // through to the innermost-wins walk. The gate is the `idx == 0`
        // [`Bindings::has_builtin_type`] predicate, so a synthetic root-position user entry still
        // resolves by the chain walk below.
        let root = self.root_scope().bindings();
        if root.has_builtin_type(name) {
            return root.lookup_type(name, None);
        }
        self.walk_chain(|scope| {
            scope
                .bindings()
                .lookup_type(name, scope.binding_cutoff(chain))
        })
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
    ) -> Option<Rc<OperatorGroup>> {
        self.walk_chain(|scope| {
            scope
                .bindings()
                .lookup_operator_group(probe, scope.binding_cutoff(chain))
        })
    }
}
