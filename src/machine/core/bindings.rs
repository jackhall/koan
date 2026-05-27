//! The lexical binding façade: five co-mutating `RefCell` maps (`types`, `data`,
//! `functions`, `placeholders`, `pending_overloads`) plus the shared validated write
//! paths ([`Bindings::try_apply`] for `data`/`functions`,
//! [`Bindings::try_apply_type`] for `types`) that keep the function-mirror
//! invariant — every `data[name]` entry wrapping a `KFunction` lives in
//! `functions[signature.untyped_key()]`. Nominal declarations (STRUCT / UNION /
//! MODULE) go through [`Bindings::try_register_nominal`], which atomically installs
//! the identity into `types` alongside the carrier in `data`.
//!
//! Borrow discipline across the maps: `types → functions → data`, with `types`
//! only acquired when writing types. [`Scope`] embeds the façade by value so all
//! interior borrows arbitrate against one another.
//!
//! Every entry in every map is tagged with a [`BindingIndex`] capturing the lexical
//! position of the binder that installed it. `BindingIndex { idx: 0, .. }` is reserved
//! for builtins (registered before any user statement); user statements at top-level
//! start at `idx: 1`. The `nominal_binder` flag carves out STRUCT / UNION / SIG /
//! FUNCTOR / MODULE so siblings on the same block can see one another's nominal
//! identities regardless of source order (mutual recursion across nominal binders).
//!
//! Production reads go through three visibility-aware lookups owned by the façade —
//! [`Bindings::lookup_value`] / [`Bindings::lookup_type`] /
//! [`Bindings::lookup_function`] — which apply the per-entry visibility filter
//! against a caller-supplied `chain_cutoff` (the consumer's index within this scope,
//! computed via [`crate::machine::core::LexicalFrame::index_for`]). The raw map
//! accessors (`data` / `types` / `functions` / `placeholders` / `pending_overloads`)
//! are gated `#[cfg(test)]`; the value-yielding `iter_data` / `iter_types` /
//! `iter_functions` cover the few production sites that genuinely sweep all
//! members (module surface mirroring, signature shape-check, REPL reflection).

use std::cell::{Ref, RefCell};
use std::collections::HashMap;

use crate::machine::model::ast::TypeExpr;
use crate::machine::core::kfunction::{KFunction, NodeId};
use crate::machine::model::types::{KType, UntypedKey};
use crate::machine::model::values::KObject;

use super::kerror::{KError, KErrorKind};

mod pending;
pub use pending::{PendingBinderGuard, PendingTypeEntry, PendingTypes};

/// Outcome of a value-side name lookup. Returned by [`Bindings::lookup_value`]
/// and surfaced to consumers through [`crate::machine::core::Scope::resolve`]
/// (which walks the ancestor chain and selects the first scope that returns
/// `Some`). `Resolution::Placeholder` carries the producer `NodeId` the
/// consumer should park on (a binder dispatched the name but its body hasn't
/// finalized).
///
/// Invariant: within one scope, `data` and `placeholders` never both hold the
/// same name — every successful write path clears any matching placeholder.
///
/// Index-gated resolution splits the not-found cases:
/// - [`Resolution::UnboundName`] — no binding visible at this consumer's chain
///   position. Structural absence: either the name is misspelled, or the
///   binding lives at a later sibling that hasn't been ordered yet (the
///   visibility gate hides it).
/// - [`Resolution::Placeholder`] — a binder placeholder is visible and not yet
///   finalized; the consumer parks on the producer `NodeId`.
/// - [`Resolution::Value`] — the binding is finalized and visible.
pub enum Resolution<'a> {
    Value(&'a KObject<'a>),
    Placeholder(NodeId),
    UnboundName,
}

/// Outcome of a per-scope `lookup_function` call. Encapsulates today's
/// "consult `functions` first; if absent, consult `pending_overloads`" pattern
/// so callers see uniform shapes from one ancestor visit and never inspect raw
/// maps. The visibility filter (per `chain_cutoff`) is applied inside the
/// lookup; `Bucket` is non-empty (an all-filtered bucket surfaces as `None`),
/// and `Pending` is returned only if no visible bucket but a visible
/// pending-overload entry exists at this scope.
pub enum FunctionLookup<'a> {
    /// Visible candidates for this bucket at this scope. Non-empty.
    Bucket(Vec<&'a KFunction<'a>>),
    /// No live bucket at this scope but a visible `pending_overloads` entry —
    /// a sibling FN / FUNCTOR binder has dispatched a matching overload whose
    /// body hasn't finalized. The producer's `NodeId` is the park target. See
    /// [`Bindings::try_install_pending_overload`].
    Pending(NodeId),
    /// No visible bucket and no visible pending overload at this scope.
    None,
}

/// Lexical position of a binding's installing statement, paired with a `nominal_binder`
/// flag that carves out STRUCT / UNION / SIG / FUNCTOR / MODULE declarations from the
/// strict-lexical-cutoff rule (so sibling nominal binders see one another regardless of
/// source order). Stored alongside every entry in `data`, `placeholders`, `functions`
/// (per overload), `pending_overloads`, and `types`.
///
/// - `idx == 0` is reserved for builtins, which register before any user statement runs.
/// - User statements at top-level start at `idx: 1`; nested blocks restart from 0
///   relative to their enclosing block (the visibility test consults
///   [`LexicalFrame::index_for`] per scope, so the per-block index is what matters).
/// - `nominal_binder: true` means "visible to siblings regardless of cutoff". Set by the
///   nominal-decl forms (struct_def, union, sig_def, functor_def, module_def) at install
///   time. LET / FN value-side bindings pass `false`.
///
/// See [`crate::machine::core::scope::Scope::resolve`] for the predicate.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct BindingIndex {
    pub idx: usize,
    pub nominal_binder: bool,
}

impl BindingIndex {
    /// Builtins register before any user statement, so they always satisfy `b.idx < c`
    /// for any `c >= 1`. Convention — there's no `Builtin` variant; the rule is "tag with
    /// `idx: 0`".
    pub const BUILTIN: BindingIndex = BindingIndex { idx: 0, nominal_binder: false };

    /// Tag for a value-side bind (LET, FN body capture, MATCH / TRY `it`, FN parameters):
    /// strictly lexically gated, sees only earlier-positioned siblings.
    pub const fn value(idx: usize) -> Self {
        BindingIndex { idx, nominal_binder: false }
    }

    /// Tag for a nominal-decl bind (STRUCT, named UNION, SIG, FUNCTOR, MODULE): visible
    /// to siblings on the same block regardless of source order — the carve-out that lets
    /// mutual-recursive nominal decls elaborate as an SCC.
    pub const fn nominal(idx: usize) -> Self {
        BindingIndex { idx, nominal_binder: true }
    }
}

/// Façade owning the co-mutating `RefCell` maps that back every lexical binding:
/// `types` (name → `&KType`), `data` (name → value), `functions`
/// (untyped-signature bucket → overloads), `placeholders` (name → producer
/// NodeId for forward-reference *name* resolution), and `pending_overloads`
/// (UntypedKey → producer NodeId for forward-reference *dispatch* parking on
/// not-yet-finalized FN / FUNCTOR overloads).
///
/// The two placeholder maps are intentionally separate: `placeholders` is
/// consulted by name (`Scope::resolve` → `Resolution::Placeholder`) and serves
/// type / value forward references; `pending_overloads` is consulted by
/// dispatch bucket key and serves a bare-arg call form like
/// `(MAKESET IntOrd)` whose FN/FUNCTOR overload is still finalizing. Keying
/// dispatch parks by full bucket key (rather than just the lead keyword) keeps
/// `(MAKESET _)` and `(MAKESET _ USING _)` from colliding when one ships before
/// the other.
///
/// [`Bindings::try_apply`] enforces the function-mirror invariant — every `data[name]`
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
    types: RefCell<HashMap<String, (&'a KType<'a>, BindingIndex)>>,
    data: RefCell<HashMap<String, (&'a KObject<'a>, BindingIndex)>>,
    functions: RefCell<HashMap<UntypedKey, Vec<(&'a KFunction<'a>, BindingIndex)>>>,
    placeholders: RefCell<HashMap<String, (NodeId, BindingIndex)>>,
    /// Bucket-key → (producer NodeId, lexical index) for FN / FUNCTOR overloads
    /// whose binder has dispatched but not finalized. Consulted by
    /// `resolve_dispatch`'s no-bucket / no-eager-parts fallback so a bare-arg call
    /// to an inflight overload parks on the producer instead of surfacing
    /// `DispatchFailed`. Cleared in [`Bindings::try_apply`] at the same site where
    /// the overload lands in `functions`, so the wake-and-retry sees the bucket
    /// populated.
    pending_overloads: RefCell<HashMap<UntypedKey, (NodeId, BindingIndex)>>,
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
            pending_overloads: RefCell::new(HashMap::new()),
            pending: PendingTypes::new(),
            type_expr_memo: RefCell::new(HashMap::new()),
        }
    }

    pub fn type_expr_memo_get(&self, te: &TypeExpr) -> Option<&'a KType<'a>> {
        self.type_expr_memo.borrow().get(te).copied()
    }

    /// Per-scope value-side lookup. Consults `data` first, then `placeholders`,
    /// returning the first hit that satisfies the visibility predicate. The
    /// invariant that `data` and `placeholders` never both hold the same name
    /// at one scope means the `data`-first order picks the value when both
    /// would otherwise compete.
    ///
    /// `chain_cutoff` is the consumer's lexical index within *this* scope as
    /// computed by [`crate::machine::core::LexicalFrame::index_for`]. `None`
    /// means "this scope is not on the consumer's chain (it is complete) — see
    /// everything"; the unfiltered overload of `Scope::resolve` (test fixtures,
    /// builtin registration paths) also passes `None`.
    ///
    /// Returns `None` if neither map has a *visible* entry. Distinguishing
    /// `Resolution::UnboundName` (the consumer surfaces this on chain
    /// exhaustion) from per-scope absence is the caller's job — `Scope`'s
    /// resolver walk treats every `None` as "keep walking".
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
    /// `types` map. `None` denotes "no visible type binding at this scope";
    /// the caller continues walking ancestors.
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

    /// Per-scope dispatch-bucket lookup. Folds together today's
    /// `functions` / `pending_overloads` reach-arounds:
    ///
    /// 1. Filter `functions[key]` by per-overload visibility. A non-empty
    ///    result returns [`FunctionLookup::Bucket`].
    /// 2. Otherwise, if `pending_overloads[key]` is visible, return
    ///    [`FunctionLookup::Pending`] with the producer `NodeId`.
    /// 3. Otherwise [`FunctionLookup::None`].
    ///
    /// The `Bucket` arm strips `BindingIndex` from each survivor — the per-
    /// overload visibility filter has already run, and the dispatch picker
    /// reads only the function reference. The `Pending` arm is the unified
    /// home of what `pending_overload_producer` did per-scope before this
    /// surface existed.
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
        if let Some((producer, idx)) = self.pending_overloads.borrow().get(key).copied() {
            if Self::visible(idx, chain_cutoff) {
                return FunctionLookup::Pending(producer);
            }
        }
        FunctionLookup::None
    }

    /// Iterate every `(name, value)` pair in `data`, regardless of visibility.
    /// Production callers that need a value-yielding sweep over the map
    /// (`MODULE` member mirroring, signature shape-check, REPL-style
    /// reflection) consume the iterator inside the `Bindings` borrow — the
    /// returned iterator is bounded by `&self` and exposes no `Ref` to the
    /// caller. For chain-gated single-name reads, prefer [`Self::lookup_value`].
    pub fn iter_data(&self) -> Vec<(String, &'a KObject<'a>)> {
        self.data
            .borrow()
            .iter()
            .map(|(name, (obj, _))| (name.clone(), *obj))
            .collect()
    }

    /// Iterate every `(name, &KType)` pair in `types`, regardless of
    /// visibility. Same shape as [`Self::iter_data`]; for chain-gated single-
    /// name reads, prefer [`Self::lookup_type`].
    pub fn iter_types(&self) -> Vec<(String, &'a KType<'a>)> {
        self.types
            .borrow()
            .iter()
            .map(|(name, (kt, _))| (name.clone(), *kt))
            .collect()
    }

    /// Iterate every `(UntypedKey, Vec<&KFunction>)` pair in `functions`,
    /// regardless of per-overload visibility. The test-support reflection
    /// helpers (`test_support::lookup_named_overload`) consume this; production
    /// code consults [`Self::lookup_function`] instead for chain-gated picks.
    pub fn iter_functions(&self) -> Vec<(UntypedKey, Vec<&'a KFunction<'a>>)> {
        self.functions
            .borrow()
            .iter()
            .map(|(key, bucket)| (key.clone(), bucket.iter().map(|(f, _)| *f).collect()))
            .collect()
    }

    /// The `BindingIndex` of an installed placeholder, ignoring visibility.
    /// Recovery-only helper: cycle-close in `model/types/resolver.rs` re-stamps
    /// the same lexical position the placeholder install used, so the
    /// downstream `register_nominal`'s idempotent arm matches. Production code
    /// outside that one re-stamp path should read placeholders through
    /// [`Self::lookup_value`]'s `Placeholder` arm.
    pub fn placeholder_index(&self, name: &str) -> Option<BindingIndex> {
        self.placeholders.borrow().get(name).map(|(_, idx)| *idx)
    }

    /// The visibility predicate the index-gated lookups apply per entry.
    /// Mirrors [`crate::machine::core::scope::visible`] for the inside-the-
    /// lookup case where the caller has already mapped chain →
    /// `chain_cutoff: Option<usize>` for this scope.
    ///
    /// - `chain_cutoff = None` ⇒ scope is complete (or unfiltered fixture
    ///   path) ⇒ everything visible.
    /// - `chain_cutoff = Some(c)` ⇒ visible iff `b.nominal_binder || b.idx < c`
    ///   (strict-lexical predecessor unless the binder is a nominal carve-out).
    fn visible(b: BindingIndex, chain_cutoff: Option<usize>) -> bool {
        match chain_cutoff {
            None => true,
            Some(c) => b.nominal_binder || b.idx < c,
        }
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

    /// Read-only view of the data map. Test-only: production code reads
    /// through [`Self::lookup_value`] / [`Self::iter_data`], which apply the
    /// visibility predicate and don't expose `Ref<HashMap<...>>` to callers.
    /// Each entry pairs the carrier with the lexical [`BindingIndex`] of the
    /// statement that installed it.
    #[cfg(test)]
    pub fn data(&self) -> Ref<'_, HashMap<String, (&'a KObject<'a>, BindingIndex)>> {
        self.data.borrow()
    }

    /// Read-only view of the per-bucket overload list. Test-only mirror of
    /// [`Self::lookup_function`] / [`Self::iter_functions`]; each overload
    /// carries its own [`BindingIndex`] so per-overload visibility tests can
    /// inspect the lexical position directly.
    #[cfg(test)]
    pub fn functions(&self) -> Ref<'_, HashMap<UntypedKey, Vec<(&'a KFunction<'a>, BindingIndex)>>> {
        self.functions.borrow()
    }

    /// Read-only view of the dispatch-time name placeholder map. Test-only.
    /// Each placeholder carries the producer's `NodeId` (consumer parks on
    /// this for readiness) and the [`BindingIndex`] of the producing statement
    /// (consumer filters this for visibility). Production code reads
    /// placeholders through [`Self::lookup_value`]'s `Placeholder` arm.
    #[cfg(test)]
    pub fn placeholders(&self) -> Ref<'_, HashMap<String, (NodeId, BindingIndex)>> {
        self.placeholders.borrow()
    }

    /// Read-only view of the bucket-key → (producer, lexical index) map.
    /// Test-only: production code reads pending overloads through
    /// [`Self::lookup_function`]'s `Pending` arm. See
    /// [`Bindings::try_install_pending_overload`] for the writer.
    #[cfg(test)]
    pub fn pending_overloads(&self) -> Ref<'_, HashMap<UntypedKey, (NodeId, BindingIndex)>> {
        self.pending_overloads.borrow()
    }

    /// Read-only view of the types map. Test-only mirror of
    /// [`Self::lookup_type`] / [`Self::iter_types`]; each entry pairs the type
    /// with the lexical [`BindingIndex`] of the statement that installed it.
    /// Builtins use [`BindingIndex::BUILTIN`] (idx 0).
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
    /// `index` is the lexical [`BindingIndex`] of the installing statement; visibility
    /// reads against the resolver chain consult it via [`Scope::resolve`].
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

    /// Bare-`FN` overload registration. Adds `fn_ref` to the `functions` bucket keyed by
    /// its untyped signature *only* — it does **not** mirror `obj` into `data[name]`, so a
    /// bare FN keyword is dispatchable but not nameable as a value (use `LET f = (FN …)`
    /// for that). Errors:
    /// - `DuplicateOverload` if the bucket already holds an exact-signature equal function.
    ///
    /// `index` tags the registered overload with its installing statement's lexical
    /// position; per-overload tagging matters because overloads sharing one bucket can
    /// sit at different positions (`OverloadBucket::pick` filters per-overload).
    ///
    /// `obj` is unused on the write side today (no `data` insert) but kept in the signature
    /// so the call site, which has a `&KObject` carrier in hand, stays uniform with
    /// [`Bindings::try_bind_value`].
    pub fn try_register_function(
        &self,
        name: &str,
        fn_ref: &'a KFunction<'a>,
        obj: &'a KObject<'a>,
        index: BindingIndex,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply(name, obj, Some(fn_ref), false, index)
    }

    /// Register `name` → `kt` in the type-binding map. Errors `Rebind` if
    /// `types[name]` already exists; returns `Ok(Conflict)` on borrow contention
    /// (caller queues — same shape as [`Bindings::try_bind_value`] and
    /// [`Bindings::try_register_function`]). Best-effort placeholder clear on success.
    /// `index` tags the binding with its installing statement's lexical position.
    pub fn try_register_type(
        &self,
        name: &str,
        kt: &'a KType<'a>,
        index: BindingIndex,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply_type(name, kt, index)
    }

    /// Atomic install for nominal declarations (STRUCT / UNION / MODULE): inserts
    /// identity `kt` into `types[name]` and runtime carrier `obj` into `data[name]`.
    /// Borrow order is `types → data` (the `functions` map is untouched — nominal
    /// carriers are not callable verbs).
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
                // Cycle-close-idempotent: SCC pre-registration already wrote the
                // identity (with its own index). Carrier-write below completes the
                // pair; keep the pre-installed index so cycle members agree on the
                // single visibility tag for both maps.
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
    ///
    /// `index` tags the placeholder with its installing statement's lexical position.
    /// The eventual `try_bind_value` / `try_register_*` call must carry the same index
    /// (and the same `nominal_binder` flag) so the consumer's visibility test sees a
    /// consistent answer across the placeholder → finalized-binding transition.
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

    /// Install a dispatch-time pending-overload entry: `bucket → producer`. The
    /// bucket key MUST equal what `KExpression::untyped_key` would compute for
    /// a *call* to the eventual overload (not the binder call itself), so the
    /// no-bucket fallback in `resolve_dispatch` finds the producer by the same
    /// key. Multiple in-flight FN/FUNCTOR binders sharing a lead keyword but
    /// differing in later keywords get separate entries — keying by the full
    /// `UntypedKey` (rather than just the lead keyword) is the whole point.
    ///
    /// Idempotent if re-entered with the same `(bucket, idx)`; rejects `Rebind`
    /// on a different `idx`. If the bucket is already populated in `functions`
    /// (the overload finalized concurrently), silently no-ops — the next
    /// dispatch will hit the live bucket directly.
    ///
    /// Panics on borrow conflict, mirroring [`Bindings::try_install_placeholder`].
    ///
    /// `index` is the producing binder's lexical position; consumers filter
    /// `pending_overloads` hits by this index in the same way the live `functions`
    /// bucket filters per-overload.
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
        if let Some((existing, _)) = pending.get(&bucket).copied() {
            if existing == idx {
                return Ok(());
            }
            return Err(KError::new(KErrorKind::Rebind {
                name: format!("pending-overload bucket {bucket:?}"),
            }));
        }
        pending.insert(bucket, (idx, index));
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
            self.clear_pending_overload_best_effort(&bucket);
        }
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

    /// Companion to [`Bindings::clear_placeholder_best_effort`] for the bucket-keyed
    /// pending-overload table. Same tolerant pattern — a caller mid-read up the stack
    /// is fine; the entry is purely a wakeable forward reference, and the bucket is
    /// already populated by the time this runs.
    fn clear_pending_overload_best_effort(&self, bucket: &UntypedKey) {
        if let Ok(mut p) = self.pending_overloads.try_borrow_mut() {
            p.remove(bucket);
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
