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
//! Every table lives in the scope's own **region bump** — bucket arrays, keys, and the text an
//! entry carries alike — so dropping a table frees nothing and runs no per-entry glue, and frame
//! death walks O(scopes) rather than O(entries). [`bump_table`] carries the compile-time proof that
//! no entry brings drop glue with it; the write verbs re-home what they store through the brand
//! [`Bindings`] holds. A lookup is an O(1) hash probe either way — bump-backing costs it nothing.
//!
//! Every entry carries a [`BindingIndex`] naming its installing statement's lexical
//! position, gated by the strict cutoff `idx < c`, so a forward reference (a
//! later-positioned binding) is invisible — type binders included. A type entry pairs
//! that index with its installing [`Installer`] in a [`DeclarationSite`]: the installer
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
use std::mem::ManuallyDrop;

use crate::machine::CarrierWitness;
use crate::machine::ProducerId;
use crate::machine::core::RegionBrand;
use crate::machine::core::StatementId;
use crate::machine::core::carrier_witness::{
    GroupSeal, OverloadSeal, SealedFunction, SealedOperatorGroup,
};
use crate::machine::model::CarriedFamily;
use crate::machine::model::{KType, UntypedKey};
use crate::machine::model::{
    StoredDispatchTokenElement, StoredElement, StoredKeyProbe, UntypedKeyProbe, owned_untyped_key,
    restore_stored_key, store_untyped_key,
};
use crate::witnessed::BumpBackedMap;
use crate::witnessed::{BumpAllocator, Sealed};

use super::kerror::{KError, KErrorKind};

mod gate;
mod ops;

pub use gate::WriteGate;
pub(crate) use ops::{TypeWritePolicy, WriteOp, powerset_probes};

/// A value binding's dormant carrier: the bound value fused to the exact reach description minted
/// for it at bind time. The entry owns no pins — the binding scope's **region** owns the one deduped
/// union bundle that keeps every reached region alive for the region's life, so a read hands out a
/// bit-copy of this seal with no refcount traffic and the value can only be re-anchored under a pin
/// ([`Sealed::open_at`], the [`Delivered`](crate::machine::DeliveredCarried) lift).
pub type SealedValue<'home> = Sealed<'home, CarriedFamily, CarrierWitness>;

pub use crate::machine::model::BindKind;

/// Outcome of a single-scope name lookup: the name is `Bound` to a `T`, or `Parked` on the
/// [`ProducerId`] of an earlier still-finalizing binder for the name — the producer a consumer
/// wires its own park off, whose delivery is destined at this scope's region. A miss is the
/// enclosing `Option`'s `None` — the caller keeps walking ancestors — so "unbound" is not a
/// variant here; the terminal unbound disposition (with its diagnostic) is materialized one level
/// up on the resolution path ([`crate::machine::model::TypeResolution`] /
/// the execute-side `Resolution`).
///
/// Invariant: within one scope a value name is bound xor pending, never both — the two are arms of
/// one [`ValueSlot`], so the exclusivity is a type-level fact rather than cross-map discipline.
#[derive(Copy, Clone, Debug)]
pub enum NameLookup<T> {
    Bound(T),
    Parked(ProducerId),
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

    /// Map the bound payload, threading a `Parked` edge through unchanged — the combinator the
    /// carrier ladder uses to re-wrap a hit without restating the two-arm match.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> NameLookup<U> {
        match self {
            NameLookup::Bound(payload) => NameLookup::Bound(f(payload)),
            NameLookup::Parked(edge) => NameLookup::Parked(edge),
        }
    }
}

/// A still-finalizing binder occupying its destination slot: the [`ProducerId`] naming the binder's
/// own submission, tagged with the binder's lexical [`BindingIndex`] so the
/// same visibility predicate gates a pending arm and the binding it becomes. A consumer parks by
/// wiring its **own** edge off this one, inheriting the destination — which is what makes a
/// placeholder park deliver into the scope the name was claimed in. Installed at statement
/// submission ([`Bindings::install_placeholder`] / [`Bindings::install_pending_overload`]) and
/// overwritten in place by the producer's write path; the edge itself is released by the installing
/// slot when it terminalizes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PendingBinding {
    pub producer: ProducerId,
    pub index: BindingIndex,
}

/// One `data` slot: bound, or claimed by a still-finalizing binder. The two are exclusive by
/// construction — a value name is never pending and bound at once, and the enum is what says so.
pub(crate) enum ValueSlot<'a> {
    Bound(DataEntry<'a>),
    Pending(PendingBinding),
}

impl<'a> ValueSlot<'a> {
    /// The committed entry, or `None` for a still-finalizing binder.
    pub(crate) fn bound(&self) -> Option<&DataEntry<'a>> {
        match self {
            ValueSlot::Bound(entry) => Some(entry),
            ValueSlot::Pending(_) => None,
        }
    }

    /// The in-flight claim, or `None` once the slot is committed.
    pub(crate) fn pending(&self) -> Option<PendingBinding> {
        match self {
            ValueSlot::Bound(_) => None,
            ValueSlot::Pending(p) => Some(*p),
        }
    }
}

/// One `types` slot. Unlike [`ValueSlot`], bound and pending are **not** exclusive: a parallel
/// nominal finalize pre-installs the name's external identity while its producer is still in
/// flight, and the finalize gate parks on that binder's edge
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

    /// The in-flight claim, if any — the `Pending` and `BoundWithPending` arms.
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
pub(crate) enum OverloadSlot<'a> {
    Sealed(FunctionBucketEntry<'a>),
    Pending(PendingBinding),
}

impl<'a> OverloadSlot<'a> {
    /// The finalized overload, or `None` for a still-finalizing sibling.
    pub(crate) fn sealed(&self) -> Option<&FunctionBucketEntry<'a>> {
        match self {
            OverloadSlot::Sealed(entry) => Some(entry),
            OverloadSlot::Pending(_) => None,
        }
    }

    /// The in-flight claim, or `None` for a finalized overload.
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
pub(crate) struct DataEntry<'a> {
    index: BindingIndex,
    sealed: SealedValue<'a>,
}

impl<'a> DataEntry<'a> {
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
pub(crate) struct FunctionBucketEntry<'a> {
    pub(crate) index: BindingIndex,
    /// The stored form of the duplicate-overload predicate: an incoming callable whose token
    /// matches this run is `DuplicateOverload`. A bumped run rather than the owned
    /// [`DispatchToken`], so the entry carries no `Drop` and a bucket's death frees nothing.
    token: &'a [StoredDispatchTokenElement<'a>],
    /// The overload's rendered signature, for the `DuplicateOverload` diagnostic — bumped for
    /// [`Self::token`]'s reason.
    summary: &'a str,
    pub(crate) sealed: SealedFunction<'a>,
}

impl<'a> FunctionBucketEntry<'a> {
    /// A bit-copy of the entry, for the bulk-install snapshot — like [`DataEntry::duplicate`]. The
    /// bumped run and text copy as the borrows they are; only the seal needs a verb.
    fn duplicate(&self) -> Self {
        FunctionBucketEntry {
            index: self.index,
            token: self.token,
            summary: self.summary,
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
pub(crate) struct OperatorEntry<'a> {
    index: BindingIndex,
    /// The registered record's address — the upsert's cheap identity arm.
    address: usize,
    /// The registered record's rendered mode + member set — the upsert's structural arm, bumped so
    /// the entry carries no `Drop`.
    declaration: &'a str,
    sealed: SealedOperatorGroup<'a>,
}

/// The value-or-type a name resolves to in one classified result — for ATTR module/signature
/// member access. Produced by [`crate::machine::core::Scope::lookup_member`], which checks the
/// module-own value side then the type side in one call. The `data`/`types` cross-kind exclusion
/// keeps the two arms from ever both matching within a scope.
pub enum MemberResolution<'a> {
    /// The member's dormant carrier, duplicated off the module's own `data` entry — so an ATTR read
    /// replays the *stored* claim (value and reach as one unit) rather than re-asserting
    /// single-frame co-location.
    Value(SealedValue<'a>),
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
/// consumer parks on the earliest-index visible claim's edge; on wake it
/// re-dispatches and either picks from the now-live bucket or re-parks on the
/// next-earliest pending sibling.
pub struct FunctionLookup<'a> {
    /// The visible finalized overloads, each a bit-copy of the bucket's dormant carrier — value and
    /// proven reach as one unit, re-anchored only by an [`open`](crate::witnessed::Sealed::open_at)
    /// under a named pin. Copied out so no caller holds the `functions` borrow across a candidate
    /// walk.
    pub overloads: Vec<SealedFunction<'a>>,
    pub pending: Option<ProducerId>,
}

/// Lexical position of a binding's installing statement: a binding at `idx` is visible to a
/// consumer at cutoff `c` iff `idx < c`. Every binder — value and type alike — gates its
/// references against its own position, so a forward reference is a position error and
/// mutual recursion is expressed by co-declaring the types in a module body. `idx == 0` is the first
/// position (FN parameters, MATCH/TRY `it`) and also tags the builtins in the immutable
/// root — [`BindingIndex::BUILTIN`]; per-block indices restart inside nested blocks (see
/// [`crate::machine::core::scope::Scope::resolve`] for the predicate).
///
/// One plain index per entry: every scope a statement can write into owns its own table and
/// numbers its own statements, `USING` blocks included — the block's statements run in an owned
/// layer stacked inside the window ([`crate::machine::core::Scope::open_module_window`]), not in
/// the window itself.
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

/// What installed a binding — the identity a same-declaration check compares.
///
/// A statement-driven install names its statement, and nothing scheduler-shaped: a
/// [`StatementId`] is minted by koan, never recycled, and stays unique for longer than the
/// entry holding it lives. A slot or edge index could not stand in for it — both recycle, so
/// a later declaration can be handed a freed index and compare equal to the entry it should
/// be rebinding. An install no statement drove carries no id at all rather than a reserved
/// counter value, so it can never collide with a live statement.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Installer {
    /// Registered by no statement — builtin installs, and bindings a scope takes on at
    /// birth. All such installs share this one identity, so re-registering a name under
    /// [`TypeWritePolicy::UpsertEqual`] upserts rather than raising `Rebind`.
    NoStatement,
    /// Installed by the statement submitted as `statement`.
    Statement(StatementId),
}

/// The identity of the declaration statement that installed a `types` entry: its
/// [`Installer`] (the identity signal — same-declaration checks compare only this) plus its
/// lexical position (the visibility signal — `idx < cutoff` reads it, independent of installer
/// identity: builtins sit at `idx 0` with no statement behind them, and a persistent scope's
/// separate runs can each land a declaration at the same index).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct DeclarationSite {
    pub installer: Installer,
    pub index: BindingIndex,
}

impl DeclarationSite {
    /// Builtin registration: no statement installed it.
    pub const BUILTIN: DeclarationSite = DeclarationSite {
        installer: Installer::NoStatement,
        index: BindingIndex::BUILTIN,
    };

    /// A binding installed when its scope is **born**, rather than by a declaration statement
    /// running in it — a type-denoting FN parameter landing in the fresh per-call scope, an
    /// ascription seeding the newborn view scope's type members. No statement installed it, so the
    /// installer is [`Installer::NoStatement`] and same-declaration checks never key on a
    /// statement; the index is `value(0)`, the parameter position the visibility predicate admits.
    pub const AT_CONSTRUCTION: DeclarationSite = DeclarationSite {
        installer: Installer::NoStatement,
        index: BindingIndex::value(0),
    };
}

/// Every lexical binding of one scope, in one cell. A still-finalizing binder lives in the table
/// it will resolve into — as a pending arm of the very slot it claims — so a name is looked up in
/// one probe and finalization overwrites the slot rather than moving between containers. `data`
/// and `types` park by name (value/type forward references); `functions` parks by full dispatch
/// bucket key, which keeps `(MAKESET _)` and `(MAKESET _ USING _)` from colliding.
///
/// Every table is a `hashbrown` map over the scope's own region bump ([`BumpBackedMap`]) with
/// bumped `&'a str` / `&'a [StoredElement]` keys, so a table's death frees nothing and walks no
/// entry — which is what lets frame teardown cost O(scopes) rather than O(entries). Lookup is the
/// same O(1) hash probe a std map would run.
struct Tables<'a> {
    /// Each bound type slot stores its type and its [`DeclarationSite`] — the installing
    /// [`Installer`] (declaration identity) plus its lexical [`BindingIndex`] (visibility). A
    /// `KType` is a `Copy` handle into the run frame's registry, so a slot carries no reach: a
    /// read copies the handle under the home-frame pin alone, and the same handle names the same
    /// type in every region. A [`TypeSlot`] may carry an in-flight producer beside the bound
    /// identity — see its doc for why the two coexist here and not in `data`.
    types: BumpBackedMap<'a, &'a str, TypeSlot>,
    /// Each bound value slot stores its value fused to its exact reach in one dormant
    /// [`SealedValue`], plus its lexical [`BindingIndex`]. Reads hand out a bit-copy of the seal
    /// ([`Bindings::lookup_value`]) and re-anchor the value only under a pin, so a read replays the
    /// stored claim rather than re-asserting single-frame co-location. Description members are
    /// `Weak`, and the owning pins live in the region's union bundle rather than here — either a
    /// strong member or a per-entry `Rc` on the scope's own frame would close a
    /// `frame → region → scope → bindings → frame` cycle and leak the region.
    data: BumpBackedMap<'a, &'a str, ValueSlot<'a>>,
    /// Each sealed bucket slot stores its callable fused to its reach claim in one dormant
    /// [`SealedFunction`], beside the precomputed data the write path dedupes on
    /// ([`FunctionBucketEntry`]). Written only by the `FN` / `OP` registration doors — an `FN`
    /// registration binds no value, and a value bind writes no bucket. Like `data`, the entry
    /// owns nothing: the reached regions are held by the region's union bundle, and a read hands
    /// out a bit-copy the caller re-anchors under a pin. Sibling binders that have dispatched but
    /// not finalized sit in the same bucket as [`OverloadSlot::Pending`]; consumers park on the
    /// earliest-index visible one, and a finalize overwrites only its own slot, leaving the other
    /// siblings as wake sources.
    /// Keyed on the **stored** run rather than an owned [`UntypedKey`] so a node dispatching
    /// through its own bumped key probes without materializing one; an owned key probes the same
    /// bucket through [`UntypedKeyProbe`].
    functions: BumpBackedMap<'a, &'a [StoredElement<'a>], Bucket<'a>>,
    /// Per-scope operator registry: a chain's sorted-joined operator probe key → the dormant
    /// [`SealedOperatorGroup`] carrier of the group it resolves to, beside its lexical
    /// [`BindingIndex`] ([`OperatorEntry`] — the `data`/`functions` entry shape). A module installs
    /// one entry per nonempty subset of its declared operators (the per-group powerset), each
    /// subset key holding a bit-copy of the same seal over the same region-hosted record, so any
    /// subset used in one expression resolves in a single hit, a cross-group mix simply misses, and
    /// the whole install allocates nothing past its probe keys. Walked through the scope chain like
    /// every other name (innermost visible wins).
    operators: BumpBackedMap<'a, &'a str, OperatorEntry<'a>>,
}

/// One dispatch bucket: the overload slots sharing a key, in a vec whose buffer is bump-backed like
/// every other table allocation here.
///
/// `ManuallyDrop` because the vec has a destructor and running it would be pure waste: its elements
/// carry no glue (the assert below), and its buffer is bump memory the region releases whole.
/// Suppressing it is also what makes `needs_drop::<Bucket>()` false, so the map holding buckets
/// passes [`bump_table`]'s assert and its own teardown never walks them. No `unsafe` — the
/// suppressed destructor had nothing to do.
type Bucket<'a> = ManuallyDrop<allocator_api2::vec::Vec<OverloadSlot<'a>, BumpAllocator<'a>>>;

/// The element proof the `ManuallyDrop` above would otherwise swallow. `needs_drop` is false for
/// *any* `ManuallyDrop<U>`, so the wrapper that makes a bucket storable in a bump-backed table also
/// makes [`bump_table`]'s entry assert say nothing about what the bucket holds. Stated here instead,
/// against the element type directly, so an `OverloadSlot` arm that later brings a destructor back
/// fails the build rather than leaking silently.
const _: () = assert!(!std::mem::needs_drop::<OverloadSlot<'static>>());

/// Build one of a scope's tables over its region bump, **proving at compile time** that its entries
/// carry no drop glue. The bump runs no destructor, so a `Drop`-bearing key or value would silently
/// leak whatever it owns; the assert is monomorphization-checked, so a future entry field that
/// brings glue back is a build error at the declaration that admitted it rather than a leak.
///
/// This is where each table's storage choice is stated: all five tables route here, so none has an
/// unstated exemption.
pub(in crate::machine::core) fn bump_table<'a, K, V>(
    brand: RegionBrand<'a>,
) -> BumpBackedMap<'a, K, V> {
    const {
        assert!(
            !std::mem::needs_drop::<K>() && !std::mem::needs_drop::<V>(),
            "a bump-backed table's entries must carry no drop glue: the bump runs no destructor",
        )
    };
    hashbrown::HashMap::with_hasher_in(hashbrown::DefaultHashBuilder::default(), brand.allocator())
}

/// An empty dispatch bucket over the same bump — the `functions` table's value constructor.
fn bump_bucket(brand: RegionBrand<'_>) -> Bucket<'_> {
    ManuallyDrop::new(allocator_api2::vec::Vec::new_in(brand.allocator()))
}

/// One scope's bindings: the four maps under a single [`RefCell`], and nothing else.
///
/// One cell rather than one per map: with writes reachable only under a [`WriteGate`], a read can
/// never overlap a write, so per-map cells bought nothing but a borrow-ordering rule to obey. Every
/// verb takes exactly one borrow, and a cross-map write — a value insert screened against `types`,
/// a type insert screened against `data` — is atomic under it.
///
/// The brand rides beside the cell because every write re-homes what it stores: a key's text, an
/// overload's summary and dispatch token all land in the same region the tables' buckets do, so a
/// table never points at bytes that can die before it.
pub struct Bindings<'a> {
    brand: RegionBrand<'a>,
    /// `ManuallyDrop` for [`Bucket`]'s reason, one level up: a `hashbrown` map has a destructor
    /// whose only act is to hand its bucket array back to the allocator, which for a bump-backed
    /// table is a no-op. Suppressing it is what makes [`Bindings`] contribute **zero** drop glue to
    /// the `Scope` holding it — the assert below is the proof, and it is what a scope skipping its
    /// own destructor rests on.
    tables: RefCell<ManuallyDrop<Tables<'a>>>,
}

/// A scope's binding state carries **no drop glue at all** — not an entry walk, not a vacuous free
/// of a bucket array the bump owns. Stated as a compile-time fact rather than a comment, so a future
/// entry field or table type that brings a destructor back fails the build here, and so
/// [`Scope`](crate::machine::core::Scope)'s own remaining `Drop` provably names no binding-table
/// state.
const _: () = assert!(!std::mem::needs_drop::<Bindings<'static>>());

impl<'a> Bindings<'a> {
    /// Empty tables over `brand`'s region. There is no `Default`: a binding table cannot exist
    /// without the region its storage lives in.
    pub fn new(brand: RegionBrand<'a>) -> Self {
        Self {
            brand,
            tables: RefCell::new(ManuallyDrop::new(Tables {
                types: bump_table(brand),
                data: bump_table(brand),
                functions: bump_table(brand),
                operators: bump_table(brand),
            })),
        }
    }

    /// Per-scope value-side lookup. One probe of `data[name]`: a visible bound slot answers
    /// `Bound`, a visible pending slot answers `Parked` on its claim's edge. `cutoff = None` means the
    /// scope is off-chain (or unfiltered) — everything is visible. `None` return
    /// means no visible entry at this scope; the caller keeps walking
    /// ancestors, and chain exhaustion stays `None` (the terminal unbound
    /// disposition is materialized on the resolution path, not here).
    pub fn lookup_value(
        &self,
        name: &str,
        cutoff: Option<usize>,
    ) -> Option<NameLookup<SealedValue<'a>>> {
        match self.tables.borrow().data.get(name)? {
            ValueSlot::Bound(entry) => Self::visible(entry.index, cutoff)
                .then(|| NameLookup::Bound(entry.sealed.duplicate())),
            ValueSlot::Pending(p) => {
                Self::visible(p.index, cutoff).then_some(NameLookup::Parked(p.producer))
            }
        }
    }

    /// Per-scope type-side lookup. The type-language mirror of [`Self::lookup_value`]: one probe of
    /// `types[name]`, preferring the slot's bound arm over its pending one, returning the first
    /// visible hit as a [`NameLookup`], or `None` so the caller keeps walking. Bound-preferred is
    /// load-bearing: on a slot carrying both, a consumer that can read the identity must not park.
    pub fn lookup_type(&self, name: &str, cutoff: Option<usize>) -> Option<NameLookup<KType>> {
        let tables = self.tables.borrow();
        let slot = tables.types.get(name)?;
        if let Some((kt, site)) = slot.bound()
            && Self::visible(site.index, cutoff)
        {
            return Some(NameLookup::Bound(kt));
        }
        slot.pending()
            .filter(|p| Self::visible(p.index, cutoff))
            .map(|p| NameLookup::Parked(p.producer))
    }

    /// Classified per-scope member lookup for ATTR module / signature access: the value-or-type
    /// `name` resolves to, read from **this scope's own** `data` then `types` in one pass. A
    /// module member is module-own — the lookup deliberately does **not** consult the builtin
    /// root or walk lexical ancestors, so `m.Type` (a builtin type name) or `m.SomeOuterType`
    /// is "no member", not a fall-through. The cross-kind exclusion keeps the two arms from both
    /// matching, so the result is unambiguous. Bound arms only — a read module is finalized, so a
    /// pending arm never surfaces here.
    pub fn lookup_member(&self, name: &str, cutoff: Option<usize>) -> Option<MemberResolution<'a>> {
        let tables = self.tables.borrow();
        if let Some(entry) = tables.data.get(name).and_then(ValueSlot::bound)
            && Self::visible(entry.index, cutoff)
        {
            return Some(MemberResolution::Value(entry.sealed.duplicate()));
        }
        if let Some((kt, site)) = tables.types.get(name).and_then(TypeSlot::bound)
            && Self::visible(site.index, cutoff)
        {
            return Some(MemberResolution::Type { kt });
        }
        None
    }

    /// The [`ProducerId`] of a still-finalizing **type** binder named `name`, read straight from
    /// the slot's pending arm — *not* through [`Self::lookup_type`], which prefers the (possibly
    /// seal-pre-installed, still-unsealed) bound arm. The finalize gate uses this to park the
    /// type-identifier memo on an in-flight binder even when the seal has already pre-installed
    /// the name's external identity into `types` — the [`TypeSlot::BoundWithPending`] case.
    /// Visibility-unfiltered: this is dependency tracking, not consumer-visibility enforcement.
    pub fn type_placeholder_producer(&self, name: &str) -> Option<ProducerId> {
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
    pub fn lookup_function(&self, key: &UntypedKey, cutoff: Option<usize>) -> FunctionLookup<'a> {
        self.lookup_function_probe(&UntypedKeyProbe(key), cutoff)
    }

    /// [`Self::lookup_function`] from a node's **own** bumped key — the dispatch hot path, which
    /// reads the run the node already carries instead of materializing an owned key per call.
    pub fn lookup_function_stored(
        &self,
        key: &[StoredElement<'_>],
        cutoff: Option<usize>,
    ) -> FunctionLookup<'a> {
        self.lookup_function_probe(&StoredKeyProbe(key), cutoff)
    }

    /// The one bucket read, over whichever key form the caller holds: hashbrown resolves both
    /// through `Equivalent`, and the two forms hash identically by construction (see
    /// [`UntypedKeyProbe`]).
    fn lookup_function_probe<Q>(&self, key: &Q, cutoff: Option<usize>) -> FunctionLookup<'a>
    where
        Q: std::hash::Hash + hashbrown::Equivalent<&'a [StoredElement<'a>]> + ?Sized,
    {
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
            .filter(|entry| Self::visible(entry.index, cutoff))
            .map(|entry| entry.sealed.duplicate())
            .collect();
        // Earliest-index visible claim: most likely to finalize first.
        let pending = bucket
            .iter()
            .filter_map(OverloadSlot::pending)
            .filter(|p| Self::visible(p.index, cutoff))
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
        cutoff: Option<usize>,
    ) -> Option<SealedOperatorGroup<'a>> {
        let tables = self.tables.borrow();
        let entry = tables.operators.get(probe)?;
        Self::visible(entry.index, cutoff).then(|| entry.sealed.duplicate())
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
        seal: &GroupSeal<'a>,
        index: BindingIndex,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let mut tables = self.tables.borrow_mut();
        if let Some(entry) = tables.operators.get(probe.as_str()) {
            if entry.address == seal.address || entry.declaration == seal.declaration {
                return Ok(());
            }
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "operator `{probe}` is already declared in this scope with a different \
                 chaining mode or member set; one scope declares one chaining mode per operator",
            ))));
        }
        // Key and declaration re-home on the insert alone. A powerset install bumps the declaration
        // once per subset entry; the byte cost is bounded by the group's own powerset, which is
        // small, and cross-call sharing would cost an intern table to save it.
        tables.operators.insert(
            self.brand.allocator().text(&probe),
            OperatorEntry {
                index,
                address: seal.address,
                declaration: self.brand.allocator().text(&seal.declaration),
                sealed: seal.sealed.duplicate(),
            },
        );
        Ok(())
    }

    /// Snapshot every bound `(name, dormant carrier)` pair in `data`, ignoring visibility. Each
    /// seal is a bit-copy; the caller re-anchors what it needs under its own pin. Pending slots are
    /// invisible to bulk reads — there is no carrier to hand out. For chain-gated single-name
    /// reads use [`Self::lookup_value`].
    pub fn iter_data(&self) -> Vec<(String, SealedValue<'a>)> {
        self.tables
            .borrow()
            .data
            .iter()
            .filter_map(|(name, slot)| Some((name.to_string(), slot.bound()?.sealed.duplicate())))
            .collect()
    }

    /// Snapshot every bound `(name, KType)` pair in `types`, ignoring visibility.
    pub fn iter_types(&self) -> Vec<(String, KType)> {
        self.tables
            .borrow()
            .types
            .iter()
            .filter_map(|(name, slot)| Some((name.to_string(), slot.bound()?.0)))
            .collect()
    }

    /// Snapshot every `(UntypedKey, Vec<SealedFunction>)` pair in `functions`, ignoring per-overload
    /// visibility. Each seal is a bit-copy; the caller re-anchors what it needs under its own pin.
    /// Sealed slots only, and a bucket holding none is skipped — a key claimed by pending siblings
    /// alone publishes no dispatch surface to snapshot. For chain-gated picks use
    /// [`Self::lookup_function`].
    pub fn iter_functions(&self) -> Vec<(UntypedKey, Vec<SealedFunction<'a>>)> {
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
                (!sealed.is_empty()).then(|| (owned_untyped_key(key), sealed))
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
        self.has_builtin_function_probe(&UntypedKeyProbe(key))
    }

    /// [`Self::has_builtin_function`] from a node's own bumped key — the no-shadow consult's hot
    /// path, paired with [`Self::lookup_function_stored`].
    pub fn has_builtin_function_stored(&self, key: &[StoredElement<'_>]) -> bool {
        self.has_builtin_function_probe(&StoredKeyProbe(key))
    }

    fn has_builtin_function_probe<Q>(&self, key: &Q) -> bool
    where
        Q: std::hash::Hash + hashbrown::Equivalent<&'a [StoredElement<'a>]> + ?Sized,
    {
        self.tables.borrow().functions.get(key).is_some_and(|b| {
            b.iter()
                .filter_map(OverloadSlot::sealed)
                .any(|e| e.index == BindingIndex::BUILTIN)
        })
    }

    /// Visibility predicate — the only place a cutoff is applied. `cutoff = None` (the reader is
    /// off this scope's chain, so the scope is complete) ⇒ visible; `Some(c)` ⇒ `idx < c`.
    fn visible(b: BindingIndex, cutoff: Option<usize>) -> bool {
        match cutoff {
            None => true,
            Some(c) => b.idx < c,
        }
    }

    #[cfg(test)]
    pub(crate) fn data(&self) -> Ref<'_, BumpBackedMap<'a, &'a str, ValueSlot<'a>>> {
        Ref::map(self.tables.borrow(), |t| &t.data)
    }

    #[cfg(test)]
    pub(crate) fn functions(
        &self,
    ) -> Ref<'_, BumpBackedMap<'a, &'a [StoredElement<'a>], Bucket<'a>>> {
        Ref::map(self.tables.borrow(), |t| &t.functions)
    }

    #[cfg(test)]
    pub(crate) fn types(&self) -> Ref<'_, BumpBackedMap<'a, &'a str, TypeSlot>> {
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
    pub fn pending_names(&self) -> Vec<(String, BindKind, ProducerId)> {
        let tables = self.tables.borrow();
        let values = tables
            .data
            .iter()
            .filter_map(|(n, s)| Some((n.to_string(), BindKind::Value, s.pending()?.producer)));
        let types = tables
            .types
            .iter()
            .filter_map(|(n, s)| Some((n.to_string(), BindKind::Type, s.pending()?.producer)));
        values.chain(types).collect()
    }

    /// Every pending sibling in one dispatch bucket, in slot order.
    #[cfg(test)]
    pub fn pending_overload_entries(&self, bucket: &UntypedKey) -> Vec<PendingBinding> {
        self.tables
            .borrow()
            .functions
            .get(&UntypedKeyProbe(bucket))
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
    /// declaration — declaration identity is the installing [`Installer`], so an existing entry
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
            (TypeWritePolicy::UpsertEqual, Some(existing))
                if existing.installer != site.installer =>
            {
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
                    .insert(self.brand.allocator().text(name), TypeSlot::Bound(kt, site));
            }
        }
        Ok(())
    }

    /// Claim `name`'s slot in its destination table for the binder edge `edge` — the dispatch-time
    /// forward-reference stamp. `edge` is the slot's own installed edge, destined at this scope's
    /// region, so a consumer parking on the claim inherits that destination.
    ///
    /// Errors `Rebind` if the claim collides: a committed `data[name]` (bindings are bind-once), or
    /// an existing pending arm naming a different edge. Idempotent on same-edge re-entry.
    /// A `types` slot already carrying a bound identity keeps it and gains the pending arm
    /// ([`TypeSlot::BoundWithPending`]): a parallel nominal finalize pre-installs the external
    /// identity while its binder is still in flight.
    ///
    /// The eventual [`Self::write_value`] / [`Self::write_type`] call must carry the
    /// same `index` so the consumer's visibility test stays consistent across
    /// the pending → finalized transition. `kind` picks the destination table, so a value bind
    /// never satisfies a type claim (or the reverse) — see [`Bindings::lookup_value`] /
    /// [`Bindings::lookup_type`], each of which probes only its own table.
    pub fn install_placeholder(
        &self,
        name: &str,
        producer: ProducerId,
        index: BindingIndex,
        kind: BindKind,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let mut tables = self.tables.borrow_mut();
        let claim = PendingBinding { producer, index };
        let rebind = || {
            KError::new(KErrorKind::Rebind {
                name: name.to_string(),
            })
        };
        match kind {
            BindKind::Value => match tables.data.get_mut(name) {
                Some(ValueSlot::Bound(_)) => Err(rebind()),
                Some(ValueSlot::Pending(existing)) if existing.producer == producer => Ok(()),
                Some(ValueSlot::Pending(_)) => Err(rebind()),
                None => {
                    tables
                        .data
                        .insert(self.brand.allocator().text(name), ValueSlot::Pending(claim));
                    Ok(())
                }
            },
            BindKind::Type => match tables.types.get_mut(name) {
                Some(slot) => match slot.pending() {
                    Some(existing) if existing.producer == producer => Ok(()),
                    Some(_) => Err(rebind()),
                    None => {
                        let (kt, site) = slot
                            .bound()
                            .expect("a TypeSlot with no pending arm is Bound");
                        *slot = TypeSlot::BoundWithPending(kt, site, claim);
                        Ok(())
                    }
                },
                None => {
                    tables
                        .types
                        .insert(self.brand.allocator().text(name), TypeSlot::Pending(claim));
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
        producer: ProducerId,
        index: BindingIndex,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let mut tables = self.tables.borrow_mut();
        let claim = OverloadSlot::Pending(PendingBinding { producer, index });
        // Probe-then-insert rather than an `entry` call: the key a miss inserts has to be re-homed
        // through the brand, which the entry API has no way to defer. The second hash is paid only
        // on the first claim of a shape.
        match tables.functions.get_mut(&UntypedKeyProbe(&bucket)) {
            Some(slots) => slots.push(claim),
            None => {
                let key = store_untyped_key(self.brand, &bucket);
                let mut slots = bump_bucket(self.brand);
                slots.push(claim);
                tables.functions.insert(key, slots);
            }
        }
        Ok(())
    }

    /// Replay another `Bindings`'s bound `data` entries through [`Self::write_value`] on self, and
    /// its sealed `functions` slots by direct entry duplication — a view of a module preserves the
    /// module's keyworded dispatch surface as-is (keyword → keyword), it does not re-derive it from
    /// the value bindings. Pending arms are not replayed: a claim names a producer in the source's
    /// own scheduler run, so copying one would hand the target a park on a node that will never
    /// wake it. The `types` table is not replayed: a view's type interface is its own, seeded by
    /// [`Scope::alloc_module_view`](crate::machine::core::Scope) from the ascribed signature rather
    /// than inherited from the source. Snapshots the source maps and releases the source `Ref`
    /// before the replay so re-entrant ascription cannot deadlock.
    pub(crate) fn bulk_install_from(
        &self,
        src: &Bindings<'a>,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        // Duplicate each entry into the snapshot: each seal is a bit-copy naming the source's own
        // minted description, so the replayed entry replays that same claim. The reached regions
        // stay owned by the *source* scope's region union — the replay target's own region must
        // already outlive it (a bulk install is same-run re-entrant ascription). The snapshot's
        // keys are `Copy` borrows into the source's region and outlive the `Ref` they were read
        // under, so nothing is cloned to release the borrow.
        let (data, functions) = {
            let tables = src.tables.borrow();
            let data: Vec<(&str, DataEntry)> = tables
                .data
                .iter()
                .filter_map(|(k, slot)| Some((*k, slot.bound()?.duplicate())))
                .collect();
            let functions: Vec<(&[StoredElement<'_>], Vec<OverloadSlot<'a>>)> = tables
                .functions
                .iter()
                .filter_map(|(key, bucket)| {
                    let sealed: Vec<OverloadSlot<'a>> = bucket
                        .iter()
                        .filter_map(OverloadSlot::sealed)
                        .map(|e| OverloadSlot::Sealed(e.duplicate()))
                        .collect();
                    (!sealed.is_empty()).then_some((*key, sealed))
                })
                .collect();
            (data, functions)
        };
        for (name, entry) in data {
            self.write_value(name, entry.index, entry.sealed, gate)?;
        }
        let mut tables = self.tables.borrow_mut();
        for (key, slots) in functions {
            // The key is re-homed into *this* table's region, matching the value replay above —
            // `write_value` re-homes its name through the brand, so both tables end up keyed on
            // bytes the target owns. It buys no independence from the source region and is not
            // trying to: everything else a replayed entry carries — the sealed carrier, the
            // dispatch token, the summary — stays a borrow into the source, which is sound for the
            // reason stated at the snapshot above, and is why re-homing the key is symmetry rather
            // than a guard. The relation is held by **retention, not by `'a`**: `'a` covers both
            // regions and orders neither, but the view module escaping
            // [`Scope::alloc_module_view`](crate::machine::core::Scope) composes the source
            // module's reach into its own region, so the source is pinned for as long as the view
            // is reachable at all. A read of these bytes past the source's death is therefore
            // unrepresentable rather than merely unobserved.
            match tables.functions.get_mut(key) {
                Some(bucket) => bucket.extend(slots),
                None => {
                    let mut bucket = bump_bucket(self.brand);
                    bucket.extend(slots);
                    tables
                        .functions
                        .insert(restore_stored_key(self.brand, key), bucket);
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
        sealed: SealedValue<'a>,
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
                    self.brand.allocator().text(name),
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
        seal: OverloadSeal<'a>,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let mut tables = self.tables.borrow_mut();
        // A builtin seed, a direct registration and a bulk install all reach a shape no binder
        // claimed, so the miss arm re-homes the key and seeds an empty bucket; an FN binder's own
        // claim already created it.
        if !tables.functions.contains_key(&UntypedKeyProbe(&seal.key)) {
            let key = store_untyped_key(self.brand, &seal.key);
            tables.functions.insert(key, bump_bucket(self.brand));
        }
        let bucket = tables
            .functions
            .get_mut(&UntypedKeyProbe(&seal.key))
            .expect("the bucket was just seeded if it was missing");
        // Dedupe against the stored runs where they sit — no allocation to decide it, and the
        // incoming token is re-homed only once it has passed.
        if let Some(existing) = bucket
            .iter()
            .filter_map(OverloadSlot::sealed)
            .find(|existing| seal.token.matches_stored(existing.token))
        {
            return Err(KError::new(KErrorKind::DuplicateOverload {
                name: name.to_string(),
                signature: existing.summary.to_string(),
            }));
        }
        let entry = FunctionBucketEntry {
            index,
            token: seal.token.store_in(self.brand),
            summary: self.brand.allocator().text(&seal.summary),
            sealed: seal.sealed,
        };
        // The claim this write finalizes, if the binder made one; a builtin seed, a direct
        // registration or a bulk install has none and simply appends.
        match bucket
            .iter()
            .position(|slot| slot.pending().is_some_and(|p| p.index == index))
        {
            Some(at) => bucket[at] = OverloadSlot::Sealed(entry),
            None => bucket.push(OverloadSlot::Sealed(entry)),
        }
        Ok(())
    }

    /// Drop every pending arm naming one of the given producers, in all three claim-bearing tables — the
    /// retirement companion to the installs, run when the claiming slot terminalizes. The success
    /// write paths finalize a binder's own claim in place, so this normally finds only what a
    /// failed body left behind; running it on every terminal is what guarantees no arm survives
    /// naming a [`ProducerId`] whose edge its owner is about to release. A `types` slot that also
    /// holds a bound identity keeps it — only the pending arm is dropped. One bucket-keyed binder
    /// claims a slot in every inner-call bucket it declares, so the `functions` walk purges across
    /// all of them and drops a bucket the purge empties.
    ///
    /// The purge keys on the [`PendingBinding::producer`] each slot already carries; no table's
    /// key participates. The argument is a slot's own claim list, so it is short and a linear scan
    /// of it per slot is the whole cost.
    ///
    /// This is the one path that strands bump bytes: a removed key's text and an emptied bucket's
    /// buffer are abandoned rather than freed. It is bounded by the number of binders that fail —
    /// every success path overwrites its claim where it sits — so a table's peak occupancy stays
    /// its final binding count plus that error tail.
    pub fn clear_placeholders_for_producers(
        &self,
        producers: &[ProducerId],
        _gate: &mut WriteGate,
    ) {
        let mut tables = self.tables.borrow_mut();
        let named = |p: &PendingBinding| producers.contains(&p.producer);
        let claims = |slot: Option<PendingBinding>| slot.as_ref().is_some_and(named);
        tables.data.retain(|_, slot| !claims(slot.pending()));
        tables.types.retain(|_, slot| match slot {
            TypeSlot::Pending(p) => !named(p),
            TypeSlot::BoundWithPending(kt, site, p) if named(p) => {
                *slot = TypeSlot::Bound(*kt, *site);
                true
            }
            _ => true,
        });
        tables.functions.retain(|_, bucket| {
            bucket.retain(|slot| !claims(slot.pending()));
            !bucket.is_empty()
        });
    }
}

#[cfg(test)]
mod tests;
