//! The write doors on [`Scope`]: the **construction** halves of the fused value binds (mint, copy,
//! seal — the step-branded work), the submission-channel placeholder installs, and the
//! construction-time `*_direct` writes for scopes no other node can reach.
//!
//! A write into a **published** scope is not performed here: a builtin seals its value through one
//! of the `seal_*` doors and returns a [`WriteOp`] on its `Action`, which the run loop applies
//! after the step's continuation returns. The `*_direct` doors are for the scopes that need no such
//! discipline — the run-global root at startup, a not-yet-published per-call scope, a freshly minted
//! child scope before its body dispatches, test fixtures — and they route through the same
//! [`WriteOp::apply`] interpreter, so the table-write rules exist once.
//!
//! Every door here takes a [`WriteGate`], which only `crate::machine` can mint. A builtin-side
//! caller therefore never holds the capability: it either receives one as a parameter from the
//! `machine`-side caller that owns the construction, or calls an allocate-and-seed door —
//! [`Scope::alloc_group_child`], [`Scope::alloc_module_view`] — which births the scope it writes
//! into and so mints its own.
//!
//! Split out of the parent `scope` module.

use super::{Scope, ScopeKind};
use crate::machine::DeliveredCarried;
use crate::machine::ProducerId;
use crate::machine::core::bindings::powerset_probes;
use crate::machine::core::bindings::{
    BindingIndex, DeclarationSite, SealedValue, TypeWritePolicy, WriteGate, WriteOp,
};
use crate::machine::core::carrier_witness::{DeliveredFunction, GroupSeal, OverloadSeal};
use crate::machine::core::{KError, KErrorKind};
#[cfg(test)]
use crate::machine::model::KeywordSymbol;
use crate::machine::model::RunRegistries;
use crate::machine::model::{
    BinderSymbol, Carried, KObject, KType, ReductionMode, TypeSymbol, ValueSymbol, render_label,
};

impl<'a> Scope<'a> {
    /// Spike guard: a bind after [`Self::close`] means the scope's defining block finished yet a
    /// write still arrived. `debug_assert` so release builds pay nothing.
    pub(crate) fn assert_open(&self, name: impl std::fmt::Debug) {
        debug_assert!(
            !self.closed.get(),
            "bind {name:?} into closed scope {:?}",
            self.id,
        );
    }

    /// Spike guard: every write target owns its own binding table. A `USING` window borrows the
    /// opened module's, and is never one — the block's statements run in the owned child stacked
    /// inside it ([`Self::open_module_window`]), so a write reaching a borrowed table would mean the
    /// window leaked out of that door. `debug_assert` so release builds pay nothing.
    pub(crate) fn assert_owns_bindings(&self) {
        debug_assert!(
            !self.bindings.is_borrowed(),
            "write into the borrowed bindings of USING window {:?}",
            self.id,
        );
    }

    /// Fused MODULE-finish value **construction**: merge the resident module reference into this
    /// scope's region ([`Self::store_module_object`]), which mints and retains the child's region as
    /// the module value's reach. Membership is derived by the composition, never hand-asserted.
    pub(crate) fn seal_module(
        &self,
        module: &'a crate::machine::model::Module<'a>,
    ) -> SealedValue<'a> {
        self.store_module_object(module)
    }

    /// Construction-time value bind: apply a [`WriteOp::Value`] against this scope immediately.
    /// For scopes no other node can reach — a not-yet-published per-call scope (parameters,
    /// MATCH / TRY `it`), the run-global root, test fixtures. A published-scope bind rides the
    /// step outcome instead.
    pub(crate) fn bind_value_direct(
        &self,
        name: ValueSymbol,
        sealed: SealedValue<'a>,
        index: BindingIndex,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        WriteOp::Value {
            name,
            index,
            sealed,
        }
        .apply(self, registries, gate)
    }

    /// [`Self::adopt_for_binding`] + [`Self::bind_value_direct`] — the construction-door spelling
    /// of a delivered value bind. Returns a duplicate of the entry's own [`SealedValue`], from
    /// which the caller lifts its terminal envelope ([`Self::lift_resident`]).
    pub(crate) fn bind_delivered_direct(
        &'a self,
        name: ValueSymbol,
        cell: &DeliveredCarried,
        index: BindingIndex,
        project: impl for<'b> Fn(&Carried<'b>) -> Result<&'b KObject<'b>, KError>,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) -> Result<SealedValue<'a>, KError> {
        let sealed = self.adopt_for_binding(cell, project)?;
        // Duplicate the seal: one binds into the entry, the other rides the caller's terminal
        // carrier out of the step. Neither owns pins — the region's union bundle does — so the
        // reach is covered on both the resident and in-transit paths.
        self.bind_value_direct(name, sealed.duplicate(), index, registries, gate)?;
        Ok(sealed)
    }

    /// Test affordance: bind an already-arena-resident `obj` under a region-pure reach, for an
    /// assertion suite that allocated the value itself and only needs it findable by name.
    /// `#[cfg(test)]`-gated so production value binds keep going through a door that derives the
    /// reach from the value it seals.
    #[cfg(test)]
    pub(crate) fn bind_resident_for_test(
        &self,
        name: ValueSymbol,
        obj: &'a KObject<'a>,
        index: BindingIndex,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let sealed = self.seal_resident(Carried::Object(obj));
        self.bind_value_direct(name, sealed, index, registries, gate)
    }

    /// Construction-time overload registration: seal the callable `cell` carries and add it to this
    /// scope's `functions` bucket. The builtin-seeding door — the run-global root registers its own
    /// overloads at [`BindingIndex::BUILTIN`], where the shadow guard is a no-op anyway. `cell` is
    /// the birth envelope, so the seal rests the description the callable's own construction
    /// composed.
    pub(crate) fn register_function_direct(
        &'a self,
        name: String,
        cell: &DeliveredFunction,
        index: BindingIndex,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        WriteOp::Overload {
            name,
            index,
            seal: OverloadSeal::of_delivered(self, cell, registries),
            builtin_shadow_guard: true,
        }
        .apply(self, registries, gate)
    }

    /// Construction-time type registration (strict insert-if-absent, no builtin-shadow consult):
    /// a parameter's type annotation binding into a fresh per-call scope, and the builtin seeds.
    pub(crate) fn register_type_direct(
        &self,
        name: TypeSymbol,
        ktype: crate::machine::model::KType,
        site: DeclarationSite,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        WriteOp::Type {
            name,
            kt: ktype,
            site,
            policy: TypeWritePolicy::Insert,
            builtin_shadow_guard: false,
        }
        .apply(self, registries, gate)
    }

    /// Builtin type registration: [`Self::register_type_direct`] at [`DeclarationSite::BUILTIN`].
    /// Infallible — a name collision at builtin registration is a programming error, so the
    /// [`KError`] is dropped.
    pub(crate) fn register_builtin_type(
        &self,
        name: TypeSymbol,
        ktype: crate::machine::model::KType,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) {
        let _ = self.register_type_direct(name, ktype, DeclarationSite::BUILTIN, registries, gate);
    }

    /// Record a SIG value slot: insert `ktype` into the nearest enclosing SIG decl scope's slot
    /// collector. Duplicate slot name is a `Rebind`. The slot is a schema entry, not a binding — it
    /// takes no [`BindingIndex`] (no lexical read can see it) and touches no binding map. The
    /// walk is over the static scope chain, so it finds the same collector at apply time that it
    /// would have found in-step.
    pub(crate) fn write_sig_slot(
        &self,
        name: ValueSymbol,
        ktype: crate::machine::model::KType,
        registries: &RunRegistries,
    ) -> Result<(), KError> {
        let outside_sig = || {
            KError::new(KErrorKind::ShapeError(
                "VAL slot outside a SIG body reached the slot door".to_string(),
            ))
        };
        let target = self.nearest_opaque().ok_or_else(outside_sig)?;
        let ScopeKind::Sig { slots, .. } = &target.kind else {
            return Err(outside_sig());
        };
        target.assert_open(name);
        if slots.borrow().contains_key(&name) {
            return Err(KError::new(KErrorKind::Rebind {
                name: render_label(name.symbol(), registries),
            }));
        }
        slots.borrow_mut().insert(name, ktype);
        Ok(())
    }

    /// Install a dispatch-time placeholder for `name` -> the binder slot's own `edge`. See
    /// [`Bindings::install_placeholder`] for the `Rebind` rules. Submission-channel: the stamp
    /// happens where dispatch submits the binder, which is already run-loop-owned — moving it to
    /// the op-apply position would let a concurrent sibling see `UnboundName` instead of parking.
    pub fn install_placeholder(
        &self,
        name: BinderSymbol,
        producer: ProducerId,
        index: BindingIndex,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        self.assert_owns_bindings();
        self.bindings()
            .install_placeholder(name, producer, index, registries, gate)
    }

    /// Size this scope's claim run for a block of `statements` statements fanning out into it. See
    /// [`Bindings::begin_block`].
    pub fn begin_block(&self, statements: usize, gate: &mut WriteGate) {
        self.assert_owns_bindings();
        self.bindings().begin_block(statements, gate);
    }

    /// Retirement companion to both [`Self::install_placeholder`] and
    /// [`Self::install_pending_overload`]: drop every claim the statement at `index` still holds.
    /// Routes to the same target the installs used, and runs as the claiming slot terminalizes, so
    /// no claim survives naming an edge its owner is about to release — into a later run on a
    /// persistent scope least of all. See [`Bindings::retire_claims`].
    pub fn retire_claims(&self, index: BindingIndex, gate: &mut WriteGate) {
        self.assert_owns_bindings();
        self.bindings().retire_claims(index, gate);
    }

    /// Bucket-keyed companion to [`Self::install_placeholder`]: claims `bucket` in the scope's
    /// claim store so dispatch's no-bucket fallback parks bare-arg calls on the producing FN
    /// binder. Sibling installs sharing the bucket each add a distinct claim at their own
    /// `BindingIndex`, and the sealing binder retires only its own. See
    /// [`Bindings::install_pending_overload`].
    pub fn install_pending_overload(
        &self,
        bucket: crate::machine::model::UntypedKey,
        producer: ProducerId,
        index: BindingIndex,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        self.assert_owns_bindings();
        self.bindings()
            .install_pending_overload(bucket, producer, index, gate)
    }

    /// Construction-time single-probe operator-registry write. Test affordance: production
    /// registers a whole declaration at once ([`Self::register_group_under_all_subsets_direct`], or
    /// a [`WriteOp::Group`] riding an `OP` declaration's step outcome), so a lone probe key is a
    /// shape only an assertion suite builds.
    #[cfg(test)]
    pub(crate) fn register_operator_group_direct(
        &self,
        probe: KeywordSymbol,
        seal: GroupSeal<'a>,
        index: BindingIndex,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        WriteOp::Group {
            probes: vec![probe],
            seal,
            index,
        }
        .apply(self, registries, gate)
    }

    /// Allocate an ascription view's scope under `outer`, replay `src`'s bindings into it — value
    /// entries and dispatch buckets both, so the view preserves the source module's keyworded
    /// surface as-is — and seed the view's own type members from `type_entries`, which receives the
    /// newborn scope's id (the generativity nonce a per-call abstract mint folds in). The replay is
    /// pure seal duplication; the binding table opens nothing. The seeded `types` table *is* the
    /// view's type interface: what the table does not hold — the source's representation types
    /// behind an abstract member — is unreachable through the view by construction.
    ///
    /// Born-inside-the-door like [`Self::alloc_group_child`]: the view scope is returned only once
    /// the replay and the seeding have landed, and nothing else has a reference to it before then,
    /// so the door mints its own [`WriteGate`].
    pub(crate) fn alloc_module_view(
        outer: &'a Scope<'a>,
        src: &'a crate::machine::core::Bindings<'a>,
        registries: &RunRegistries,
        type_entries: impl FnOnce(crate::machine::core::ScopeId) -> Vec<(TypeSymbol, KType)>,
    ) -> Result<&'a Scope<'a>, KError> {
        let view = outer.alloc_child_under_module(None);
        view.bindings().bulk_install_from(
            src,
            registries,
            &mut WriteGate::for_unpublished_scope(),
        )?;
        // A view's type member is installed by the ascription, not by a declaration statement
        // running in the view scope, so it takes the born-with-the-scope site.
        for (name, ktype) in type_entries(view.id) {
            view.register_type_direct(
                name,
                ktype,
                DeclarationSite::AT_CONSTRUCTION,
                registries,
                &mut WriteGate::for_unpublished_scope(),
            )?;
        }
        Ok(view)
    }

    /// Allocate the `GROUP` binder's group record and child scope, and pre-register the member
    /// powerset into it, at index 0 — the same no-lexical-ordering visibility parameters and `USING`
    /// imports take, so a run anywhere in the body resolves the group, including above the `OP`
    /// declarations naming it.
    ///
    /// The record is hosted in `outer`'s region, which the child scope shares, so the scope's bare
    /// `&'a OperatorGroup<'a>` payload and every registry entry's sealed carrier name one pointee
    /// that dies with that region.
    ///
    /// The child is **born inside this door** and handed back only once the registry seeding has
    /// landed, so no other node can have reached it while it was written. That is what lets the door
    /// mint its own [`WriteGate`]: the "unpublished scope" premise is structural here, not a claim
    /// the caller makes.
    pub(crate) fn alloc_group_child(
        outer: &'a Scope<'a>,
        members: &[&str],
        mode: ReductionMode<'_>,
        announced: Option<crate::machine::model::AnnouncedData>,
        registries: &RunRegistries,
    ) -> Result<&'a Scope<'a>, KError> {
        let cell = outer.birth_operator_group(members, mode);
        let seal = GroupSeal::of_delivered(outer, &cell);
        let record = outer.adopt_group_record(&cell);
        let child = outer.alloc_child_under_group(record, announced);
        child.register_group_under_all_subsets_direct(
            members,
            seal,
            BindingIndex::value(0),
            registries,
            &mut WriteGate::for_unpublished_scope(),
        )?;
        Ok(child)
    }

    /// Construction-time operator-registry seeding: apply the whole-powerset [`WriteOp::Group`]
    /// immediately. The builtin seeds and the `GROUP` binder's pre-dispatch registration into its
    /// own freshly minted child scope; an `OP` declaration's registry entry rides its step outcome
    /// instead.
    pub(crate) fn register_group_under_all_subsets_direct(
        &self,
        members: &[&str],
        seal: GroupSeal<'a>,
        index: BindingIndex,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        WriteOp::Group {
            probes: powerset_probes(members, &registries.labels),
            seal,
            index,
        }
        .apply(self, registries, gate)
    }
}
