//! Name-resolution ladders on [`Scope`]: value / type / operator-group lookup, the shared
//! `walk_chain` traversal, the visibility `binding_cutoff`, and the
//! builtin-shadow consults. Split out of the parent `scope` module; the `Scope` struct,
//! its constructors, and small accessors live there.

// `AdoptSeam` and `KObject` serve the `#[cfg(test)]` bare-read ladder; a delivering resolution
// verb hands back an envelope instead.
#[cfg(test)]
use super::AdoptSeam;
use super::Scope;
use crate::machine::core::LexicalFrame;
use crate::machine::core::bindings::NameLookup;
#[cfg(test)]
use crate::machine::model::KObject;
use crate::machine::model::{KType, KeywordSymbol, TypeSymbol, ValueSymbol};
use crate::machine::{DeliveredCarried, DeliveredOperatorGroup};

impl<'a> Scope<'a> {
    /// True iff `name` is a builtin type. The builtins live once in the immutable
    /// run-global root, so a user type declaration colliding with one is a `Rebind` at
    /// any depth — the consult hits the root directly rather than each layer of the
    /// `outer` chain. TraceFrame-local bindings (FN parameters, MATCH/TRY `it`) live below
    /// the root, so ordinary user-vs-user cross-scope shadowing is unaffected.
    pub(crate) fn shadows_builtin_type(&self, name: TypeSymbol) -> bool {
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
    /// The adoption is the price of a bare `&KObject`: a read that only *inspects* a binding takes
    /// [`Self::lookup_value_delivered`] instead and reads under the envelope's own pins, retaining
    /// nothing. This ladder is `#[cfg(test)]` — it survives for the assertion suites alone, and
    /// production has no route to it. The integration tests in `tests/`, which
    /// compile against the crate without `cfg(test)`, reach the same shape through
    /// [`test_support::lookup_binding`](crate::builtins::test_support::lookup_binding).
    #[cfg(test)]
    pub fn lookup(&self, name: &str) -> Option<&'a KObject<'a>> {
        self.lookup_with_chain(name, None)
    }

    /// Chain-gated companion to [`Self::lookup`]. Filter consults `chain` per
    /// [`visible`].
    #[cfg(test)]
    pub fn lookup_with_chain(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<&'a KObject<'a>> {
        self.resolve_with_chain(name, chain)
            .and_then(NameLookup::bound)
    }

    /// Resolve `name` against this scope and the `outer` chain. Stops at the first
    /// per-scope hit — one probe of `data`, then one of the scope's claim store. An inner
    /// claim shadows an outer value binding, because the inner producer hasn't
    /// finalized and the consumer must park rather than read through.
    ///
    /// Type-side bindings are not consulted — see [`Self::resolve_type`].
    /// Visibility unfiltered; the adoption cost is [`Self::lookup`]'s.
    #[cfg(test)]
    pub fn resolve(&self, name: &str) -> Option<NameLookup<&'a KObject<'a>>> {
        self.resolve_with_chain(name, None)
    }

    /// The chain-derived visibility cutoff for a per-scope `bindings` lookup: this scope's own
    /// statement position on the reader's `chain`, or `None` when no frame names it — the scope is
    /// complete to this reader and every entry in it is visible. A `USING` window
    /// ([`Self::child_transparent`]) is never named by a frame: the block's statements run in the
    /// owned layer stacked inside it, so the window surfaces the module's members throughout the
    /// block with no lexical-ordering relationship to the reading position, exactly as builtins do.
    pub(crate) fn binding_cutoff(&self, chain: Option<&LexicalFrame>) -> Option<usize> {
        chain.and_then(|c| c.index_for(self.id))
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
    ///
    /// The test ladder takes a spelling where the production entry
    /// ([`Self::resolve_value_delivered`]) takes a [`ValueSymbol`]: a fixture writes a name it just
    /// spelled, so it classifies here through the hidden funnel. A non-value spelling names nothing
    /// on this channel and answers `None`.
    #[cfg(test)]
    pub fn resolve_with_chain(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<NameLookup<&'a KObject<'a>>> {
        self.resolve_value_delivered(ValueSymbol::classify(name)?, chain)
            .map(|hit| {
                hit.map(|delivered| {
                    self.adopt_carried(&delivered, AdoptSeam::Retaining)
                        .object()
                })
            })
    }

    /// Resolve `name` down the outer chain and **lift** the hit into a delivery envelope pinned by
    /// its declaring scope's region owner — the read form of a binding, its exact reach upgraded
    /// `Weak → Rc` so the value's whole reach travels owned. Walks the shared `walk_chain`
    /// traversal, so shadowing agrees; the lift happens at the **binding** scope, whose arena
    /// hosts the description. The non-`Bound` dispositions mirror the bare resolution.
    ///
    /// Takes the classified symbol, not text: a value name arrives minted and interned by the
    /// parse that read the token, so the ladder walks on symbol bits end to end.
    pub(crate) fn resolve_value_delivered(
        &self,
        name: ValueSymbol,
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
    pub(crate) fn lookup_value_delivered(&self, name: ValueSymbol) -> Option<DeliveredCarried> {
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
        let name = ValueSymbol::classify(name)?;
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
    pub fn resolve_type(&self, name: TypeSymbol) -> Option<crate::machine::model::KType> {
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
        name: TypeSymbol,
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

    /// Resolve a chain's operator-group probe against this scope and the `outer` chain, **lifting**
    /// the hit into a delivery envelope pinned by its declaring scope's region owner — the mirror of
    /// [`Self::resolve_value_delivered`]. The lift happens at the **hit** scope, so a group declared
    /// in an ancestor travels with that ancestor's region owned in the envelope's coverage.
    ///
    /// Per-scope `operators` hits are filtered through [`visible`], so the innermost
    /// visible registration wins and operator shadowing falls out of the walk. The
    /// builtin groups the run-global root seeds are found last, so they are defaults a
    /// declaring scope may override. Unlike the type and function ladders this walk is
    /// **not** builtin-first: a registry hit carries a member set and a mode but no
    /// operand types, so it cannot type-gate the way the root's function buckets do —
    /// the root's `+` still wins for `Number` operands through the strict bucket gate,
    /// while a scope that declares `+` over its own operand type reduces its own runs.
    /// `chain = None` is the test/builtin-registration unfiltered mode.
    ///
    /// `probe` is the chain's cached probe symbol
    /// ([`KExpression::operator_probe`](crate::machine::model::ast::KExpression::operator_probe)),
    /// minted once at construction — the walk compares symbol bits and hashes no text per call.
    pub fn resolve_operator_group_delivered(
        &self,
        probe: KeywordSymbol,
        chain: Option<&LexicalFrame>,
    ) -> Option<DeliveredOperatorGroup> {
        self.walk_chain(|scope| {
            scope
                .bindings()
                .lookup_operator_group(probe, scope.binding_cutoff(chain))
                .map(|sealed| scope.lift_resident(sealed))
        })
    }
}
