//! Lexical binding façade: co-mutating `RefCell` maps (`types`, `data`,
//! `functions`, `placeholders`, `pending_overloads`) behind validated write
//! paths that keep the function-mirror invariant — every `data[name]` wrapping
//! a `KFunction` lives in `functions[signature.untyped_key()]`. Nominal
//! declarations (STRUCT / UNION / MODULE) install identity into `types`
//! alongside the carrier in `data` atomically.
//!
//! Borrow discipline across the maps: `types → functions → data`.
//!
//! Every entry is tagged with a [`BindingIndex`] naming its installing statement's
//! lexical position, gated by the strict cutoff `idx < c`, so a forward reference (a
//! later-positioned binding) is invisible — type binders included. `idx == 0` is the
//! first position: FN parameters and MATCH/TRY `it` sit there, and the builtins are
//! registered there in the immutable run-global root. The builtins stay reachable
//! because that root is off the lexical chain (its cutoff is `None`, so every entry in
//! it is visible) and is consulted in one hop through each scope's direct root
//! reference — not through an `idx == 0`-always-visible carve-out. The `idx == 0` tag
//! is what [`Bindings::has_builtin_type`] / [`Bindings::has_builtin_function`] /
//! [`Bindings::has_builtin_operator`] read to mark a genuine builtin for the no-shadow
//! and root-first consults.
//!
//! Production reads use the visibility-aware [`Bindings::lookup_value`] /
//! [`Bindings::lookup_type`] / [`Bindings::lookup_function`], passing a
//! `chain_cutoff` computed via [`crate::machine::core::LexicalFrame::index_for`].
//! Raw map accessors are `#[cfg(test)]`.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;

use crate::machine::core::kfunction::{KFunction, NodeId};
use crate::machine::model::ast::TypeIdentifier;
use crate::machine::model::operators::OperatorGroup;
use crate::machine::model::types::{KType, UntypedKey};
use crate::machine::model::values::KObject;

use super::kerror::{KError, KErrorKind};

mod pending;
pub use pending::{PendingBinderGuard, PendingTypeEntry, PendingTypes};

/// Outcome of a value-side name lookup. `Resolution::Placeholder` carries the
/// producer `NodeId` the consumer should park on.
///
/// Invariant: within one scope, `data` and `placeholders` never both hold the
/// same name — every successful write path clears any matching placeholder.
pub enum Resolution<'a> {
    Value(&'a KObject<'a>),
    Placeholder(NodeId),
    UnboundName,
}

/// Outcome of a per-scope `lookup_function` call. Visibility (per
/// `chain_cutoff`) is applied inside the lookup; `overloads` holds only
/// visible finalized overloads (may be empty) and `pending` the earliest-index
/// visible in-flight producer (if any). Both are surfaced together so the
/// scope walk can decide pending-vs-finalized precedence at the scope that
/// raised them — a bucket may hold a finalized overload AND an in-flight
/// pending sibling at once. A no-hit lookup is `overloads.is_empty() &&
/// pending.is_none()`.
///
/// `pending` names a visible `pending_overloads` entry — a sibling FN/FUNCTOR
/// binder has dispatched a matching overload whose body hasn't finalized. The
/// consumer parks on the earliest-index visible producer; on wake it
/// re-dispatches and either picks from the now-live bucket or re-parks on the
/// next-earliest pending sibling.
pub struct FunctionLookup<'a> {
    pub overloads: Vec<&'a KFunction<'a>>,
    pub pending: Option<NodeId>,
}

/// Lexical position of a binding's installing statement: a binding at `idx` is visible to a
/// consumer at cutoff `c` iff `idx < c`. Every binder — value and type alike — gates its
/// references against its own position, so a forward reference is a position error and
/// mutual recursion is expressed with a `RECURSIVE TYPES` block. `idx == 0` is the first
/// position (FN parameters, MATCH/TRY `it`) and also tags the builtins in the immutable
/// root — [`BindingIndex::BUILTIN`]; per-block indices restart inside nested blocks (see
/// [`crate::machine::core::scope::Scope::resolve`] for the predicate).
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct BindingIndex {
    pub idx: usize,
}

impl BindingIndex {
    pub const BUILTIN: BindingIndex = BindingIndex { idx: 0 };

    /// A binding at lexical position `idx`. FN / STRUCT / etc. all install here; FN
    /// *parameters* and MATCH / TRY `it` sit at `idx 0`, with the body's statements at
    /// `idx >= 1`, so the strict `idx < cutoff` predicate admits them.
    pub const fn value(idx: usize) -> Self {
        BindingIndex { idx }
    }
}

/// Co-mutating `RefCell` maps backing every lexical binding. `placeholders`
/// and `pending_overloads` are intentionally separate: the former is consulted
/// by name (value/type forward references); the latter by full dispatch bucket
/// key (a bare-arg call whose FN/FUNCTOR overload is still finalizing). Keying
/// dispatch parks by the full bucket key keeps `(MAKESET _)` and
/// `(MAKESET _ USING _)` from colliding.
///
/// Borrow discipline: `types → functions → data`. Lifetime `'a` is the region
/// lifetime of the stored references.
pub struct Bindings<'a> {
    types: RefCell<HashMap<String, (&'a KType<'a>, BindingIndex)>>,
    data: RefCell<HashMap<String, (&'a KObject<'a>, BindingIndex)>>,
    functions: RefCell<HashMap<UntypedKey, Vec<(&'a KFunction<'a>, BindingIndex)>>>,
    placeholders: RefCell<HashMap<String, (NodeId, BindingIndex)>>,
    /// Bucket-key → entries for FN / FUNCTOR overloads whose binder has
    /// dispatched but not finalized. Sibling binders sharing one inner-call
    /// bucket key each install their own entry; consumers park on the
    /// earliest-index visible one. On finalize only that entry is removed;
    /// other siblings remain as wake sources.
    pending_overloads: RefCell<HashMap<UntypedKey, Vec<(NodeId, BindingIndex)>>>,
    /// Per-scope operator registry: a chain's sorted-joined operator probe key →
    /// the shared [`OperatorGroup`] it resolves to. A module installs one record per
    /// size-≥2 subset of its declared operators (the per-group powerset), each subset
    /// key pointing at the same region-allocated group, so any subset used in one
    /// expression resolves in a single hit and a cross-group mix simply misses.
    /// Walked through the scope chain like every other name (innermost visible wins).
    operators: RefCell<HashMap<String, (&'a OperatorGroup, BindingIndex)>>,
    /// In-flight named-type binders (STRUCT / named-UNION). A consumer referencing an
    /// earlier still-finalizing type parks on its producer node; this map marks which names
    /// are in flight. See [`pending`] for the surface methods.
    pending: PendingTypes<'a>,
    /// Scope-bound `TypeIdentifier` → `&KType` resolution cache. Monotonic — entries are written
    /// only when the elaborated `KType` and every user-type it references are fully
    /// finalized; the finalize gate prevents caching a not-yet-sealed type.
    /// Keyed by `(TypeIdentifier, chain cutoff)`: a forward consumer (smaller cutoff) and a
    /// backward consumer (larger cutoff) at the same scope resolve the same name to
    /// different verdicts under lexical gating, so they must not share a cache entry.
    type_identifier_memo: RefCell<HashMap<(TypeIdentifier, Option<usize>), &'a KType<'a>>>,
}

impl<'a> Bindings<'a> {
    pub fn new() -> Self {
        Self {
            types: RefCell::new(HashMap::new()),
            data: RefCell::new(HashMap::new()),
            functions: RefCell::new(HashMap::new()),
            placeholders: RefCell::new(HashMap::new()),
            pending_overloads: RefCell::new(HashMap::new()),
            operators: RefCell::new(HashMap::new()),
            pending: PendingTypes::new(),
            type_identifier_memo: RefCell::new(HashMap::new()),
        }
    }

    pub fn type_identifier_memo_get(
        &self,
        te: &TypeIdentifier,
        cutoff: Option<usize>,
    ) -> Option<&'a KType<'a>> {
        self.type_identifier_memo
            .borrow()
            .get(&(te.clone(), cutoff))
            .copied()
    }

    /// Per-scope value-side lookup. Consults `data` then `placeholders`,
    /// returning the first visible hit. `chain_cutoff = None` means the scope
    /// is off-chain (or unfiltered) — everything is visible. `None` return
    /// means no visible entry at this scope; the caller keeps walking
    /// ancestors and surfaces `UnboundName` on chain exhaustion.
    pub fn lookup_value(&self, name: &str, chain_cutoff: Option<usize>) -> Option<Resolution<'a>> {
        if let Some((obj, idx)) = self.data.borrow().get(name).copied() {
            if Self::visible(idx, chain_cutoff) {
                return Some(Resolution::Value(obj));
            }
        }
        if let Some((id, idx)) = self.placeholders.borrow().get(name).copied() {
            if Self::visible(idx, chain_cutoff) {
                return Some(Resolution::Placeholder(id));
            }
        }
        None
    }

    /// Per-scope type-side lookup. Mirrors [`Self::lookup_value`] for the
    /// `types` map.
    pub fn lookup_type(&self, name: &str, chain_cutoff: Option<usize>) -> Option<&'a KType<'a>> {
        let types = self.types.borrow();
        let (kt, idx) = types.get(name).copied()?;
        if Self::visible(idx, chain_cutoff) {
            Some(kt)
        } else {
            None
        }
    }

    /// Per-scope dispatch-bucket lookup. Surfaces visible finalized overloads
    /// (`functions[key]`, filtered per-overload) AND the earliest-index visible
    /// `pending_overloads[key]` producer together — one pass over each map. The
    /// scope walk decides pending-vs-finalized precedence with both in hand.
    pub fn lookup_function(
        &self,
        key: &UntypedKey,
        chain_cutoff: Option<usize>,
    ) -> FunctionLookup<'a> {
        let overloads: Vec<&'a KFunction<'a>> = self
            .functions
            .borrow()
            .get(key)
            .map(|bucket| {
                bucket
                    .iter()
                    .filter(|(_, idx)| Self::visible(*idx, chain_cutoff))
                    .map(|(f, _)| *f)
                    .collect()
            })
            .unwrap_or_default();
        // Earliest-index visible producer: most likely to finalize first; on
        // wake the consumer re-dispatches and picks the live bucket or re-parks
        // on the next-earliest sibling.
        let pending = self
            .pending_overloads
            .borrow()
            .get(key)
            .and_then(|entries| {
                entries
                    .iter()
                    .filter(|(_, idx)| Self::visible(*idx, chain_cutoff))
                    .min_by_key(|(_, idx)| idx.idx)
                    .map(|(producer, _)| *producer)
            });
        FunctionLookup { overloads, pending }
    }

    /// Per-scope operator-group lookup. Mirrors [`Self::lookup_value`] for the
    /// `operators` map: returns the visible group registered under `probe` (the
    /// sorted-joined unique operators of a chain), or `None` at this scope so the
    /// caller keeps walking ancestors.
    pub fn lookup_operator_group(
        &self,
        probe: &str,
        chain_cutoff: Option<usize>,
    ) -> Option<&'a OperatorGroup> {
        let operators = self.operators.borrow();
        let (group, idx) = operators.get(probe).copied()?;
        if Self::visible(idx, chain_cutoff) {
            Some(group)
        } else {
            None
        }
    }

    /// Register `probe → group` in the operator registry. The `OP` binder installs
    /// one entry per size-≥2 subset of the declared operators (all pointing at the
    /// same `group`); test fixtures register the subsets they exercise. Idempotent on
    /// a pointer-equal re-register; a different group under the same key is a
    /// programming error (`Rebind`).
    pub fn try_register_operator_group(
        &self,
        probe: String,
        group: &'a OperatorGroup,
        index: BindingIndex,
    ) -> Result<ApplyOutcome, KError> {
        let mut operators = match self.operators.try_borrow_mut() {
            Ok(o) => o,
            Err(_) => return Ok(ApplyOutcome::Conflict),
        };
        if let Some((existing, _)) = operators.get(&probe).copied() {
            if std::ptr::eq(existing, group) {
                return Ok(ApplyOutcome::Applied);
            }
            return Err(KError::new(KErrorKind::Rebind { name: probe }));
        }
        operators.insert(probe, (group, index));
        Ok(ApplyOutcome::Applied)
    }

    /// Snapshot every `(name, value)` pair in `data`, ignoring visibility.
    /// For chain-gated single-name reads use [`Self::lookup_value`].
    pub fn iter_data(&self) -> Vec<(String, &'a KObject<'a>)> {
        self.data
            .borrow()
            .iter()
            .map(|(name, (obj, _))| (name.clone(), *obj))
            .collect()
    }

    /// Snapshot every `(name, &KType)` pair in `types`, ignoring visibility.
    pub fn iter_types(&self) -> Vec<(String, &'a KType<'a>)> {
        self.types
            .borrow()
            .iter()
            .map(|(name, (kt, _))| (name.clone(), *kt))
            .collect()
    }

    /// Snapshot every `(UntypedKey, Vec<&KFunction>)` pair in `functions`,
    /// ignoring per-overload visibility. For chain-gated picks use
    /// [`Self::lookup_function`].
    pub fn iter_functions(&self) -> Vec<(UntypedKey, Vec<&'a KFunction<'a>>)> {
        self.functions
            .borrow()
            .iter()
            .map(|(key, bucket)| (key.clone(), bucket.iter().map(|(f, _)| *f).collect()))
            .collect()
    }

    /// True iff `types[name]` was registered at [`BindingIndex::BUILTIN`]. The
    /// no-shadow consult gates on this — a genuine builtin, not a user type that a
    /// synthetic test happens to have placed in a root-position scope.
    pub fn has_builtin_type(&self, name: &str) -> bool {
        self.types
            .borrow()
            .get(name)
            .is_some_and(|(_, idx)| *idx == BindingIndex::BUILTIN)
    }

    /// True iff `functions[key]` holds an overload registered at
    /// [`BindingIndex::BUILTIN`] — a genuine builtin dispatch bucket, distinct from a
    /// user bucket the no-shadow consult must not gate.
    pub fn has_builtin_function(&self, key: &UntypedKey) -> bool {
        self.functions
            .borrow()
            .get(key)
            .is_some_and(|bucket| bucket.iter().any(|(_, idx)| *idx == BindingIndex::BUILTIN))
    }

    /// True iff `operators[probe]` was registered at [`BindingIndex::BUILTIN`].
    pub fn has_builtin_operator(&self, probe: &str) -> bool {
        self.operators
            .borrow()
            .get(probe)
            .is_some_and(|(_, idx)| *idx == BindingIndex::BUILTIN)
    }

    /// Visibility predicate: `None` ⇒ everything visible; `Some(c)` ⇒ `b.idx < c`.
    /// Mirrors [`crate::machine::core::scope::visible`].
    fn visible(b: BindingIndex, chain_cutoff: Option<usize>) -> bool {
        match chain_cutoff {
            None => true,
            Some(c) => b.idx < c,
        }
    }

    /// Insert `(te → kt)` into the resolution cache. Caller region-allocates
    /// `kt` and gates on finalize. Monotonic: a collision means equal values,
    /// so we keep the existing entry rather than panic.
    pub fn type_identifier_memo_insert(
        &self,
        te: TypeIdentifier,
        cutoff: Option<usize>,
        kt: &'a KType<'a>,
    ) {
        let mut memo = self.type_identifier_memo.borrow_mut();
        memo.entry((te, cutoff)).or_insert(kt);
    }

    #[cfg(test)]
    pub fn data(&self) -> Ref<'_, HashMap<String, (&'a KObject<'a>, BindingIndex)>> {
        self.data.borrow()
    }

    #[cfg(test)]
    pub fn functions(
        &self,
    ) -> Ref<'_, HashMap<UntypedKey, Vec<(&'a KFunction<'a>, BindingIndex)>>> {
        self.functions.borrow()
    }

    #[cfg(test)]
    pub fn placeholders(&self) -> Ref<'_, HashMap<String, (NodeId, BindingIndex)>> {
        self.placeholders.borrow()
    }

    #[cfg(test)]
    pub fn pending_overloads(&self) -> Ref<'_, HashMap<UntypedKey, Vec<(NodeId, BindingIndex)>>> {
        self.pending_overloads.borrow()
    }

    #[cfg(test)]
    pub fn types(&self) -> Ref<'_, HashMap<String, (&'a KType<'a>, BindingIndex)>> {
        self.types.borrow()
    }

    #[cfg(test)]
    pub fn expect_value(&self, name: &str) -> &'a KObject<'a> {
        self.data
            .borrow()
            .get(name)
            .map(|(obj, _)| *obj)
            .unwrap_or_else(|| panic!("expected bindings.data[{name:?}] to be present"))
    }

    #[cfg(test)]
    pub fn expect_type(&self, name: &str) -> &'a KType<'a> {
        self.types
            .borrow()
            .get(name)
            .map(|(kt, _)| *kt)
            .unwrap_or_else(|| panic!("expected bindings.types[{name:?}] to be present"))
    }

    /// In-flight named-type binder map. The sole non-test writer is
    /// [`Bindings::insert_pending_type`] (the guard's Drop removes the entry); a consumer
    /// reads it to decide whether to park on an earlier still-finalizing type.
    pub fn pending_types(&self) -> Ref<'_, HashMap<String, PendingTypeEntry<'a>>> {
        self.pending.get()
    }

    pub fn insert_pending_type(
        &self,
        name: String,
        entry: PendingTypeEntry<'a>,
    ) -> PendingBinderGuard<'a> {
        self.pending.insert(name, entry)
    }

    /// Exercises the guard Drop's "tolerates absent entry" path.
    #[cfg(test)]
    pub fn pending_remove(&self, name: &str) {
        self.pending.remove(name);
    }

    /// LET-style value bind. Errors `Rebind` if `data[name]` already exists.
    /// When `obj` wraps a `KFunction` it is also mirrored into
    /// `functions[signature.untyped_key()]` so dispatch finds it (`LET f = (FN ...)`).
    ///
    /// `Conflict` means borrow contention (caller queues); `Err` is semantic rejection.
    pub fn try_bind_value(
        &self,
        name: &str,
        obj: &'a KObject<'a>,
        index: BindingIndex,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply(name, obj, obj.as_function(), true, index)
    }

    /// Bare-`FN` overload registration: adds `fn_ref` to the `functions`
    /// bucket only — `data[name]` is left untouched, so a bare FN keyword is
    /// dispatchable but not nameable as a value (use `LET f = (FN …)` for that).
    /// Errors `DuplicateOverload` on an exact-signature collision.
    ///
    /// Per-overload `index` tagging matters because overloads sharing a bucket
    /// can sit at different lexical positions (the dispatch picker filters
    /// per-overload). `obj` is unused on the write side but keeps the call
    /// site uniform with [`Bindings::try_bind_value`].
    pub fn try_register_function(
        &self,
        name: &str,
        fn_ref: &'a KFunction<'a>,
        obj: &'a KObject<'a>,
        index: BindingIndex,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply(name, obj, Some(fn_ref), false, index)
    }

    /// Register `name` → `kt` in `types`. Errors `Rebind` if already present;
    /// `Ok(Conflict)` on borrow contention. Best-effort placeholder clear on
    /// success.
    pub fn try_register_type(
        &self,
        name: &str,
        kt: &'a KType<'a>,
        index: BindingIndex,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply_type(name, kt, index)
    }

    /// Upsert `name` → `kt` in `types` for nominal finalize. Insert-if-absent;
    /// on a `PartialEq`-equal existing entry **overwrite** the stored `&KType` (and
    /// `index`) so the `SetRef` an SCC seal pre-installed (same set + index) is rewritten
    /// in place. A non-equal existing entry is a genuine collision — `Err(Rebind)`.
    ///
    /// Distinct from [`Self::try_register_type`], whose strict insert-if-absent arm
    /// would `Rebind` on the seal pre-install rather than overwrite it.
    /// `Ok(Conflict)` on borrow contention. Best-effort placeholder clear on success.
    pub fn try_register_type_upsert(
        &self,
        name: &str,
        kt: &'a KType<'a>,
        index: BindingIndex,
    ) -> Result<ApplyOutcome, KError> {
        let mut types = match self.types.try_borrow_mut() {
            Ok(t) => t,
            Err(_) => return Ok(ApplyOutcome::Conflict),
        };
        match types.get(name).map(|(t, _)| *t) {
            Some(existing) if *existing != *kt => {
                return Err(KError::new(KErrorKind::Rebind {
                    name: name.to_string(),
                }));
            }
            // Absent, or identity-equal (the seal's pre-installed `SetRef`): write the
            // identity, rewriting any pre-install in place.
            _ => {
                types.insert(name.to_string(), (kt, index));
            }
        }
        drop(types);
        self.clear_placeholder_best_effort(name);
        Ok(ApplyOutcome::Applied)
    }

    /// Install a dispatch-time placeholder for `name` → producer slot `idx`.
    ///
    /// Lenient when `data[name]` already holds a `KObject::KFunction`: silent
    /// no-op (a new FN overload joins the existing bucket on finalize without
    /// consumers needing to park). Errors `Rebind` if `data[name]` holds a
    /// non-function or if `placeholders[name]` maps to a different `NodeId`;
    /// idempotent on same-`NodeId` re-entry.
    ///
    /// The eventual `try_bind_value` / `try_register_*` call must carry the
    /// same `index` so the consumer's visibility test stays consistent across
    /// the placeholder → finalized transition.
    pub fn try_install_placeholder(
        &self,
        name: String,
        idx: NodeId,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if let Some((existing, _)) = self.data.borrow().get(&name).copied() {
            if matches!(existing, KObject::KFunction(_)) {
                return Ok(());
            }
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        let mut ph = self.placeholders.borrow_mut();
        if let Some((existing, _)) = ph.get(&name).copied() {
            if existing == idx {
                return Ok(());
            }
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        ph.insert(name, (idx, index));
        Ok(())
    }

    /// Install a dispatch-time pending-overload entry: `bucket → producer`.
    /// The bucket key MUST equal what `KExpression::untyped_key` would compute
    /// for a *call* to the eventual overload (not the binder call itself).
    ///
    /// **Append, never deduplicate**: sibling FN/FUNCTOR binders sharing one
    /// inner-call bucket key — `FN (PICK xs :A) -> ...` then
    /// `FN (PICK xs :B) -> ...` — each install their own entry at their own
    /// [`BindingIndex`]. The entry is removed in [`Bindings::try_apply`] when
    /// the producing binder lands in `functions[bucket]`; other siblings stay
    /// pending as wake sources.
    ///
    /// Recorded even when the bucket is already live in `functions`: a pending
    /// sibling sits *alongside* a finalized overload so the scope walk can park
    /// the bucket until the sibling finalizes (Decision 5).
    pub fn try_install_pending_overload(
        &self,
        bucket: UntypedKey,
        idx: NodeId,
        index: BindingIndex,
    ) -> Result<(), KError> {
        let mut pending = self.pending_overloads.borrow_mut();
        pending.entry(bucket).or_default().push((idx, index));
        Ok(())
    }

    /// Replay another `Bindings`'s `data` through `try_apply` on self.
    /// Snapshots `src.data` and releases the source `Ref` before the replay so
    /// re-entrant ascription cannot deadlock. Routing through `try_apply`
    /// re-mirrors `KFunction` entries into `functions`, so callers do not walk
    /// `src.functions` separately. Panics on `Conflict` — a fresh `Bindings`
    /// should never hit a borrow conflict against itself.
    pub fn try_bulk_install_from(&self, src: &Bindings<'a>) -> Result<(), KError> {
        let snapshot: Vec<(String, &'a KObject<'a>, BindingIndex)> = src
            .data
            .borrow()
            .iter()
            .map(|(k, (v, idx))| (k.clone(), *v, *idx))
            .collect();
        for (name, obj, index) in snapshot {
            match self.try_apply(&name, obj, obj.as_function(), true, index)? {
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

    /// Shared write path for type-only bindings.
    /// `Conflict` is borrow contention; `Err(Rebind)` is semantic rejection.
    fn try_apply_type(
        &self,
        name: &str,
        kt: &'a KType<'a>,
        index: BindingIndex,
    ) -> Result<ApplyOutcome, KError> {
        let mut types = match self.types.try_borrow_mut() {
            Ok(t) => t,
            Err(_) => return Ok(ApplyOutcome::Conflict),
        };
        if types.contains_key(name) {
            return Err(KError::new(KErrorKind::Rebind {
                name: name.to_string(),
            }));
        }
        types.insert(name.to_string(), (kt, index));
        drop(types);
        self.clear_placeholder_best_effort(name);
        Ok(ApplyOutcome::Applied)
    }

    /// Shared write path for `data`/`functions`. Borrows `functions` first
    /// (only when `fn_part.is_some()`), then `data` — skipping the `functions`
    /// borrow otherwise keeps non-fn binds deadlock-free under callers that
    /// hold a live outer `functions` borrow.
    ///
    /// `write_data`: `true` for value-carrying paths (LET, LET-binds-FN);
    /// `false` for bare-`FN` (dispatch-only, no `data` insert). The only
    /// combinations that occur are `(None, true)`, `(Some, true)`, `(Some, false)`.
    ///
    /// Dedupe when `fn_part.is_some()`: `ptr::eq` is a silent-success
    /// short-circuit (preserves intentional aliases like `LET g = (f)`);
    /// `exact_equal` raises `DuplicateOverload`.
    fn try_apply(
        &self,
        name: &str,
        obj: &'a KObject<'a>,
        fn_part: Option<&'a KFunction<'a>>,
        write_data: bool,
        index: BindingIndex,
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
            if let Some((existing, _)) = data.get(name) {
                match fn_part {
                    None => {
                        return Err(KError::new(KErrorKind::Rebind {
                            name: name.to_string(),
                        }))
                    }
                    Some(_) => {
                        if !matches!(existing, KObject::KFunction(_)) {
                            return Err(KError::new(KErrorKind::Rebind {
                                name: name.to_string(),
                            }));
                        }
                    }
                }
            }
        }
        let mut cleared_overload_bucket: Option<UntypedKey> = None;
        if let (Some(f_ref), Some(functions)) = (fn_part, functions_handle.as_mut()) {
            let key = f_ref.signature.untyped_key();
            let bucket = functions.entry(key.clone()).or_default();
            let mut already_present = false;
            for (existing, _) in bucket.iter() {
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
                bucket.push((f_ref, index));
            }
            cleared_overload_bucket = Some(key);
        }
        if let Some(data) = data.as_mut() {
            data.insert(name.to_string(), (obj, index));
        }
        drop(data);
        drop(functions_handle);
        self.clear_placeholder_best_effort(name);
        if let Some(bucket) = cleared_overload_bucket {
            // Remove only this binder's pending entry; siblings stay as wake sources.
            self.clear_pending_overload_best_effort(&bucket, index);
        }
        Ok(ApplyOutcome::Applied)
    }

    /// Shared tail of every successful write path. `try_borrow_mut().ok()`
    /// tolerates a caller holding a placeholder borrow up the stack — a
    /// hard `borrow_mut()` would panic on legitimate reads across a write.
    fn clear_placeholder_best_effort(&self, name: &str) {
        if let Ok(mut ph) = self.placeholders.try_borrow_mut() {
            ph.remove(name);
        }
    }

    /// Remove every value-side placeholder pointing at `producer`. The success write
    /// paths clear a binder's placeholder by name on finalize; this is the error-path
    /// companion, called when `producer`'s node finalizes with an error so a binder body
    /// that failed before its write path does not leak a scheduler-local [`NodeId`] into
    /// a later run on a persistent scope. Same tolerant `try_borrow_mut`.
    pub fn clear_placeholders_for_producer(&self, producer: NodeId) {
        if let Ok(mut ph) = self.placeholders.try_borrow_mut() {
            ph.retain(|_, (id, _)| *id != producer);
        }
    }

    /// Bucket-keyed companion to [`Self::clear_placeholder_best_effort`].
    /// Removes only the entry whose `BindingIndex` matches — sibling binders
    /// stay as wake sources. Empties drop the map entry. Same tolerant
    /// `try_borrow_mut` pattern.
    fn clear_pending_overload_best_effort(&self, bucket: &UntypedKey, index: BindingIndex) {
        if let Ok(mut p) = self.pending_overloads.try_borrow_mut() {
            if let Some(entries) = p.get_mut(bucket) {
                entries.retain(|(_, idx)| *idx != index);
                if entries.is_empty() {
                    p.remove(bucket);
                }
            }
        }
    }
}

impl<'a> Default for Bindings<'a> {
    fn default() -> Self {
        Self::new()
    }
}

/// `Conflict` is the queueable borrow-contention signal; semantic errors come
/// through `Err(KError)`.
pub enum ApplyOutcome {
    Applied,
    Conflict,
}

#[cfg(test)]
mod tests;
