//! The lexical binding façade: four co-mutating `RefCell` maps (`types`, `data`,
//! `functions`, `placeholders`) plus the shared validated write paths
//! ([`Bindings::try_apply`] for `data`/`functions`, [`Bindings::try_apply_type`] for
//! `types`) that keep the dual-map invariant — every `data[name]` entry wrapping a
//! `KFunction` lives in `functions[signature.untyped_key()]` — in one place.
//! [`Bindings::try_register_nominal`] (stage 1.3) is the transactional dual-write
//! primitive for nominal declarations (STRUCT / UNION / MODULE), atomically inserting
//! identity into `types` and the runtime carrier into `data`. The `types` map (added
//! in stage 1.2 of per-type identity) is the dedicated home for type-side bindings,
//! populated as of stage 1.4 by `Scope::register_type` (which routes through
//! [`Bindings::try_register_type`]); `try_register_nominal` will start writing into it
//! at stage 3 (STRUCT / UNION / MODULE migration). Borrow discipline across the four
//! maps: `types → functions → data`, with `types` only acquired when writing types.
//! Lives in its own module so the façade's surface (write methods + read handles)
//! stays focused; [`Scope`] embeds it by value so all interior borrows arbitrate
//! against one another.
//!
//! `ApplyOutcome` is `pub` so [`scope`](super::scope)'s match arms on the
//! [`Bindings::try_bind_value`] / [`Bindings::try_register_function`] results compile, but
//! it isn't re-exported beyond `core::` — the `Conflict` variant is an internal queueing
//! signal, not part of any user-visible API.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;

use crate::machine::model::ast::TypeExpr;
use crate::machine::core::kfunction::{KFunction, NodeId};
use crate::machine::model::types::{KType, UntypedKey};
use crate::machine::model::values::KObject;

use super::kerror::{KError, KErrorKind};

mod pending;
pub use pending::{PendingBinderGuard, PendingTypeEntry, PendingTypes};

/// Façade owning the four co-mutating `RefCell` maps that back every lexical binding:
/// `types` (name → `&KType`, the dedicated type-binding home introduced in stage 1.2 of
/// per-type identity; no consumer reads it yet — wiring lands in stage 1.4),
/// `data` (name → value), `functions` (untyped-signature bucket → overloads), and
/// `placeholders` (name → producer NodeId for dispatch-time forward references).
///
/// The shared private [`Bindings::try_apply`] enforces the dual-map invariant —
/// every `data[name]` entry wrapping a `KFunction` lives in
/// `functions[signature.untyped_key()]` — in one place, and unifies dedupe (`ptr::eq`
/// fast-path then `signatures_exact_equal`) across the LET-binds-FN and `FN`-decl paths.
/// [`Bindings::try_apply_type`] is the parallel write primitive for the `types` map; it
/// borrows only `types`, so it composes cleanly with code holding live `data`/`functions`
/// borrows. [`Bindings::try_register_nominal`] composes `types` + `data` writes
/// transactionally for nominal declarations (no `functions` involvement — nominal
/// carriers are not callable verbs).
///
/// Borrow discipline across the four maps: `types → functions → data`. `types` is only
/// acquired by the type-side write path, so the existing `functions → data` ordering used
/// by [`Bindings::try_apply`] is unaffected.
///
/// Lifetime `'a` matches the arena lifetime of the stored references; the façade itself
/// is embedded by value on [`super::scope::Scope`] so all interior borrows arbitrate
/// against one another.
pub struct Bindings<'a> {
    types: RefCell<HashMap<String, &'a KType>>,
    data: RefCell<HashMap<String, &'a KObject<'a>>>,
    functions: RefCell<HashMap<UntypedKey, Vec<&'a KFunction<'a>>>>,
    placeholders: RefCell<HashMap<String, NodeId>>,
    /// In-flight named-type binders (STRUCT / named-UNION). Owns its own
    /// `RefCell`; see [`pending`] for the surface methods and the guard's Drop
    /// path. Populated by struct_def / union before elaboration; consulted by
    /// the elaborator's `Resolution::Placeholder` arm to record dependency
    /// edges and run DFS cycle detection.
    pending: PendingTypes<'a>,
    /// Layer-2 scope-bound TypeExpr resolution cache. Maps a surface
    /// [`TypeExpr`] to the arena-allocated `&KType` `Scope::resolve_type_expr`
    /// produced in this scope. Monotonic — once an entry is written it never
    /// changes (Koan data is immutable; rebinding within a scope is illegal,
    /// so a fully-finalized type's resolution is stable for the scope's lifetime).
    /// Entries are written only when the elaborated `KType` and every user-type
    /// it references are fully finalized — the finalize gate prevents caching
    /// mid-SCC pre-close identities. See
    /// `Scope::resolve_type_expr` for the writer + finalize-gate logic.
    type_expr_memo: RefCell<HashMap<TypeExpr, &'a KType>>,
}

impl<'a> Bindings<'a> {
    pub fn new() -> Self {
        Self {
            types: RefCell::new(HashMap::new()),
            data: RefCell::new(HashMap::new()),
            functions: RefCell::new(HashMap::new()),
            placeholders: RefCell::new(HashMap::new()),
            pending: PendingTypes::new(),
            type_expr_memo: RefCell::new(HashMap::new()),
        }
    }

    /// Layer-2 cache reader: look up `te` in this scope's `type_expr_memo`. Returns
    /// the cached `&'a KType` if present. `Scope::resolve_type_expr` owns the writer
    /// side — see the finalize-gate logic there.
    pub fn type_expr_memo_get(&self, te: &TypeExpr) -> Option<&'a KType> {
        self.type_expr_memo.borrow().get(te).copied()
    }

    /// Layer-2 cache writer: insert `(te → kt)` into this scope's `type_expr_memo`.
    /// Caller is responsible for arena-allocating `kt` and checking the
    /// finalize gate before writing. Monotonic — overwrites are not expected and
    /// would indicate a violation of the immutable-binding invariant; we silently
    /// keep the existing entry rather than panic since the value would be equal
    /// by definition.
    pub fn type_expr_memo_insert(&self, te: TypeExpr, kt: &'a KType) {
        let mut memo = self.type_expr_memo.borrow_mut();
        memo.entry(te).or_insert(kt);
    }

    /// Read-only handle for the ~12 read sites in builtins and resolver code. The returned
    /// `Ref<'_, _>` has the lifetime of `&self`, so the usual `for (k, v) in
    /// scope.bindings().data().iter()` pattern extends the temporary through the loop —
    /// same semantics as the prior `RefCell::borrow()` calls.
    pub fn data(&self) -> Ref<'_, HashMap<String, &'a KObject<'a>>> {
        self.data.borrow()
    }

    /// Read-only handle for `resolve_dispatch`'s outer-chain walk and the submission-time
    /// `pre_run` extractor. Same `Ref<'_, _>` semantics as [`Bindings::data`].
    pub fn functions(&self) -> Ref<'_, HashMap<UntypedKey, Vec<&'a KFunction<'a>>>> {
        self.functions.borrow()
    }

    /// Read-only handle for the resolver's placeholder lookup. Same semantics as
    /// [`Bindings::data`].
    pub fn placeholders(&self) -> Ref<'_, HashMap<String, NodeId>> {
        self.placeholders.borrow()
    }

    /// Read-only handle for the type-side resolver path landing in stage 1.4
    /// (`Scope::resolve_type`) and the eventual type-class consumer migration.
    /// Same `Ref<'_, _>` semantics as [`Bindings::data`].
    pub fn types(&self) -> Ref<'_, HashMap<String, &'a KType>> {
        self.types.borrow()
    }

    /// Test helper: assert a slot is present in `bindings.data` and return the
    /// `KObject`. Collapses the `data().get(name).copied().expect(msg)` /
    /// `let data = ...; let v = data.get(name).expect(msg)` patterns repeated
    /// across SIG-shape unit tests.
    #[cfg(test)]
    pub fn expect_value(&self, name: &str) -> &'a KObject<'a> {
        self.data
            .borrow()
            .get(name)
            .copied()
            .unwrap_or_else(|| panic!("expected bindings.data[{name:?}] to be present"))
    }

    /// Test helper: assert a slot is present in `bindings.types`. Same role as
    /// [`Bindings::expect_value`] for the type-side map.
    #[cfg(test)]
    pub fn expect_type(&self, name: &str) -> &'a KType {
        self.types
            .borrow()
            .get(name)
            .copied()
            .unwrap_or_else(|| panic!("expected bindings.types[{name:?}] to be present"))
    }

    /// Read-only handle for the SCC pre-registration map. Same `Ref<'_, _>` semantics
    /// as [`Bindings::data`]. Stage-3.2 writers are [`Bindings::insert_pending_type`]
    /// (returns a [`PendingBinderGuard`] whose Drop removes the entry) and
    /// [`Bindings::record_pending_edge`].
    pub fn pending_types(&self) -> Ref<'_, HashMap<String, PendingTypeEntry<'a>>> {
        self.pending.get()
    }

    /// Delegates to [`PendingTypes::insert`]; see that method for the lifecycle
    /// contract and panic conditions.
    pub fn insert_pending_type(
        &'a self,
        name: String,
        entry: PendingTypeEntry<'a>,
    ) -> PendingBinderGuard<'a> {
        self.pending.insert(name, entry)
    }

    /// Delegates to [`PendingTypes::record_edge`].
    pub fn record_pending_edge(&self, from: &str, to: String) {
        self.pending.record_edge(from, to);
    }

    /// Test helper: explicitly remove a pending-type entry to exercise the
    /// guard Drop's "tolerates absent entry" path.
    #[cfg(test)]
    pub fn pending_remove(&self, name: &str) {
        self.pending.remove(name);
    }

    /// LET-style value bind. Errors `Rebind` if `data[name]` already exists. When `obj`
    /// wraps a `KFunction`, the function is *also* mirrored into the `functions` bucket
    /// keyed by its untyped signature so dispatch finds it — supports `LET f = (FN ...)`
    /// where the bound name doubles as a callable verb.
    ///
    /// `Conflict` outcome means borrow contention (caller queues); `Err` is semantic
    /// rejection (not queued).
    pub fn try_bind_value(
        &self,
        name: &str,
        obj: &'a KObject<'a>,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply(name, obj, obj.as_function())
    }

    /// FN-style overload registration. Adds `fn_ref` to the `functions` bucket keyed by
    /// its untyped signature, then inserts `obj` into `data[name]`. Errors:
    /// - `DuplicateOverload` if the bucket already holds an exact-signature equal function.
    /// - `Rebind` if `data[name]` holds a non-function.
    pub fn try_register_function(
        &self,
        name: &str,
        fn_ref: &'a KFunction<'a>,
        obj: &'a KObject<'a>,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply(name, obj, Some(fn_ref))
    }

    /// Register `name` → `kt` in the dedicated type-binding map. Errors `Rebind`
    /// if `types[name]` already exists; returns `Ok(Conflict)` on borrow
    /// contention (caller queues — same shape as [`Bindings::try_bind_value`]
    /// and [`Bindings::try_register_function`]). Best-effort placeholder clear
    /// on success.
    ///
    /// Called by [`super::scope::Scope::register_type`] (rewired in stage 1.4) and
    /// by [`super::pending::PendingQueue`]'s `Type`-variant drain arm.
    pub fn try_register_type(
        &self,
        name: &str,
        kt: &'a KType,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply_type(name, kt)
    }

    /// Transactional dual-write for nominal declarations (STRUCT / UNION / MODULE):
    /// inserts identity `kt` into `types[name]` and runtime carrier `obj` into
    /// `data[name]` atomically. Borrow order is `types → data` (the `functions` map
    /// is deliberately untouched — nominal carriers are not callable verbs).
    ///
    /// Contract:
    /// - Returns `Ok(Conflict)` if either `types` or `data` is borrowed elsewhere,
    ///   with no write attempted (mirrors [`Bindings::try_bind_value`] /
    ///   [`Bindings::try_register_function`] queueing).
    /// - Stage 3.2 *cycle-close-idempotent* path: if `types[name]` is already
    ///   populated with a `KType` value-equal to the new `kt` AND `data[name]` is
    ///   empty, write only the carrier. This is the post-SCC-pre-registration
    ///   finalize path — the SCC sweep installs each cycle member's identity into
    ///   `types` synchronously before any member's body / Combine-finish builds
    ///   its carrier, so the eventual `register_nominal` call hits this arm with
    ///   matching identity.
    /// - Returns `Err(Rebind)` if `data[name]` already exists OR `types[name]`
    ///   exists with a *different* `KType`. The pre-check runs before any insert,
    ///   so a collision leaves both maps untouched — no partial write to roll back.
    /// - On success inserts into both maps (or just `data` on the idempotent arm),
    ///   then best-effort clears any matching `placeholders[name]` (same tail as
    ///   [`Bindings::try_apply`] / [`Bindings::try_apply_type`]).
    pub fn try_register_nominal(
        &self,
        name: &str,
        kt: &'a KType,
        obj: &'a KObject<'a>,
    ) -> Result<ApplyOutcome, KError> {
        let mut types = match self.types.try_borrow_mut() {
            Ok(t) => t,
            Err(_) => return Ok(ApplyOutcome::Conflict),
        };
        let mut data = match self.data.try_borrow_mut() {
            Ok(d) => d,
            Err(_) => {
                // Drop the `types` handle implicitly on return; no write happened
                // yet, so nothing to roll back.
                drop(types);
                return Ok(ApplyOutcome::Conflict);
            }
        };
        if data.contains_key(name) {
            return Err(KError::new(KErrorKind::Rebind { name: name.to_string() }));
        }
        match types.get(name).copied() {
            None => {
                types.insert(name.to_string(), kt);
            }
            Some(existing) if existing == kt => {
                // Cycle-close-idempotent: SCC pre-registration already wrote the
                // identity. Skip the types write; carrier-write below completes the
                // pair.
            }
            Some(_) => {
                return Err(KError::new(KErrorKind::Rebind { name: name.to_string() }));
            }
        }
        data.insert(name.to_string(), obj);
        drop(data);
        drop(types);
        self.clear_placeholder_best_effort(name);
        Ok(ApplyOutcome::Applied)
    }

    /// Install a dispatch-time placeholder for `name` -> producer slot `idx`.
    ///
    /// Lenient when `data[name]` already holds a `KObject::KFunction`: silent no-op.
    /// Forward references resolve through the existing function value; a new FN overload
    /// joins the per-signature bucket on finalize without consumers needing to park.
    ///
    /// Errors `Rebind` if `data[name]` holds a non-function or if `placeholders[name]`
    /// already maps to a *different* `NodeId`. Idempotent if re-entered with the same
    /// `NodeId`.
    ///
    /// Unlike [`Bindings::try_bind_value`] and [`Bindings::try_register_function`], this
    /// method panics on borrow conflict rather than returning `Conflict` — placeholder
    /// installs happen at dispatch-time outside the re-entrant-bind hot path, so a conflict
    /// here indicates a programming error, not a queueable retry.
    pub fn try_install_placeholder(&self, name: String, idx: NodeId) -> Result<(), KError> {
        if let Some(existing) = self.data.borrow().get(&name) {
            if matches!(existing, KObject::KFunction(_, _)) {
                return Ok(());
            }
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        let mut ph = self.placeholders.borrow_mut();
        if let Some(existing) = ph.get(&name).copied() {
            if existing == idx {
                return Ok(());
            }
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        ph.insert(name, idx);
        Ok(())
    }

    /// Replay another `Bindings`'s `data` through `try_apply` on self. The single entry
    /// point for ascription's bulk-install — snapshots the source `data` into a `Vec` and
    /// releases `src`'s `Ref` before the replay so re-entrant ascription cannot deadlock.
    /// The shared helper re-mirrors `KFunction` entries into `functions` exactly once, so
    /// the caller does not need to walk `src.functions` separately (that's the point of
    /// routing through `try_apply`).
    ///
    /// The replay is order-independent: the dispatch bucket is order-insensitive once
    /// dedupe is applied. Panics on `Conflict` — a fresh `Bindings` should never hit a
    /// borrow conflict against itself, and a conflict here is a programming error.
    pub fn try_bulk_install_from(&self, src: &Bindings<'a>) -> Result<(), KError> {
        let snapshot: Vec<(String, &'a KObject<'a>)> = src
            .data()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        for (name, obj) in snapshot {
            match self.try_apply(&name, obj, obj.as_function())? {
                ApplyOutcome::Applied => {}
                ApplyOutcome::Conflict => {
                    unreachable!(
                        "try_bulk_install_from on a fresh Bindings should not hit borrow conflict",
                    );
                }
            }
        }
        Ok(())
    }

    /// The shared write path for type-only bindings. Borrows `types` only —
    /// no `data`/`functions` involvement, since this primitive writes a single
    /// map. [`Bindings::try_register_nominal`] inlines an analogous
    /// `types → data` pre-check + insert (rather than reusing this helper)
    /// because it adds the second-map dependency to the transaction.
    ///
    /// `Conflict` is reserved for borrow contention; `Err(Rebind)` is the
    /// semantic-rejection path. On success, also clears any matching
    /// placeholder (best-effort, mirrors `try_apply`'s tail).
    fn try_apply_type(
        &self,
        name: &str,
        kt: &'a KType,
    ) -> Result<ApplyOutcome, KError> {
        let mut types = match self.types.try_borrow_mut() {
            Ok(t) => t,
            Err(_) => return Ok(ApplyOutcome::Conflict),
        };
        if types.contains_key(name) {
            return Err(KError::new(KErrorKind::Rebind { name: name.to_string() }));
        }
        types.insert(name.to_string(), kt);
        drop(types);
        self.clear_placeholder_best_effort(name);
        Ok(ApplyOutcome::Applied)
    }

    /// The shared write path. Borrows `functions` first (only when `fn_part.is_some()`),
    /// then `data` — preserves the non-fn shortcut so `register_type`, LET-body, and
    /// param-binding flows that run under a live outer `functions` borrow stay
    /// deadlock-free. `Conflict` is reserved for borrow contention; semantic errors come
    /// through `Err(KError)`.
    ///
    /// Unified dedupe: when `fn_part.is_some()` and a same-name `data` entry exists, walk
    /// the bucket — `ptr::eq` is silent-success short-circuit (preserves intentional
    /// aliases like `LET g = (f)`), `signatures_exact_equal` is `DuplicateOverload`. Both
    /// `FN`-decl and `LET`-binds-`FN` paths see both rules. This closes the latent gap
    /// where `try_apply_value`'s pointer-only dedupe could silently double the bucket on a
    /// structurally identical but pointer-distinct re-bind.
    fn try_apply(
        &self,
        name: &str,
        obj: &'a KObject<'a>,
        fn_part: Option<&'a KFunction<'a>>,
    ) -> Result<ApplyOutcome, KError> {
        // Borrow `functions` first only when there is a function-side mirror to write —
        // skipping otherwise preserves the non-fn shortcut documented above.
        let mut functions_handle = if fn_part.is_some() {
            match self.functions.try_borrow_mut() {
                Ok(g) => Some(g),
                Err(_) => return Ok(ApplyOutcome::Conflict),
            }
        } else {
            None
        };
        let mut data = match self.data.try_borrow_mut() {
            Ok(d) => d,
            Err(_) => return Ok(ApplyOutcome::Conflict),
        };
        // Semantic rejection on existing `data[name]`. The `fn_part.is_some()` and entry-
        // already-`KFunction` case falls through to bucket dedupe (overload-add path).
        if let Some(existing) = data.get(name) {
            match fn_part {
                None => return Err(KError::new(KErrorKind::Rebind { name: name.to_string() })),
                Some(_) => {
                    if !matches!(existing, KObject::KFunction(_, _)) {
                        return Err(KError::new(KErrorKind::Rebind { name: name.to_string() }));
                    }
                }
            }
        }
        if let (Some(f_ref), Some(functions)) = (fn_part, functions_handle.as_mut()) {
            let key = f_ref.signature.untyped_key();
            let bucket = functions.entry(key).or_default();
            let mut already_present = false;
            for existing in bucket.iter() {
                if std::ptr::eq(*existing, f_ref) {
                    already_present = true;
                    break;
                }
                if existing.signature.exact_equal(&f_ref.signature) {
                    return Err(KError::new(KErrorKind::DuplicateOverload {
                        name: name.to_string(),
                        signature: existing.summarize(),
                    }));
                }
            }
            if !already_present {
                bucket.push(f_ref);
            }
        }
        data.insert(name.to_string(), obj);
        drop(data);
        drop(functions_handle);
        self.clear_placeholder_best_effort(name);
        Ok(ApplyOutcome::Applied)
    }

    /// Remove any matching placeholder. `try_borrow_mut().ok()` tolerates a
    /// caller holding a placeholder borrow up the stack — promoting this to
    /// `borrow_mut()` would panic for a previously-tolerated case. Shared
    /// tail of every successful write path ([`Bindings::try_apply`],
    /// [`Bindings::try_apply_type`], [`Bindings::try_register_nominal`]).
    fn clear_placeholder_best_effort(&self, name: &str) {
        if let Ok(mut ph) = self.placeholders.try_borrow_mut() {
            ph.remove(name);
        }
    }
}

impl<'a> Default for Bindings<'a> {
    fn default() -> Self {
        Self::new()
    }
}

/// `Conflict` is reserved for borrow contention; semantic errors come through `Err(KError)`.
/// `pub` so [`super::scope`]'s match arms compile; not re-exported beyond `core::`.
pub enum ApplyOutcome {
    Applied,
    Conflict,
}

#[cfg(test)]
mod tests;
