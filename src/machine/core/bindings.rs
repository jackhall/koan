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
//! Every entry carries a [`BindingIndex`] naming its installing statement's lexical
//! position, gated by the strict cutoff `idx < c`, so a forward reference (a
//! later-positioned binding) is invisible — type binders included. A type entry pairs
//! that index with its installing [`NodeHandle`] in a [`DeclarationSite`]: the handle
//! alone answers the same-declaration question, and the index there does visibility
//! only. `idx == 0` is the
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
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::machine::core::carrier_witness::{CallableIdentity, FunctionMirror, SealedFunction};
use crate::machine::core::kfunction::NodeId;
use crate::machine::core::RunId;
use crate::machine::model::CarriedFamily;
use crate::machine::model::DispatchToken;
use crate::machine::model::OperatorGroup;
use crate::machine::model::TypeIdentifier;
use crate::machine::model::{KType, UntypedKey};
use crate::machine::CarrierWitness;
use crate::witnessed::Sealed;

use super::kerror::{KError, KErrorKind};

/// A value binding's dormant carrier: the bound value fused to the exact reach description minted
/// for it at bind time. The entry owns no pins — the binding scope's **region** owns the one deduped
/// union bundle that keeps every reached region alive for the region's life, so a read hands out a
/// bit-copy of this seal with no refcount traffic and the value can only be re-anchored under a pin
/// ([`Sealed::open_at`], the [`Delivered`](crate::machine::DeliveredCarried) lift).
pub type SealedValue = Sealed<CarriedFamily, CarrierWitness>;

pub use crate::machine::model::BindKind;

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

/// A value binding entry: its lexical [`BindingIndex`], the dormant [`SealedValue`] carrier fusing
/// the bound value with the exact reach description minted for it, and the dispatch bucket the
/// callable that value wraps was mirrored into, if any.
///
/// The entry owns **nothing**: liveness for every region the value reaches lives in the binding
/// scope's region-owned union bundle
/// ([`Scope::retain_reach`](crate::machine::core::Scope::retain_reach)), folded in at bind time and
/// dropped whole at region death. Bindings are bind-once and an entry never dies before its scope,
/// so region death and entry death are the same schedule — the entry is `Copy`-cheap to read out
/// and carries no `Drop`. Fusing value and reach in the seal keeps the write door from ever pairing
/// a value with a reach derived for a different value.
pub(crate) struct DataEntry {
    index: BindingIndex,
    sealed: SealedValue,
    /// The dispatch bucket the value's callable was mirrored into, or `None` when the value wraps
    /// no callable. The mirror **seal** is stored once — in that bucket, which is what dispatch
    /// reads — so this side keeps only the classification: the overload-add rebind gate and the
    /// placeholder leniency probe ask whether the entry is callable, never what it is.
    callable: Option<UntypedKey>,
}

impl DataEntry {
    /// A bit-copy of the entry — the dormant seal duplicated (value bit-copy + reference-only
    /// witness clone, no refcount traffic) beside the `Copy` index and the bucket key. Every read
    /// hands one of these out so no caller holds the `data` `RefCell` borrow across a carrier build.
    /// The one reader is the bulk-install snapshot, which is ascription-only.
    #[cfg_attr(not(feature = "ascription"), allow(dead_code))]
    fn duplicate(&self) -> Self {
        DataEntry {
            index: self.index,
            sealed: self.sealed.duplicate(),
            callable: self.callable.clone(),
        }
    }
}

/// One finalized overload in a dispatch bucket: the dormant callable carrier plus the plain data
/// the write path dedupes on, all of it computed at mirror-seal time
/// ([`FunctionMirror`]) where the callable was open. Overloads sharing a bucket sit at different
/// lexical positions, so each carries its own [`BindingIndex`] and the dispatch picker filters
/// per-overload.
pub(crate) struct FunctionBucketEntry {
    pub(crate) index: BindingIndex,
    /// The stored form of the duplicate-overload predicate: token equality against an incoming
    /// callable is `DuplicateOverload`.
    token: DispatchToken,
    /// Address identity, compared against an incoming callable's to spot an intentional alias.
    identity: CallableIdentity,
    /// The overload's rendered signature, for the `DuplicateOverload` diagnostic.
    summary: String,
    pub(crate) sealed: SealedFunction,
}

/// The value-or-type a name resolves to in one classified result — for ATTR module/signature
/// member access. Produced by [`crate::machine::core::Scope::lookup_member`], which checks the
/// module-own value side then the type side in one call. The `data`/`types` cross-kind exclusion
/// keeps the two arms from ever both matching within a scope.
pub enum MemberResolution {
    /// The member's dormant carrier, duplicated off the module's own `data` entry — so an ATTR read
    /// replays the *stored* claim (value and reach as one unit) rather than re-asserting
    /// single-frame co-location.
    Value(SealedValue),
    Type {
        /// The member type as a `Copy` handle — interned in the run frame's registry, so an ATTR
        /// type read copies the handle with no reach to replay.
        kt: KType,
    },
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
pub struct FunctionLookup {
    /// The visible finalized overloads, each a bit-copy of the bucket's dormant carrier — value and
    /// proven reach as one unit, re-anchored only by an [`open`](crate::witnessed::Sealed::open_at)
    /// under a named pin. Copied out so no caller holds the `functions` borrow across a candidate
    /// walk.
    pub overloads: Vec<SealedFunction>,
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

/// The scheduler slot that installed a binding, qualified by its run: [`NodeId`]s are
/// scheduler-local and restart per runtime, so only the pair identifies a declaration
/// statement across the lifetime of a persistent scope.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct NodeHandle {
    pub run: RunId,
    pub node: NodeId,
}

/// The identity of the declaration statement that installed a `types` entry: the installing
/// slot (the identity signal — same-declaration checks compare only this) plus its lexical
/// position (the visibility signal — `idx < cutoff` reads it; under a detached chain the
/// index is 0 and deliberately names no statement).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct DeclarationSite {
    pub node: NodeHandle,
    pub index: BindingIndex,
}

impl DeclarationSite {
    /// Off-scheduler builtin registration: no slot installed it.
    pub const BUILTIN: DeclarationSite = DeclarationSite {
        node: NodeHandle {
            run: RunId::OFF_SCHEDULER,
            node: NodeId(0),
        },
        index: BindingIndex::BUILTIN,
    };
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
    /// Each type entry stores its bound type and its [`DeclarationSite`] — the installing
    /// [`NodeHandle`] (declaration identity) plus its lexical [`BindingIndex`] (visibility). A
    /// `KType` is a `Copy` handle into the run frame's registry, so an entry carries no reach: a
    /// read copies the handle under the home-frame pin alone, and the same handle names the same
    /// type in every region.
    types: RefCell<HashMap<String, (KType, DeclarationSite)>>,
    /// Each value entry stores its bound value fused to its exact reach in one dormant
    /// [`SealedValue`], plus its lexical [`BindingIndex`]. Reads hand out a bit-copy of the seal
    /// ([`Self::lookup_value`]) and re-anchor the value only under a pin, so a read replays the
    /// stored claim rather than re-asserting single-frame co-location. Description members are
    /// `Weak`, and the owning pins live in the region's union bundle rather than here — either a
    /// strong member or a per-entry `Rc` on the scope's own frame would close a
    /// `frame → region → scope → bindings → frame` cycle and leak the region.
    data: RefCell<HashMap<String, DataEntry>>,
    /// Each dispatch-bucket entry stores its callable fused to the same exact reach its mirrored
    /// `data` entry claims, in one dormant [`SealedFunction`], beside the precomputed data the
    /// write path dedupes on ([`FunctionBucketEntry`]). This is the **only** place a mirror seal is
    /// stored — bare `FN` has no `data` entry and dispatch reads buckets. Like `data`, the entry
    /// owns nothing: the reached regions are held by the region's union bundle, and a read hands
    /// out a bit-copy the caller re-anchors under a pin.
    functions: RefCell<HashMap<UntypedKey, Vec<FunctionBucketEntry>>>,
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
    /// earlier still-finalizing type parks on its producer node; this set marks which names
    /// are in flight. See [`pending`] for the surface methods.
    pending: PendingTypes,
    /// Scope-bound `TypeIdentifier` → `KType` resolution cache. Monotonic — entries are written
    /// only when the elaborated `KType` and every user-type it references are fully
    /// finalized; the finalize gate prevents caching a not-yet-sealed type.
    /// Keyed by `(TypeIdentifier, chain cutoff)`: a forward consumer (smaller cutoff) and a
    /// backward consumer (larger cutoff) at the same scope resolve the same name to
    /// different verdicts under lexical gating, so they must not share a cache entry.
    type_identifier_memo: RefCell<HashMap<(TypeIdentifier, Option<usize>), TypeMemoEntry>>,
}

/// A `type_identifier_memo` value: the cached `KType` handle, so a memo hit rebuilds the read
/// carrier without re-walking the chain.
type TypeMemoEntry = KType;

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
    ) -> Option<KType> {
        self.type_identifier_memo
            .borrow()
            .get(&(te.clone(), cutoff))
            .copied()
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
    ) -> Option<NameLookup<SealedValue>> {
        if let Some(entry) = self.data.borrow().get(name) {
            if Self::visible(entry.index, chain_cutoff) {
                return Some(NameLookup::Bound(entry.sealed.duplicate()));
            }
        }
        self.value_placeholder(name, chain_cutoff)
            .map(NameLookup::Parked)
    }

    /// Whether a **visible** value entry named `name` exists at this scope — the presence-only
    /// probe, for a write gate that must not read the bound value (the USING-window collision
    /// check). A still-finalizing placeholder is not an entry and reads `false`.
    pub fn has_value(&self, name: &str, chain_cutoff: Option<usize>) -> bool {
        self.data
            .borrow()
            .get(name)
            .is_some_and(|entry| Self::visible(entry.index, chain_cutoff))
    }

    /// The value-side placeholder producer for `name`, or `None` — the placeholder arm
    /// [`Self::lookup_value`] falls through to.
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
    ) -> Option<NameLookup<KType>> {
        if let Some((kt, site)) = self.types.borrow().get(name) {
            if Self::visible(site.index, chain_cutoff) {
                return Some(NameLookup::Bound(*kt));
            }
        }
        self.type_placeholder(name, chain_cutoff)
            .map(NameLookup::Parked)
    }

    /// The type-side placeholder producer for `name`, or `None` — the placeholder arm
    /// [`Self::lookup_type`] falls through to.
    fn type_placeholder(&self, name: &str, chain_cutoff: Option<usize>) -> Option<NodeId> {
        if let Some((id, idx, kind)) = self.placeholders.borrow().get(name).copied() {
            if kind == BindKind::Type && Self::visible(idx, chain_cutoff) {
                return Some(id);
            }
        }
        None
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
    ) -> Option<MemberResolution> {
        if let Some(entry) = self.data.borrow().get(name) {
            if Self::visible(entry.index, chain_cutoff) {
                return Some(MemberResolution::Value(entry.sealed.duplicate()));
            }
        }
        if let Some((kt, site)) = self.types.borrow().get(name) {
            if Self::visible(site.index, chain_cutoff) {
                return Some(MemberResolution::Type { kt: *kt });
            }
        }
        None
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
    pub fn lookup_function(&self, key: &UntypedKey, chain_cutoff: Option<usize>) -> FunctionLookup {
        let overloads: Vec<SealedFunction> = self
            .functions
            .borrow()
            .get(key)
            .map(|bucket| {
                bucket
                    .iter()
                    .filter(|entry| Self::visible(entry.index, chain_cutoff))
                    .map(|entry| entry.sealed.duplicate())
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

    /// Snapshot every `(name, dormant carrier)` pair in `data`, ignoring visibility. Each seal is a
    /// bit-copy; the caller re-anchors what it needs under its own pin. For chain-gated single-name
    /// reads use [`Self::lookup_value`].
    pub fn iter_data(&self) -> Vec<(String, SealedValue)> {
        self.data
            .borrow()
            .iter()
            .map(|(name, entry)| (name.clone(), entry.sealed.duplicate()))
            .collect()
    }

    /// Snapshot every `(name, KType)` pair in `types`, ignoring visibility.
    pub fn iter_types(&self) -> Vec<(String, KType)> {
        self.types
            .borrow()
            .iter()
            .map(|(name, (kt, _site))| (name.clone(), *kt))
            .collect()
    }

    /// Snapshot every `(UntypedKey, Vec<SealedFunction>)` pair in `functions`, ignoring per-overload
    /// visibility. Each seal is a bit-copy; the caller re-anchors what it needs under its own pin.
    /// For chain-gated picks use [`Self::lookup_function`].
    pub fn iter_functions(&self) -> Vec<(UntypedKey, Vec<SealedFunction>)> {
        self.functions
            .borrow()
            .iter()
            .map(|(key, bucket)| {
                (
                    key.clone(),
                    bucket
                        .iter()
                        .map(|entry| entry.sealed.duplicate())
                        .collect(),
                )
            })
            .collect()
    }

    /// True iff `types[name]` was registered at [`BindingIndex::BUILTIN`]. The
    /// no-shadow consult gates on this — a genuine builtin, not a user type that a
    /// synthetic test happens to have placed in a root-position scope.
    pub fn has_builtin_type(&self, name: &str) -> bool {
        self.types
            .borrow()
            .get(name)
            .is_some_and(|(_, site)| site.index == BindingIndex::BUILTIN)
    }

    /// True iff `functions[key]` holds an overload registered at
    /// [`BindingIndex::BUILTIN`] — a genuine builtin dispatch bucket, distinct from a
    /// user bucket the no-shadow consult must not gate.
    pub fn has_builtin_function(&self, key: &UntypedKey) -> bool {
        self.functions
            .borrow()
            .get(key)
            .is_some_and(|bucket| bucket.iter().any(|e| e.index == BindingIndex::BUILTIN))
    }

    /// Visibility predicate: `None` ⇒ everything visible; `Some(c)` ⇒ `b.idx < c`.
    /// Mirrors [`crate::machine::core::scope::visible`].
    fn visible(b: BindingIndex, chain_cutoff: Option<usize>) -> bool {
        match chain_cutoff {
            None => true,
            Some(c) => b.idx < c,
        }
    }

    /// Insert `(te → kt)` into the resolution cache. Caller region-allocates `kt` and gates on
    /// finalize. Monotonic: a collision means equal values, so we keep the existing entry rather
    /// than panic.
    pub fn type_identifier_memo_insert(
        &self,
        te: TypeIdentifier,
        cutoff: Option<usize>,
        kt: KType,
    ) {
        let mut memo = self.type_identifier_memo.borrow_mut();
        memo.entry((te, cutoff)).or_insert(kt);
    }

    #[cfg(test)]
    pub(crate) fn data(&self) -> Ref<'_, HashMap<String, DataEntry>> {
        self.data.borrow()
    }

    #[cfg(test)]
    pub(crate) fn functions(&self) -> Ref<'_, HashMap<UntypedKey, Vec<FunctionBucketEntry>>> {
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
    pub fn types(&self) -> Ref<'_, HashMap<String, (KType, DeclarationSite)>> {
        self.types.borrow()
    }

    #[cfg(test)]
    pub fn expect_type(&self, name: &str) -> KType {
        self.types
            .borrow()
            .get(name)
            .map(|(kt, _site)| *kt)
            .unwrap_or_else(|| panic!("expected bindings.types[{name:?}] to be present"))
    }

    /// In-flight named-type binder names. The sole non-test writer is
    /// [`Bindings::insert_pending_type`] (the guard's Drop removes the name); a consumer
    /// reads it to decide whether to park on an earlier still-finalizing type.
    pub fn pending_types(&self) -> Ref<'_, HashSet<String>> {
        self.pending.get()
    }

    pub fn insert_pending_type(&self, name: String) -> PendingBinderGuard {
        self.pending.insert(name)
    }

    /// Exercises the guard Drop's "tolerates absent entry" path.
    #[cfg(test)]
    pub fn pending_remove(&self, name: &str) {
        self.pending.remove(name);
    }

    /// LET-style value bind. Errors `Rebind` if `data[name]` already exists, or if `name`
    /// is a committed type (`types[name]`) — the value/type partition is mutually exclusive.
    /// When the bound value wraps a `KFunction` it is also mirrored into
    /// `functions[signature.untyped_key()]` so dispatch finds it (`LET f = (FN ...)`).
    ///
    /// The value and its exact reach arrive fused in one [`SealedValue`] — the write door cannot
    /// pair a value with a reach derived for a different value. `mirror` is that same value's
    /// **mirror bundle** ([`Scope::seal_function_mirror`](crate::machine::core::Scope)): the
    /// callable's seal plus the bucket key, dedupe token and identity computed where the callable
    /// was open, so the bucket write re-anchors nothing itself.
    ///
    /// `Conflict` means borrow contention (caller queues); `Err` is semantic rejection.
    pub(crate) fn try_bind_value(
        &self,
        name: &str,
        index: BindingIndex,
        sealed: SealedValue,
        mirror: Option<FunctionMirror>,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply(name, mirror, index, Some(sealed))
    }

    /// Bare-`FN` overload registration: adds `fn_ref` to the `functions`
    /// bucket only — `data[name]` is left untouched, so a bare FN keyword is
    /// dispatchable but not nameable as a value (use `LET f = (FN …)` for that).
    /// Errors `DuplicateOverload` on an exact-signature collision.
    ///
    /// Per-overload `index` tagging matters because overloads sharing a bucket
    /// can sit at different lexical positions (the dispatch picker filters
    /// per-overload).
    pub(crate) fn try_register_function(
        &self,
        name: &str,
        mirror: FunctionMirror,
        index: BindingIndex,
    ) -> Result<ApplyOutcome, KError> {
        // A bare-`FN` registration writes `functions` only, not `data`: no sealed value, so no
        // entry lands and nothing is minted or retained.
        self.try_apply(name, Some(mirror), index, None)
    }

    /// Register `name` → `kt` in `types`. Errors `Rebind` if already present in `types`, or
    /// if `name` is a committed value (`data[name]`) — the partition is mutually exclusive.
    /// `Ok(Conflict)` on borrow contention. Best-effort placeholder clear on success.
    pub fn try_register_type(
        &self,
        name: &str,
        kt: KType,
        site: DeclarationSite,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply_type(name, kt, site)
    }

    /// Upsert `name` → `kt` in `types` for nominal finalize. Declaration identity is the
    /// installing [`NodeHandle`]: an existing entry whose handle differs from `site`'s is a
    /// different declaration of the name — `Err(Rebind)` — as is a committed value at `data[name]`
    /// (the value/type partition is mutually exclusive). A same-handle hit is the same slot in the
    /// same run re-entering (a parallel finalize), whose re-elaboration cannot differ, so it
    /// overwrites idempotently. Content plays no part in the same-declaration decision.
    ///
    /// Distinct from [`Self::try_register_type`], whose strict insert-if-absent arm would `Rebind`
    /// on a parallel finalize rather than overwrite it.
    /// `Ok(Conflict)` on borrow contention. Best-effort placeholder clear on success.
    pub fn try_register_type_upsert(
        &self,
        name: &str,
        kt: KType,
        site: DeclarationSite,
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
        match types.get(name).map(|(_, s)| *s) {
            Some(existing) if existing.node != site.node => {
                return Err(KError::new(KErrorKind::Rebind {
                    name: name.to_string(),
                }));
            }
            // Absent, or the same slot in the same run re-entering (a parallel finalize): write the
            // identity, an idempotent overwrite.
            _ => {
                types.insert(name.to_string(), (kt, site));
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
        if let Some(entry) = self.data.borrow().get(&name) {
            if entry.callable.is_some() {
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
    ///
    /// A replayed entry stores only its callable's bucket key, so the mirror bundle the bucket
    /// write needs is re-derived per entry by `mirror_of` — the `Scope` layer's
    /// [`seal_function_mirror`](crate::machine::core::Scope::seal_function_mirror), the one place
    /// that holds a pin to open the seal under.
    #[cfg_attr(not(feature = "ascription"), allow(dead_code))]
    pub(crate) fn try_bulk_install_from(
        &self,
        src: &Bindings<'a>,
        mirror_of: impl Fn(&SealedValue) -> Option<FunctionMirror>,
    ) -> Result<(), KError> {
        // Duplicate each entry into the snapshot: the seal is a bit-copy naming the source's own
        // minted description, so the replayed entry replays that same claim. The reached regions
        // stay owned by the *source* scope's region union — the replay target's own region must
        // already outlive it (a bulk install is same-run re-entrant ascription).
        let snapshot: Vec<(String, DataEntry)> = src
            .data
            .borrow()
            .iter()
            .map(|(k, entry)| (k.clone(), entry.duplicate()))
            .collect();
        for (name, entry) in snapshot {
            let index = entry.index;
            let mirror = mirror_of(&entry.sealed);
            match self.try_apply(&name, mirror, index, Some(entry.sealed))? {
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
    /// against the map it is entering is a hard error, not a convention, with no exception: every
    /// value-token write to `types` and every Type-token write to `data` is rejected. This is the single
    /// enforcement point: every binder reaches its map through [`Bindings::try_apply`] /
    /// [`Bindings::try_apply_type`], so no caller can bind across the line, and none needs its own check.
    /// A keyword-class name (all-uppercase, no lowercase) is not a Type token, so a builtin's dispatch
    /// registration passes the value-side gate. See [design/typing/tokens.md](../../../design/typing/tokens.md).
    fn partition_guard(&self, name: &str, into: BindKind) -> Result<(), KError> {
        let is_type_token = crate::parse::is_type_name(name);
        match into {
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
        kt: KType,
        site: DeclarationSite,
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
        types.insert(name.to_string(), (kt, site));
        drop(types);
        self.clear_placeholder_best_effort(name, BindKind::Type);
        Ok(ApplyOutcome::Applied)
    }

    /// Shared write path for `data`/`functions`. Borrows `functions` first
    /// (only when `mirror.is_some()`), then `data` — skipping the `functions`
    /// borrow otherwise keeps non-fn binds deadlock-free under callers that
    /// hold a live outer `functions` borrow.
    ///
    /// `sealed`: `Some` for value-carrying paths (LET, LET-binds-FN) — the fused value+reach the
    /// entry stores; `None` for bare-`FN` (dispatch-only, no `data` insert). The only combinations
    /// that occur are `(None, Some)`, `(Some, Some)`, `(Some, None)`.
    ///
    /// `mirror` arrives as plain data beside its dormant seal: the bucket key, dedupe token and
    /// address identity were all computed at mirror-seal time, where the callable was open. The
    /// bucket write is therefore pure table mutation — it stores the mirror's seal, so
    /// `functions[key]` and `data[name]` carry one claim about one value, and no carrier is opened
    /// and no bare reference crosses the door.
    ///
    /// Dedupe when `mirror.is_some()`: identity equality is a silent-success short-circuit
    /// (preserves intentional aliases like `LET g = (f)`); token equality raises
    /// `DuplicateOverload`.
    fn try_apply(
        &self,
        name: &str,
        mirror: Option<FunctionMirror>,
        index: BindingIndex,
        sealed: Option<SealedValue>,
    ) -> Result<ApplyOutcome, KError> {
        let write_data = sealed.is_some();
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
        let mut functions_handle = if mirror.is_some() {
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
        // `mirror.is_some()` + existing `KFunction` falls through to bucket dedupe
        // (overload-add path); everything else is a rebind error.
        if let Some(data) = data.as_ref() {
            if let Some(existing) = data.get(name) {
                match mirror {
                    None => {
                        return Err(KError::new(KErrorKind::Rebind {
                            name: name.to_string(),
                        }))
                    }
                    Some(_) => {
                        if existing.callable.is_none() {
                            return Err(KError::new(KErrorKind::Rebind {
                                name: name.to_string(),
                            }));
                        }
                    }
                }
            }
        }
        // The bucket this write touched, if any: the `data` entry's callable classification and the
        // pending-overload clear both name it.
        let mut bucket_key: Option<UntypedKey> = None;
        if let (Some(mirror), Some(functions)) = (mirror, functions_handle.as_mut()) {
            let bucket = functions.entry(mirror.key.clone()).or_default();
            let verdict = bucket.iter().find_map(|existing| {
                if existing.identity == mirror.identity {
                    // The same callable is already registered (an intentional alias like
                    // `LET g = (f)`), so the bucket keeps its first entry.
                    Some(Ok(()))
                } else if existing.token == mirror.token {
                    Some(Err(existing.summary.clone()))
                } else {
                    None
                }
            });
            match verdict {
                Some(Err(signature)) => {
                    return Err(KError::new(KErrorKind::DuplicateOverload {
                        name: name.to_string(),
                        signature,
                    }));
                }
                Some(Ok(())) => {}
                None => bucket.push(FunctionBucketEntry {
                    index,
                    token: mirror.token,
                    identity: mirror.identity,
                    summary: mirror.summary,
                    sealed: mirror.sealed,
                }),
            }
            bucket_key = Some(mirror.key);
        }
        if let (Some(data), Some(sealed)) = (data.as_mut(), sealed) {
            data.insert(
                name.to_string(),
                DataEntry {
                    index,
                    sealed,
                    callable: bucket_key.clone(),
                },
            );
        }
        drop(data);
        drop(functions_handle);
        self.clear_placeholder_best_effort(name, BindKind::Value);
        if let Some(bucket) = bucket_key {
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

// In-flight named-type binder tracking. [`Bindings`] embeds a [`PendingTypes`] by value and
// delegates the surface methods. A binder records its name here for its body's duration so a
// consumer referencing an *earlier* still-finalizing type can find the producer node to park
// on (the finalize gate in `resolve_type_identifier`). Membership is the whole signal. MODULE
// does not participate — module bodies park on the outer scheduler, not on type-name resolution
// inside elaboration.

/// The in-flight binder set, `Rc`-shared so a [`PendingBinderGuard`] can hold an *owning* stake
/// in it rather than a borrow: the guard outlives the `&Scope` borrow it was minted from (it rides
/// into the binder's combine finish and drops there), so refcounting — not a lifetime — is what
/// keeps the set alive for the guard's Drop. Interior mutation stays sound via the inner `RefCell`.
type PendingMap = Rc<RefCell<HashSet<String>>>;

pub struct PendingTypes {
    map: PendingMap,
}

impl PendingTypes {
    pub fn new() -> Self {
        Self {
            map: Rc::new(RefCell::new(HashSet::new())),
        }
    }

    pub fn get(&self) -> Ref<'_, HashSet<String>> {
        self.map.borrow()
    }

    /// Record a binder as in-flight and return an RAII guard whose Drop removes
    /// the name.
    ///
    /// Panics on borrow conflict — pending-type writes happen at body-entry,
    /// outside the re-entrant `try_apply` hot path. Panics on duplicate name —
    /// placeholders should block a second dispatch from reaching body-entry.
    pub fn insert(&self, name: String) -> PendingBinderGuard {
        let mut map = self.map.borrow_mut();
        if map.contains(&name) {
            panic!(
                "insert_pending_type = `{name}` already in flight — duplicate dispatch \
                 reached body-entry, which the placeholder install should have blocked",
            );
        }
        map.insert(name.clone());
        drop(map);
        PendingBinderGuard {
            map: Rc::clone(&self.map),
            name,
        }
    }

    #[cfg(test)]
    pub fn remove(&self, name: &str) {
        self.map.borrow_mut().remove(name);
    }
}

impl Default for PendingTypes {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII handle returned by [`PendingTypes::insert`]. Dropping the guard removes
/// the matching name; this is the *only* removal path outside `#[cfg(test)]`.
///
/// `try_borrow_mut` in Drop is defensive: no caller is expected to hold the
/// pending-types borrow when a guard drops. Silent skip is safe — the name
/// persists until the next drain point, and no later code observes a stale
/// name once the matching binder has finalized.
#[must_use = "PendingBinderGuard removes the pending-types entry on drop; \
              bind it for the elaboration's lifetime"]
pub struct PendingBinderGuard {
    map: PendingMap,
    name: String,
}

impl Drop for PendingBinderGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = self.map.try_borrow_mut() {
            map.remove(&self.name);
        }
    }
}

#[cfg(test)]
mod tests;
