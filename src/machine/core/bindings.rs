//! Lexical binding façade: one `RefCell<Tables>` — `types`, `data`, `functions`, `operators` —
//! behind validated write paths. A still-finalizing binder occupies a *slot of its destination
//! table* ([`ValueSlot::Pending`], [`TypeSlot::Pending`], [`OverloadSlot::Pending`]), so a lookup
//! answers "bound or parked" from one probe and finalization overwrites that slot in place rather
//! than moving the entry between containers. `data` and `functions` are
//! separate surfaces: a `data` entry is a value binding, callable by **name** alone (the
//! `FunctionValueCall` lane), while a `functions` bucket holds the keyworded overloads that only
//! the `FN` / `OP` registration doors and the builtin seeds write — binding a function *value*
//! publishes no keyworded expression. Nominal type
//! declarations (NEWTYPE / UNION / SIG) install their identity into `types`
//! only — there is no value-side carrier; a module is a value and binds into
//! `data`. The `data` and `types` maps
//! are a structural partition: a name is committed to one xor the other, never
//! both, enforced by the cross-kind check the value and type write paths run.
//!
//! Every write verb here takes a [`WriteGate`] — the zero-sized capability minted only inside
//! `crate::machine`, at the run loop's door and the construction-time door for scopes no other
//! node can reach. A builtin body cannot mint one, so it cannot name a write verb: "one path
//! mutates a published table" is a resolution rule, not a convention. That is what lets the verbs
//! take firm `borrow_mut`s — no koan frame is on the stack to hold a competing borrow, so
//! contention is unrepresentable. See [`gate`] for the capability, [`ops`] for the currency, and
//! [design/memory-model.md](../../../design/memory-model.md).
//!
//! There is no borrow order to keep: reads never overlap the single gated write site, so every
//! verb takes exactly one borrow of the one cell and a cross-map write is atomic under it.
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
//! [`crate::machine::core::Scope::resolve_operator_group_delivered`]).
//!
//! Production reads use the visibility-aware [`Bindings::lookup_value`] /
//! [`Bindings::lookup_type`] / [`Bindings::lookup_function`], passing a
//! `chain_cutoff` computed via [`crate::machine::core::LexicalFrame::index_for`].
//! Raw map accessors are `#[cfg(test)]`.

#[cfg(test)]
use std::cell::Ref;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::machine::core::carrier_witness::{
    GroupSeal, OverloadSeal, SealedFunction, SealedOperatorGroup,
};
use crate::machine::core::kfunction::NodeId;
use crate::machine::core::RunId;
use crate::machine::model::CarriedFamily;
use crate::machine::model::DispatchToken;
use crate::machine::model::{KType, UntypedKey};
use crate::machine::CarrierWitness;
use crate::witnessed::Sealed;

use super::kerror::{KError, KErrorKind};

mod gate;
mod ops;

pub use gate::WriteGate;
pub(crate) use ops::{powerset_probes, TypeWritePolicy, WriteOp};

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
/// Invariant: within one scope a value name is bound xor pending, never both — the two are arms of
/// one [`ValueSlot`], so the exclusivity is a type-level fact rather than cross-map discipline.
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

/// A still-finalizing binder occupying its destination slot: the producer node a consumer parks
/// on, tagged with the binder's lexical [`BindingIndex`] so the same visibility predicate gates a
/// pending arm and the binding it becomes. Installed at statement submission
/// ([`Bindings::install_placeholder`] / [`Bindings::install_pending_overload`]) and overwritten in
/// place by the producer's write path.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PendingBinding {
    pub producer: NodeId,
    pub index: BindingIndex,
}

/// One `data` slot: bound, or claimed by a still-finalizing binder. The two are exclusive by
/// construction — a value name is never pending and bound at once, and the enum is what says so.
pub(crate) enum ValueSlot {
    Bound(DataEntry),
    Pending(PendingBinding),
}

impl ValueSlot {
    /// The committed entry, or `None` for a still-finalizing binder.
    pub(crate) fn bound(&self) -> Option<&DataEntry> {
        match self {
            ValueSlot::Bound(entry) => Some(entry),
            ValueSlot::Pending(_) => None,
        }
    }

    /// The in-flight producer, or `None` once the slot is committed.
    pub(crate) fn pending(&self) -> Option<PendingBinding> {
        match self {
            ValueSlot::Bound(_) => None,
            ValueSlot::Pending(p) => Some(*p),
        }
    }
}

/// One `types` slot. Unlike [`ValueSlot`], bound and pending are **not** exclusive: a parallel
/// nominal finalize pre-installs the name's external identity while its producer is still in
/// flight, and the finalize gate parks on that producer
/// ([`Bindings::type_placeholder_producer`]). The third arm makes that coexistence — and the
/// impossibility of an empty slot — type-level facts, so a reader cannot mistake the slot for an
/// exclusive one. Reads go through [`Self::bound`] / [`Self::pending`]; only the three transition
/// sites match the arms directly.
pub(crate) enum TypeSlot {
    Bound(KType, DeclarationSite),
    Pending(PendingBinding),
    BoundWithPending(KType, DeclarationSite, PendingBinding),
}

impl TypeSlot {
    /// The bound identity, if any — the `Bound` and `BoundWithPending` arms.
    pub(crate) fn bound(&self) -> Option<(KType, DeclarationSite)> {
        match self {
            TypeSlot::Bound(kt, site) | TypeSlot::BoundWithPending(kt, site, _) => {
                Some((*kt, *site))
            }
            TypeSlot::Pending(_) => None,
        }
    }

    /// The in-flight producer, if any — the `Pending` and `BoundWithPending` arms.
    pub(crate) fn pending(&self) -> Option<PendingBinding> {
        match self {
            TypeSlot::Pending(p) | TypeSlot::BoundWithPending(_, _, p) => Some(*p),
            TypeSlot::Bound(..) => None,
        }
    }
}

/// One slot of a dispatch bucket: a finalized overload, or a sibling FN binder still finalizing
/// that consumers park on. Both live in the same `Vec` because both answer the same lookup — a
/// bucket legitimately holds sealed overloads and pending siblings at once.
pub(crate) enum OverloadSlot {
    Sealed(FunctionBucketEntry),
    Pending(PendingBinding),
}

impl OverloadSlot {
    /// The finalized overload, or `None` for a still-finalizing sibling.
    pub(crate) fn sealed(&self) -> Option<&FunctionBucketEntry> {
        match self {
            OverloadSlot::Sealed(entry) => Some(entry),
            OverloadSlot::Pending(_) => None,
        }
    }

    /// The in-flight producer, or `None` for a finalized overload.
    pub(crate) fn pending(&self) -> Option<PendingBinding> {
        match self {
            OverloadSlot::Sealed(_) => None,
            OverloadSlot::Pending(p) => Some(*p),
        }
    }
}

/// A value binding entry: its lexical [`BindingIndex`] and the dormant [`SealedValue`] carrier
/// fusing the bound value with the exact reach description minted for it.
///
/// The entry owns **nothing**: liveness for every region the value reaches lives in the binding
/// scope's region-owned union bundle, folded in by the mint that derived the entry's description
/// ([`Scope::mint_retained`](crate::machine::core::Scope::mint_retained)) and dropped whole at
/// region death. Bindings are bind-once and an entry never dies before its scope,
/// so region death and entry death are the same schedule — the entry is `Copy`-cheap to read out
/// and carries no `Drop`. Fusing value and reach in the seal keeps the write door from ever pairing
/// a value with a reach derived for a different value.
pub(crate) struct DataEntry {
    index: BindingIndex,
    sealed: SealedValue,
}

impl DataEntry {
    /// A bit-copy of the entry — the dormant seal duplicated (value bit-copy + reference-only
    /// witness clone, no refcount traffic) beside the `Copy` index. Every read
    /// hands one of these out so no caller holds the `tables` borrow across a carrier build.
    /// The one reader is the bulk-install snapshot, which is ascription-only.
    fn duplicate(&self) -> Self {
        DataEntry {
            index: self.index,
            sealed: self.sealed.duplicate(),
        }
    }
}

/// One finalized overload in a dispatch bucket: the dormant callable carrier plus the plain data
/// the write path dedupes on, all of it computed at seal time
/// ([`OverloadSeal`]) where the callable was open. Overloads sharing a bucket sit at different
/// lexical positions, so each carries its own [`BindingIndex`] and the dispatch picker filters
/// per-overload.
pub(crate) struct FunctionBucketEntry {
    pub(crate) index: BindingIndex,
    /// The stored form of the duplicate-overload predicate: token equality against an incoming
    /// callable is `DuplicateOverload`.
    token: DispatchToken,
    /// The overload's rendered signature, for the `DuplicateOverload` diagnostic.
    summary: String,
    pub(crate) sealed: SealedFunction,
}

impl FunctionBucketEntry {
    /// A bit-copy of the entry, for the bulk-install snapshot — like [`DataEntry::duplicate`].
    fn duplicate(&self) -> Self {
        FunctionBucketEntry {
            index: self.index,
            token: self.token.clone(),
            summary: self.summary.clone(),
            sealed: self.sealed.duplicate(),
        }
    }
}

/// One operator-registry entry: its lexical [`BindingIndex`], the dormant
/// [`SealedOperatorGroup`] carrier of the group record the probe key resolves to, and the plain
/// data the upsert decides identity on — all of it computed at seal time
/// ([`GroupSeal`]), where the record was open, so the write verb opens nothing.
///
/// The same shape [`DataEntry`] and [`FunctionBucketEntry`] take, and for the same reason: the
/// entry owns nothing of the record. The record lives in the declaring scope's region bump and the
/// regions its reach names are held by that region's union bundle, so the entry carries no `Drop`
/// over it and dies with the region that hosts what it names — a group whose declaring region has
/// died is unreachable rather than kept alive by a stray refcount.
pub(crate) struct OperatorEntry {
    index: BindingIndex,
    /// The registered record's address — the upsert's cheap identity arm.
    address: usize,
    /// The registered record's rendered mode + member set — the upsert's structural arm.
    declaration: String,
    sealed: SealedOperatorGroup,
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
/// `pending` names a visible [`OverloadSlot::Pending`] in the bucket — a sibling FN
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

/// Every lexical binding of one scope, in one cell. A still-finalizing binder lives in the table
/// it will resolve into — as a pending arm of the very slot it claims — so a name is looked up in
/// one probe and finalization overwrites the slot rather than moving between containers. `data`
/// and `types` park by name (value/type forward references); `functions` parks by full dispatch
/// bucket key, which keeps `(MAKESET _)` and `(MAKESET _ USING _)` from colliding.
#[derive(Default)]
struct Tables {
    /// Each bound type slot stores its type and its [`DeclarationSite`] — the installing
    /// [`NodeHandle`] (declaration identity) plus its lexical [`BindingIndex`] (visibility). A
    /// `KType` is a `Copy` handle into the run frame's registry, so a slot carries no reach: a
    /// read copies the handle under the home-frame pin alone, and the same handle names the same
    /// type in every region. A [`TypeSlot`] may carry an in-flight producer beside the bound
    /// identity — see its doc for why the two coexist here and not in `data`.
    types: HashMap<String, TypeSlot>,
    /// Each bound value slot stores its value fused to its exact reach in one dormant
    /// [`SealedValue`], plus its lexical [`BindingIndex`]. Reads hand out a bit-copy of the seal
    /// ([`Bindings::lookup_value`]) and re-anchor the value only under a pin, so a read replays the
    /// stored claim rather than re-asserting single-frame co-location. Description members are
    /// `Weak`, and the owning pins live in the region's union bundle rather than here — either a
    /// strong member or a per-entry `Rc` on the scope's own frame would close a
    /// `frame → region → scope → bindings → frame` cycle and leak the region.
    data: HashMap<String, ValueSlot>,
    /// Each sealed bucket slot stores its callable fused to its reach claim in one dormant
    /// [`SealedFunction`], beside the precomputed data the write path dedupes on
    /// ([`FunctionBucketEntry`]). Written only by the `FN` / `OP` registration doors — an `FN`
    /// registration binds no value, and a value bind writes no bucket. Like `data`, the entry
    /// owns nothing: the reached regions are held by the region's union bundle, and a read hands
    /// out a bit-copy the caller re-anchors under a pin. Sibling binders that have dispatched but
    /// not finalized sit in the same bucket as [`OverloadSlot::Pending`]; consumers park on the
    /// earliest-index visible one, and a finalize overwrites only its own slot, leaving the other
    /// siblings as wake sources.
    functions: HashMap<UntypedKey, Vec<OverloadSlot>>,
    /// Per-scope operator registry: a chain's sorted-joined operator probe key → the dormant
    /// [`SealedOperatorGroup`] carrier of the group it resolves to, beside its lexical
    /// [`BindingIndex`] ([`OperatorEntry`] — the `data`/`functions` entry shape). A module installs
    /// one entry per nonempty subset of its declared operators (the per-group powerset), each
    /// subset key holding a bit-copy of the same seal over the same region-hosted record, so any
    /// subset used in one expression resolves in a single hit, a cross-group mix simply misses, and
    /// the whole install allocates nothing past its probe keys. Walked through the scope chain like
    /// every other name (innermost visible wins).
    operators: HashMap<String, OperatorEntry>,
}

/// One scope's bindings: the four maps under a single [`RefCell`], and nothing else.
///
/// One cell rather than one per map: with writes reachable only under a [`WriteGate`], a read can
/// never overlap a write, so per-map cells bought nothing but a borrow-ordering rule to obey. Every
/// verb takes exactly one borrow, and a cross-map write — a value insert screened against `types`,
/// a type insert screened against `data` — is atomic under it.
pub struct Bindings {
    tables: RefCell<Tables>,
}

impl Bindings {
    pub fn new() -> Self {
        Self {
            tables: RefCell::new(Tables::default()),
        }
    }

    /// Per-scope value-side lookup. One probe of `data[name]`: a visible bound slot answers
    /// `Bound`, a visible pending slot answers `Parked` on its producer.
    /// `chain_cutoff = None` means the scope
    /// is off-chain (or unfiltered) — everything is visible. `None` return
    /// means no visible entry at this scope; the caller keeps walking
    /// ancestors, and chain exhaustion stays `None` (the terminal unbound
    /// disposition is materialized on the resolution path, not here).
    pub fn lookup_value(
        &self,
        name: &str,
        chain_cutoff: Option<usize>,
    ) -> Option<NameLookup<SealedValue>> {
        match self.tables.borrow().data.get(name)? {
            ValueSlot::Bound(entry) => Self::visible(entry.index, chain_cutoff)
                .then(|| NameLookup::Bound(entry.sealed.duplicate())),
            ValueSlot::Pending(p) => {
                Self::visible(p.index, chain_cutoff).then_some(NameLookup::Parked(p.producer))
            }
        }
    }

    /// Whether a **visible** value entry named `name` exists at this scope — the presence-only
    /// probe, for a write gate that must not read the bound value (the USING-window collision
    /// check). A still-finalizing pending slot is not an entry and reads `false`.
    pub fn has_value(&self, name: &str, chain_cutoff: Option<usize>) -> bool {
        self.tables
            .borrow()
            .data
            .get(name)
            .and_then(ValueSlot::bound)
            .is_some_and(|entry| Self::visible(entry.index, chain_cutoff))
    }

    /// Per-scope type-side lookup. The type-language mirror of [`Self::lookup_value`]: one probe of
    /// `types[name]`, preferring the slot's bound arm over its pending one, returning the first
    /// visible hit as a [`NameLookup`], or `None` so the caller keeps walking. Bound-preferred is
    /// load-bearing: on a slot carrying both, a consumer that can read the identity must not park.
    pub fn lookup_type(
        &self,
        name: &str,
        chain_cutoff: Option<usize>,
    ) -> Option<NameLookup<KType>> {
        let tables = self.tables.borrow();
        let slot = tables.types.get(name)?;
        if let Some((kt, site)) = slot.bound() {
            if Self::visible(site.index, chain_cutoff) {
                return Some(NameLookup::Bound(kt));
            }
        }
        slot.pending()
            .filter(|p| Self::visible(p.index, chain_cutoff))
            .map(|p| NameLookup::Parked(p.producer))
    }

    /// Classified per-scope member lookup for ATTR module / signature access: the value-or-type
    /// `name` resolves to, read from **this scope's own** `data` then `types` in one pass. A
    /// module member is module-own — the lookup deliberately does **not** consult the builtin
    /// root or walk lexical ancestors, so `m.Type` (a builtin type name) or `m.SomeOuterType`
    /// is "no member", not a fall-through. The cross-kind exclusion keeps the two arms from both
    /// matching, so the result is unambiguous. Bound arms only — a read module is finalized, so a
    /// pending arm never surfaces here.
    pub fn lookup_member(
        &self,
        name: &str,
        chain_cutoff: Option<usize>,
    ) -> Option<MemberResolution> {
        let tables = self.tables.borrow();
        if let Some(entry) = tables.data.get(name).and_then(ValueSlot::bound) {
            if Self::visible(entry.index, chain_cutoff) {
                return Some(MemberResolution::Value(entry.sealed.duplicate()));
            }
        }
        if let Some((kt, site)) = tables.types.get(name).and_then(TypeSlot::bound) {
            if Self::visible(site.index, chain_cutoff) {
                return Some(MemberResolution::Type { kt });
            }
        }
        None
    }

    /// The producer `NodeId` of a still-finalizing **type** binder named `name`, read straight from
    /// the slot's pending arm — *not* through [`Self::lookup_type`], which prefers the (possibly
    /// seal-pre-installed, still-unsealed) bound arm. The finalize gate uses this to park the
    /// type-identifier memo on an in-flight producer even when the seal has already pre-installed
    /// the name's external identity into `types` — the [`TypeSlot::BoundWithPending`] case.
    /// Visibility-unfiltered: this is producer-dependency tracking, not consumer-visibility
    /// enforcement.
    pub fn type_placeholder_producer(&self, name: &str) -> Option<NodeId> {
        self.tables
            .borrow()
            .types
            .get(name)
            .and_then(TypeSlot::pending)
            .map(|p| p.producer)
    }

    /// Per-scope dispatch-bucket lookup. One pass over `functions[key]` surfaces the visible sealed
    /// overloads AND the earliest-index visible pending sibling together, so the scope walk decides
    /// pending-vs-finalized precedence with both in hand.
    pub fn lookup_function(&self, key: &UntypedKey, chain_cutoff: Option<usize>) -> FunctionLookup {
        let tables = self.tables.borrow();
        let Some(bucket) = tables.functions.get(key) else {
            return FunctionLookup {
                overloads: Vec::new(),
                pending: None,
            };
        };
        let overloads: Vec<SealedFunction> = bucket
            .iter()
            .filter_map(OverloadSlot::sealed)
            .filter(|entry| Self::visible(entry.index, chain_cutoff))
            .map(|entry| entry.sealed.duplicate())
            .collect();
        // Earliest-index visible producer: most likely to finalize first.
        let pending = bucket
            .iter()
            .filter_map(OverloadSlot::pending)
            .filter(|p| Self::visible(p.index, chain_cutoff))
            .min_by_key(|p| p.index.idx)
            .map(|p| p.producer);
        FunctionLookup { overloads, pending }
    }

    /// Per-scope operator-group lookup. Mirrors [`Self::lookup_value`] for the
    /// `operators` map: returns the visible group registered under `probe` (the
    /// sorted-joined unique operators of a chain), or `None` at this scope so the
    /// caller keeps walking ancestors.
    pub(in crate::machine::core) fn lookup_operator_group(
        &self,
        probe: &str,
        chain_cutoff: Option<usize>,
    ) -> Option<SealedOperatorGroup> {
        let tables = self.tables.borrow();
        let entry = tables.operators.get(probe)?;
        Self::visible(entry.index, chain_cutoff).then(|| entry.sealed.duplicate())
    }

    /// Register `probe → seal`'s group in the operator registry. The `OP` / `GROUP` binder installs
    /// one entry per nonempty subset of the declared operators (all bit-copies of the same seal over
    /// one record); test fixtures register the subsets they exercise.
    ///
    /// Upsert: an existing entry whose record is the one being registered — the same address, or an
    /// equal mode + member set (two `OP` statements over the same symbol and distinct operand types
    /// are two bucket overloads but one registry entry, and each builds its own record) — is a
    /// silent no-op, keeping the first entry's index. A record that disagrees is a chaining-mode
    /// conflict on `probe`: the same scope cannot say the symbol both folds and pairs.
    ///
    /// Both identity arms read plain data [`GroupSeal`] computed where the record was open, so this
    /// verb opens no carrier and needs no pin — the same discipline [`Self::write_overload`]
    /// follows for the `functions` table.
    pub(crate) fn write_operator_group(
        &self,
        probe: String,
        seal: &GroupSeal,
        index: BindingIndex,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let mut tables = self.tables.borrow_mut();
        if let Some(entry) = tables.operators.get(&probe) {
            if entry.address == seal.address || entry.declaration == seal.declaration {
                return Ok(());
            }
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "operator `{probe}` is already declared in this scope with a different \
                 chaining mode or member set; one scope declares one chaining mode per operator",
            ))));
        }
        tables.operators.insert(
            probe,
            OperatorEntry {
                index,
                address: seal.address,
                declaration: seal.declaration.clone(),
                sealed: seal.sealed.duplicate(),
            },
        );
        Ok(())
    }

    /// Snapshot every bound `(name, dormant carrier)` pair in `data`, ignoring visibility. Each
    /// seal is a bit-copy; the caller re-anchors what it needs under its own pin. Pending slots are
    /// invisible to bulk reads — there is no carrier to hand out. For chain-gated single-name
    /// reads use [`Self::lookup_value`].
    pub fn iter_data(&self) -> Vec<(String, SealedValue)> {
        self.tables
            .borrow()
            .data
            .iter()
            .filter_map(|(name, slot)| Some((name.clone(), slot.bound()?.sealed.duplicate())))
            .collect()
    }

    /// Snapshot every bound `(name, KType)` pair in `types`, ignoring visibility.
    pub fn iter_types(&self) -> Vec<(String, KType)> {
        self.tables
            .borrow()
            .types
            .iter()
            .filter_map(|(name, slot)| Some((name.clone(), slot.bound()?.0)))
            .collect()
    }

    /// Snapshot every `(UntypedKey, Vec<SealedFunction>)` pair in `functions`, ignoring per-overload
    /// visibility. Each seal is a bit-copy; the caller re-anchors what it needs under its own pin.
    /// Sealed slots only, and a bucket holding none is skipped — a key claimed by pending siblings
    /// alone publishes no dispatch surface to snapshot. For chain-gated picks use
    /// [`Self::lookup_function`].
    pub fn iter_functions(&self) -> Vec<(UntypedKey, Vec<SealedFunction>)> {
        self.tables
            .borrow()
            .functions
            .iter()
            .filter_map(|(key, bucket)| {
                let sealed: Vec<SealedFunction> = bucket
                    .iter()
                    .filter_map(OverloadSlot::sealed)
                    .map(|entry| entry.sealed.duplicate())
                    .collect();
                (!sealed.is_empty()).then(|| (key.clone(), sealed))
            })
            .collect()
    }

    /// True iff `types[name]` is bound at [`BindingIndex::BUILTIN`]. The
    /// no-shadow consult gates on this — a genuine builtin, not a user type that a
    /// synthetic test happens to have placed in a root-position scope.
    pub fn has_builtin_type(&self, name: &str) -> bool {
        self.tables
            .borrow()
            .types
            .get(name)
            .and_then(TypeSlot::bound)
            .is_some_and(|(_, site)| site.index == BindingIndex::BUILTIN)
    }

    /// True iff `functions[key]` holds a sealed overload registered at
    /// [`BindingIndex::BUILTIN`] — a genuine builtin dispatch bucket, distinct from a
    /// user bucket the no-shadow consult must not gate.
    pub fn has_builtin_function(&self, key: &UntypedKey) -> bool {
        self.tables.borrow().functions.get(key).is_some_and(|b| {
            b.iter()
                .filter_map(OverloadSlot::sealed)
                .any(|e| e.index == BindingIndex::BUILTIN)
        })
    }

    /// Visibility predicate: `None` ⇒ everything visible; `Some(c)` ⇒ `b.idx < c`. The cutoff
    /// itself comes from [`Scope::binding_cutoff`](crate::machine::Scope); this is the only place
    /// it is applied.
    fn visible(b: BindingIndex, chain_cutoff: Option<usize>) -> bool {
        match chain_cutoff {
            None => true,
            Some(c) => b.idx < c,
        }
    }

    #[cfg(test)]
    pub(crate) fn data(&self) -> Ref<'_, HashMap<String, ValueSlot>> {
        Ref::map(self.tables.borrow(), |t| &t.data)
    }

    #[cfg(test)]
    pub(crate) fn functions(&self) -> Ref<'_, HashMap<UntypedKey, Vec<OverloadSlot>>> {
        Ref::map(self.tables.borrow(), |t| &t.functions)
    }

    #[cfg(test)]
    pub(crate) fn types(&self) -> Ref<'_, HashMap<String, TypeSlot>> {
        Ref::map(self.tables.borrow(), |t| &t.types)
    }

    /// The pending arm of `data[name]`, if any — the value-side forward-reference probe.
    #[cfg(test)]
    pub fn pending_value(&self, name: &str) -> Option<PendingBinding> {
        self.tables
            .borrow()
            .data
            .get(name)
            .and_then(ValueSlot::pending)
    }

    /// Every pending name-keyed arm across `data` and `types`, tagged with the language it resolves
    /// in — the hygiene probe for "this declaration left no in-flight producer behind".
    #[cfg(test)]
    pub fn pending_names(&self) -> Vec<(String, BindKind, NodeId)> {
        let tables = self.tables.borrow();
        let values = tables
            .data
            .iter()
            .filter_map(|(n, s)| Some((n.clone(), BindKind::Value, s.pending()?.producer)));
        let types = tables
            .types
            .iter()
            .filter_map(|(n, s)| Some((n.clone(), BindKind::Type, s.pending()?.producer)));
        values.chain(types).collect()
    }

    /// Every pending sibling in one dispatch bucket, in slot order.
    #[cfg(test)]
    pub fn pending_overload_entries(&self, bucket: &UntypedKey) -> Vec<PendingBinding> {
        self.tables
            .borrow()
            .functions
            .get(bucket)
            .map(|b| b.iter().filter_map(OverloadSlot::pending).collect())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub fn expect_type(&self, name: &str) -> KType {
        self.tables
            .borrow()
            .types
            .get(name)
            .and_then(TypeSlot::bound)
            .map(|(kt, _site)| kt)
            .unwrap_or_else(|| panic!("expected bindings.types[{name:?}] to be bound"))
    }

    /// Write `name` → `kt` into `types` under `policy`. [`TypeWritePolicy::Insert`] is strict
    /// insert-if-absent; [`TypeWritePolicy::UpsertEqual`] admits a re-entry of the *same*
    /// declaration — declaration identity is the installing [`NodeHandle`], so an existing entry
    /// whose handle differs from `site`'s is a different declaration of the name and `Rebind`s,
    /// while a same-handle hit is the same slot in the same run re-entering (a parallel nominal
    /// finalize), whose re-elaboration cannot differ, so it overwrites idempotently. Content plays
    /// no part in the same-declaration decision.
    ///
    /// A committed value at `data[name]` is a `Rebind` under either policy — the value/type
    /// partition is mutually exclusive. On success the slot is overwritten in place with a plain
    /// `Bound` arm, so finalizing drops any pending arm the slot carried.
    pub(crate) fn write_type(
        &self,
        name: &str,
        kt: KType,
        site: DeclarationSite,
        policy: TypeWritePolicy,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        self.partition_guard(name, BindKind::Type)?;
        let mut tables = self.tables.borrow_mut();
        // Cross-kind exclusion: a type name may not collide with a committed value. A pending value
        // slot does not block — only a bound one is a committed binding.
        if tables.data.get(name).is_some_and(|s| s.bound().is_some()) {
            return Err(KError::new(KErrorKind::Rebind {
                name: name.to_string(),
            }));
        }
        match (
            policy,
            tables
                .types
                .get(name)
                .and_then(TypeSlot::bound)
                .map(|(_, s)| s),
        ) {
            (TypeWritePolicy::Insert, Some(_)) => {
                return Err(KError::new(KErrorKind::Rebind {
                    name: name.to_string(),
                }));
            }
            (TypeWritePolicy::UpsertEqual, Some(existing)) if existing.node != site.node => {
                return Err(KError::new(KErrorKind::Rebind {
                    name: name.to_string(),
                }));
            }
            // Absent, or the same declaration re-entering: write the identity.
            _ => {}
        }
        match tables.types.get_mut(name) {
            // Whatever the slot held — a pending arm, a bound identity, or both — the finalized
            // identity replaces it where it sits, so the key is never re-keyed.
            Some(slot) => *slot = TypeSlot::Bound(kt, site),
            None => {
                tables
                    .types
                    .insert(name.to_string(), TypeSlot::Bound(kt, site));
            }
        }
        Ok(())
    }

    /// Claim `name`'s slot in its destination table for producer node `idx` — the dispatch-time
    /// forward-reference stamp.
    ///
    /// Errors `Rebind` if the claim collides: a committed `data[name]` (bindings are bind-once), or
    /// an existing pending arm naming a different producer. Idempotent on same-producer re-entry.
    /// A `types` slot already carrying a bound identity keeps it and gains the pending arm
    /// ([`TypeSlot::BoundWithPending`]): a parallel nominal finalize pre-installs the external
    /// identity while its producer is still in flight.
    ///
    /// The eventual [`Self::write_value`] / [`Self::write_type`] call must carry the
    /// same `index` so the consumer's visibility test stays consistent across
    /// the pending → finalized transition. `kind` picks the destination table, so a value bind
    /// never satisfies a type claim (or the reverse) — see [`Bindings::lookup_value`] /
    /// [`Bindings::lookup_type`], each of which probes only its own table.
    pub fn install_placeholder(
        &self,
        name: String,
        idx: NodeId,
        index: BindingIndex,
        kind: BindKind,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let mut tables = self.tables.borrow_mut();
        let claim = PendingBinding {
            producer: idx,
            index,
        };
        match kind {
            BindKind::Value => match tables.data.get_mut(&name) {
                Some(ValueSlot::Bound(_)) => Err(KError::new(KErrorKind::Rebind { name })),
                Some(ValueSlot::Pending(existing)) if existing.producer == idx => Ok(()),
                Some(ValueSlot::Pending(_)) => Err(KError::new(KErrorKind::Rebind { name })),
                None => {
                    tables.data.insert(name, ValueSlot::Pending(claim));
                    Ok(())
                }
            },
            BindKind::Type => match tables.types.get_mut(&name) {
                Some(slot) => match slot.pending() {
                    Some(existing) if existing.producer == idx => Ok(()),
                    Some(_) => Err(KError::new(KErrorKind::Rebind { name })),
                    None => {
                        let (kt, site) = slot
                            .bound()
                            .expect("a TypeSlot with no pending arm is Bound");
                        *slot = TypeSlot::BoundWithPending(kt, site, claim);
                        Ok(())
                    }
                },
                None => {
                    tables.types.insert(name, TypeSlot::Pending(claim));
                    Ok(())
                }
            },
        }
    }

    /// Install a dispatch-time pending-overload entry: `bucket → producer`.
    /// The bucket key MUST equal what `KExpression::untyped_key` would compute
    /// for a *call* to the eventual overload (not the binder call itself).
    ///
    /// **Append, never deduplicate**: sibling FN binders sharing one
    /// inner-call bucket key — `FN (PICK xs :A) -> ...` then
    /// `FN (PICK xs :B) -> ...` — each claim their own slot at their own
    /// [`BindingIndex`]. The slot is overwritten in place by
    /// [`Bindings::write_overload`] when the producing binder seals; other siblings stay
    /// pending as wake sources.
    ///
    /// Appended even when the bucket already holds sealed overloads: a pending
    /// sibling sits *alongside* a finalized one so the scope walk can park
    /// the bucket until the sibling finalizes.
    pub fn install_pending_overload(
        &self,
        bucket: UntypedKey,
        idx: NodeId,
        index: BindingIndex,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        self.tables
            .borrow_mut()
            .functions
            .entry(bucket)
            .or_default()
            .push(OverloadSlot::Pending(PendingBinding {
                producer: idx,
                index,
            }));
        Ok(())
    }

    /// Replay another `Bindings`'s bound `data` entries through [`Self::write_value`] on self, and
    /// its sealed `functions` slots by direct entry duplication — a view of a module preserves the
    /// module's keyworded dispatch surface as-is (keyword → keyword), it does not re-derive it from
    /// the value bindings. Pending arms are not replayed: a claim names a producer in the source's
    /// own scheduler run, so copying one would hand the target a park on a node that will never
    /// wake it. Snapshots the source maps and releases the source `Ref` before the replay
    /// so re-entrant ascription cannot deadlock.
    pub(crate) fn bulk_install_from(
        &self,
        src: &Bindings,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        // Duplicate each entry into the snapshot: each seal is a bit-copy naming the source's own
        // minted description, so the replayed entry replays that same claim. The reached regions
        // stay owned by the *source* scope's region union — the replay target's own region must
        // already outlive it (a bulk install is same-run re-entrant ascription).
        let (data, functions) = {
            let tables = src.tables.borrow();
            let data: Vec<(String, DataEntry)> = tables
                .data
                .iter()
                .filter_map(|(k, slot)| Some((k.clone(), slot.bound()?.duplicate())))
                .collect();
            let functions: Vec<(UntypedKey, Vec<OverloadSlot>)> = tables
                .functions
                .iter()
                .filter_map(|(key, bucket)| {
                    let sealed: Vec<OverloadSlot> = bucket
                        .iter()
                        .filter_map(OverloadSlot::sealed)
                        .map(|e| OverloadSlot::Sealed(e.duplicate()))
                        .collect();
                    (!sealed.is_empty()).then(|| (key.clone(), sealed))
                })
                .collect();
            (data, functions)
        };
        for (name, entry) in data {
            self.write_value(&name, entry.index, entry.sealed, gate)?;
        }
        let mut tables = self.tables.borrow_mut();
        for (key, bucket) in functions {
            tables.functions.entry(key).or_default().extend(bucket);
        }
        Ok(())
    }

    /// The token-class partition: `types` holds Type-token names, `data` holds value-token names, and a
    /// name may not cross. The two maps are different universes — a Type token names something that can
    /// type a field, a value token names something a field can hold — so a write whose name classifies
    /// against the map it is entering is a hard error, not a convention, with no exception: every
    /// value-token write to `types` and every Type-token write to `data` is rejected. This is the single
    /// enforcement point: every binder reaches its map through [`Bindings::write_value`] /
    /// [`Bindings::write_type`], so no caller can bind across the line, and none needs its own check.
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

    /// The `data` write path: commit `name` → `sealed` as a bind-once value binding. Runs the
    /// token-class partition guard and the cross-kind probe, then writes the slot — overwriting
    /// this name's pending arm in place if it had one, so the key is stored once and finalizing
    /// abandons nothing. All under one borrow.
    pub(crate) fn write_value(
        &self,
        name: &str,
        index: BindingIndex,
        sealed: SealedValue,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        self.partition_guard(name, BindKind::Value)?;
        let rebind = || {
            KError::new(KErrorKind::Rebind {
                name: name.to_string(),
            })
        };
        let mut tables = self.tables.borrow_mut();
        // Cross-kind exclusion: a value name may not collide with a committed type — the
        // `data`/`types` partition is structural, not convention. A type slot that is only pending
        // holds no committed identity, so it does not block.
        if tables.types.get(name).is_some_and(|s| s.bound().is_some()) {
            return Err(rebind());
        }
        match tables.data.get_mut(name) {
            Some(ValueSlot::Bound(_)) => return Err(rebind()),
            // The pending arm this write finalizes: overwritten in place, keyed by name alone —
            // a write whose producer differs from the one that claimed the slot still finalizes it.
            Some(slot @ ValueSlot::Pending(_)) => {
                *slot = ValueSlot::Bound(DataEntry { index, sealed });
            }
            None => {
                tables.data.insert(
                    name.to_string(),
                    ValueSlot::Bound(DataEntry { index, sealed }),
                );
            }
        }
        Ok(())
    }

    /// The `functions` write path: add `seal`'s callable to its dispatch bucket. The bucket key,
    /// dedupe token and diagnostic summary were all computed at seal time, where the callable was
    /// open — the write is pure table mutation, no carrier is opened and no bare reference crosses
    /// the door. Token equality against a sealed bucket sibling raises `DuplicateOverload`; pending
    /// siblings don't participate in the dedupe. The seal lands **in this binder's own pending
    /// slot** when it has one — same index, overwritten in place — so the key is stored once and
    /// other siblings stay as wake sources. Bucket order is not observable: the picker returns a
    /// unique winner or a tie that surfaces as deferred/ambiguous either way.
    pub(crate) fn write_overload(
        &self,
        name: &str,
        index: BindingIndex,
        seal: OverloadSeal,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let mut tables = self.tables.borrow_mut();
        let bucket = tables.functions.entry(seal.key.clone()).or_default();
        if let Some(existing) = bucket
            .iter()
            .filter_map(OverloadSlot::sealed)
            .find(|existing| existing.token == seal.token)
        {
            return Err(KError::new(KErrorKind::DuplicateOverload {
                name: name.to_string(),
                signature: existing.summary.clone(),
            }));
        }
        let entry = FunctionBucketEntry {
            index,
            token: seal.token,
            summary: seal.summary,
            sealed: seal.sealed,
        };
        // The claim this write finalizes, if the binder made one; a builtin seed, a direct
        // registration or a bulk install has none and simply appends.
        match bucket
            .iter()
            .position(|slot| slot.pending().is_some_and(|p| p.index == index))
        {
            Some(at) => {
                bucket[at] = OverloadSlot::Sealed(entry);
                // Any further claim at the same index is a leak from a prior run's failed binder —
                // the pending-overload channel has no error-path sweep, so this write is where it
                // dies.
                bucket.retain(|slot| !slot.pending().is_some_and(|p| p.index == index));
            }
            None => bucket.push(OverloadSlot::Sealed(entry)),
        }
        Ok(())
    }

    /// Drop every name-keyed pending arm pointing at `producer`. The success write
    /// paths finalize a binder's own claim in place; this is the error-path
    /// companion, called when `producer`'s node finalizes with an error so a binder body
    /// that failed before its write path does not leak a scheduler-local [`NodeId`] into
    /// a later run on a persistent scope. A `types` slot that also holds a bound identity keeps
    /// it — only the pending arm is dropped. Function buckets are untouched: a pending overload
    /// dies only under a later same-index [`Self::write_overload`].
    pub fn clear_placeholders_for_producer(&self, producer: NodeId, _gate: &mut WriteGate) {
        let mut tables = self.tables.borrow_mut();
        let claims = |slot: Option<PendingBinding>| slot.is_some_and(|p| p.producer == producer);
        tables.data.retain(|_, slot| !claims(slot.pending()));
        tables.types.retain(|_, slot| match slot {
            TypeSlot::Pending(p) => p.producer != producer,
            TypeSlot::BoundWithPending(kt, site, p) if p.producer == producer => {
                *slot = TypeSlot::Bound(*kt, *site);
                true
            }
            _ => true,
        });
    }
}

impl Default for Bindings {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
