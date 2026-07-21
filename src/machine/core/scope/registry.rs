//! The bind / register write doors on [`Scope`]: value and type binds (the fused delivered /
//! checked tiers), function and operator registration, placeholders, and the USING-window
//! forwarding + conditional-defer shape they share. Split out of the parent `scope`
//! module.

use super::{Scope, ScopeKind};
use crate::machine::core::bindings::{ApplyOutcome, BindKind, BindingIndex, NameLookup};
use crate::machine::core::{KError, KErrorKind, KFunction, NodeId, StoredReach};
use crate::machine::model::{probe_key, Carried, KObject, OperatorGroup, TypeRegistry};
use crate::machine::DeliveredCarried;

impl<'a> Scope<'a> {
    /// Spike guard: a bind after [`Self::close`] means the scope's defining block finished yet a
    /// write still arrived. `debug_assert` so release builds pay nothing.
    fn assert_open(&self, name: &str) {
        debug_assert!(
            !self.closed.get(),
            "bind `{name}` into closed scope {:?}",
            self.id,
        );
    }

    /// Call-site scope a `Borrowed` window forwards writes to. Panics if `Borrowed`
    /// but rootless — the transparent constructor always sets `outer`, so this would
    /// be a construction bug.
    fn write_target(&self) -> &Scope<'a> {
        self.outer().expect(
            "a Borrowed (USING transparent) scope must have an outer call-site to forward \
             writes to",
        )
    }

    /// Bind `name` in this scope. Errors `Rebind` if `data` already holds `name`
    /// (same-scope rebind rejected; cross-scope shadowing allowed). Removes any
    /// matching placeholder this scope owns on success.
    ///
    /// Conditional-defer: direct mutation first, falls back to the `pending` queue
    /// iff a borrow conflict would otherwise panic.
    ///
    /// The private tail the fused value doors ([`Self::bind_delivered`], [`Self::bind_checked`])
    /// call after deriving the value's stored reach: it takes the reach as a parameter, so it is
    /// crate-internal — every production value bind routes through a fused door that derives the
    /// token rather than asserting it here.
    pub(crate) fn bind_value(
        &self,
        name: String,
        obj: &'a KObject<'a>,
        index: BindingIndex,
        reach: StoredReach<'a>,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            // Transparent `USING` window: reads consult the window before the call
            // site, so a local bind whose name is already a surfaced module member
            // would be silently shadowed. Reject it; otherwise forward to the call
            // site under the caller's `index` (the bind belongs to the call site's
            // block, at the call site's statement position), carrying the value's reach.
            if matches!(
                self.bindings.get().lookup_value(&name, None),
                Some(NameLookup::Bound(_))
            ) {
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "USING: local bind `{name}` collides with a surfaced module member; \
                     rename it to avoid silently shadowing the module's `{name}`",
                ))));
            }
            return self.write_target().bind_value(name, obj, index, reach);
        }
        self.assert_open(&name);
        match self
            .bindings
            .get()
            .try_bind_value(&name, obj, index, reach)?
        {
            ApplyOutcome::Applied => Ok(()),
            ApplyOutcome::Conflict => {
                self.pending.defer_value(name, obj, index, reach);
                Ok(())
            }
        }
    }

    /// Fused value bind: derive the bound value's stored reach off the delivered `cell` (copied
    /// mode — the value is deep-cloned into this scope's own region), deep-copy the `project`ed
    /// value in under that derived evidence, bind it, and return the resident reference paired with
    /// the same token (the caller seals its terminal carrier from it via
    /// [`Self::resident_value_carrier`]). The mint runs *before* the copy — the copy's own residence
    /// audit sees the evidence — exactly as the alloc-then-bind adjacency it fuses did. `project`
    /// selects what to copy out of the delivered value (identity for a whole-value bind, the Ok/Err
    /// payload for TRY) under the envelope's own pin. The bind itself preserves
    /// [`Self::bind_value`]'s USING-window forwarding and conditional-defer behavior.
    pub(crate) fn bind_delivered(
        &self,
        name: String,
        cell: &DeliveredCarried,
        index: BindingIndex,
        project: impl for<'b> FnOnce(&Carried<'b>) -> Result<&'b KObject<'b>, KError>,
        types: &TypeRegistry,
    ) -> Result<(&'a KObject<'a>, StoredReach<'a>), KError> {
        let stored = self.adopted_reach_of(cell);
        let allocated = cell.open(|live| {
            let projected = project(&live)?;
            self.alloc_object_delivered(
                projected.deep_clone(),
                std::slice::from_ref(&stored),
                types,
            )
        })?;
        self.bind_value(name, allocated, index, stored)?;
        Ok((allocated, stored))
    }

    /// Fused region-pure / fresh-value bind: checked move-in of `value` into this scope's own
    /// region with a `(None, bit)` token derived from the checked audit's own saw-a-region-pointer
    /// walk ([`Self::alloc_object_checked_stored`]), then bind — one call, no caller-asserted reach.
    /// Returns the resident reference paired with the same derived token (the pure-value twin of
    /// [`Self::bind_delivered`]'s return, so a caller seals its terminal carrier from it via
    /// [`Self::resident_value_carrier`]). Preserves [`Self::bind_value`]'s USING-window forwarding and
    /// conditional-defer behavior.
    pub(crate) fn bind_checked(
        &self,
        name: String,
        value: KObject<'_>,
        index: BindingIndex,
        types: &TypeRegistry,
    ) -> Result<(&'a KObject<'a>, StoredReach<'a>), KError> {
        let (obj, stored) = self.alloc_object_checked_stored(value, types)?;
        self.bind_value(name, obj, index, stored)?;
        Ok((obj, stored))
    }

    /// Add `fn_ref` to the `functions` bucket keyed by its untyped signature. `data[name]` is
    /// left untouched: a bare `FN` is dispatchable but not nameable as a value (use
    /// `LET f = (FN …)` for that). Errors:
    /// - `DuplicateOverload` if the bucket already holds an exact-signature match.
    /// - `Rebind` if a non-`BUILTIN` overload would join a builtin's bucket.
    ///
    /// Same conditional-defer shape as `bind_value`.
    pub fn register_function(
        &self,
        name: String,
        fn_ref: &'a KFunction<'a>,
        obj: &'a KObject<'a>,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            return self
                .write_target()
                .register_function(name, fn_ref, obj, index);
        }
        self.assert_open(&name);
        // A user overload may not join a builtin's bucket — builtins are immutable and
        // unshadowable. The root registers its own builtins at `BUILTIN`, so only a
        // non-`BUILTIN` index is gated.
        if index != BindingIndex::BUILTIN
            && self.shadows_builtin_function(&fn_ref.signature.untyped_key())
        {
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        match self
            .bindings
            .get()
            .try_register_function(&name, fn_ref, obj, index)?
        {
            ApplyOutcome::Applied => Ok(()),
            ApplyOutcome::Conflict => {
                self.pending.defer_function(name, fn_ref, obj, index);
                Ok(())
            }
        }
    }

    /// The operator door onto the `functions` bucket: [`Self::register_function`] without the
    /// builtin-shadowing guard, so a user module may declare an operator the root already
    /// declares (`OP #(+) OVER :(LIST OF Number)`). Shadowing an operator is **type-gated**, not
    /// free: dispatch consults the immutable root bucket first, so the builtin `+` still wins
    /// for the operand types it declares and only other operand types reach the module's body.
    /// Ordinary user `FN`s keep the guard — this door is reachable only from the operator
    /// registration door in `builtins::op_def`, through which the builtin `|` also seeds.
    ///
    /// Bare-`FN` style: the overload lands in `functions` only, never in `data`. Exact-signature
    /// collisions still surface as `DuplicateOverload`, and the same conditional-defer shape
    /// applies.
    pub fn register_operator_function(
        &self,
        name: String,
        fn_ref: &'a KFunction<'a>,
        obj: &'a KObject<'a>,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            return self
                .write_target()
                .register_operator_function(name, fn_ref, obj, index);
        }
        self.assert_open(&name);
        match self
            .bindings
            .get()
            .try_register_function(&name, fn_ref, obj, index)?
        {
            ApplyOutcome::Applied => Ok(()),
            ApplyOutcome::Conflict => {
                self.pending.defer_function(name, fn_ref, obj, index);
                Ok(())
            }
        }
    }

    /// Register `name` as a type-valued binding. Lives in [`Bindings::types`] as a `Copy` `KType`
    /// handle; reads go through [`Self::resolve_type`]. Same conditional-defer shape as
    /// [`Self::bind_value`]. Infallible: a name collision at builtin registration is a
    /// programming error, so the [`KError`] is dropped.
    pub(crate) fn register_type(
        &self,
        name: String,
        ktype: crate::machine::model::KType,
        index: BindingIndex,
    ) {
        if self.bindings.is_borrowed() {
            self.write_target().register_type(name, ktype, index);
            return;
        }
        self.assert_open(&name);
        match self.bindings.get().try_register_type(&name, ktype, index) {
            Ok(ApplyOutcome::Applied) => {}
            Ok(ApplyOutcome::Conflict) => self.pending.defer_type(name, ktype, index),
            Err(_) => {}
        }
    }

    /// Upsert install for a type-only nominal finalize (STRUCT / named UNION / Result /
    /// MODULE). Writes the sealed `SetMember` identity into [`Bindings::types`], overwriting
    /// a `PartialEq`-equal `SetMember` a `RECURSIVE TYPES` block pre-installed (same set + index).
    /// Returns the `Copy` `KType` handle so the caller can yield it as a
    /// `Carried::Type`. Same conditional-defer shape as [`Self::register_type`];
    /// `Err(Rebind)` on a genuine non-equal collision.
    ///
    /// Finalize runs post-dep-finish, past the re-entrant queue point — a `Conflict` here
    /// is a programming error, so it panics rather than deferring (deferring would risk
    /// a window where the type resolves with the pre-install's empty payload).
    ///
    /// The nominal finalizes (STRUCT / named UNION / Result / recursive-types / SIG) call this
    /// directly and consume the returned handle.
    pub(crate) fn register_type_upsert(
        &self,
        name: String,
        ktype: crate::machine::model::KType,
        index: BindingIndex,
    ) -> Result<crate::machine::model::KType, KError> {
        if self.bindings.is_borrowed() {
            return self.write_target().register_type_upsert(name, ktype, index);
        }
        if self.shadows_builtin_type(&name) {
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        match self
            .bindings
            .get()
            .try_register_type_upsert(&name, ktype, index)?
        {
            ApplyOutcome::Applied => Ok(ktype),
            ApplyOutcome::Conflict => panic!(
                "register_type_upsert borrow conflict on `{name}` — nominal finalize sites \
                 run post-dep-finish outside the re-entrant bind hot path",
            ),
        }
    }

    /// Nominal upsert: install a nominal `SetMember` identity (STRUCT/UNION/NEWTYPE/RECURSIVE member)
    /// — a heap-`Rc` set index. The nominal-finalize sites' name for
    /// [`Self::register_type_upsert`].
    pub(crate) fn register_nominal_upsert(
        &self,
        name: String,
        identity: crate::machine::model::KType,
        index: BindingIndex,
    ) -> Result<crate::machine::model::KType, KError> {
        self.register_type_upsert(name, identity, index)
    }

    /// Delivered type registration: register the RHS type handle (strict insert-if-absent,
    /// conditional-defer), returning the handle so the caller seals its terminal from it. The
    /// handle names the same interned type in every region — `ktype` is already the caller's copy
    /// out of the RHS envelope — so no reach is derived and the RHS carrier pins nothing here.
    pub(crate) fn register_type_delivered(
        &self,
        name: String,
        ktype: crate::machine::model::KType,
        index: BindingIndex,
    ) -> Result<crate::machine::model::KType, KError> {
        if self.bindings.is_borrowed() {
            return self
                .write_target()
                .register_type_delivered(name, ktype, index);
        }
        self.assert_open(&name);
        match self.bindings.get().try_register_type(&name, ktype, index)? {
            ApplyOutcome::Applied => Ok(ktype),
            ApplyOutcome::Conflict => {
                self.pending.defer_type(name, ktype, index);
                Ok(ktype)
            }
        }
    }

    /// Record a SIG value slot: insert the declared type handle into the nearest enclosing SIG
    /// decl scope's slot collector. Duplicate slot name is a `Rebind`. The slot is a schema
    /// entry, not a binding — it takes no `BindingIndex` (no lexical read can see it) and touches
    /// no binding map.
    pub(crate) fn register_sig_slot_delivered(
        &self,
        name: String,
        ktype: crate::machine::model::KType,
    ) -> Result<crate::machine::model::KType, KError> {
        // Mirrors `is_in_sig_body`'s walk exactly: a scope with `sig_slots: Some` wins; a
        // `Module` scope short-circuits (no SIG body encloses); `Root`/`Anonymous`/`Sig`
        // (a `Sig` scope always carries `sig_slots: Some` by construction, so it never
        // reaches the `None` arm in practice) fall through transparently.
        let target = self
            .ancestors()
            .find_map(|s| match (&s.sig_slots, &s.kind) {
                (Some(_), _) => Some(Some(s)),
                (None, ScopeKind::Module { .. }) => Some(None),
                (None, ScopeKind::Root | ScopeKind::Anonymous | ScopeKind::Sig { .. }) => None,
            })
            .flatten()
            .ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(
                    "VAL slot outside a SIG body reached the slot door".to_string(),
                ))
            })?;
        target.assert_open(&name);
        let slots = target
            .sig_slots
            .as_ref()
            .expect("the walk above selects only a scope with sig_slots: Some");
        if slots.borrow().contains_key(&name) {
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        slots.borrow_mut().insert(name, ktype);
        Ok(ktype)
    }

    /// User-facing twin of [`Self::register_type_delivered`] for `LET <TypeIdentifier> = …` / `VAL`:
    /// rejects a collision with a builtin type before deriving and registering. Builtins are
    /// immutable and unshadowable, so a user type that names one is a `Rebind` at any depth —
    /// including a SIG/MODULE-local abstract member — and the [`Self::shadows_builtin_type`] consult
    /// reads the root directly.
    pub(crate) fn register_user_type_delivered(
        &self,
        name: String,
        ktype: crate::machine::model::KType,
        index: BindingIndex,
    ) -> Result<crate::machine::model::KType, KError> {
        if self.shadows_builtin_type(&name) {
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        self.register_type_delivered(name, ktype, index)
    }

    /// Fused MODULE-finish value bind: derive the module's stored reach off its `child` scope
    /// ([`Self::child_module_reach`]) — never by walking the built value — allocate the Object-arm
    /// module value under that evidence, and bind it into [`Bindings::data`]. Returns the resident
    /// `&KObject` paired with the token so the caller seals its terminal from the same evidence
    /// ([`Self::resident_value_carrier`]). The home-borrow bit is derived by the mint, never
    /// hand-asserted. A module name is an Identifier and every builtin type name is a Type token, so
    /// no builtin-type shadow is reachable here; [`Self::bind_value`] raises the ordinary `Rebind`.
    pub(crate) fn bind_module(
        &self,
        name: String,
        module: &'a crate::machine::model::Module<'a>,
        child: &Scope<'a>,
        index: BindingIndex,
        types: &TypeRegistry,
    ) -> Result<(&'a KObject<'a>, StoredReach<'a>), KError> {
        let stored = self.child_module_reach(child);
        let obj = self.alloc_object_reaching(KObject::Module(module), &stored, types)?;
        self.bind_value(name, obj, index, stored)?;
        Ok((obj, stored))
    }

    /// Builtin type registration: [`Self::register_type`] at [`BindingIndex::BUILTIN`], same
    /// infallible contract.
    pub(crate) fn register_builtin_type(
        &self,
        name: String,
        ktype: crate::machine::model::KType,
        index: BindingIndex,
    ) {
        self.register_type(name, ktype, index);
    }

    /// Apply queued writes between dispatch nodes. Items that still hit a borrow
    /// conflict stay queued (eventually-consistent), and drain-time `Err`s are
    /// debug-asserted (production drops them — dispatch nodes have no caller frame to
    /// surface them to).
    pub fn drain_pending(&self) {
        // Transparent `USING` window writes forward to the call site, so its pending
        // queue lives there too — flush the call site.
        if self.bindings.is_borrowed() {
            self.write_target().drain_pending();
            return;
        }
        self.pending.drain(self.bindings.get());
    }

    /// Install a dispatch-time placeholder for `name` -> producer slot `idx`. See
    /// [`Bindings::try_install_placeholder`] for `Rebind` rules and the asymmetry with
    /// `try_bind_*` (panics on borrow conflict rather than queueing).
    pub fn install_placeholder(
        &self,
        name: String,
        idx: NodeId,
        index: BindingIndex,
        kind: BindKind,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            return self
                .write_target()
                .install_placeholder(name, idx, index, kind);
        }
        self.bindings
            .get()
            .try_install_placeholder(name, idx, index, kind)
    }

    /// Error-path companion to [`Self::install_placeholder`]: remove any value-side
    /// placeholder pointing at `producer`. Routes to the same target the install used so a
    /// failed binder body can't leak a scheduler-local placeholder into a later run on a
    /// persistent scope. See [`Bindings::clear_placeholders_for_producer`].
    pub fn clear_placeholders_for_producer(&self, producer: NodeId) {
        if self.bindings.is_borrowed() {
            self.write_target()
                .clear_placeholders_for_producer(producer);
            return;
        }
        self.bindings
            .get()
            .clear_placeholders_for_producer(producer);
    }

    /// Bucket-keyed companion to [`Self::install_placeholder`]: appends a
    /// `pending_overloads[bucket]` entry so dispatch's no-bucket fallback parks
    /// bare-arg calls on the producing FN binder. Sibling installs sharing the
    /// bucket each append a distinct entry; entries are removed on finalize by
    /// matching the producing binder's `BindingIndex`. See
    /// [`Bindings::try_install_pending_overload`].
    pub fn install_pending_overload(
        &self,
        bucket: crate::machine::model::UntypedKey,
        idx: NodeId,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            return self
                .write_target()
                .install_pending_overload(bucket, idx, index);
        }
        self.bindings
            .get()
            .try_install_pending_overload(bucket, idx, index)
    }

    /// Register `probe → group` in this scope's operator registry. The `OP` / `GROUP`
    /// binder installs one entry per nonempty subset of the declared operators (see
    /// [`Self::register_group_under_all_subsets`]); test fixtures register the subsets
    /// they exercise. Same conditional-defer-free shape as the type registry — a borrow
    /// conflict is not expected here (registration runs outside the re-entrant bind hot
    /// path), so `Conflict` panics. Re-registering an equal record under the same probe
    /// is an idempotent no-op; a record that disagrees is an error
    /// ([`Bindings::try_register_operator_group`]).
    pub fn register_operator_group(
        &self,
        probe: String,
        group: &'a OperatorGroup,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            return self
                .write_target()
                .register_operator_group(probe, group, index);
        }
        match self
            .bindings
            .get()
            .try_register_operator_group(probe.clone(), group, index)?
        {
            ApplyOutcome::Applied => Ok(()),
            ApplyOutcome::Conflict => panic!(
                "register_operator_group borrow conflict on `{probe}` — operator \
                 registration runs outside the re-entrant bind hot path",
            ),
        }
    }

    /// Register `group` in this scope under every nonempty subset of `members` — the
    /// powerset-key story [`crate::machine::model::operators`] describes, shared by the
    /// builtin seeds and by the `GROUP` binder. `members.len()` stays small, so the
    /// `2^n - 1` bitmask walk over subsets is cheap; each subset's key is derived through
    /// [`probe_key`] rather than hand-enumerated, so a registration key always agrees with a
    /// real chain's probe.
    pub fn register_group_under_all_subsets(
        &self,
        members: &[&str],
        group: &'a OperatorGroup,
        index: BindingIndex,
    ) -> Result<(), KError> {
        let subset_count = 1usize << members.len();
        for mask in 1..subset_count {
            let subset: Vec<&str> = members
                .iter()
                .enumerate()
                .filter(|(bit, _)| mask & (1 << bit) != 0)
                .map(|(_, op)| *op)
                .collect();
            let key = probe_key(&subset);
            self.register_operator_group(key, group, index)?;
        }
        Ok(())
    }
}
