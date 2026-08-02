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
//! `machine`-side caller that owns the construction ([`crate::machine::block_tail`]'s seed, the
//! builtin-seeding entry point) or calls an allocate-and-seed door — [`Scope::alloc_group_child`],
//! [`Scope::alloc_module_view`] — which births the scope it writes into and so mints its own.
//!
//! Split out of the parent `scope` module.

use std::rc::Rc;

use super::{Scope, ScopeKind};
use crate::machine::core::bindings::operator_group_ops;
use crate::machine::core::bindings::{
    BindKind, BindingIndex, DeclarationSite, SealedValue, TypeWritePolicy, WriteGate, WriteOp,
};
use crate::machine::core::carrier_witness::OverloadSeal;
use crate::machine::core::{KError, KErrorKind, KFunction, NodeId};
use crate::machine::model::{Carried, KObject, OperatorGroup};
use crate::machine::DeliveredCarried;

impl<'a> Scope<'a> {
    /// Spike guard: a bind after [`Self::close`] means the scope's defining block finished yet a
    /// write still arrived. `debug_assert` so release builds pay nothing.
    pub(crate) fn assert_open(&self, name: &str) {
        debug_assert!(
            !self.closed.get(),
            "bind `{name}` into closed scope {:?}",
            self.id,
        );
    }

    /// Whether this scope is a transparent `USING` window — its bindings are the surfaced module's,
    /// so a write here belongs at the call site ([`Self::write_scope`]).
    pub(crate) fn is_using_window(&self) -> bool {
        self.bindings.is_borrowed()
    }

    /// The scope a write against `self` lands in: `self`, or — through one or more transparent
    /// `USING` windows — the innermost enclosing call site that owns its own bindings. The single
    /// site expressing the forwarding decision, shared by the op-apply interpreter and the
    /// submission-channel installs. Panics if a window is rootless: the transparent constructor
    /// always sets `outer`, so that would be a construction bug.
    pub(crate) fn write_scope<'s>(&'s self) -> &'s Scope<'a> {
        let mut target: &'s Scope<'a> = self;
        while target.is_using_window() {
            target = target.outer().expect(
                "a Borrowed (USING transparent) scope must have an outer call-site to forward \
                 writes to",
            );
        }
        target
    }

    /// Fused MODULE-finish value **construction**: merge the resident module reference into this
    /// scope's region ([`Self::store_module_object`]), which mints and retains the child's region as
    /// the module value's reach. Membership is derived by the composition, never hand-asserted.
    pub(crate) fn seal_module(
        &self,
        module: &'a crate::machine::model::Module<'a>,
        child: &Scope<'a>,
    ) -> SealedValue {
        self.store_module_object(module, child)
    }

    /// Construction-time value bind: apply a [`WriteOp::Value`] against this scope immediately.
    /// For scopes no other node can reach — a not-yet-published per-call scope (parameters,
    /// MATCH / TRY `it`), the run-global root, test fixtures. A published-scope bind rides the
    /// step outcome instead.
    pub(crate) fn bind_value_direct(
        &self,
        name: String,
        sealed: SealedValue,
        index: BindingIndex,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        WriteOp::Value {
            name,
            index,
            sealed,
        }
        .apply(self, gate)
    }

    /// [`Self::adopt_for_binding`] + [`Self::bind_value_direct`] — the construction-door spelling
    /// of a delivered value bind. Returns a duplicate of the entry's own [`SealedValue`], from
    /// which the caller lifts its terminal carrier ([`Self::lift_resident_parts`]).
    pub(crate) fn bind_delivered_direct(
        &self,
        name: String,
        cell: &DeliveredCarried,
        index: BindingIndex,
        project: impl for<'b> Fn(&Carried<'b>) -> Result<&'b KObject<'b>, KError>,
        gate: &mut WriteGate,
    ) -> Result<SealedValue, KError> {
        let sealed = self.adopt_for_binding(cell, project)?;
        // Duplicate the seal: one binds into the entry, the other rides the caller's terminal
        // carrier out of the step. Neither owns pins — the region's union bundle does — so the
        // reach is covered on both the resident and in-transit paths.
        self.bind_value_direct(name, sealed.duplicate(), index, gate)?;
        Ok(sealed)
    }

    /// Test affordance: bind an already-arena-resident `obj` under a region-pure reach, for an
    /// assertion suite that allocated the value itself and only needs it findable by name.
    /// `#[cfg(test)]`-gated so production value binds keep going through a door that derives the
    /// reach from the value it seals.
    #[cfg(test)]
    pub(crate) fn bind_resident_for_test(
        &self,
        name: String,
        obj: &'a KObject<'a>,
        index: BindingIndex,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let sealed = self.seal_resident(Carried::Object(obj));
        self.bind_value_direct(name, sealed, index, gate)
    }

    /// Construction-time overload registration: seal `fn_ref` and add it to this scope's
    /// `functions` bucket. The builtin-seeding door — the run-global root registers its own
    /// overloads at [`BindingIndex::BUILTIN`], where the shadow guard is a no-op anyway.
    pub(crate) fn register_function_direct(
        &self,
        name: String,
        fn_ref: &'a KFunction<'a>,
        index: BindingIndex,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        WriteOp::Overload {
            name,
            index,
            seal: OverloadSeal::of_resident(self, fn_ref),
            builtin_shadow_guard: true,
        }
        .apply(self, gate)
    }

    /// Construction-time type registration (strict insert-if-absent, no builtin-shadow consult):
    /// a parameter's type annotation binding into a fresh per-call scope, and the builtin seeds.
    pub(crate) fn register_type_direct(
        &self,
        name: String,
        ktype: crate::machine::model::KType,
        site: DeclarationSite,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        WriteOp::Type {
            name,
            kt: ktype,
            site,
            policy: TypeWritePolicy::Insert,
            builtin_shadow_guard: false,
        }
        .apply(self, gate)
    }

    /// Builtin type registration: [`Self::register_type_direct`] at [`DeclarationSite::BUILTIN`].
    /// Infallible — a name collision at builtin registration is a programming error, so the
    /// [`KError`] is dropped.
    pub(crate) fn register_builtin_type(
        &self,
        name: String,
        ktype: crate::machine::model::KType,
        gate: &mut WriteGate,
    ) {
        let _ = self.register_type_direct(name, ktype, DeclarationSite::BUILTIN, gate);
    }

    /// Record a SIG value slot: insert `ktype` into the nearest enclosing SIG decl scope's slot
    /// collector. Duplicate slot name is a `Rebind`. The slot is a schema entry, not a binding — it
    /// takes no [`BindingIndex`] (no lexical read can see it) and touches no binding map. The
    /// walk is over the static scope chain, so it finds the same collector at apply time that it
    /// would have found in-step.
    pub(crate) fn write_sig_slot(
        &self,
        name: String,
        ktype: crate::machine::model::KType,
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
        target.assert_open(&name);
        if slots.borrow().contains_key(&name) {
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        slots.borrow_mut().insert(name, ktype);
        Ok(())
    }

    /// Install a dispatch-time placeholder for `name` -> producer slot `idx`. See
    /// [`Bindings::install_placeholder`] for the `Rebind` rules. Submission-channel: the stamp
    /// happens where dispatch submits the binder, which is already run-loop-owned — moving it to
    /// the op-apply position would let a concurrent sibling see `UnboundName` instead of parking.
    pub fn install_placeholder(
        &self,
        name: String,
        idx: NodeId,
        index: BindingIndex,
        kind: BindKind,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        self.write_scope()
            .bindings()
            .install_placeholder(name, idx, index, kind, gate)
    }

    /// Error-path companion to [`Self::install_placeholder`]: remove any value-side
    /// placeholder pointing at `producer`. Routes to the same target the install used so a
    /// failed binder body can't leak a scheduler-local placeholder into a later run on a
    /// persistent scope. See [`Bindings::clear_placeholders_for_producer`].
    pub fn clear_placeholders_for_producer(&self, producer: NodeId, gate: &mut WriteGate) {
        self.write_scope()
            .bindings()
            .clear_placeholders_for_producer(producer, gate);
    }

    /// Bucket-keyed companion to [`Self::install_placeholder`]: appends a
    /// `pending_overloads[bucket]` entry so dispatch's no-bucket fallback parks
    /// bare-arg calls on the producing FN binder. Sibling installs sharing the
    /// bucket each append a distinct entry; entries are removed on finalize by
    /// matching the producing binder's `BindingIndex`. See
    /// [`Bindings::install_pending_overload`].
    pub fn install_pending_overload(
        &self,
        bucket: crate::machine::model::UntypedKey,
        idx: NodeId,
        index: BindingIndex,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        self.write_scope()
            .bindings()
            .install_pending_overload(bucket, idx, index, gate)
    }

    /// Construction-time single-probe operator-registry write.
    pub fn register_operator_group_direct(
        &self,
        probe: String,
        group: Rc<OperatorGroup>,
        index: BindingIndex,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        WriteOp::Group {
            probe,
            group,
            index,
        }
        .apply(self, gate)
    }

    /// Allocate an ascription view's scope under `outer` and replay `src`'s bindings into it —
    /// value entries and dispatch buckets both, so the view preserves the source module's
    /// keyworded surface as-is. The replay is pure seal duplication; the binding table opens
    /// nothing.
    ///
    /// Born-inside-the-door like [`Self::alloc_group_child`]: the view scope is returned only once
    /// the replay has landed, and nothing else has a reference to it before then, so the door mints
    /// its own [`WriteGate`]. `:|` / `:!` run builtin-side, where no gate can be minted.
    pub(crate) fn alloc_module_view(
        outer: &'a Scope<'a>,
        path: String,
        src: &crate::machine::core::Bindings,
    ) -> Result<&'a Scope<'a>, KError> {
        let view = outer
            .brand()
            .alloc_scope(Scope::child_under_module(outer, path));
        view.bindings()
            .bulk_install_from(src, &mut WriteGate::for_unpublished_scope())?;
        Ok(view)
    }

    /// Allocate the `GROUP` binder's child scope and pre-register the member powerset into it, at
    /// index 0 — the same no-lexical-ordering visibility parameters and `USING` imports take, so a
    /// run anywhere in the body resolves the group, including above the `OP` declarations naming it.
    ///
    /// The child is **born inside this door** and handed back only once the registry seeding has
    /// landed, so no other node can have reached it while it was written. That is what lets the door
    /// mint its own [`WriteGate`]: the "unpublished scope" premise is structural here, not a claim
    /// the caller makes. The `GROUP` binder runs builtin-side, where no gate can be minted.
    pub(crate) fn alloc_group_child(
        outer: &'a Scope<'a>,
        name: String,
        group: Rc<OperatorGroup>,
        members: &[&str],
    ) -> Result<&'a Scope<'a>, KError> {
        let child =
            outer
                .brand()
                .alloc_scope(Scope::child_under_group(outer, name, Rc::clone(&group)));
        child.register_group_under_all_subsets_direct(
            members,
            &group,
            BindingIndex::value(0),
            &mut WriteGate::for_unpublished_scope(),
        )?;
        Ok(child)
    }

    /// Construction-time operator-registry seeding: apply [`operator_group_ops`] immediately. The
    /// builtin seeds and the `GROUP` binder's pre-dispatch registration into its own freshly minted
    /// child scope; an `OP` declaration's registry entry rides its step outcome instead.
    pub fn register_group_under_all_subsets_direct(
        &self,
        members: &[&str],
        group: &Rc<OperatorGroup>,
        index: BindingIndex,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        for op in operator_group_ops(members, group, index) {
            op.apply(self, gate)?;
        }
        Ok(())
    }
}
