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

use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;

use crate::runtime::machine::model::ast::KExpression;
use crate::runtime::machine::core::kfunction::{KFunction, NodeId};
use crate::runtime::machine::model::types::{KType, UntypedKey, UserTypeKind};
use crate::runtime::machine::model::values::KObject;

use super::kerror::{KError, KErrorKind};

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
    /// In-flight named-type binders (STRUCT / named-UNION) that have entered their
    /// elaborator and may park on cross-references. Populated by struct_def / union
    /// before elaboration; consulted by the elaborator's `Resolution::Placeholder`
    /// arm to record dependency edges and run DFS cycle detection. Drained either by
    /// the happy-path finalize or by `close_type_cycle` on cycle close. MODULE does
    /// NOT participate — module bodies park on the outer scheduler, not on type-name
    /// resolution inside elaboration (see roadmap stage 3.2).
    pending_types: RefCell<HashMap<String, PendingTypeEntry<'a>>>,
}

/// Per-binder state captured at the moment a STRUCT / named-UNION enters its
/// elaborator. `schema_expr` is the unelaborated body the cycle-close sweep
/// re-runs against the post-pre-registration scope; `kind` and `scope_id` are
/// the identity fields the cycle-close writes into `bindings.types` as
/// `KType::UserType { kind, scope_id, name }`; `edges` is the adjacency list
/// the elaborator appends to each time this binder parks on a fellow in-flight
/// binder's placeholder.
pub struct PendingTypeEntry<'a> {
    pub kind: UserTypeKind,
    pub scope_id: usize,
    pub schema_expr: KExpression<'a>,
    pub edges: Vec<String>,
}

impl<'a> Bindings<'a> {
    pub fn new() -> Self {
        Self {
            types: RefCell::new(HashMap::new()),
            data: RefCell::new(HashMap::new()),
            functions: RefCell::new(HashMap::new()),
            placeholders: RefCell::new(HashMap::new()),
            pending_types: RefCell::new(HashMap::new()),
        }
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
        *self
            .data
            .borrow()
            .get(name)
            .unwrap_or_else(|| panic!("expected bindings.data[{name:?}] to be present"))
    }

    /// Test helper: assert a slot is present in `bindings.types`. Same role as
    /// [`Bindings::expect_value`] for the type-side map.
    #[cfg(test)]
    pub fn expect_type(&self, name: &str) -> &'a KType {
        *self
            .types
            .borrow()
            .get(name)
            .unwrap_or_else(|| panic!("expected bindings.types[{name:?}] to be present"))
    }

    /// Read-only handle for the SCC pre-registration map. Same `Ref<'_, _>` semantics
    /// as [`Bindings::data`]. Stage-3.2 writers are [`Bindings::insert_pending_type`]
    /// / [`Bindings::record_pending_edge`] / [`Bindings::remove_pending_type`] plus
    /// the `RefMut` accessor [`Bindings::pending_types_mut`] for the cycle-close sweep.
    pub fn pending_types(&self) -> Ref<'_, HashMap<String, PendingTypeEntry<'a>>> {
        self.pending_types.borrow()
    }

    /// Mutable handle for the cycle-close sweep — it removes every cycle member's
    /// entry under one borrow before re-elaborating any schema. Callers at the
    /// elaborator's `Resolution::Placeholder` arm should use the targeted
    /// [`Bindings::record_pending_edge`] / [`Bindings::remove_pending_type`]
    /// methods instead so that nested elaboration calls don't deadlock against a
    /// held `RefMut`.
    pub fn pending_types_mut(&self) -> RefMut<'_, HashMap<String, PendingTypeEntry<'a>>> {
        self.pending_types.borrow_mut()
    }

    /// Install a new in-flight binder entry. Called by struct_def / union before
    /// running the elaborator so the elaborator's placeholder arm can observe the
    /// binder's `pending_types` presence and record cross-binder edges.
    ///
    /// Panics on borrow conflict — pending-type writes happen at body-entry, outside
    /// the re-entrant `try_apply` hot path; a conflict here is a programming error.
    /// Panics on duplicate name — same scope cannot have two in-flight binders for
    /// one name (placeholders block the second dispatch from progressing this far).
    pub fn insert_pending_type(&self, name: String, entry: PendingTypeEntry<'a>) {
        let mut map = self.pending_types.borrow_mut();
        if map.contains_key(&name) {
            panic!(
                "insert_pending_type: `{name}` already in flight — duplicate dispatch \
                 reached body-entry, which the placeholder install should have blocked",
            );
        }
        map.insert(name, entry);
    }

    /// Append `to` to `from`'s adjacency list (no-op if `from` isn't a pending binder —
    /// the elaborator can be running under a non-binder context). Used by the
    /// elaborator's `Resolution::Placeholder` arm when the parked-on name is itself
    /// an in-flight binder.
    ///
    /// Panics on borrow conflict for the same reason as
    /// [`Bindings::insert_pending_type`]; deduplicates against existing edges so a
    /// re-elaboration that re-parks on the same name doesn't grow the list.
    pub fn record_pending_edge(&self, from: &str, to: String) {
        let mut map = self.pending_types.borrow_mut();
        if let Some(entry) = map.get_mut(from) {
            if !entry.edges.iter().any(|e| e == &to) {
                entry.edges.push(to);
            }
        }
    }

    /// Remove and return `name`'s entry. The cycle-close sweep removes every cycle
    /// member before re-elaborating, and the happy-path finalize removes the
    /// just-finalized binder. Panics on borrow conflict (same rationale as
    /// [`Bindings::insert_pending_type`]).
    pub fn remove_pending_type(&self, name: &str) -> Option<PendingTypeEntry<'a>> {
        self.pending_types.borrow_mut().remove(name)
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
        let fn_part = match obj {
            KObject::KFunction(f, _) => Some(*f),
            _ => None,
        };
        self.try_apply(name, obj, fn_part)
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
        if let Ok(mut ph) = self.placeholders.try_borrow_mut() {
            ph.remove(name);
        }
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
            let fn_part = match obj {
                KObject::KFunction(f, _) => Some(*f),
                _ => None,
            };
            match self.try_apply(&name, obj, fn_part)? {
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
        if let Ok(mut ph) = self.placeholders.try_borrow_mut() {
            ph.remove(name);
        }
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
                if signatures_exact_equal(&existing.signature, &f_ref.signature) {
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
        // Best-effort placeholder clear — `try_borrow_mut().ok()` tolerates a caller
        // holding a placeholder borrow up the stack. Promoting this to `borrow_mut()`
        // would panic for a previously-tolerated case.
        if let Ok(mut ph) = self.placeholders.try_borrow_mut() {
            ph.remove(name);
        }
        Ok(ApplyOutcome::Applied)
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

/// Structural equality on shape + per-Argument `KType` + return type. Independent of
/// `Argument::name` — two overloads with matching shape and types collide for dispatch
/// regardless of parameter naming.
///
/// Return-type equality flows through [`crate::runtime::machine::model::types::ReturnType`]'s
/// `PartialEq` impl. `Resolved` compares by inner `KType`; `Deferred` compares by
/// carrier variant + payload (see `ReturnType::eq`'s docstring for the equality
/// rule on the parens-form `Expression` variant). Two FN-defs whose deferred
/// carriers are structurally identical surface as `DuplicateOverload`, which
/// matches the existing semantic — they are interchangeable for dispatch.
fn signatures_exact_equal<'a>(
    a: &crate::runtime::machine::model::types::ExpressionSignature<'a>,
    b: &crate::runtime::machine::model::types::ExpressionSignature<'a>,
) -> bool {
    use crate::runtime::machine::model::types::SignatureElement;
    if a.return_type != b.return_type {
        return false;
    }
    if a.elements.len() != b.elements.len() {
        return false;
    }
    a.elements.iter().zip(b.elements.iter()).all(|(x, y)| match (x, y) {
        (SignatureElement::Keyword(s), SignatureElement::Keyword(t)) => s == t,
        (SignatureElement::Argument(ax), SignatureElement::Argument(ay)) => ax.ktype == ay.ktype,
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    //! Unit coverage for the stage-1.2 `types` map and its `try_register_type` write
    //! primitive, plus the stage-1.3 `try_register_nominal` dual-write primitive.
    //! `try_register_type` is now live (stage 1.4 wired `Scope::register_type` onto it);
    //! `try_register_nominal` remains unused until stage 3 migrates STRUCT / UNION /
    //! MODULE finalize paths onto it. These tests directly exercise `Bindings` against
    //! `RuntimeArena`-allocated `&KType` / `&KObject` values.

    use super::*;
    use crate::runtime::machine::core::arena::RuntimeArena;
    use crate::runtime::machine::model::types::KType;
    use crate::runtime::machine::model::values::KObject;

    #[test]
    fn try_register_type_inserts_into_types_map() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let kt: &KType = arena.alloc_ktype(KType::Number);
        let outcome = bindings
            .try_register_type("Foo", kt)
            .expect("try_register_type should succeed on fresh bindings");
        assert!(matches!(outcome, ApplyOutcome::Applied));
        // Type-side storage is the only home for this binding — `data` stays empty.
        let stored = *bindings.types().get("Foo").expect("Foo should be in types map");
        assert!(std::ptr::eq(stored, kt));
        assert!(bindings.data().get("Foo").is_none());
    }

    #[test]
    fn try_register_type_rejects_collision_with_rebind() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let kt1: &KType = arena.alloc_ktype(KType::Number);
        let kt2: &KType = arena.alloc_ktype(KType::Str);
        bindings.try_register_type("Foo", kt1).expect("first register should succeed");
        let err = match bindings.try_register_type("Foo", kt2) {
            Err(e) => e,
            Ok(_) => panic!("second register on same name should error, not succeed"),
        };
        assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "Foo"));
        // First binding remains intact — the collision must not overwrite.
        let stored = *bindings.types().get("Foo").expect("Foo should still be present");
        assert!(std::ptr::eq(stored, kt1));
    }

    #[test]
    fn try_register_type_yields_conflict_on_live_types_borrow() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let kt: &KType = arena.alloc_ktype(KType::Number);
        let _r = bindings.types();
        let outcome = bindings
            .try_register_type("Foo", kt)
            .expect("conflict path returns Ok(Conflict), not Err");
        assert!(matches!(outcome, ApplyOutcome::Conflict));
        // Live read borrow blocked the write; nothing was inserted.
        assert!(_r.get("Foo").is_none());
    }

    #[test]
    fn try_register_type_clears_matching_placeholder() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let kt: &KType = arena.alloc_ktype(KType::Number);
        bindings
            .try_install_placeholder("Bar".to_string(), NodeId(7))
            .expect("placeholder install should succeed on fresh bindings");
        assert!(bindings.placeholders().contains_key("Bar"));
        bindings
            .try_register_type("Bar", kt)
            .expect("type register should succeed and clear placeholder");
        assert!(!bindings.placeholders().contains_key("Bar"));
    }

    #[test]
    fn try_register_type_does_not_touch_data_or_functions() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let kt: &KType = arena.alloc_ktype(KType::Number);
        bindings.try_register_type("Foo", kt).expect("register should succeed");
        assert!(bindings.data().is_empty());
        assert!(bindings.functions().is_empty());
    }

    #[test]
    fn try_register_nominal_inserts_into_both_maps() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let kt: &KType = arena.alloc_ktype(KType::Number);
        let obj: &KObject<'_> = arena.alloc_object(KObject::Number(1.0));
        let outcome = bindings
            .try_register_nominal("Foo", kt, obj)
            .expect("try_register_nominal should succeed on fresh bindings");
        assert!(matches!(outcome, ApplyOutcome::Applied));
        // Dual-write: both maps hold the exact pointers we supplied.
        let stored_kt = *bindings.types().get("Foo").expect("Foo should be in types map");
        let stored_obj = *bindings.data().get("Foo").expect("Foo should be in data map");
        assert!(std::ptr::eq(stored_kt, kt));
        assert!(std::ptr::eq(stored_obj, obj));
    }

    #[test]
    fn try_register_nominal_rejects_collision_in_types_with_rebind() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let kt_existing: &KType = arena.alloc_ktype(KType::Number);
        let kt_new: &KType = arena.alloc_ktype(KType::Str);
        let obj: &KObject<'_> = arena.alloc_object(KObject::Number(1.0));
        bindings
            .try_register_type("Foo", kt_existing)
            .expect("pre-seed types[Foo] should succeed");
        let err = match bindings.try_register_nominal("Foo", kt_new, obj) {
            Err(e) => e,
            Ok(_) => panic!("collision on types side must Err(Rebind), not Ok"),
        };
        assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "Foo"));
        // Pre-check rejected the transaction before either insert: data side untouched.
        assert!(bindings.data().get("Foo").is_none());
        // First types binding survives intact.
        let stored = *bindings.types().get("Foo").expect("Foo should still be in types");
        assert!(std::ptr::eq(stored, kt_existing));
    }

    #[test]
    fn try_register_nominal_rejects_collision_in_data_with_rebind() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let kt: &KType = arena.alloc_ktype(KType::Number);
        let obj_existing: &KObject<'_> = arena.alloc_object(KObject::Number(42.0));
        let obj_new: &KObject<'_> = arena.alloc_object(KObject::Number(7.0));
        bindings
            .try_bind_value("Foo", obj_existing)
            .expect("pre-seed data[Foo] should succeed");
        let err = match bindings.try_register_nominal("Foo", kt, obj_new) {
            Err(e) => e,
            Ok(_) => panic!("collision on data side must Err(Rebind), not Ok"),
        };
        assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "Foo"));
        // Pre-check rejected the transaction before either insert: types side untouched.
        assert!(bindings.types().get("Foo").is_none());
        // First data binding survives intact.
        let stored = *bindings.data().get("Foo").expect("Foo should still be in data");
        assert!(std::ptr::eq(stored, obj_existing));
    }

    #[test]
    fn try_register_nominal_yields_conflict_on_live_types_borrow() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let kt: &KType = arena.alloc_ktype(KType::Number);
        let obj: &KObject<'_> = arena.alloc_object(KObject::Number(1.0));
        let _r = bindings.types();
        let outcome = bindings
            .try_register_nominal("Foo", kt, obj)
            .expect("conflict path returns Ok(Conflict), not Err");
        assert!(matches!(outcome, ApplyOutcome::Conflict));
        // Borrow contention on `types` blocked the write: both maps untouched.
        assert!(_r.get("Foo").is_none());
        assert!(bindings.data().get("Foo").is_none());
    }

    /// Stage 3.0d scaffolding: `Bindings::new()` initializes `pending_types` empty.
    /// No writer in 3.0 — the field is observable only as an empty map until stage 3.2
    /// wires the SCC pre-registration pass.
    #[test]
    fn new_bindings_has_empty_pending_types() {
        let bindings: Bindings<'_> = Bindings::new();
        assert!(bindings.pending_types().is_empty());
    }

    /// Stage 3.2: the SCC cycle-close sweep pre-installs each member's identity via
    /// `try_register_type`. The eventual `try_register_nominal` call observes the
    /// matching pre-installed identity and writes only the carrier into `data`. Pins
    /// the idempotent arm against regression.
    #[test]
    fn try_register_nominal_is_idempotent_against_matching_pre_installed_types() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        // Build two pointer-distinct but value-equal KTypes — cycle-close and finalize
        // each alloc their own.
        let kt_pre: &KType = arena.alloc_ktype(KType::UserType {
            kind: UserTypeKind::Struct,
            scope_id: 0xDEAD_BEEF,
            name: "Foo".into(),
        });
        let kt_finalize: &KType = arena.alloc_ktype(KType::UserType {
            kind: UserTypeKind::Struct,
            scope_id: 0xDEAD_BEEF,
            name: "Foo".into(),
        });
        assert!(!std::ptr::eq(kt_pre, kt_finalize), "alloc should produce distinct pointers");
        assert_eq!(*kt_pre, *kt_finalize, "values must be equal");
        let obj: &KObject<'_> = arena.alloc_object(KObject::Number(1.0));
        bindings.try_register_type("Foo", kt_pre).unwrap();
        // try_register_nominal: types[Foo] already populated with matching identity,
        // data[Foo] empty → idempotent path, write only data.
        let outcome = bindings
            .try_register_nominal("Foo", kt_finalize, obj)
            .expect("idempotent arm should succeed");
        assert!(matches!(outcome, ApplyOutcome::Applied));
        // The types entry keeps the PRE-installed pointer (not the finalize's).
        let stored_kt = *bindings.types().get("Foo").expect("Foo in types");
        assert!(std::ptr::eq(stored_kt, kt_pre));
        // The data entry is the finalize's carrier.
        let stored_obj = *bindings.data().get("Foo").expect("Foo in data");
        assert!(std::ptr::eq(stored_obj, obj));
    }

    #[test]
    fn try_register_nominal_clears_matching_placeholder() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let kt: &KType = arena.alloc_ktype(KType::Number);
        let obj: &KObject<'_> = arena.alloc_object(KObject::Number(1.0));
        bindings
            .try_install_placeholder("Bar".to_string(), NodeId(7))
            .expect("placeholder install should succeed on fresh bindings");
        assert!(bindings.placeholders().contains_key("Bar"));
        bindings
            .try_register_nominal("Bar", kt, obj)
            .expect("nominal register should succeed and clear placeholder");
        assert!(!bindings.placeholders().contains_key("Bar"));
    }
}
