//! Lexical binding façade: one `RefCell<Tables>` — `types`, `data`, `functions`, `placeholders`,
//! `pending_overloads`, `operators` — behind validated write paths. `data` and `functions` are
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
use std::rc::Rc;

use crate::machine::core::carrier_witness::{OverloadSeal, SealedFunction, SealedOperatorGroup};
use crate::machine::core::kfunction::NodeId;
use crate::machine::core::{FrameStorage, RunId};
use crate::machine::model::CarriedFamily;
use crate::machine::model::DispatchToken;
use crate::machine::model::OperatorGroup;
use crate::machine::model::{KType, UntypedKey};
use crate::machine::CarrierWitness;
use crate::witnessed::Sealed;

use super::kerror::{KError, KErrorKind};

mod gate;
mod ops;

pub use gate::WriteGate;
pub(crate) use ops::{operator_group_ops, TypeWritePolicy, WriteOp};

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

/// One operator-registry entry: its lexical [`BindingIndex`] and the dormant
/// [`SealedOperatorGroup`] carrier of the group record the probe key resolves to.
///
/// The same shape [`DataEntry`] takes, and for the same reason: the entry owns nothing. The record
/// lives in the declaring scope's region bump and the regions its reach names are held by that
/// region's union bundle, so the entry is `Copy`-cheap to read out, carries no `Drop`, and dies
/// with the region that hosts what it names — a group whose declaring region has died is
/// unreachable rather than kept alive by a stray refcount.
pub(crate) struct OperatorEntry {
    index: BindingIndex,
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

/// Every lexical binding of one scope, in one cell. `placeholders` and `pending_overloads` are
/// intentionally separate maps: the former is consulted by name (value/type forward references);
/// the latter by full dispatch bucket key (a bare-arg call whose FN overload is still finalizing).
/// Keying dispatch parks by the full bucket key keeps `(MAKESET _)` and `(MAKESET _ USING _)` from
/// colliding.
#[derive(Default)]
struct Tables {
    /// Each type entry stores its bound type and its [`DeclarationSite`] — the installing
    /// [`NodeHandle`] (declaration identity) plus its lexical [`BindingIndex`] (visibility). A
    /// `KType` is a `Copy` handle into the run frame's registry, so an entry carries no reach: a
    /// read copies the handle under the home-frame pin alone, and the same handle names the same
    /// type in every region.
    types: HashMap<String, (KType, DeclarationSite)>,
    /// Each value entry stores its bound value fused to its exact reach in one dormant
    /// [`SealedValue`], plus its lexical [`BindingIndex`]. Reads hand out a bit-copy of the seal
    /// ([`Bindings::lookup_value`]) and re-anchor the value only under a pin, so a read replays the
    /// stored claim rather than re-asserting single-frame co-location. Description members are
    /// `Weak`, and the owning pins live in the region's union bundle rather than here — either a
    /// strong member or a per-entry `Rc` on the scope's own frame would close a
    /// `frame → region → scope → bindings → frame` cycle and leak the region.
    data: HashMap<String, DataEntry>,
    /// Each dispatch-bucket entry stores its callable fused to its reach claim in one dormant
    /// [`SealedFunction`], beside the precomputed data the write path dedupes on
    /// ([`FunctionBucketEntry`]). Written only by the `FN` / `OP` registration doors — an `FN`
    /// registration binds no value, and a value bind writes no bucket. Like `data`, the entry
    /// owns nothing: the reached regions are held by the region's union bundle, and a read hands
    /// out a bit-copy the caller re-anchors under a pin.
    functions: HashMap<UntypedKey, Vec<FunctionBucketEntry>>,
    placeholders: HashMap<String, (NodeId, BindingIndex, BindKind)>,
    /// Bucket-key → entries for FN overloads whose binder has
    /// dispatched but not finalized. Sibling binders sharing one inner-call
    /// bucket key each install their own entry; consumers park on the
    /// earliest-index visible one. On finalize only that entry is removed;
    /// other siblings remain as wake sources.
    pending_overloads: HashMap<UntypedKey, Vec<(NodeId, BindingIndex)>>,
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

/// One scope's bindings: the six maps under a single [`RefCell`], and nothing else.
///
/// One cell rather than one per map: with writes reachable only under a [`WriteGate`], a read can
/// never overlap a write, so per-map cells bought nothing but a borrow-ordering rule to obey. Every
/// verb takes exactly one borrow, and a cross-map write — a bucket insert plus its
/// pending-overload clear, a type insert plus its placeholder clear — is atomic under it.
pub struct Bindings {
    tables: RefCell<Tables>,
}

impl Bindings {
    pub fn new() -> Self {
        Self {
            tables: RefCell::new(Tables::default()),
        }
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
        let tables = self.tables.borrow();
        if let Some(entry) = tables.data.get(name) {
            if Self::visible(entry.index, chain_cutoff) {
                return Some(NameLookup::Bound(entry.sealed.duplicate()));
            }
        }
        Self::value_placeholder(&tables, name, chain_cutoff).map(NameLookup::Parked)
    }

    /// Whether a **visible** value entry named `name` exists at this scope — the presence-only
    /// probe, for a write gate that must not read the bound value (the USING-window collision
    /// check). A still-finalizing placeholder is not an entry and reads `false`.
    pub fn has_value(&self, name: &str, chain_cutoff: Option<usize>) -> bool {
        self.tables
            .borrow()
            .data
            .get(name)
            .is_some_and(|entry| Self::visible(entry.index, chain_cutoff))
    }

    /// The value-side placeholder producer for `name`, or `None` — the placeholder arm
    /// [`Self::lookup_value`] falls through to.
    fn value_placeholder(
        tables: &Tables,
        name: &str,
        chain_cutoff: Option<usize>,
    ) -> Option<NodeId> {
        if let Some((id, idx, kind)) = tables.placeholders.get(name).copied() {
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
        let tables = self.tables.borrow();
        if let Some((kt, site)) = tables.types.get(name) {
            if Self::visible(site.index, chain_cutoff) {
                return Some(NameLookup::Bound(*kt));
            }
        }
        Self::type_placeholder(&tables, name, chain_cutoff).map(NameLookup::Parked)
    }

    /// The type-side placeholder producer for `name`, or `None` — the placeholder arm
    /// [`Self::lookup_type`] falls through to.
    fn type_placeholder(
        tables: &Tables,
        name: &str,
        chain_cutoff: Option<usize>,
    ) -> Option<NodeId> {
        if let Some((id, idx, kind)) = tables.placeholders.get(name).copied() {
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
        let tables = self.tables.borrow();
        if let Some(entry) = tables.data.get(name) {
            if Self::visible(entry.index, chain_cutoff) {
                return Some(MemberResolution::Value(entry.sealed.duplicate()));
            }
        }
        if let Some((kt, site)) = tables.types.get(name) {
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
        match self.tables.borrow().placeholders.get(name).copied() {
            Some((id, _, BindKind::Type)) => Some(id),
            _ => None,
        }
    }

    /// Per-scope dispatch-bucket lookup. Surfaces visible finalized overloads
    /// (`functions[key]`, filtered per-overload) AND the earliest-index visible
    /// `pending_overloads[key]` producer together — one pass over each map. The
    /// scope walk decides pending-vs-finalized precedence with both in hand.
    pub fn lookup_function(&self, key: &UntypedKey, chain_cutoff: Option<usize>) -> FunctionLookup {
        let tables = self.tables.borrow();
        let overloads: Vec<SealedFunction> = tables
            .functions
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
        let pending = tables.pending_overloads.get(key).and_then(|entries| {
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
    ) -> Option<SealedOperatorGroup> {
        let tables = self.tables.borrow();
        let entry = tables.operators.get(probe)?;
        Self::visible(entry.index, chain_cutoff).then(|| entry.sealed.duplicate())
    }

    /// Register `probe → group` in the operator registry. The `OP` / `GROUP` binder
    /// installs one entry per nonempty subset of the declared operators (all bit-copies of the same
    /// seal over one record); test fixtures register the subsets they exercise.
    ///
    /// Upsert: an existing entry whose record is the one being registered — the same address, or an
    /// equal mode + member set (two `OP` statements over the same symbol and distinct operand types
    /// are two bucket overloads but one registry entry, and each builds its own record) — is a
    /// silent no-op, keeping the first entry's index. A record that disagrees is a chaining-mode
    /// conflict on `probe`: the same scope cannot say the symbol both folds and pairs.
    ///
    /// Both records are read under `pin`, the write scope's own region owner. That covers them
    /// because a scope's operator entries are written only by declarations at that scope, whose
    /// records [`OperatorGroup::alloc`](crate::machine::model::OperatorGroup::alloc) placed in that
    /// scope's own region — a `USING` window forwards its writes to the call site, which is
    /// same-region with the window.
    pub(crate) fn write_operator_group(
        &self,
        probe: String,
        group: SealedOperatorGroup,
        index: BindingIndex,
        pin: &Rc<FrameStorage>,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let mut tables = self.tables.borrow_mut();
        if let Some(entry) = tables.operators.get(&probe) {
            let agrees = entry.sealed.open_with(pin, |current| {
                group.open_with(pin, |incoming| {
                    std::ptr::eq::<OperatorGroup<'_>>(current, incoming)
                        || current.same_declaration(incoming)
                })
            });
            if agrees {
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
                sealed: group,
            },
        );
        Ok(())
    }

    /// Snapshot every `(name, dormant carrier)` pair in `data`, ignoring visibility. Each seal is a
    /// bit-copy; the caller re-anchors what it needs under its own pin. For chain-gated single-name
    /// reads use [`Self::lookup_value`].
    pub fn iter_data(&self) -> Vec<(String, SealedValue)> {
        self.tables
            .borrow()
            .data
            .iter()
            .map(|(name, entry)| (name.clone(), entry.sealed.duplicate()))
            .collect()
    }

    /// Snapshot every `(name, KType)` pair in `types`, ignoring visibility.
    pub fn iter_types(&self) -> Vec<(String, KType)> {
        self.tables
            .borrow()
            .types
            .iter()
            .map(|(name, (kt, _site))| (name.clone(), *kt))
            .collect()
    }

    /// Snapshot every `(UntypedKey, Vec<SealedFunction>)` pair in `functions`, ignoring per-overload
    /// visibility. Each seal is a bit-copy; the caller re-anchors what it needs under its own pin.
    /// For chain-gated picks use [`Self::lookup_function`].
    pub fn iter_functions(&self) -> Vec<(UntypedKey, Vec<SealedFunction>)> {
        self.tables
            .borrow()
            .functions
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
        self.tables
            .borrow()
            .types
            .get(name)
            .is_some_and(|(_, site)| site.index == BindingIndex::BUILTIN)
    }

    /// True iff `functions[key]` holds an overload registered at
    /// [`BindingIndex::BUILTIN`] — a genuine builtin dispatch bucket, distinct from a
    /// user bucket the no-shadow consult must not gate.
    pub fn has_builtin_function(&self, key: &UntypedKey) -> bool {
        self.tables
            .borrow()
            .functions
            .get(key)
            .is_some_and(|bucket| bucket.iter().any(|e| e.index == BindingIndex::BUILTIN))
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
    pub(crate) fn data(&self) -> Ref<'_, HashMap<String, DataEntry>> {
        Ref::map(self.tables.borrow(), |t| &t.data)
    }

    #[cfg(test)]
    pub(crate) fn functions(&self) -> Ref<'_, HashMap<UntypedKey, Vec<FunctionBucketEntry>>> {
        Ref::map(self.tables.borrow(), |t| &t.functions)
    }

    #[cfg(test)]
    pub fn placeholders(&self) -> Ref<'_, HashMap<String, (NodeId, BindingIndex, BindKind)>> {
        Ref::map(self.tables.borrow(), |t| &t.placeholders)
    }

    #[cfg(test)]
    pub fn pending_overloads(&self) -> Ref<'_, HashMap<UntypedKey, Vec<(NodeId, BindingIndex)>>> {
        Ref::map(self.tables.borrow(), |t| &t.pending_overloads)
    }

    #[cfg(test)]
    pub fn types(&self) -> Ref<'_, HashMap<String, (KType, DeclarationSite)>> {
        Ref::map(self.tables.borrow(), |t| &t.types)
    }

    #[cfg(test)]
    pub fn expect_type(&self, name: &str) -> KType {
        self.tables
            .borrow()
            .types
            .get(name)
            .map(|(kt, _site)| *kt)
            .unwrap_or_else(|| panic!("expected bindings.types[{name:?}] to be present"))
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
    /// partition is mutually exclusive. On success the matching type-side placeholder is removed.
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
        // Cross-kind exclusion: a type name may not collide with a committed value.
        if tables.data.contains_key(name) {
            return Err(KError::new(KErrorKind::Rebind {
                name: name.to_string(),
            }));
        }
        match (policy, tables.types.get(name).map(|(_, s)| *s)) {
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
        tables.types.insert(name.to_string(), (kt, site));
        tables.clear_placeholder(name, BindKind::Type);
        Ok(())
    }

    /// Install a dispatch-time placeholder for `name` → producer slot `idx`.
    ///
    /// Errors `Rebind` if `data[name]` is already committed — bindings are
    /// bind-once — or if `placeholders[name]` maps to a different `NodeId`;
    /// idempotent on same-`NodeId` re-entry.
    ///
    /// The eventual [`Self::write_value`] / [`Self::write_type`] call must carry the
    /// same `index` so the consumer's visibility test stays consistent across
    /// the placeholder → finalized transition. `kind` records which language the
    /// forward reference resolves in, so a value bind never satisfies a type
    /// placeholder (or the reverse) — see [`Bindings::lookup_value`] /
    /// [`Bindings::lookup_type`], each of which surfaces only its own kind.
    pub fn install_placeholder(
        &self,
        name: String,
        idx: NodeId,
        index: BindingIndex,
        kind: BindKind,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let mut tables = self.tables.borrow_mut();
        if tables.data.contains_key(&name) {
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        if let Some((existing, _, _)) = tables.placeholders.get(&name).copied() {
            if existing == idx {
                return Ok(());
            }
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        tables.placeholders.insert(name, (idx, index, kind));
        Ok(())
    }

    /// Install a dispatch-time pending-overload entry: `bucket → producer`.
    /// The bucket key MUST equal what `KExpression::untyped_key` would compute
    /// for a *call* to the eventual overload (not the binder call itself).
    ///
    /// **Append, never deduplicate**: sibling FN binders sharing one
    /// inner-call bucket key — `FN (PICK xs :A) -> ...` then
    /// `FN (PICK xs :B) -> ...` — each install their own entry at their own
    /// [`BindingIndex`]. The entry is removed in [`Bindings::write_overload`] when
    /// the producing binder lands in `functions[bucket]`; other siblings stay
    /// pending as wake sources.
    ///
    /// Recorded even when the bucket is already live in `functions`: a pending
    /// sibling sits *alongside* a finalized overload so the scope walk can park
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
            .pending_overloads
            .entry(bucket)
            .or_default()
            .push((idx, index));
        Ok(())
    }

    /// Replay another `Bindings`'s `data` entries through [`Self::write_value`] on self, and its
    /// `functions` buckets by direct entry duplication — a view of a module preserves the module's
    /// keyworded dispatch surface as-is (keyword → keyword), it does not re-derive it from the
    /// value bindings. Snapshots the source maps and releases the source `Ref` before the replay
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
                .map(|(k, entry)| (k.clone(), entry.duplicate()))
                .collect();
            let functions: Vec<(UntypedKey, Vec<FunctionBucketEntry>)> = tables
                .functions
                .iter()
                .map(|(key, bucket)| (key.clone(), bucket.iter().map(|e| e.duplicate()).collect()))
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
    /// token-class partition guard and the cross-kind probe, inserts, and clears the matching
    /// value-side placeholder — all under one borrow.
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
        // `data`/`types` partition is structural, not convention.
        if tables.types.contains_key(name) {
            return Err(rebind());
        }
        if tables.data.contains_key(name) {
            return Err(rebind());
        }
        tables
            .data
            .insert(name.to_string(), DataEntry { index, sealed });
        tables.clear_placeholder(name, BindKind::Value);
        Ok(())
    }

    /// The `functions` write path: add `seal`'s callable to its dispatch bucket. The bucket key,
    /// dedupe token and diagnostic summary were all computed at seal time, where the callable was
    /// open — the write is pure table mutation, no carrier is opened and no bare reference crosses
    /// the door. Token equality against a bucket sibling raises `DuplicateOverload`. On success
    /// this binder's own `pending_overloads` entry is removed under the same borrow; siblings stay
    /// as wake sources.
    pub(crate) fn write_overload(
        &self,
        name: &str,
        index: BindingIndex,
        seal: OverloadSeal,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let mut tables = self.tables.borrow_mut();
        let bucket = tables.functions.entry(seal.key.clone()).or_default();
        if let Some(existing) = bucket.iter().find(|existing| existing.token == seal.token) {
            return Err(KError::new(KErrorKind::DuplicateOverload {
                name: name.to_string(),
                signature: existing.summary.clone(),
            }));
        }
        bucket.push(FunctionBucketEntry {
            index,
            token: seal.token,
            summary: seal.summary,
            sealed: seal.sealed,
        });
        tables.clear_pending_overload(&seal.key, index);
        Ok(())
    }

    /// Remove every value-side placeholder pointing at `producer`. The success write
    /// paths clear a binder's placeholder by name on finalize; this is the error-path
    /// companion, called when `producer`'s node finalizes with an error so a binder body
    /// that failed before its write path does not leak a scheduler-local [`NodeId`] into
    /// a later run on a persistent scope.
    pub fn clear_placeholders_for_producer(&self, producer: NodeId, _gate: &mut WriteGate) {
        self.tables
            .borrow_mut()
            .placeholders
            .retain(|_, (id, _, _)| *id != producer);
    }
}

impl Tables {
    /// Shared tail of every successful write path. Removes a *matching-kind* placeholder
    /// for `name`: a value write clears only a [`BindKind::Value`] entry, a type write only
    /// a [`BindKind::Type`] one, so a value bind never clears an in-flight type producer's
    /// placeholder (or the reverse). Runs under the write's own borrow, so a binding and its
    /// placeholder clear land together.
    fn clear_placeholder(&mut self, name: &str, kind: BindKind) {
        if matches!(self.placeholders.get(name), Some((_, _, k)) if *k == kind) {
            self.placeholders.remove(name);
        }
    }

    /// Bucket-keyed companion to [`Self::clear_placeholder`].
    /// Removes only the entry whose `BindingIndex` matches — sibling binders
    /// stay as wake sources. Empties drop the map entry.
    fn clear_pending_overload(&mut self, bucket: &UntypedKey, index: BindingIndex) {
        if let Some(entries) = self.pending_overloads.get_mut(bucket) {
            entries.retain(|(_, idx)| *idx != index);
            if entries.is_empty() {
                self.pending_overloads.remove(bucket);
            }
        }
    }
}

impl Default for Bindings {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
