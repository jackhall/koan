//! The lexical binding façade: four co-mutating `RefCell` maps (`types`, `data`,
//! `functions`, `placeholders`) plus the shared validated write paths
//! ([`Bindings::try_apply`] for `data`/`functions`, [`Bindings::try_apply_type`] for
//! `types`) that keep the dual-map invariant — every `data[name]` entry wrapping a
//! `KFunction` lives in `functions[signature.untyped_key()]`. Nominal declarations
//! (STRUCT / UNION / MODULE) go through [`Bindings::try_register_nominal`], a
//! transactional dual-write into `types` + `data`.
//!
//! Borrow discipline across the four maps: `types → functions → data`, with `types`
//! only acquired when writing types. [`Scope`] embeds the façade by value so all
//! interior borrows arbitrate against one another.

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
/// `types` (name → `&KType`), `data` (name → value), `functions`
/// (untyped-signature bucket → overloads), and `placeholders` (name → producer
/// NodeId for dispatch-time forward references).
///
/// [`Bindings::try_apply`] enforces the dual-map invariant — every `data[name]`
/// entry wrapping a `KFunction` lives in `functions[signature.untyped_key()]` — and
/// unifies dedupe (`ptr::eq` fast-path then `signatures_exact_equal`) across the
/// LET-binds-FN and `FN`-decl paths. [`Bindings::try_apply_type`] is the parallel
/// write primitive for the `types` map. [`Bindings::try_register_nominal`] composes
/// `types` + `data` writes transactionally for nominal declarations (nominal
/// carriers are not callable verbs, so `functions` is untouched).
///
/// Borrow discipline: `types → functions → data`.
///
/// Lifetime `'a` matches the arena lifetime of the stored references.
pub struct Bindings<'a> {
    types: RefCell<HashMap<String, &'a KType<'a>>>,
    data: RefCell<HashMap<String, &'a KObject<'a>>>,
    functions: RefCell<HashMap<UntypedKey, Vec<&'a KFunction<'a>>>>,
    placeholders: RefCell<HashMap<String, NodeId>>,
    /// In-flight named-type binders (STRUCT / named-UNION). Populated by
    /// struct_def / union before elaboration; consulted by the elaborator's
    /// `Resolution::Placeholder` arm to record dependency edges and run DFS
    /// cycle detection. See [`pending`] for the surface methods.
    pending: PendingTypes<'a>,
    /// Scope-bound `TypeExpr` → `&KType` resolution cache. Monotonic — entries
    /// are written only when the elaborated `KType` and every user-type it
    /// references are fully finalized; the finalize gate prevents caching
    /// mid-SCC pre-close identities. `Scope::resolve_type_expr` owns the writer.
    type_expr_memo: RefCell<HashMap<TypeExpr, &'a KType<'a>>>,
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

    pub fn type_expr_memo_get(&self, te: &TypeExpr) -> Option<&'a KType<'a>> {
        self.type_expr_memo.borrow().get(te).copied()
    }

    /// Insert `(te → kt)` into the resolution cache. Caller is responsible for
    /// arena-allocating `kt` and checking the finalize gate before writing.
    /// Monotonic: overwrites would indicate a violation of the immutable-binding
    /// invariant; we silently keep the existing entry rather than panic since
    /// the value would be equal by definition.
    pub fn type_expr_memo_insert(&self, te: TypeExpr, kt: &'a KType<'a>) {
        let mut memo = self.type_expr_memo.borrow_mut();
        memo.entry(te).or_insert(kt);
    }

    pub fn data(&self) -> Ref<'_, HashMap<String, &'a KObject<'a>>> {
        self.data.borrow()
    }

    pub fn functions(&self) -> Ref<'_, HashMap<UntypedKey, Vec<&'a KFunction<'a>>>> {
        self.functions.borrow()
    }

    pub fn placeholders(&self) -> Ref<'_, HashMap<String, NodeId>> {
        self.placeholders.borrow()
    }

    pub fn types(&self) -> Ref<'_, HashMap<String, &'a KType<'a>>> {
        self.types.borrow()
    }

    #[cfg(test)]
    pub fn expect_value(&self, name: &str) -> &'a KObject<'a> {
        self.data
            .borrow()
            .get(name)
            .copied()
            .unwrap_or_else(|| panic!("expected bindings.data[{name:?}] to be present"))
    }

    #[cfg(test)]
    pub fn expect_type(&self, name: &str) -> &'a KType<'a> {
        self.types
            .borrow()
            .get(name)
            .copied()
            .unwrap_or_else(|| panic!("expected bindings.types[{name:?}] to be present"))
    }

    /// Read-only handle for the SCC pre-registration map. Writers are
    /// [`Bindings::insert_pending_type`] (returns a [`PendingBinderGuard`] whose
    /// Drop removes the entry) and [`Bindings::record_pending_edge`].
    pub fn pending_types(&self) -> Ref<'_, HashMap<String, PendingTypeEntry<'a>>> {
        self.pending.get()
    }

    pub fn insert_pending_type(
        &'a self,
        name: String,
        entry: PendingTypeEntry<'a>,
    ) -> PendingBinderGuard<'a> {
        self.pending.insert(name, entry)
    }

    pub fn record_pending_edge(&self, from: &str, to: String) {
        self.pending.record_edge(from, to);
    }

    /// Exercises the guard Drop's "tolerates absent entry" path.
    #[cfg(test)]
    pub fn pending_remove(&self, name: &str) {
        self.pending.remove(name);
    }

    /// LET-style value bind. Errors `Rebind` if `data[name]` already exists. When `obj`
    /// wraps a `KFunction`, the function is *also* mirrored into the `functions` bucket
    /// keyed by its untyped signature so dispatch finds it — supports `LET f = (FN ...)`
    /// where the bound name doubles as a callable verb.
    ///
    /// `Conflict` means borrow contention (caller queues); `Err` is semantic rejection.
    pub fn try_bind_value(
        &self,
        name: &str,
        obj: &'a KObject<'a>,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply(name, obj, obj.as_function(), true)
    }

    /// Bare-`FN` overload registration. Adds `fn_ref` to the `functions` bucket keyed by
    /// its untyped signature *only* — it does **not** mirror `obj` into `data[name]`, so a
    /// bare FN keyword is dispatchable but not nameable as a value (use `LET f = (FN …)`
    /// for that). Errors:
    /// - `DuplicateOverload` if the bucket already holds an exact-signature equal function.
    ///
    /// `obj` is unused on the write side today (no `data` insert) but kept in the signature
    /// so the call site, which has a `&KObject` carrier in hand, stays uniform with
    /// [`Bindings::try_bind_value`].
    pub fn try_register_function(
        &self,
        name: &str,
        fn_ref: &'a KFunction<'a>,
        obj: &'a KObject<'a>,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply(name, obj, Some(fn_ref), false)
    }

    /// Register `name` → `kt` in the type-binding map. Errors `Rebind` if
    /// `types[name]` already exists; returns `Ok(Conflict)` on borrow contention
    /// (caller queues — same shape as [`Bindings::try_bind_value`] and
    /// [`Bindings::try_register_function`]). Best-effort placeholder clear on success.
    pub fn try_register_type(
        &self,
        name: &str,
        kt: &'a KType<'a>,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply_type(name, kt)
    }

    /// Transactional dual-write for nominal declarations (STRUCT / UNION / MODULE):
    /// inserts identity `kt` into `types[name]` and runtime carrier `obj` into
    /// `data[name]` atomically. Borrow order is `types → data` (the `functions` map
    /// is untouched — nominal carriers are not callable verbs).
    ///
    /// Contract:
    /// - Returns `Ok(Conflict)` if either `types` or `data` is borrowed elsewhere,
    ///   with no write attempted.
    /// - *Cycle-close-idempotent* path: if `types[name]` is already populated with
    ///   a `KType` value-equal to the new `kt` AND `data[name]` is empty, write
    ///   only the carrier. SCC pre-registration installs each cycle member's
    ///   identity into `types` synchronously before any member's body builds its
    ///   carrier, so the eventual `register_nominal` call hits this arm with
    ///   matching identity.
    /// - Returns `Err(Rebind)` if `data[name]` already exists OR `types[name]`
    ///   exists with a *different* `KType`. The pre-check runs before any insert,
    ///   so a collision leaves both maps untouched.
    /// - On success inserts into both maps (or just `data` on the idempotent arm),
    ///   then best-effort clears any matching `placeholders[name]`.
    pub fn try_register_nominal(
        &self,
        name: &str,
        kt: &'a KType<'a>,
        obj: &'a KObject<'a>,
    ) -> Result<ApplyOutcome, KError> {
        let mut types = match self.types.try_borrow_mut() {
            Ok(t) => t,
            Err(_) => return Ok(ApplyOutcome::Conflict),
        };
        let mut data = match self.data.try_borrow_mut() {
            Ok(d) => d,
            Err(_) => {
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
                // identity. Carrier-write below completes the pair.
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
    /// Panics on borrow conflict (unlike [`Bindings::try_bind_value`] /
    /// [`Bindings::try_register_function`]): placeholder installs happen at
    /// dispatch-time outside the re-entrant-bind hot path, so a conflict here
    /// indicates a programming error.
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

    /// Replay another `Bindings`'s `data` through `try_apply` on self. Snapshots the
    /// source `data` into a `Vec` and releases `src`'s `Ref` before the replay so
    /// re-entrant ascription cannot deadlock. Routing through `try_apply` re-mirrors
    /// `KFunction` entries into `functions` exactly once, so the caller does not need
    /// to walk `src.functions` separately.
    ///
    /// Order-independent: the dispatch bucket is order-insensitive once dedupe is
    /// applied. Panics on `Conflict` — a fresh `Bindings` should never hit a borrow
    /// conflict against itself.
    pub fn try_bulk_install_from(&self, src: &Bindings<'a>) -> Result<(), KError> {
        let snapshot: Vec<(String, &'a KObject<'a>)> = src
            .data()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        for (name, obj) in snapshot {
            match self.try_apply(&name, obj, obj.as_function(), true)? {
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

    /// Shared write path for type-only bindings. Borrows `types` only.
    /// [`Bindings::try_register_nominal`] inlines an analogous `types → data`
    /// pre-check + insert rather than reusing this helper because it adds the
    /// second-map dependency to the transaction.
    ///
    /// `Conflict` is reserved for borrow contention; `Err(Rebind)` is the
    /// semantic-rejection path. On success, best-effort clears any matching
    /// placeholder.
    fn try_apply_type(
        &self,
        name: &str,
        kt: &'a KType<'a>,
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

    /// Shared write path for `data`/`functions`. Borrows `functions` first (only
    /// when `fn_part.is_some()`), then `data` — skipping the `functions` borrow
    /// otherwise keeps non-fn binds deadlock-free under callers that hold a live
    /// outer `functions` borrow. `Conflict` is reserved for borrow contention;
    /// semantic errors come through `Err(KError)`.
    ///
    /// `write_data` selects between the value-carrying paths (LET value, LET-binds-FN
    /// capture: `true`) and the bare-`FN` dispatch-only path (`false`). When `false`,
    /// only the `functions` bucket is touched — no `data` borrow, no rebind pre-check,
    /// no insert — so a bare FN keyword never lands as a value binding. The
    /// `(fn_part, write_data)` matrix that actually occurs: `(None, true)` plain LET
    /// value, `(Some, true)` LET-fn capture, `(Some, false)` bare FN. `(None, false)`
    /// never occurs (only `try_register_function` passes `false`, and it always has a
    /// `fn_part`).
    ///
    /// Unified dedupe: when `fn_part.is_some()`, walk the bucket — `ptr::eq` is
    /// silent-success short-circuit (preserves intentional aliases like `LET g = (f)`),
    /// `exact_equal` raises `DuplicateOverload`. Both `FN`-decl and `LET`-binds-`FN`
    /// paths see both rules.
    fn try_apply(
        &self,
        name: &str,
        obj: &'a KObject<'a>,
        fn_part: Option<&'a KFunction<'a>>,
        write_data: bool,
    ) -> Result<ApplyOutcome, KError> {
        let mut functions_handle = if fn_part.is_some() {
            match self.functions.try_borrow_mut() {
                Ok(g) => Some(g),
                Err(_) => return Ok(ApplyOutcome::Conflict),
            }
        } else {
            None
        };
        // Bare FN: skip the `data` borrow, pre-check, and insert entirely — the
        // dispatch surface lives in `functions` only.
        let mut data = if write_data {
            match self.data.try_borrow_mut() {
                Ok(d) => Some(d),
                Err(_) => return Ok(ApplyOutcome::Conflict),
            }
        } else {
            None
        };
        // `fn_part.is_some()` + existing `KFunction` falls through to bucket dedupe
        // (overload-add path); everything else is a rebind error.
        if let Some(data) = data.as_ref() {
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
        if let Some(data) = data.as_mut() {
            data.insert(name.to_string(), obj);
        }
        drop(data);
        drop(functions_handle);
        self.clear_placeholder_best_effort(name);
        Ok(ApplyOutcome::Applied)
    }

    /// Shared tail of every successful write path. `try_borrow_mut().ok()` tolerates
    /// a caller holding a placeholder borrow up the stack — promoting to
    /// `borrow_mut()` would panic for callers that legitimately read placeholders
    /// across a write.
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

/// `Conflict` is the queueable borrow-contention signal; semantic errors come
/// through `Err(KError)`. Not re-exported beyond `core::`.
pub enum ApplyOutcome {
    Applied,
    Conflict,
}

#[cfg(test)]
mod tests;
