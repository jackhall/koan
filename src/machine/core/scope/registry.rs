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
use crate::machine::core::carrier_witness::{
    DeliveredFunction, DeliveredOperatorGroup, GroupSeal, OverloadSeal,
};
use crate::machine::core::{KError, KErrorKind};
use crate::machine::model::KeyElement;
use crate::machine::model::KeywordSymbol;
use crate::machine::model::RunRegistries;
use crate::machine::model::{
    BinderSymbol, Carried, KObject, KType, ReductionMode, TypeSymbol, ValueSymbol,
    render_keyworded_head, render_label,
};
use allocator_api2::vec::Vec as AllocVec;
use std::mem::ManuallyDrop;

/// What an ascription decides about a view's members once the newborn view scope's id — the
/// generativity nonce every per-call mint folds in — is known. Handed to
/// [`Scope::alloc_module_view`] as the plan the door fills the scope from, so the ordering
/// obligation ("the nonce first, then the members") is discharged by the signature rather than by a
/// caller's statement order.
pub(crate) struct ViewMembers {
    /// The view's own type members, seeded into its `types` table: a per-call mint per abstract
    /// member of the ascribed signature, plus each manifest member at its fixed type.
    pub(crate) types: Vec<(TypeSymbol, KType)>,
    /// SIG value-slot name → the slot's **declared** type, for every member whose value has to be
    /// rewritten as the replay installs it. The declared type is the coercion walk's root; a slot
    /// whose two substitutions agree is absent, and its member replays verbatim.
    pub(crate) coerced_slots:
        std::collections::HashMap<ValueSymbol, KType, crate::machine::model::IdentityBuildHasher>,
    /// The two member bindings every coerced slot is rewritten between — the source module's, and
    /// the view's own mints.
    pub(crate) coercion: crate::machine::model::MemberCoercion,
}

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

    /// [`Self::adopt_for_capture`] + [`Self::bind_value_direct`] — the construction-door spelling of
    /// a `CLOSE OVER` **capture**. The only difference from [`Self::bind_delivered_direct`] is the
    /// adoption seam: severing rather than binding, so the copy always runs and the entry's composed
    /// reach names only what the rebuilt value still borrows. This is the one public constructor of
    /// [`AdoptSeam::Severing`](super::AdoptSeam) — a caller cannot select the always-copy
    /// disposition any other way.
    ///
    /// Nothing is handed back: a capture is bound and read by name from inside the block, never
    /// lifted into the capturing step's own terminal.
    pub(crate) fn bind_delivered_severed(
        &'a self,
        name: ValueSymbol,
        cell: &DeliveredCarried,
        index: BindingIndex,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let sealed = self.adopt_for_capture(cell, |carried| Ok(carried.object()))?;
        self.bind_value_direct(name, sealed, index, registries, gate)
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
        cell: &DeliveredFunction,
        index: BindingIndex,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        WriteOp::Overload {
            index,
            seal: OverloadSeal::of_delivered(self, cell),
            builtin_shadow_guard: true,
        }
        .apply(self, registries, gate)
    }

    /// Copy a dispatch registration into this scope **pinned** — a `CLOSE OVER` capture pattern's
    /// install, and implicit close's. `cell` is a registration lifted at its own defining scope;
    /// [`OverloadSeal::of_delivered`] rests it here, which lodges the envelope's whole coverage in
    /// this region's union bundle. That lodging *is* the pin: the callable stays where it was born
    /// and this region holds its home — and, transitively, everything its captured scope's own
    /// bindings reach — alive for this region's life.
    ///
    /// The bucket key and the dispatch token are re-derived from the callable's own signature, so
    /// the copy lands in the same bucket it came out of with no key threaded alongside it.
    ///
    /// A [`KErrorKind::DuplicateOverload`] is **not** an error here, it is the shadow rule: the
    /// capture walk runs innermost-first, so an entry whose token is already installed is one an
    /// inner scope already shadowed. Every other write failure surfaces.
    pub(crate) fn adopt_registration(
        &'a self,
        cell: &DeliveredFunction,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let outcome = WriteOp::Overload {
            index: BindingIndex::value(0),
            seal: OverloadSeal::of_delivered(self, cell),
            builtin_shadow_guard: true,
        }
        .apply(self, registries, gate);
        match outcome {
            Err(error) if matches!(error.kind, KErrorKind::DuplicateOverload { .. }) => Ok(()),
            other => other,
        }
    }

    /// [`Self::adopt_registration`] for the operator registry: rest the lifted group record here
    /// under `probe`, pinning its declaring region.
    ///
    /// **A probe this scope already holds is skipped.** Operator resolution
    /// ([`Scope::resolve_operator_group_delivered`]) stops at the first scope with a visible entry
    /// for the probe — pure innermost-wins, with none of the "keep walking past a bucket that does
    /// not match" the function ladder does — so a use site that could have reached the outer
    /// declaration does not exist: an inner one shadows it whole. Flattening the chain therefore
    /// keeps the innermost entry and drops the rest, which is the resolution the walk would have
    /// given, and the two declarations never meet.
    ///
    /// Reaching [`Bindings::write_operator_group`] with both would be worse than lossy. That verb
    /// admits a second write only when the entries agree by record address or by
    /// mode-plus-member-set, and refuses a disagreement as a chaining-mode conflict — a rule about
    /// what *one scope's own declarations* may say, which two shadowing scopes were never subject
    /// to. Handing it a flattened chain would turn a legal shadow into an error and make the block
    /// non-transparent: a run that reduces outside it would refuse to compile inside.
    pub(crate) fn adopt_operator_registration(
        &'a self,
        probe: KeywordSymbol,
        cell: &DeliveredOperatorGroup,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        // Unfiltered: everything seeded into a block scope sits at index 0, so no cutoff can hide a
        // standing entry from the walk that is about to shadow it.
        if self.bindings().lookup_operator_group(probe, None).is_some() {
            return Ok(());
        }
        WriteOp::Group {
            probes: vec![probe],
            seal: GroupSeal::of_delivered(self, cell),
            index: BindingIndex::value(0),
        }
        .apply(self, registries, gate)
    }

    /// Copy a value binding into this scope **pinned**, without the relocation a bind normally runs:
    /// the envelope rests here, so the value stays in its producer region and this region's bundle
    /// holds that region alive. Implicit close's module-binding install — a module's whole member
    /// closure lives in its child scope's region, which pinning names in one claim.
    ///
    /// Distinct from [`Self::bind_delivered_severed`], which is the opposite verb for the opposite
    /// purpose: an explicit data capture copies so the producer can die, a closed-over module pins
    /// because only a callable reaches the consolidate verb, so a module value's own environment is
    /// never rebuilt
    /// ([module-scope-consolidation.md](../../../../roadmap/foundation/module-scope-consolidation.md));
    /// a closed-over *callable* takes the severing door and consolidates.
    pub(crate) fn adopt_binding_pinned(
        &'a self,
        name: ValueSymbol,
        cell: &DeliveredCarried,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let sealed = cell.rest_in(self.brand().handle());
        self.bind_value_direct(name, sealed, BindingIndex::value(0), registries, gate)
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

    /// Record a SIG keyworded member: append `fn_type` to `key`'s overload set in the nearest
    /// enclosing SIG decl scope's keyworded collector — [`Self::write_sig_slot`]'s twin for
    /// the dispatch-bucket half of the interface. A second declaration of the *same* key at the
    /// *same* type is a `Rebind`, the keyworded reading of the duplicate-slot rule; a same-key
    /// declaration at a different type is an overload and joins the set. Like a value slot, a
    /// keyworded member is a schema entry rather than a binding: it takes no [`BindingIndex`],
    /// claims no dispatch bucket, and touches no binding map.
    ///
    /// The key run is bumped into the SIG scope's own region on first use, so the collector's own
    /// storage stays region-hosted and a probe against a caller's owned key needs no copy.
    pub(crate) fn write_sig_keyworded(
        &self,
        key: &[KeyElement],
        fn_type: crate::machine::model::KType,
        registries: &RunRegistries,
    ) -> Result<(), KError> {
        let outside_sig = || {
            KError::new(KErrorKind::ShapeError(
                "keyworded member outside a SIG body reached the member door".to_string(),
            ))
        };
        let target = self.nearest_opaque().ok_or_else(outside_sig)?;
        let ScopeKind::Sig { keyworded, .. } = &target.kind else {
            return Err(outside_sig());
        };
        target.assert_open(key);
        if let Some(overloads) = keyworded.borrow().get(key)
            && overloads.contains(&fn_type)
        {
            return Err(KError::new(KErrorKind::Rebind {
                name: render_keyworded_head(key, fn_type, registries),
            }));
        }
        let mut table = keyworded.borrow_mut();
        if let Some(overloads) = table.get_mut(key) {
            overloads.push(fn_type);
            return Ok(());
        }
        // First declaration under this key: the run is bumped once, here, and every later overload
        // under it probes against the stored run without materializing a key of its own.
        let stored = target.brand().allocator().slice(key);
        let mut overloads = ManuallyDrop::new(AllocVec::new_in(target.brand().allocator()));
        overloads.push(fn_type);
        table.insert(stored, overloads);
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
        bucket: &[KeyElement],
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

    /// Allocate an ascription view's scope under `outer`, replay `source`'s bindings into it —
    /// value entries and dispatch buckets both, so the view preserves the source module's keyworded
    /// surface as-is — and seed the view's own type members, all from the [`ViewMembers`] `plan`
    /// decides once the newborn scope's id (the generativity nonce a per-call abstract mint folds
    /// in) is known. The seeded `types` table *is* the view's type interface: what the table does
    /// not hold — the source's representation types behind an abstract member — is unreachable
    /// through the view by construction.
    ///
    /// A member the plan names is **born coerced**: the replayed entry is not the source's seal but
    /// a value rewritten into this scope's region so it inhabits the view's types
    /// ([`Scope::seal_coerced_member`]). Every other member — and every non-SIG member — replays as
    /// pure seal duplication. Coercing here rather than at each read is what makes ATTR, a `USING`
    /// window over this same table, a dynamic read, and a functor's deferred return agree by
    /// construction.
    ///
    /// Born-inside-the-door like [`Self::alloc_group_child`]: the view scope is returned only once
    /// the replay and the seeding have landed, and nothing else has a reference to it before then,
    /// so the door mints its own [`WriteGate`].
    pub(crate) fn alloc_module_view(
        outer: &'a Scope<'a>,
        source: &'a Scope<'a>,
        registries: &RunRegistries,
        plan: impl FnOnce(crate::machine::core::ScopeId) -> ViewMembers,
    ) -> Result<&'a Scope<'a>, KError> {
        let view = outer.alloc_child_under_module(None);
        let members = plan(view.id);
        let tables = members.coercion.tables(&registries.types);
        view.bindings().bulk_install_from(
            source.bindings(),
            registries,
            &mut WriteGate::for_unpublished_scope(),
            |name, sealed| match members.coerced_slots.get(&name) {
                Some(declared) => {
                    view.seal_coerced_member(source, sealed, *declared, &tables, registries)
                }
                None => sealed,
            },
        )?;
        // A view's type member is installed by the ascription, not by a declaration statement
        // running in the view scope, so it takes the born-with-the-scope site.
        for (name, ktype) in members.types {
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
        members: &[KeywordSymbol],
        mode: ReductionMode,
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
        members: &[KeywordSymbol],
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
