//! Lexical binding façade: co-mutating `RefCell` maps (`types`, `data`,
//! `functions`, `placeholders`, `pending_overloads`) behind validated write
//! paths that keep the function-mirror invariant — every `data[name]` wrapping
//! a `KFunction` lives in `functions[signature.untyped_key()]`. Nominal
//! declarations (STRUCT / UNION / MODULE) install identity into `types`
//! alongside the carrier in `data` atomically.
//!
//! Borrow discipline across the maps: `types → functions → data`.
//!
//! Every entry is tagged with a [`BindingIndex`]. `idx == 0` is reserved for
//! builtins. `nominal_binder: true` (STRUCT / UNION / SIG / FUNCTOR / MODULE)
//! lets siblings on the same block see one another's nominal identities
//! regardless of source order (mutual recursion).
//!
//! Production reads use the visibility-aware [`Bindings::lookup_value`] /
//! [`Bindings::lookup_type`] / [`Bindings::lookup_function`], passing a
//! `chain_cutoff` computed via [`crate::machine::core::LexicalFrame::index_for`].
//! Raw map accessors are `#[cfg(test)]`.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;

use crate::machine::model::ast::TypeExpr;
use crate::machine::core::kfunction::{KFunction, NodeId};
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
/// `chain_cutoff`) is applied inside the lookup; `Bucket` is non-empty.
pub enum FunctionLookup<'a> {
    Bucket(Vec<&'a KFunction<'a>>),
    /// No live bucket but a visible `pending_overloads` entry — a sibling
    /// FN/FUNCTOR binder has dispatched a matching overload whose body hasn't
    /// finalized. The consumer parks on the earliest-index visible producer;
    /// on wake it re-dispatches and either picks from the now-live bucket or
    /// re-parks on the next-earliest pending sibling.
    Pending(NodeId),
    None,
}

/// Lexical position of a binding's installing statement.
///
/// `nominal_binder: true` exempts the entry from the strict-lexical cutoff so
/// STRUCT / UNION / SIG / FUNCTOR / MODULE siblings see one another regardless
/// of source order (mutual recursion). `idx == 0` is reserved for builtins;
/// per-block indices restart inside nested blocks (see
/// [`crate::machine::core::scope::Scope::resolve`] for the predicate).
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct BindingIndex {
    pub idx: usize,
    pub nominal_binder: bool,
}

impl BindingIndex {
    pub const BUILTIN: BindingIndex = BindingIndex { idx: 0, nominal_binder: false };

    /// LET, FN body capture, MATCH / TRY `it`, FN parameters: strictly
    /// lexically gated, sees only earlier-positioned siblings.
    pub const fn value(idx: usize) -> Self {
        BindingIndex { idx, nominal_binder: false }
    }

    /// STRUCT, named UNION, SIG, FUNCTOR, MODULE: visible to siblings on the
    /// same block regardless of source order.
    pub const fn nominal(idx: usize) -> Self {
        BindingIndex { idx, nominal_binder: true }
    }
}

/// Co-mutating `RefCell` maps backing every lexical binding. `placeholders`
/// and `pending_overloads` are intentionally separate: the former is consulted
/// by name (value/type forward references); the latter by full dispatch bucket
/// key (a bare-arg call whose FN/FUNCTOR overload is still finalizing). Keying
/// dispatch parks by the full bucket key keeps `(MAKESET _)` and
/// `(MAKESET _ USING _)` from colliding.
///
/// Borrow discipline: `types → functions → data`. Lifetime `'a` is the arena
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
    /// In-flight named-type binders (STRUCT / named-UNION). Consulted by the
    /// elaborator's `Resolution::Placeholder` arm to record dependency edges
    /// and run DFS cycle detection. See [`pending`] for the surface methods.
    pending: PendingTypes<'a>,
    /// Scope-bound `TypeExpr` → `&KType` resolution cache. Monotonic — entries
    /// are written only when the elaborated `KType` and every user-type it
    /// references are fully finalized; the finalize gate prevents caching
    /// mid-SCC pre-close identities.
    type_expr_memo: RefCell<HashMap<TypeExpr, &'a KType<'a>>>,
}

impl<'a> Bindings<'a> {
    pub fn new() -> Self {
        Self {
            types: RefCell::new(HashMap::new()),
            data: RefCell::new(HashMap::new()),
            functions: RefCell::new(HashMap::new()),
            placeholders: RefCell::new(HashMap::new()),
            pending_overloads: RefCell::new(HashMap::new()),
            pending: PendingTypes::new(),
            type_expr_memo: RefCell::new(HashMap::new()),
        }
    }

    pub fn type_expr_memo_get(&self, te: &TypeExpr) -> Option<&'a KType<'a>> {
        self.type_expr_memo.borrow().get(te).copied()
    }

    /// Per-scope value-side lookup. Consults `data` then `placeholders`,
    /// returning the first visible hit. `chain_cutoff = None` means the scope
    /// is off-chain (or unfiltered) — everything is visible. `None` return
    /// means no visible entry at this scope; the caller keeps walking
    /// ancestors and surfaces `UnboundName` on chain exhaustion.
    pub fn lookup_value(
        &self,
        name: &str,
        chain_cutoff: Option<usize>,
    ) -> Option<Resolution<'a>> {
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
    pub fn lookup_type(
        &self,
        name: &str,
        chain_cutoff: Option<usize>,
    ) -> Option<&'a KType<'a>> {
        let types = self.types.borrow();
        let (kt, idx) = types.get(name).copied()?;
        if Self::visible(idx, chain_cutoff) {
            Some(kt)
        } else {
            None
        }
    }

    /// Per-scope dispatch-bucket lookup. Filters `functions[key]` by
    /// per-overload visibility; on empty falls through to a visible
    /// `pending_overloads[key]` entry.
    pub fn lookup_function(
        &self,
        key: &UntypedKey,
        chain_cutoff: Option<usize>,
    ) -> FunctionLookup<'a> {
        let functions = self.functions.borrow();
        if let Some(bucket) = functions.get(key) {
            let visible: Vec<&'a KFunction<'a>> = bucket
                .iter()
                .filter(|(_, idx)| Self::visible(*idx, chain_cutoff))
                .map(|(f, _)| *f)
                .collect();
            if !visible.is_empty() {
                return FunctionLookup::Bucket(visible);
            }
        }
        drop(functions);
        let pending = self.pending_overloads.borrow();
        if let Some(entries) = pending.get(key) {
            // Earliest-index visible producer: most likely to finalize first;
            // on wake the consumer re-dispatches and picks the live bucket or
            // re-parks on the next-earliest sibling.
            let earliest = entries
                .iter()
                .filter(|(_, idx)| Self::visible(*idx, chain_cutoff))
                .min_by_key(|(_, idx)| idx.idx);
            if let Some((producer, _)) = earliest {
                return FunctionLookup::Pending(*producer);
            }
        }
        FunctionLookup::None
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

    /// `BindingIndex` of an installed placeholder, ignoring visibility.
    /// Cycle-close in `model/types/resolver.rs` re-stamps the placeholder's
    /// lexical position so the downstream finalize (`register_type_upsert`, or
    /// `register_nominal` for SIG) installs at the matching index. Other reads
    /// should go through [`Self::lookup_value`].
    pub fn placeholder_index(&self, name: &str) -> Option<BindingIndex> {
        self.placeholders.borrow().get(name).map(|(_, idx)| *idx)
    }

    /// Visibility predicate: `None` ⇒ everything visible; `Some(c)` ⇒
    /// `b.nominal_binder || b.idx < c`. Mirrors
    /// [`crate::machine::core::scope::visible`].
    fn visible(b: BindingIndex, chain_cutoff: Option<usize>) -> bool {
        match chain_cutoff {
            None => true,
            Some(c) => b.nominal_binder || b.idx < c,
        }
    }

    /// Insert `(te → kt)` into the resolution cache. Caller arena-allocates
    /// `kt` and gates on finalize. Monotonic: a collision means equal values,
    /// so we keep the existing entry rather than panic.
    pub fn type_expr_memo_insert(&self, te: TypeExpr, kt: &'a KType<'a>) {
        let mut memo = self.type_expr_memo.borrow_mut();
        memo.entry(te).or_insert(kt);
    }

    #[cfg(test)]
    pub fn data(&self) -> Ref<'_, HashMap<String, (&'a KObject<'a>, BindingIndex)>> {
        self.data.borrow()
    }

    #[cfg(test)]
    pub fn functions(&self) -> Ref<'_, HashMap<UntypedKey, Vec<(&'a KFunction<'a>, BindingIndex)>>> {
        self.functions.borrow()
    }

    #[cfg(test)]
    pub fn placeholders(&self) -> Ref<'_, HashMap<String, (NodeId, BindingIndex)>> {
        self.placeholders.borrow()
    }

    #[cfg(test)]
    pub fn pending_overloads(
        &self,
    ) -> Ref<'_, HashMap<UntypedKey, Vec<(NodeId, BindingIndex)>>> {
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

    /// SCC pre-registration map. Writers: [`Bindings::insert_pending_type`]
    /// (Drop on the guard removes the entry) and
    /// [`Bindings::record_pending_edge`].
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
    /// on a `PartialEq`-equal existing entry **overwrite** the stored `&KType`
    /// (and `index`) so the payload-empty identity an SCC cycle-close pre-installed
    /// is replaced by the schema-bearing one finalize built. A non-equal existing
    /// entry is a genuine collision — `Err(Rebind)`.
    ///
    /// Distinct from [`Self::try_register_type`], whose strict insert-if-absent arm
    /// would `Rebind` on the cycle-close pre-install rather than overwrite it.
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
                return Err(KError::new(KErrorKind::Rebind { name: name.to_string() }));
            }
            // Absent, or identity-equal (cycle-close pre-install): write the
            // schema-bearing identity, replacing any payload-empty pre-install.
            _ => {
                types.insert(name.to_string(), (kt, index));
            }
        }
        drop(types);
        self.clear_placeholder_best_effort(name);
        Ok(ApplyOutcome::Applied)
    }

    /// Atomic `(types, data)` install — the lone remaining caller is a SIG
    /// declaration (and a SIG-alias `LET`): `types[name] = kt` (the
    /// `SatisfiesSignature` constraint) + `data[name] = obj` (the `Signature`
    /// value). Borrow order `types → data`; `functions` is untouched. STRUCT /
    /// UNION / MODULE / Result are type-only now via [`super::Scope::register_type_upsert`];
    /// retiring this path entirely is the `eliminate-sig-dual-write` roadmap item.
    ///
    /// Idempotent path: if `types[name]` already holds a `KType` value-equal to
    /// `kt` AND `data[name]` is empty, write only the carrier.
    ///
    /// `Ok(Conflict)` on borrow contention. `Err(Rebind)` if `data[name]`
    /// exists OR `types[name]` exists with a different `KType`. The pre-check
    /// runs before any insert, so a collision leaves both maps untouched.
    pub fn try_register_nominal(
        &self,
        name: &str,
        kt: &'a KType<'a>,
        obj: &'a KObject<'a>,
        index: BindingIndex,
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
        match types.get(name).map(|(t, _)| *t) {
            None => {
                types.insert(name.to_string(), (kt, index));
            }
            Some(existing) if existing == kt => {
                // Cycle-close-idempotent: keep the pre-installed index so
                // cycle members agree on one visibility tag for both maps.
            }
            Some(_) => {
                return Err(KError::new(KErrorKind::Rebind { name: name.to_string() }));
            }
        }
        data.insert(name.to_string(), (obj, index));
        drop(data);
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
    /// same `index` (and `nominal_binder` flag) so the consumer's visibility
    /// test stays consistent across the placeholder → finalized transition.
    pub fn try_install_placeholder(
        &self,
        name: String,
        idx: NodeId,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if let Some((existing, _)) = self.data.borrow().get(&name).copied() {
            if matches!(existing, KObject::KFunction(_, _)) {
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
    /// Lenient when the bucket is already live in `functions` (concurrent
    /// finalize): silent no-op.
    pub fn try_install_pending_overload(
        &self,
        bucket: UntypedKey,
        idx: NodeId,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if self.functions.borrow().contains_key(&bucket) {
            return Ok(());
        }
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

    /// Shared write path for type-only bindings. `try_register_nominal`
    /// inlines its own `types → data` transaction rather than reusing this.
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
            return Err(KError::new(KErrorKind::Rebind { name: name.to_string() }));
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
                    None => return Err(KError::new(KErrorKind::Rebind { name: name.to_string() })),
                    Some(_) => {
                        if !matches!(existing, KObject::KFunction(_, _)) {
                            return Err(KError::new(KErrorKind::Rebind { name: name.to_string() }));
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
