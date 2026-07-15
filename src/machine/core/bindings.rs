//! Lexical binding façade: co-mutating `RefCell` maps (`types`, `data`,
//! `functions`, `placeholders`, `pending_overloads`) behind validated write
//! paths that keep the function-mirror invariant — every `data[name]` wrapping
//! a `KFunction` lives in `functions[signature.untyped_key()]`. Nominal type
//! declarations (NEWTYPE / UNION / SIG) install their identity into `types`
//! only — there is no value-side carrier; a module is a value and binds into
//! `data`. The `data` and `types` maps
//! are a structural partition: a name is committed to one xor the other, never
//! both, enforced by the cross-kind check the value and type write paths run.
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
//! is what [`Bindings::has_builtin_type`] / [`Bindings::has_builtin_function`] read to
//! mark a genuine builtin for the no-shadow and root-first consults. The operator
//! registry takes no such consult: its walk is innermost-wins, so the root's builtin
//! groups are found last and act as defaults (see
//! [`crate::machine::core::Scope::resolve_operator_group_with_chain`]).
//!
//! Production reads use the visibility-aware [`Bindings::lookup_value`] /
//! [`Bindings::lookup_type`] / [`Bindings::lookup_function`], passing a
//! `chain_cutoff` computed via [`crate::machine::core::LexicalFrame::index_for`].
//! Raw map accessors are `#[cfg(test)]`.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;

use crate::machine::core::arena::FrameSet;
use crate::machine::core::kfunction::{KFunction, NodeId};
use crate::machine::model::KObject;
use crate::machine::model::OperatorGroup;
use crate::machine::model::TypeIdentifier;
use crate::machine::model::{KType, UntypedKey};

use super::kerror::{KError, KErrorKind};

mod pending;
pub use pending::{PendingBinderGuard, PendingTypeEntry, PendingTypes};

/// Whether a binding — committed or an in-flight placeholder — lives in the value
/// language or the type language. The `data`/`types` partition is mutually exclusive
/// (a name is one xor the other; see the cross-kind check in the write paths), and a
/// forward-reference placeholder is tagged with its kind so a type placeholder is
/// never satisfied by a value bind, nor the reverse.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum BindKind {
    Value,
    Type,
}

/// Outcome of a single-scope name lookup: the name is `Bound` to a `T`, or `Parked` on the
/// producer `NodeId` of an earlier still-finalizing binder the consumer waits on. A miss is the
/// enclosing `Option`'s `None` — the caller keeps walking ancestors — so "unbound" is not a
/// variant here; the terminal unbound disposition (with its diagnostic) is materialized one level
/// up on the resolution path ([`crate::machine::model::TypeResolution`] /
/// [`crate::machine::NameOutcome`]).
///
/// Invariant: within one scope, `data` and a `BindKind::Value` `placeholders` entry never both
/// hold the same name — every successful value write path clears its matching value placeholder.
#[derive(Copy, Clone, Debug)]
pub enum NameLookup<T> {
    Bound(T),
    Parked(NodeId),
}

impl<T> NameLookup<T> {
    /// The bound payload, or `None` for an in-flight placeholder — for callers that act only on a
    /// finalized binding and treat a still-running producer as "not bound yet".
    pub fn bound(self) -> Option<T> {
        match self {
            NameLookup::Bound(payload) => Some(payload),
            NameLookup::Parked(_) => None,
        }
    }

    /// Map the bound payload, threading a `Parked` producer through unchanged — the combinator the
    /// carrier ladder uses to re-wrap a hit without restating the two-arm match.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> NameLookup<U> {
        match self {
            NameLookup::Bound(payload) => NameLookup::Bound(f(payload)),
            NameLookup::Parked(id) => NameLookup::Parked(id),
        }
    }
}

/// A binding's stored reach plus the one-bit answer to "does the bound value borrow into **this**
/// scope's own region?" ([`Self::borrows_into_home`]). `foreign` is a reference to a set hosted in
/// the binding scope's own region arena — minted at bind time via [`RegionSet::mint`], never owned
/// here — home-omitted so it never names the scope's own home frame, whose `Rc` stored in-region
/// would close the `frame → region → scope → bindings → frame` cycle; that fact is remembered as
/// the bit instead. `None` is the faithful encoding of the empty set (a region-pure value pins
/// nothing), not a missing value — a read materializes the bit back into an explicit reach member;
/// until then a bind threads it through unread.
///
/// [`Self::empty`] defaults the bit to `false`: a value delivered by a region-pure or foreign
/// carrier borrows into no home region, which is every builtin registration and every test bind.
/// Only a production bind whose delivered carrier's reach covers the binding scope's region sets it
/// `true`.
#[derive(Clone, Copy)]
pub struct StoredReach<'a> {
    pub(in crate::machine::core) foreign: Option<&'a FrameSet>,
    pub(in crate::machine::core) borrows_into_home: bool,
}

impl<'a> StoredReach<'a> {
    /// The empty reach that borrows into no home — the region-pure / no-carrier default.
    ///
    /// Deliberately **not** a [`Default`] impl, and deliberately not visible outside
    /// `crate::machine::core`. A `Default` would be a public trait method on a public struct, which
    /// hands the whole crate the power to mint a reach out of thin air and pair it with a value it
    /// was never derived from — the exact forgery the reach-token discipline exists to prevent. The
    /// only reaches code outside `core` can hold are ones a fused door derived for a specific value
    /// (a [`TypeHit`], a delivered carrier's bind), so it cannot assert coverage it has no evidence
    /// for. Keep it that way: a `#[derive(Default)]` here silently reopens that door.
    pub(in crate::machine::core) fn empty() -> Self {
        StoredReach {
            foreign: None,
            borrows_into_home: false,
        }
    }

    /// Narrow test affordance: assemble a token from explicit parts for in-crate `mod tests` only.
    #[cfg(test)]
    pub(crate) fn for_test(foreign: Option<&'a FrameSet>, borrows_into_home: bool) -> Self {
        StoredReach {
            foreign,
            borrows_into_home,
        }
    }

    /// Whether the token names a non-empty foreign reach — a semantic query over the opaque token
    /// for in-crate `mod tests` that assert a reach round-tripped, without decomposing the fields.
    #[cfg(test)]
    pub(crate) fn names_a_region(&self) -> bool {
        self.foreign.is_some()
    }
}

/// The value-or-type a name resolves to in one classified result — for ATTR module/signature
/// member access. Produced by [`crate::machine::core::Scope::lookup_member`], which checks the
/// module-own value side then the type side in one call. The `data`/`types` cross-kind exclusion
/// keeps the two arms from ever both matching within a scope.
pub enum MemberResolution<'a> {
    Value {
        obj: &'a KObject<'a>,
        /// The member's stored reach (home-omitted foreign reach + the home-borrow bit), copied
        /// whole off the module's own `data` entry — so an ATTR read replays the same opaque token
        /// into a resident carrier rather than re-asserting single-frame co-location.
        stored: StoredReach<'a>,
    },
    Type {
        kt: &'a KType<'a>,
        /// The member type's stored reach, copied whole off the module's own `types` entry — so an
        /// ATTR type read witnesses the existing `&KType` in place from the replayed token.
        stored: StoredReach<'a>,
    },
}

/// The value-side reach-carrying payload of a `NameLookup<ValueHit>`: the bound value plus the
/// binding's home-omitted foreign reach, copied out (a `&'a FrameSet` reference, not a clone) so
/// the read wrapper does not hold the `data` `RefCell` borrow across the carrier build. Produced by
/// [`Bindings::lookup_value_carrier`] so a name read builds a self-contained witness from the
/// stored reach.
pub struct ValueHit<'a> {
    pub obj: &'a KObject<'a>,
    /// The binding's stored reach (home-omitted foreign reach + the home-borrow bit), copied whole
    /// off the `data` entry so the read wrapper does not hold the `RefCell` borrow across the
    /// carrier build.
    pub stored: StoredReach<'a>,
}

/// The type-side reach-carrying payload of a `NameLookup<TypeHit>`: the bound `&KType` witnessed in
/// place plus the binding's home-omitted foreign reach (empty for owned data, the child-scope reach
/// for a module), copied out (a `&'a FrameSet` reference, not a clone) so the read wrapper does not
/// hold the `types` `RefCell` borrow. Produced by [`Bindings::lookup_type_carrier`] so a type read
/// witnesses the existing `&KType` in place from the stored reach, and reused as the `Done` payload
/// of the type-identifier resolution memo (`TypeResolution<TypeHit>`), which carries the same
/// `&KType` + stored reach.
pub struct TypeHit<'a> {
    pub kt: &'a KType<'a>,
    /// The binding's stored reach (home-omitted foreign reach + the home-borrow bit), copied whole
    /// off the `types` entry so the read wrapper does not hold the `RefCell` borrow, and reused as
    /// the `Done` payload of the type-identifier resolution memo.
    pub stored: StoredReach<'a>,
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
/// `pending` names a visible `pending_overloads` entry — a sibling FN
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
/// key (a bare-arg call whose FN overload is still finalizing). Keying
/// dispatch parks by the full bucket key keeps `(MAKESET _)` and
/// `(MAKESET _ USING _)` from colliding.
///
/// Borrow discipline: `types → functions → data`. Lifetime `'a` is the region
/// lifetime of the stored references.
pub struct Bindings<'a> {
    /// Whether this scope's `types` map is a **signature slot table** rather than a type-binding
    /// universe. A SIG body records each `VAL <name> :Type` slot's declared type into `types` keyed
    /// by the slot's *value* name, alongside its `TYPE <Name>` abstract members — one table that
    /// [`SigSchema`](crate::machine::model::SigSchema) splits back apart by token class. That
    /// is a schema, not a binding, so it is the one place a value token legitimately appears on the
    /// type side, and the token-class partition ([`Self::partition_guard`]) stands down for it.
    slot_table: bool,
    /// Each type entry stores its bound type, its lexical [`BindingIndex`], and its **reach** — the
    /// home-omitted foreign [`FrameSet`] the type borrows into. Empty for owned data (`Number`, a
    /// struct `SetRef`, an `AbstractType`); non-empty for a type borrowing a foreign region (a
    /// `Signature`). A carrier-oriented read
    /// ([`Self::lookup_type_carrier`]) hands the reach back so the read witnesses the existing
    /// `&'a KType` in place from its stored reach, never re-deriving it by walking the value.
    /// Foreign-only (home-omitted) for the same cycle-safety rule as [`Self::data`].
    types: RefCell<HashMap<String, (&'a KType<'a>, BindingIndex, StoredReach<'a>)>>,
    /// Each value entry stores its bound value, its lexical [`BindingIndex`], and its **reach** —
    /// the home-omitted foreign [`FrameSet`] the value borrows into, captured at bind time from the
    /// delivered carrier. A carrier-oriented read ([`Self::lookup_value_carrier`]) hands the reach
    /// back so the read wraps the value in a self-contained witness built from its stored reach,
    /// rather than re-asserting single-frame co-location. The reach is foreign-only (home-omitted)
    /// so it never stores the region's own home frame `Rc` in-region — that would close a
    /// `frame → region → scope → bindings → frame` strong cycle and leak the region.
    data: RefCell<HashMap<String, (&'a KObject<'a>, BindingIndex, StoredReach<'a>)>>,
    functions: RefCell<HashMap<UntypedKey, Vec<(&'a KFunction<'a>, BindingIndex)>>>,
    placeholders: RefCell<HashMap<String, (NodeId, BindingIndex, BindKind)>>,
    /// Bucket-key → entries for FN overloads whose binder has
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
    type_identifier_memo: RefCell<HashMap<(TypeIdentifier, Option<usize>), TypeMemoEntry<'a>>>,
}

/// A `type_identifier_memo` value: the cached `&KType` plus the resolved binding's [`StoredReach`]
/// (home-omitted reach + the home-borrow bit), so a memo hit rebuilds the read carrier without
/// re-walking the chain.
type TypeMemoEntry<'a> = (&'a KType<'a>, StoredReach<'a>);

impl<'a> Bindings<'a> {
    pub fn new() -> Self {
        Self::with_role(false)
    }

    /// A SIG body's bindings: its `types` map is the signature's slot table (see
    /// [`Self::slot_table`]).
    pub fn new_slot_table() -> Self {
        Self::with_role(true)
    }

    fn with_role(slot_table: bool) -> Self {
        Self {
            slot_table,
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
    ) -> Option<(&'a KType<'a>, StoredReach<'a>)> {
        self.type_identifier_memo
            .borrow()
            .get(&(te.clone(), cutoff))
            .map(|(kt, reach)| (*kt, *reach))
    }

    /// Per-scope value-side lookup. Consults `data` then `placeholders`,
    /// returning the first visible hit. `chain_cutoff = None` means the scope
    /// is off-chain (or unfiltered) — everything is visible. `None` return
    /// means no visible entry at this scope; the caller keeps walking
    /// ancestors, and chain exhaustion stays `None` (the terminal unbound
    /// disposition is materialized on the resolution path, not here).
    pub fn lookup_value(
        &self,
        name: &str,
        chain_cutoff: Option<usize>,
    ) -> Option<NameLookup<&'a KObject<'a>>> {
        if let Some((obj, idx, _reach)) = self.data.borrow().get(name) {
            if Self::visible(*idx, chain_cutoff) {
                return Some(NameLookup::Bound(obj));
            }
        }
        self.value_placeholder(name, chain_cutoff)
            .map(NameLookup::Parked)
    }

    /// The value-side placeholder producer for `name`, or `None` — shared by
    /// [`Self::lookup_value`] and [`Self::lookup_value_carrier`], which differ only in the
    /// `data` arm.
    fn value_placeholder(&self, name: &str, chain_cutoff: Option<usize>) -> Option<NodeId> {
        if let Some((id, idx, kind)) = self.placeholders.borrow().get(name).copied() {
            if kind == BindKind::Value && Self::visible(idx, chain_cutoff) {
                return Some(id);
            }
        }
        None
    }

    /// Per-scope type-side lookup. The type-language mirror of [`Self::lookup_value`]:
    /// consults `types` then the `BindKind::Type` `placeholders` entries, returning the
    /// first visible hit as a [`NameLookup`], or `None` so the caller keeps walking.
    pub fn lookup_type(
        &self,
        name: &str,
        chain_cutoff: Option<usize>,
    ) -> Option<NameLookup<&'a KType<'a>>> {
        if let Some((kt, idx, _reach)) = self.types.borrow().get(name) {
            if Self::visible(*idx, chain_cutoff) {
                return Some(NameLookup::Bound(kt));
            }
        }
        self.type_placeholder(name, chain_cutoff)
            .map(NameLookup::Parked)
    }

    /// The type-side placeholder producer for `name`, or `None` — shared by [`Self::lookup_type`]
    /// and [`Self::lookup_type_carrier`], which differ only in the `types` arm.
    fn type_placeholder(&self, name: &str, chain_cutoff: Option<usize>) -> Option<NodeId> {
        if let Some((id, idx, kind)) = self.placeholders.borrow().get(name).copied() {
            if kind == BindKind::Type && Self::visible(idx, chain_cutoff) {
                return Some(id);
            }
        }
        None
    }

    /// Carrier-oriented type lookup — the reach-carrying twin of [`Self::lookup_type`]. A `types` hit
    /// returns [`NameLookup::Bound`] with the binding's stored reach (cloned out); otherwise a
    /// visible type placeholder or a miss.
    pub fn lookup_type_carrier(
        &self,
        name: &str,
        chain_cutoff: Option<usize>,
    ) -> Option<NameLookup<TypeHit<'a>>> {
        if let Some((kt, idx, reach)) = self.types.borrow().get(name) {
            if Self::visible(*idx, chain_cutoff) {
                return Some(NameLookup::Bound(TypeHit { kt, stored: *reach }));
            }
        }
        self.type_placeholder(name, chain_cutoff)
            .map(NameLookup::Parked)
    }

    /// Classified per-scope member lookup for ATTR module / signature access: the value-or-type
    /// `name` resolves to, read from **this scope's own** `data` then `types` in one pass. A
    /// module member is module-own — the lookup deliberately does **not** consult the builtin
    /// root or walk lexical ancestors, so `m.Type` (a builtin type name) or `m.SomeOuterType`
    /// is "no member", not a fall-through. The cross-kind exclusion keeps the two arms from both
    /// matching, so the result is unambiguous. No placeholder arm — a read module is finalized.
    pub fn lookup_member(
        &self,
        name: &str,
        chain_cutoff: Option<usize>,
    ) -> Option<MemberResolution<'a>> {
        if let Some((obj, idx, reach)) = self.data.borrow().get(name) {
            if Self::visible(*idx, chain_cutoff) {
                return Some(MemberResolution::Value {
                    obj,
                    stored: *reach,
                });
            }
        }
        if let Some((kt, idx, reach)) = self.types.borrow().get(name) {
            if Self::visible(*idx, chain_cutoff) {
                return Some(MemberResolution::Type { kt, stored: *reach });
            }
        }
        None
    }

    /// Carrier-oriented value lookup — the reach-carrying twin of [`Self::lookup_value`]. A `data`
    /// hit returns [`NameLookup::Bound`] with the binding's stored reach (cloned out); otherwise a
    /// visible value placeholder or a miss, mirroring `lookup_value`'s data-then-placeholder order.
    pub fn lookup_value_carrier(
        &self,
        name: &str,
        chain_cutoff: Option<usize>,
    ) -> Option<NameLookup<ValueHit<'a>>> {
        if let Some((obj, idx, reach)) = self.data.borrow().get(name) {
            if Self::visible(*idx, chain_cutoff) {
                return Some(NameLookup::Bound(ValueHit {
                    obj,
                    stored: *reach,
                }));
            }
        }
        self.value_placeholder(name, chain_cutoff)
            .map(NameLookup::Parked)
    }

    /// The producer `NodeId` of a still-finalizing **type** binder named `name`, read straight
    /// from the kind-tagged `placeholders` map — *not* through [`Self::lookup_type`], which
    /// prefers a (possibly seal-pre-installed, still-unsealed) `types` entry. The finalize gate
    /// uses this to park the type-identifier memo on an in-flight producer even when the seal
    /// has already pre-installed the name's external identity into `types`. Visibility-unfiltered:
    /// this is producer-dependency tracking, not consumer-visibility enforcement.
    pub fn type_placeholder_producer(&self, name: &str) -> Option<NodeId> {
        match self.placeholders.borrow().get(name).copied() {
            Some((id, _, BindKind::Type)) => Some(id),
            _ => None,
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
        // Earliest-index visible producer: most likely to finalize first.
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

    /// Register `probe → group` in the operator registry. The `OP` / `GROUP` binder
    /// installs one entry per nonempty subset of the declared operators (all pointing at
    /// the same `group`); test fixtures register the subsets they exercise.
    ///
    /// Upsert: an existing entry whose record is the one being registered — pointer-equal,
    /// or an equal mode + member set (two `OP` statements over the same symbol and distinct
    /// operand types are two bucket overloads but one registry entry) — is a no-op
    /// `Applied`, keeping the first entry's index. A record that disagrees is a chaining-mode
    /// conflict on `probe`: the same scope cannot say the symbol both folds and pairs.
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
            if std::ptr::eq(existing, group) || existing == group {
                return Ok(ApplyOutcome::Applied);
            }
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "operator `{probe}` is already declared in this scope with a different \
                 chaining mode or member set; one scope declares one chaining mode per operator",
            ))));
        }
        operators.insert(probe, (group, index));
        Ok(ApplyOutcome::Applied)
    }

    /// Every binding entry's hosted reach set (`data` then `types`), for the seal-time module-reach
    /// union. Refs are `'a` (region-arena hosted), so they outlive the returned `Vec`.
    pub(crate) fn entry_reaches(&self) -> Vec<&'a FrameSet> {
        let data_reaches: Vec<&'a FrameSet> = self
            .data
            .borrow()
            .values()
            .filter_map(|(_, _, r)| r.foreign)
            .collect();
        data_reaches
            .into_iter()
            .chain(
                self.types
                    .borrow()
                    .values()
                    .filter_map(|(_, _, r)| r.foreign),
            )
            .collect()
    }

    /// Snapshot every `(name, value)` pair in `data`, ignoring visibility.
    /// For chain-gated single-name reads use [`Self::lookup_value`].
    pub fn iter_data(&self) -> Vec<(String, &'a KObject<'a>)> {
        self.data
            .borrow()
            .iter()
            .map(|(name, (obj, _, _))| (name.clone(), *obj))
            .collect()
    }

    /// Snapshot every `(name, &KType)` pair in `types`, ignoring visibility.
    pub fn iter_types(&self) -> Vec<(String, &'a KType<'a>)> {
        self.types
            .borrow()
            .iter()
            .map(|(name, (kt, _, _))| (name.clone(), *kt))
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
            .is_some_and(|(_, idx, _)| *idx == BindingIndex::BUILTIN)
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

    /// Visibility predicate: `None` ⇒ everything visible; `Some(c)` ⇒ `b.idx < c`.
    /// Mirrors [`crate::machine::core::scope::visible`].
    fn visible(b: BindingIndex, chain_cutoff: Option<usize>) -> bool {
        match chain_cutoff {
            None => true,
            Some(c) => b.idx < c,
        }
    }

    /// Insert `(te → (kt, reach))` into the resolution cache. Caller region-allocates `kt` and gates
    /// on finalize; `reach` is the resolved type binding's home-omitted foreign reach, cached so a
    /// memo hit rebuilds the read carrier without re-looking-up the binding. Monotonic: a collision
    /// means equal values, so we keep the existing entry rather than panic.
    pub fn type_identifier_memo_insert(
        &self,
        te: TypeIdentifier,
        cutoff: Option<usize>,
        kt: &'a KType<'a>,
        reach: StoredReach<'a>,
    ) {
        let mut memo = self.type_identifier_memo.borrow_mut();
        memo.entry((te, cutoff)).or_insert((kt, reach));
    }

    #[cfg(test)]
    pub fn data(
        &self,
    ) -> Ref<'_, HashMap<String, (&'a KObject<'a>, BindingIndex, StoredReach<'a>)>> {
        self.data.borrow()
    }

    #[cfg(test)]
    pub fn functions(
        &self,
    ) -> Ref<'_, HashMap<UntypedKey, Vec<(&'a KFunction<'a>, BindingIndex)>>> {
        self.functions.borrow()
    }

    #[cfg(test)]
    pub fn placeholders(&self) -> Ref<'_, HashMap<String, (NodeId, BindingIndex, BindKind)>> {
        self.placeholders.borrow()
    }

    #[cfg(test)]
    pub fn pending_overloads(&self) -> Ref<'_, HashMap<UntypedKey, Vec<(NodeId, BindingIndex)>>> {
        self.pending_overloads.borrow()
    }

    #[cfg(test)]
    pub fn types(
        &self,
    ) -> Ref<'_, HashMap<String, (&'a KType<'a>, BindingIndex, StoredReach<'a>)>> {
        self.types.borrow()
    }

    #[cfg(test)]
    pub fn expect_value(&self, name: &str) -> &'a KObject<'a> {
        self.data
            .borrow()
            .get(name)
            .map(|(obj, _, _)| *obj)
            .unwrap_or_else(|| panic!("expected bindings.data[{name:?}] to be present"))
    }

    #[cfg(test)]
    pub fn expect_type(&self, name: &str) -> &'a KType<'a> {
        self.types
            .borrow()
            .get(name)
            .map(|(kt, _, _)| *kt)
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

    /// LET-style value bind. Errors `Rebind` if `data[name]` already exists, or if `name`
    /// is a committed type (`types[name]`) — the value/type partition is mutually exclusive.
    /// When `obj` wraps a `KFunction` it is also mirrored into
    /// `functions[signature.untyped_key()]` so dispatch finds it (`LET f = (FN ...)`).
    ///
    /// `Conflict` means borrow contention (caller queues); `Err` is semantic rejection.
    pub fn try_bind_value(
        &self,
        name: &str,
        obj: &'a KObject<'a>,
        index: BindingIndex,
        reach: StoredReach<'a>,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply(name, obj, obj.as_function(), true, index, reach)
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
        // A bare-`FN` registration writes `functions` only, not `data`, so it stores no reach.
        self.try_apply(name, obj, Some(fn_ref), false, index, StoredReach::empty())
    }

    /// Register `name` → `kt` in `types`. Errors `Rebind` if already present in `types`, or
    /// if `name` is a committed value (`data[name]`) — the partition is mutually exclusive.
    /// `Ok(Conflict)` on borrow contention. Best-effort placeholder clear on success.
    pub fn try_register_type(
        &self,
        name: &str,
        kt: &'a KType<'a>,
        index: BindingIndex,
        reach: StoredReach<'a>,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply_type(name, kt, index, reach)
    }

    /// Upsert `name` → `kt` in `types` for nominal finalize. Insert-if-absent;
    /// on a `PartialEq`-equal existing entry **overwrite** the stored `&KType` (and
    /// `index`) so the `SetRef` an SCC seal pre-installed (same set + index) is rewritten
    /// in place. A non-equal existing entry is a genuine collision — `Err(Rebind)`, as is a
    /// committed value at `data[name]` (the value/type partition is mutually exclusive).
    ///
    /// Distinct from [`Self::try_register_type`], whose strict insert-if-absent arm
    /// would `Rebind` on the seal pre-install rather than overwrite it.
    /// `Ok(Conflict)` on borrow contention. Best-effort placeholder clear on success.
    pub fn try_register_type_upsert(
        &self,
        name: &str,
        kt: &'a KType<'a>,
        index: BindingIndex,
        reach: StoredReach<'a>,
    ) -> Result<ApplyOutcome, KError> {
        self.partition_guard(name, BindKind::Type)?;
        let mut types = match self.types.try_borrow_mut() {
            Ok(t) => t,
            Err(_) => return Ok(ApplyOutcome::Conflict),
        };
        // Cross-kind exclusion: a type name may not collide with a committed value. `types`
        // is already held, so probing `data` next preserves the `types → data` borrow order.
        match self.data.try_borrow() {
            Ok(data) => {
                if data.contains_key(name) {
                    return Err(KError::new(KErrorKind::Rebind {
                        name: name.to_string(),
                    }));
                }
            }
            Err(_) => return Ok(ApplyOutcome::Conflict),
        }
        match types.get(name).map(|(t, _, _)| *t) {
            Some(existing) if *existing != *kt => {
                return Err(KError::new(KErrorKind::Rebind {
                    name: name.to_string(),
                }));
            }
            // Absent, or identity-equal (the seal's pre-installed `SetRef`): write the
            // identity, rewriting any pre-install in place.
            _ => {
                types.insert(name.to_string(), (kt, index, reach));
            }
        }
        drop(types);
        self.clear_placeholder_best_effort(name, BindKind::Type);
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
    /// the placeholder → finalized transition. `kind` records which language the
    /// forward reference resolves in, so a value bind never satisfies a type
    /// placeholder (or the reverse) — see [`Bindings::lookup_value`] /
    /// [`Bindings::lookup_type`], each of which surfaces only its own kind.
    pub fn try_install_placeholder(
        &self,
        name: String,
        idx: NodeId,
        index: BindingIndex,
        kind: BindKind,
    ) -> Result<(), KError> {
        if let Some((existing, _, _)) = self.data.borrow().get(&name) {
            if matches!(existing, KObject::KFunction(_)) {
                return Ok(());
            }
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        let mut ph = self.placeholders.borrow_mut();
        if let Some((existing, _, _)) = ph.get(&name).copied() {
            if existing == idx {
                return Ok(());
            }
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        ph.insert(name, (idx, index, kind));
        Ok(())
    }

    /// Install a dispatch-time pending-overload entry: `bucket → producer`.
    /// The bucket key MUST equal what `KExpression::untyped_key` would compute
    /// for a *call* to the eventual overload (not the binder call itself).
    ///
    /// **Append, never deduplicate**: sibling FN binders sharing one
    /// inner-call bucket key — `FN (PICK xs :A) -> ...` then
    /// `FN (PICK xs :B) -> ...` — each install their own entry at their own
    /// [`BindingIndex`]. The entry is removed in [`Bindings::try_apply`] when
    /// the producing binder lands in `functions[bucket]`; other siblings stay
    /// pending as wake sources.
    ///
    /// Recorded even when the bucket is already live in `functions`: a pending
    /// sibling sits *alongside* a finalized overload so the scope walk can park
    /// the bucket until the sibling finalizes.
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
        let snapshot: Vec<(String, &'a KObject<'a>, BindingIndex, StoredReach<'a>)> = src
            .data
            .borrow()
            .iter()
            .map(|(k, (v, idx, reach))| (k.clone(), *v, *idx, *reach))
            .collect();
        for (name, obj, index, reach) in snapshot {
            match self.try_apply(&name, obj, obj.as_function(), true, index, reach)? {
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

    /// The token-class partition: `types` holds Type-token names, `data` holds value-token names, and a
    /// name may not cross. The two maps are different universes — a Type token names something that can
    /// type a field, a value token names something a field can hold — so a write whose name classifies
    /// against the map it is entering is a hard error, not a convention. This is the single enforcement
    /// point: every binder reaches its map through [`Bindings::try_apply`] /
    /// [`Bindings::try_apply_type`], so no caller can bind across the line, and none needs its own check.
    /// A keyword-class name (all-uppercase, no lowercase) is not a Type token, so a builtin's dispatch
    /// registration passes the value-side gate. See [design/typing/tokens.md](../../../design/typing/tokens.md).
    fn partition_guard(&self, name: &str, into: BindKind) -> Result<(), KError> {
        let is_type_token = crate::parse::is_type_name(name);
        match into {
            // A signature's slot table keys its value slots by their value names; it is a schema,
            // not a binding universe.
            BindKind::Type if !is_type_token && self.slot_table => Ok(()),
            BindKind::Type if !is_type_token => Err(KError::new(KErrorKind::ShapeError(format!(
                "`{name}` is a value token, so it names a value — a type binds under a Type token \
                 (uppercase-leading with at least one lowercase letter)"
            )))),
            BindKind::Value if is_type_token => Err(KError::new(KErrorKind::ShapeError(format!(
                "`{name}` is a Type token, so it names a type — a value binds under a value token \
                 (snake_case)"
            )))),
            _ => Ok(()),
        }
    }

    /// Shared write path for type-only bindings.
    /// `Conflict` is borrow contention; `Err(Rebind)` is semantic rejection.
    fn try_apply_type(
        &self,
        name: &str,
        kt: &'a KType<'a>,
        index: BindingIndex,
        reach: StoredReach<'a>,
    ) -> Result<ApplyOutcome, KError> {
        self.partition_guard(name, BindKind::Type)?;
        let mut types = match self.types.try_borrow_mut() {
            Ok(t) => t,
            Err(_) => return Ok(ApplyOutcome::Conflict),
        };
        if types.contains_key(name) {
            return Err(KError::new(KErrorKind::Rebind {
                name: name.to_string(),
            }));
        }
        // Cross-kind exclusion: a type name may not collide with a committed value. `types`
        // is already held, so probing `data` next preserves the `types → data` borrow order.
        match self.data.try_borrow() {
            Ok(data) => {
                if data.contains_key(name) {
                    return Err(KError::new(KErrorKind::Rebind {
                        name: name.to_string(),
                    }));
                }
            }
            Err(_) => return Ok(ApplyOutcome::Conflict),
        }
        types.insert(name.to_string(), (kt, index, reach));
        drop(types);
        self.clear_placeholder_best_effort(name, BindKind::Type);
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
        reach: StoredReach<'a>,
    ) -> Result<ApplyOutcome, KError> {
        // Cross-kind exclusion: a value name may not collide with a committed type — the
        // `data`/`types` partition is structural, not convention. Probe `types` first (borrow
        // order `types → functions → data`); a bare-`FN` registration (`write_data == false`)
        // binds no value, so it is exempt from both this and the token-class gate.
        if write_data {
            self.partition_guard(name, BindKind::Value)?;
            match self.types.try_borrow() {
                Ok(types) => {
                    if types.contains_key(name) {
                        return Err(KError::new(KErrorKind::Rebind {
                            name: name.to_string(),
                        }));
                    }
                }
                Err(_) => return Ok(ApplyOutcome::Conflict),
            }
        }
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
            if let Some((existing, _, _)) = data.get(name) {
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
            data.insert(name.to_string(), (obj, index, reach));
        }
        drop(data);
        drop(functions_handle);
        self.clear_placeholder_best_effort(name, BindKind::Value);
        if let Some(bucket) = cleared_overload_bucket {
            // Remove only this binder's pending entry; siblings stay as wake sources.
            self.clear_pending_overload_best_effort(&bucket, index);
        }
        Ok(ApplyOutcome::Applied)
    }

    /// Shared tail of every successful write path. Removes a *matching-kind* placeholder
    /// for `name`: a value write clears only a [`BindKind::Value`] entry, a type write only
    /// a [`BindKind::Type`] one, so a value bind never clears an in-flight type producer's
    /// placeholder (or the reverse). `try_borrow_mut().ok()` tolerates a caller holding a
    /// placeholder borrow up the stack — a hard `borrow_mut()` would panic on legitimate
    /// reads across a write.
    fn clear_placeholder_best_effort(&self, name: &str, kind: BindKind) {
        if let Ok(mut ph) = self.placeholders.try_borrow_mut() {
            if matches!(ph.get(name), Some((_, _, k)) if *k == kind) {
                ph.remove(name);
            }
        }
    }

    /// Remove every value-side placeholder pointing at `producer`. The success write
    /// paths clear a binder's placeholder by name on finalize; this is the error-path
    /// companion, called when `producer`'s node finalizes with an error so a binder body
    /// that failed before its write path does not leak a scheduler-local [`NodeId`] into
    /// a later run on a persistent scope. Same tolerant `try_borrow_mut`.
    pub fn clear_placeholders_for_producer(&self, producer: NodeId) {
        if let Ok(mut ph) = self.placeholders.try_borrow_mut() {
            ph.retain(|_, (id, _, _)| *id != producer);
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
