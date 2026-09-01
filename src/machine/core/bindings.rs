//! Lexical binding façade: one `RefCell<Tables>` — `types`, `data`, `functions`, `operators`, and
//! the [`ClaimStore`] beside them — behind validated write paths. The four maps hold **committed
//! bindings only**; a still-finalizing binder's claim lives in the store ([`claims`]), so a lookup
//! answers "bound" from its own table's probe and "parked" from the store's, and each table states
//! its own exclusivity rule with no in-flight arm to admit. `data` and `functions` are
//! separate surfaces: a `data` entry is a value binding, callable by **name** alone (the
//! `FunctionValueCall` lane), while a `functions` bucket holds the keyworded overloads a `FN` /
//! `OP` registration installs — binding a function *value* publishes no keyworded expression.
//! Nominal type declarations (NEWTYPE / UNION / SIG) install their identity into `types` only —
//! there is no value-side carrier; a module is a value and binds into `data`. The `data` and
//! `types` maps are a structural partition, and the key types are what enforce it: `data` is keyed
//! by [`ValueSymbol`] and `types` by [`TypeSymbol`], newtypes minted only from text of their own
//! token class, so a name reaching both maps is unrepresentable rather than rejected. A name that
//! classifies in neither is rejected where the text is classified — the declaration seams.
//!
//! Every write verb here takes a [`WriteGate`] — the zero-sized capability whose constructors are
//! `pub(in crate::machine)`. A builtin body cannot mint one, so it cannot name a write verb: the
//! write discipline is a resolution rule, not a convention. That is what lets the verbs take firm
//! `borrow_mut`s — no koan frame is on the stack to hold a competing borrow, so contention is
//! unrepresentable. See [`gate`] for the capability, [`ops`] for the currency, and
//! [design/memory-model.md](../../../design/memory-model.md).
//!
//! There is no borrow order to keep: every verb takes exactly one borrow of the one cell, and a
//! cross-map write is atomic under it.
//!
//! Every table lives in the scope's own **region bump** — bucket arrays and the text an entry
//! carries alike — so dropping a table frees nothing and runs no per-entry glue, and frame death
//! walks O(scopes) rather than O(entries). [`bump_table`] carries the compile-time proof that no
//! entry brings drop glue with it; the write verbs re-home the text they store through the brand
//! [`Bindings`] holds. The name-keyed tables key by a `Copy` [`Symbol`](crate::machine::model::Symbol)
//! digest under the identity
//! hasher, so a name lookup is a `u128` compare rather than a byte-wise one and a key re-homes
//! nothing at all.
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
//! [`Bindings::lookup_type`] / [`Bindings::lookup_function_stored`], passing a
//! `chain_cutoff` computed via [`crate::machine::core::LexicalFrame::index_for`].
//! Raw map accessors are `#[cfg(test)]`.

use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::vec::Vec as AllocVec;
#[cfg(test)]
use std::cell::Ref;
use std::cell::{Cell, RefCell};
use std::hash::BuildHasher;
use std::mem::ManuallyDrop;

use crate::machine::CarrierWitness;
use crate::machine::DeliveredCarried;
use crate::machine::ProducerId;
use crate::machine::core::RegionBrand;
use crate::machine::core::StatementId;
use crate::machine::core::carrier_witness::{
    DeliveredFunction, DeliveredOperatorGroup, GroupSeal, OverloadSeal, SealedFunction,
    SealedOperatorGroup,
};
#[cfg(test)]
use crate::machine::model::BindKind;
use crate::machine::model::CarriedFamily;
use crate::machine::model::object_copy_cost;
use crate::machine::model::{
    BinderSymbol, IdentityBuildHasher, KeywordSymbol, RunRegistries, TypeSymbol, ValueSymbol,
    render_label,
};
use crate::machine::model::{
    DispatchTokenElement, KeyElement, render_untyped_key, summarize_dispatch,
};
use crate::machine::model::{KType, UntypedKey};
use crate::witnessed::BumpBackedMap;
use crate::witnessed::{BumpAllocator, Sealed};

use super::kerror::{KError, KErrorKind};

mod claims;
mod gate;
mod ops;

pub use claims::Claim;
pub(crate) use claims::ClaimStore;
pub use gate::WriteGate;
pub(crate) use ops::{TypeWritePolicy, WriteOp, powerset_probes};

/// A value binding's dormant carrier: the bound value fused to the exact reach description minted
/// for it at bind time. The entry owns no pins — the binding scope's **region** owns the one deduped
/// union bundle that keeps every reached region alive for the region's life, so a read hands out a
/// bit-copy of this seal with no refcount traffic and the value can only be re-anchored under a pin
/// ([`Sealed::open_at`], the [`Delivered`](crate::machine::DeliveredCarried) lift).
pub type SealedValue<'home> = Sealed<'home, CarriedFamily, CarrierWitness>;

/// Outcome of a single-scope name lookup: the name is `Bound` to a `T`, or `Parked` on the
/// [`ProducerId`] of an earlier still-finalizing binder for the name — the producer a consumer
/// wires its own park off, whose delivery is destined at this scope's region. A miss is the
/// enclosing `Option`'s `None` — the caller keeps walking ancestors — so "unbound" is not a
/// variant here; the terminal unbound disposition (with its diagnostic) is materialized one level
/// up on the resolution path ([`crate::machine::model::TypeResolution`] /
/// the execute-side `Resolution`).
///
/// A `Bound` reads the name's own table; a `Parked` reads the [`ClaimStore`] beside it. The two
/// live in different structures, each answering its own question, which is why the value side needs
/// no exclusivity rule spanning them: a value name's claim is removed by the very commit that binds
/// it.
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
    /// matches this run is `DuplicateOverload`, and the diagnostic's text is rendered from this
    /// run. A bumped run rather than the owned [`DispatchToken`], so the entry carries no `Drop`
    /// and a bucket's death frees nothing.
    token: &'a [DispatchTokenElement],
    pub(crate) sealed: SealedFunction<'a>,
}

impl<'a> FunctionBucketEntry<'a> {
    /// A bit-copy of the entry, for the bulk-install snapshot — like [`DataEntry::duplicate`]. The
    /// bumped run copies as the borrow it is; only the seal needs a verb.
    fn duplicate(&self) -> Self {
        FunctionBucketEntry {
            index: self.index,
            token: self.token,
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
/// module-own value side then the type side in one call. The probe's own class picks the map, so
/// the two arms are exclusive by construction rather than by a checked order.
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

/// Outcome of a per-scope `lookup_function_stored` call. Visibility (per
/// `chain_cutoff`) is applied inside the lookup; `overloads` holds only
/// visible finalized overloads (may be empty) and `pending` the earliest-index
/// visible in-flight producer (if any). Both are surfaced together so the
/// scope walk can decide pending-vs-finalized precedence at the scope that
/// raised them — a bucket may hold a finalized overload AND an in-flight
/// pending sibling at once. A no-hit lookup is `overloads.is_empty() &&
/// pending.is_none()`.
///
/// `pending` names a visible claim on the bucket's key — a sibling FN
/// binder has dispatched a matching overload whose body hasn't finalized. The
/// consumer parks on the earliest-index visible claim's edge; on wake it
/// re-dispatches and either picks from the now-live bucket or re-parks on the
/// next-earliest pending sibling.
///
/// Generic in the allocator its `overloads` buffer is built over, defaulting to the global heap.
/// Every reader passes the step's scratch handle instead, so the buffer costs no heap traffic and
/// dies with the pop; the default stands for a caller that has no arena in reach.
pub struct FunctionLookup<'a, A: Allocator = Global> {
    /// The visible finalized overloads, each a bit-copy of the bucket's dormant carrier — value and
    /// proven reach as one unit, re-anchored only by an [`open`](crate::witnessed::Sealed::open_at)
    /// under a named pin. Copied out so no caller holds the `functions` borrow across a candidate
    /// walk.
    pub overloads: allocator_api2::vec::Vec<SealedFunction<'a>, A>,
    pub pending: Option<ProducerId>,
}

/// One scope's visible contribution to a `CLOSE OVER` block's implicit close — see
/// [`Bindings::visible_for_capture`], which is the only producer of one. Every field is a snapshot
/// taken under a single `tables` borrow and released before the caller re-anchors anything, so no
/// table borrow is held across a carrier lift.
/// Lifetime-free: every carrier is already **lifted** into a delivery envelope pinned by the
/// table's own region owner, so the snapshot survives the borrow it was read under and the caller
/// re-homes it wherever it likes. The four buffers are built over the caller's allocator — the one
/// caller is a builtin body, which stages them on its step scratch.
pub(crate) struct VisibleBindings<A: Allocator> {
    /// Visible value bindings. The capture walk keeps the modules among them and drops the rest —
    /// plain data reaches the block only by explicit capture.
    pub(crate) data: AllocVec<(ValueSymbol, DeliveredCarried), A>,
    /// Every visible finalized overload, across all of this scope's buckets. The bucket key and the
    /// dispatch token are re-derived from each callable's own signature at the destination, so the
    /// envelope is the whole entry.
    pub(crate) functions: AllocVec<DeliveredFunction, A>,
    /// Visible operator-registry entries, by probe key.
    pub(crate) operators: AllocVec<(KeywordSymbol, DeliveredOperatorGroup), A>,
    /// The producers behind every visible in-flight claim, name and bucket alike — what the block
    /// build parks on so its close is independent of drain order.
    pub(crate) claims: AllocVec<ProducerId, A>,
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

/// Every lexical binding of one scope, in one cell, beside the [`ClaimStore`] holding the block's
/// in-flight binders. The maps carry committed bindings only; a claim is a store entry, so a name's
/// two questions — "is it bound?" and "is a binder for it in flight?" — are one probe each of the
/// structure that answers it. `data` and `types` are claimed by name (value/type forward
/// references) — one claim channel for both, sound because the two key classes name disjoint text;
/// `functions` by full dispatch bucket key, which keeps `(MAKESET _)` and `(MAKESET _ USING _)`
/// from colliding.
///
/// Every table is a `hashbrown` map over the scope's own region bump ([`BumpBackedMap`]), keyed by
/// a `Copy` classified symbol or a bumped `&'a [KeyElement]` run, so a table's death frees
/// nothing and walks no entry — which is what lets frame teardown cost O(scopes) rather than
/// O(entries). Lookup is the same O(1) hash probe a std map would run.
struct Tables<'a> {
    /// Each bound type slot stores its type and its [`DeclarationSite`] — the installing
    /// [`Installer`] (declaration identity) plus its lexical [`BindingIndex`] (visibility). A
    /// `KType` is a `Copy` handle into the run frame's registry, so an entry carries no reach: a
    /// read copies the handle under the home-frame pin alone, and the same handle names the same
    /// type in every region. A bound identity and a live claim on one name coexist without a
    /// representation for it — a nominal's seal pre-installs the external identity here while its
    /// binder is still in flight, which is this map bound *and* the store claimed.
    types: BumpBackedMap<'a, TypeSymbol, (KType, DeclarationSite), IdentityBuildHasher>,
    /// Each bound value slot stores its value fused to its exact reach in one dormant
    /// [`SealedValue`], plus its lexical [`BindingIndex`]. Reads hand out a bit-copy of the seal
    /// ([`Bindings::lookup_value`]) and re-anchor the value only under a pin, so a read replays the
    /// stored claim rather than re-asserting single-frame co-location. Description members are
    /// `Weak`, and the owning pins live in the region's union bundle rather than here — either a
    /// strong member or a per-entry `Rc` on the scope's own frame would close a
    /// `frame → region → scope → bindings → frame` cycle and leak the region.
    data: BumpBackedMap<'a, ValueSymbol, DataEntry<'a>, IdentityBuildHasher>,
    /// Each sealed bucket slot stores its callable fused to its reach claim in one dormant
    /// [`SealedFunction`], beside the precomputed data the write path dedupes on
    /// ([`FunctionBucketEntry`]). An `FN` registration binds no value, and a value bind writes no
    /// bucket. Like `data`, the entry owns nothing: the reached regions are held by the region's
    /// union bundle, and a read hands out a bit-copy the caller re-anchors under a pin. A bucket
    /// holds sealed overloads and nothing else — the sibling binders still finalizing under the same
    /// key are claims in the store, read through the same key.
    /// Keyed on a run bumped into the region rather than an owned [`UntypedKey`], so a node
    /// dispatching through its own bumped key probes without materializing one. A [`KeyElement`] is
    /// `Copy` and lifetime-free, so the owned and bumped forms are runs of the same type and either
    /// probes through the standard `Borrow` blanket.
    functions: BumpBackedMap<'a, &'a [KeyElement], Bucket<'a>>,
    /// Per-scope operator registry: a chain's sorted-joined operator probe key → the dormant
    /// [`SealedOperatorGroup`] carrier of the group it resolves to, beside its lexical
    /// [`BindingIndex`] ([`OperatorEntry`] — the `data`/`functions` entry shape). A module installs
    /// one entry per nonempty subset of its declared operators (the per-group powerset), each
    /// subset key holding a bit-copy of the same seal over the same region-hosted record, so any
    /// subset used in one expression resolves in a single hit, a cross-group mix simply misses, and
    /// the whole install allocates nothing past its probe keys. Walked through the scope chain:
    /// innermost visible wins.
    operators: BumpBackedMap<'a, KeywordSymbol, OperatorEntry<'a>, IdentityBuildHasher>,
    /// The in-flight binders of the one block that binds into this scope — see [`claims`]. Inside
    /// `Tables` rather than beside `Bindings` so it rides the one cell and the one `ManuallyDrop`
    /// with the maps it answers beside.
    claims: ClaimStore<'a>,
}

/// One dispatch bucket: the sealed overloads sharing a key, in a vec whose buffer is bump-backed
/// like every other table allocation here.
///
/// `ManuallyDrop` because the vec has a destructor and running it would be pure waste: its elements
/// carry no glue (the assert below), and its buffer is bump memory the region releases whole.
/// Suppressing it is also what makes `needs_drop::<Bucket>()` false, so the map holding buckets
/// passes [`bump_table`]'s assert and its own teardown never walks them. No `unsafe` — the
/// suppressed destructor had nothing to do.
type Bucket<'a> =
    ManuallyDrop<allocator_api2::vec::Vec<FunctionBucketEntry<'a>, BumpAllocator<'a>>>;

/// The element proof the `ManuallyDrop` above would otherwise swallow. `needs_drop` is false for
/// *any* `ManuallyDrop<U>`, so the wrapper that makes a bucket storable in a bump-backed table also
/// makes [`bump_table`]'s entry assert say nothing about what the bucket holds. Stated here instead,
/// against the element type directly, so a [`FunctionBucketEntry`] field that later brings a
/// destructor back fails the build rather than leaking silently.
const _: () = assert!(!std::mem::needs_drop::<FunctionBucketEntry<'static>>());

/// Build one of a scope's tables over its region bump, **proving at compile time** that its entries
/// carry no drop glue. The bump runs no destructor, so a `Drop`-bearing key or value would silently
/// leak whatever it owns; the assert is monomorphization-checked, so a future entry field that
/// brings glue back is a build error at the declaration that admitted it rather than a leak.
///
/// This is where each table's storage choice is stated: all five tables route here, so none has an
/// unstated exemption.
pub(in crate::machine::core) fn bump_table<'a, K, V, S: BuildHasher + Default>(
    brand: RegionBrand<'a>,
) -> BumpBackedMap<'a, K, V, S> {
    const {
        assert!(
            !std::mem::needs_drop::<K>() && !std::mem::needs_drop::<V>(),
            "a bump-backed table's entries must carry no drop glue: the bump runs no destructor",
        )
    };
    hashbrown::HashMap::with_hasher_in(S::default(), brand.allocator())
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
/// The brand rides beside the cell because a write re-homes the text it stores: a dispatch
/// bucket's key and an overload's dispatch token all land in the same region the tables'
/// buckets do, so a table never points at bytes that can die before it.
pub struct Bindings<'a> {
    brand: RegionBrand<'a>,
    /// `ManuallyDrop` for [`Bucket`]'s reason, one level up: a `hashbrown` map has a destructor
    /// whose only act is to hand its bucket array back to the allocator, which for a bump-backed
    /// table is a no-op. Suppressing it is what makes [`Bindings`] contribute **zero** drop glue to
    /// the `Scope` holding it — the assert below is the proof, and it is what a scope skipping its
    /// own destructor rests on.
    tables: RefCell<ManuallyDrop<Tables<'a>>>,
    /// **Monotone** sum of what totally rebuilding this scope's bound values would cost, in the
    /// same [`object_copy_cost`] currency a substrate prices its cells with — bumped by
    /// [`Self::write_value`] as each value bind applies, from the weight the bound value already
    /// memoized. The pricing fact the callable escape seam sums a captured chain over
    /// ([`copy_or_pin_callable`](crate::machine::model::copy_or_pin_callable)): a closure's
    /// definition site pays nothing for it, because the bump happens where a bind was already
    /// happening.
    ///
    /// Monotone because a binding is bind-once and an entry never dies before its scope, so the sum
    /// only ever grows and no write has to subtract. A `Cell` for the same reason
    /// [`Scope::closed`](crate::machine::core::Scope) is one: it is a plain `Copy` counter beside
    /// the tables, not table state, and reading it takes no `tables` borrow.
    copy_cost: Cell<u64>,
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
                claims: ClaimStore::new(brand),
            })),
            copy_cost: Cell::new(0),
        }
    }

    /// Whether no binder is still in flight into this table — no claim stands on either channel.
    /// The unfinalized-binding half of the copy engine's readiness gate
    /// ([`Scope::is_copy_ready`](crate::machine::core::Scope)).
    pub(crate) fn has_no_claims(&self) -> bool {
        self.tables.borrow().claims.is_empty()
    }

    /// What totally rebuilding every value bound here would cost — the monotone memo
    /// [`Self::copy_cost`] accumulated at bind time, read in O(1) with no walk over the table and
    /// no `tables` borrow.
    pub(crate) fn binding_copy_cost(&self) -> u64 {
        self.copy_cost.get()
    }

    /// Per-scope value-side lookup. One probe of `data[name]`, and on a miss one probe of the claim
    /// store: a visible binding answers `Bound`, a visible claim answers `Parked` on its edge.
    /// `cutoff = None` means the scope is off-chain (or unfiltered) — everything is visible. `None`
    /// return means no visible entry at this scope; the caller keeps walking
    /// ancestors, and chain exhaustion stays `None` (the terminal unbound
    /// disposition is materialized on the resolution path, not here).
    pub fn lookup_value(
        &self,
        name: ValueSymbol,
        cutoff: Option<usize>,
    ) -> Option<NameLookup<SealedValue<'a>>> {
        let tables = self.tables.borrow();
        if let Some(entry) = tables.data.get(&name) {
            return Self::visible(entry.index, cutoff)
                .then(|| NameLookup::Bound(entry.sealed.duplicate()));
        }
        tables
            .claims
            .name_claim(name.symbol())
            .filter(|claim| Self::visible(claim.index, cutoff))
            .map(|claim| NameLookup::Parked(claim.producer))
    }

    /// Per-scope type-side lookup. The type-language mirror of [`Self::lookup_value`]: one probe of
    /// `types[name]`, then one of the claim store, returning the first visible hit as a
    /// [`NameLookup`], or `None` so the caller keeps walking. Bound-first is load-bearing: where a
    /// bound identity and a live claim both stand — a nominal's seal pre-installing the external
    /// identity while its binder is still in flight — a consumer that can read the identity must
    /// not park.
    pub fn lookup_type(
        &self,
        name: TypeSymbol,
        cutoff: Option<usize>,
    ) -> Option<NameLookup<KType>> {
        let tables = self.tables.borrow();
        if let Some((kt, site)) = tables.types.get(&name)
            && Self::visible(site.index, cutoff)
        {
            return Some(NameLookup::Bound(*kt));
        }
        tables
            .claims
            .name_claim(name.symbol())
            .filter(|claim| Self::visible(claim.index, cutoff))
            .map(|claim| NameLookup::Parked(claim.producer))
    }

    /// Classified per-scope member lookup for ATTR module / signature access: the value-or-type
    /// `name` resolves to, read from **this scope's own** `data` then `types` in one pass. A
    /// module member is module-own — the lookup deliberately does **not** consult the builtin
    /// root or walk lexical ancestors, so `m.Type` (a builtin type name) or `m.SomeOuterType`
    /// is "no member", not a fall-through. `name` carries its own class, so the read probes exactly
    /// one map and the result is unambiguous. The binding maps hold committed bindings only, so a
    /// claim cannot surface here — and a read module is finalized either way.
    pub fn lookup_member(
        &self,
        name: BinderSymbol,
        cutoff: Option<usize>,
    ) -> Option<MemberResolution<'a>> {
        let tables = self.tables.borrow();
        match name {
            BinderSymbol::Value(name) => {
                let entry = tables.data.get(&name)?;
                Self::visible(entry.index, cutoff)
                    .then(|| MemberResolution::Value(entry.sealed.duplicate()))
            }
            BinderSymbol::Type(name) => {
                let (kt, site) = tables.types.get(&name)?;
                Self::visible(site.index, cutoff).then_some(MemberResolution::Type { kt: *kt })
            }
        }
    }

    /// The [`ProducerId`] of a still-finalizing **type** binder named `name`, read straight from the
    /// claim store — *not* through [`Self::lookup_type`], which prefers the (possibly
    /// seal-pre-installed, still-unsealed) bound identity. The finalize gate uses this to park the
    /// type-identifier memo on an in-flight binder even when the seal has already pre-installed
    /// the name's external identity into `types`. Visibility-unfiltered: this is dependency
    /// tracking, not consumer-visibility enforcement.
    pub fn type_placeholder_producer(&self, name: TypeSymbol) -> Option<ProducerId> {
        self.tables
            .borrow()
            .claims
            .name_claim(name.symbol())
            .map(|claim| claim.producer)
    }

    /// Per-scope dispatch-bucket lookup. One pass over `functions[key]` surfaces the visible sealed
    /// overloads AND the earliest-index visible pending sibling together, so the scope walk decides
    /// pending-vs-finalized precedence with both in hand.
    ///
    /// The key arrives as a run of `Copy` elements — a node's own bumped key on the dispatch hot
    /// path, a scratch-staged one where a caller builds the shape it is asking about — so no reader
    /// materializes an owned key to probe with. `alloc` hosts the overload buffer, and every caller
    /// passes the step's scratch arena, so a lookup costs no heap traffic and its buffer dies with
    /// the pop.
    pub fn lookup_function_stored<A: Allocator>(
        &self,
        key: &[KeyElement],
        cutoff: Option<usize>,
        alloc: A,
    ) -> FunctionLookup<'a, A> {
        self.lookup_function_probe(key, cutoff, alloc)
    }

    /// The bucket channel's **claim** read: the earliest-index visible in-flight binder declaring
    /// `key`, and nothing else. One hash probe of the store, copying nothing out, so the `tables`
    /// borrow is over before the answer is — a `ProducerId` is a plain edge name, unlike the sealed
    /// carriers [`FunctionLookup::overloads`] has to duplicate to let a candidate walk run outside
    /// the borrow. [`Self::lookup_function_probe`] fills its `pending` field from here, and the
    /// operator chain's pending-source sweep — which never reads a sealed overload — asks it
    /// directly.
    pub fn claimed_bucket_producer(
        &self,
        key: &[KeyElement],
        cutoff: Option<usize>,
    ) -> Option<ProducerId> {
        self.tables.borrow().claims.bucket_claim(key, cutoff)
    }

    /// The one bucket read. Owned and bumped keys are runs of the same `Copy` element, so both
    /// arrive here as a plain slice and hashbrown resolves them through the standard `Borrow`
    /// blanket — one derived `Hash`, nothing to keep in agreement. The same key reaches the sealed
    /// overloads and the claims on the shape, which is what lets the scope walk decide the pair's
    /// precedence at the scope that raised it.
    ///
    /// The buffer is sized to `bucket.len()` — an upper bound on the visible finalized overloads —
    /// and filled by a push loop rather than collected, so it never reallocates. Over a bump
    /// allocator that matters twice over: a grown buffer would abandon its old bytes as dead
    /// scratch until the next reset.
    fn lookup_function_probe<A: Allocator>(
        &self,
        key: &[KeyElement],
        cutoff: Option<usize>,
        alloc: A,
    ) -> FunctionLookup<'a, A> {
        let tables = self.tables.borrow();
        let pending = tables.claims.bucket_claim(key, cutoff);
        let Some(bucket) = tables.functions.get(key) else {
            return FunctionLookup {
                overloads: allocator_api2::vec::Vec::new_in(alloc),
                pending,
            };
        };
        let mut overloads = allocator_api2::vec::Vec::with_capacity_in(bucket.len(), alloc);
        for entry in bucket
            .iter()
            .filter(|entry| Self::visible(entry.index, cutoff))
        {
            overloads.push(entry.sealed.duplicate());
        }
        FunctionLookup { overloads, pending }
    }

    /// Per-scope operator-group lookup. Mirrors [`Self::lookup_value`] for the
    /// `operators` map: returns the visible group registered under `probe` (the
    /// sorted-joined unique operators of a chain), or `None` at this scope so the
    /// caller keeps walking ancestors.
    pub(in crate::machine::core) fn lookup_operator_group(
        &self,
        probe: KeywordSymbol,
        cutoff: Option<usize>,
    ) -> Option<SealedOperatorGroup<'a>> {
        let tables = self.tables.borrow();
        let entry = tables.operators.get(&probe)?;
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
        probe: KeywordSymbol,
        seal: &GroupSeal<'a>,
        index: BindingIndex,
        registries: &RunRegistries,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let mut tables = self.tables.borrow_mut();
        if let Some(entry) = tables.operators.get(&probe) {
            if entry.address == seal.address || entry.declaration == seal.declaration {
                return Ok(());
            }
            let probe = render_label(probe.symbol(), registries);
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "operator `{probe}` is already declared in this scope with a different \
                 chaining mode or member set; one scope declares one chaining mode per operator",
            ))));
        }
        // The key is a `Copy` digest; only the declaration re-homes on the insert. A powerset
        // install bumps the declaration once per subset entry; the byte cost is bounded by the
        // group's own powerset, which is small, and cross-call sharing would cost an intern table
        // to save it.
        tables.operators.insert(
            probe,
            OperatorEntry {
                index,
                address: seal.address,
                declaration: self.brand.allocator().text(&seal.declaration),
                sealed: seal.sealed.duplicate(),
            },
        );
        Ok(())
    }

    /// Snapshot every `(name, dormant carrier)` pair in `data`, ignoring visibility. Each
    /// seal is a bit-copy; the caller re-anchors what it needs under its own pin. Claims are
    /// structurally absent — the map holds committed bindings only — so nothing is filtered. For
    /// chain-gated single-name reads use [`Self::lookup_value`].
    pub fn iter_data(&self) -> Vec<(ValueSymbol, SealedValue<'a>)> {
        self.tables
            .borrow()
            .data
            .iter()
            .map(|(name, entry)| (*name, entry.sealed.duplicate()))
            .collect()
    }

    /// Snapshot every `(name, KType)` pair in `types`, ignoring visibility.
    pub fn iter_types(&self) -> Vec<(TypeSymbol, KType)> {
        self.tables
            .borrow()
            .types
            .iter()
            .map(|(name, (kt, _site))| (*name, *kt))
            .collect()
    }

    /// Snapshot every `(UntypedKey, Vec<SealedFunction>)` pair in `functions`, ignoring per-overload
    /// visibility. Each seal is a bit-copy; the caller re-anchors what it needs under its own pin.
    /// An empty bucket is skipped — a shape whose overloads all retired publishes no dispatch
    /// surface to snapshot. For chain-gated picks use [`Self::lookup_function_stored`].
    pub fn iter_functions(&self) -> Vec<(UntypedKey, Vec<SealedFunction<'a>>)> {
        self.tables
            .borrow()
            .functions
            .iter()
            .filter_map(|(key, bucket)| {
                let sealed: Vec<SealedFunction> = bucket
                    .iter()
                    .map(|entry| entry.sealed.duplicate())
                    .collect();
                (!sealed.is_empty()).then(|| (key.to_vec(), sealed))
            })
            .collect()
    }

    /// Everything one scope publishes into a `CLOSE OVER` block's **implicit close**, gated by the
    /// reading statement's own visibility `cutoff` — the same predicate every resolution ladder
    /// applies, so the block closes over exactly what a statement at that position could have
    /// resolved.
    ///
    /// Four tables, one snapshot, because the walk visits each scope once and the four answers are
    /// consumed together. `types` is deliberately absent: a nominal type name reaches the block only
    /// as an explicit capture, and a copied registration's dispatch token holds its [`KType`]s by
    /// value, so dispatch inside the block does not depend on the type table travelling.
    ///
    /// Every carrier is lifted through **this table's own brand**, which is the region hosting the
    /// descriptions the upgrade reads. Reading it off the table rather than off the scope is what
    /// makes a `USING` window admissible: the window scope's region is the call site's, while its
    /// borrowed façade — and every seal in it — lives in the opened module's.
    pub(crate) fn visible_for_capture<A: Allocator + Copy>(
        &self,
        cutoff: Option<usize>,
        alloc: A,
    ) -> VisibleBindings<A> {
        let tables = self.tables.borrow();
        // Each buffer takes an upper bound on its own table up front. A bump vector that grows
        // abandons its old bytes as dead scratch until the pop, so the capacity is worth the count.
        let mut data = AllocVec::with_capacity_in(tables.data.len(), alloc);
        data.extend(
            tables
                .data
                .iter()
                .filter(|(_, entry)| Self::visible(entry.index, cutoff))
                .map(|(name, entry)| (*name, self.brand.lift_resident(entry.sealed.duplicate()))),
        );
        let overloads = tables.functions.values().map(|bucket| bucket.len()).sum();
        let mut functions = AllocVec::with_capacity_in(overloads, alloc);
        functions.extend(
            tables
                .functions
                .values()
                .flat_map(|bucket| bucket.iter())
                .filter(|entry| Self::visible(entry.index, cutoff))
                .map(|entry| self.brand.lift_resident(entry.sealed.duplicate())),
        );
        let mut operators = AllocVec::with_capacity_in(tables.operators.len(), alloc);
        operators.extend(
            tables
                .operators
                .iter()
                .filter(|(_, entry)| Self::visible(entry.index, cutoff))
                .map(|(probe, entry)| (*probe, self.brand.lift_resident(entry.sealed.duplicate()))),
        );
        VisibleBindings {
            data,
            functions,
            operators,
            claims: tables.claims.visible_producers(cutoff, alloc),
        }
    }

    /// One bucket key's visible contribution to a `CLOSE OVER` block's explicit pattern capture:
    /// every finalized overload registered under `key` here, lifted, plus this scope's own pending
    /// claim on it. [`Self::visible_for_capture`]'s single-key twin, and lifted through the table's
    /// own brand for the same reason — a `USING` window's seals live in the opened module's region,
    /// not in the window scope's.
    pub(crate) fn lifted_overloads_for<A: Allocator + Copy>(
        &self,
        key: &[KeyElement],
        cutoff: Option<usize>,
        alloc: A,
    ) -> (AllocVec<DeliveredFunction, A>, Option<ProducerId>) {
        let lookup = self.lookup_function_stored(key, cutoff, alloc);
        let mut lifted = AllocVec::with_capacity_in(lookup.overloads.len(), alloc);
        lifted.extend(
            lookup
                .overloads
                .into_iter()
                .map(|sealed| self.brand.lift_resident(sealed)),
        );
        (lifted, lookup.pending)
    }

    /// True iff `types[name]` is bound at [`BindingIndex::BUILTIN`]. The
    /// no-shadow consult gates on this — a genuine builtin, not a user type that a
    /// synthetic test happens to have placed in a root-position scope.
    pub fn has_builtin_type(&self, name: TypeSymbol) -> bool {
        self.tables
            .borrow()
            .types
            .get(&name)
            .is_some_and(|(_, site)| site.index == BindingIndex::BUILTIN)
    }

    /// True iff `functions[key]` holds a sealed overload registered at
    /// [`BindingIndex::BUILTIN`] — a genuine builtin dispatch bucket, distinct from a
    /// user bucket the no-shadow consult must not gate.
    pub fn has_builtin_function(&self, key: &UntypedKey) -> bool {
        self.has_builtin_function_probe(key.as_slice())
    }

    /// [`Self::has_builtin_function`] from a node's own bumped key — the no-shadow consult's hot
    /// path, paired with [`Self::lookup_function_stored`].
    pub fn has_builtin_function_stored(&self, key: &[KeyElement]) -> bool {
        self.has_builtin_function_probe(key)
    }

    fn has_builtin_function_probe(&self, key: &[KeyElement]) -> bool {
        self.tables
            .borrow()
            .functions
            .get(key)
            .is_some_and(|b| b.iter().any(|e| e.index == BindingIndex::BUILTIN))
    }

    /// Visibility predicate. `cutoff = None` (the reader is off this scope's chain, so the scope
    /// is complete) ⇒ visible; `Some(c)` ⇒ `idx < c`.
    fn visible(b: BindingIndex, cutoff: Option<usize>) -> bool {
        match cutoff {
            None => true,
            Some(c) => b.idx < c,
        }
    }

    #[cfg(test)]
    pub(crate) fn data(
        &self,
    ) -> Ref<'_, BumpBackedMap<'a, ValueSymbol, DataEntry<'a>, IdentityBuildHasher>> {
        Ref::map(self.tables.borrow(), |t| &t.data)
    }

    #[cfg(test)]
    pub(crate) fn functions(&self) -> Ref<'_, BumpBackedMap<'a, &'a [KeyElement], Bucket<'a>>> {
        Ref::map(self.tables.borrow(), |t| &t.functions)
    }

    #[cfg(test)]
    pub(crate) fn types(
        &self,
    ) -> Ref<'_, BumpBackedMap<'a, TypeSymbol, (KType, DeclarationSite), IdentityBuildHasher>> {
        Ref::map(self.tables.borrow(), |t| &t.types)
    }

    /// The claim standing on `name` in the value channel, if any — the value-side
    /// forward-reference probe.
    #[cfg(test)]
    pub fn pending_value(&self, name: ValueSymbol) -> Option<Claim> {
        self.tables.borrow().claims.name_claim(name.symbol())
    }

    /// Every standing name-channel claim, tagged with the language it resolves in — the hygiene
    /// probe for "this declaration left no in-flight producer behind". The store keys both channels
    /// in one map, and the tag is read back off the resolved name's token class, which is what
    /// makes that one map sound: the two bindable classes name disjoint text.
    #[cfg(test)]
    pub fn pending_names(&self, registries: &RunRegistries) -> Vec<(String, BindKind, ProducerId)> {
        self.tables
            .borrow()
            .claims
            .name_claims()
            .into_iter()
            .map(|(name, claim)| {
                let name = render_label(name, registries);
                let kind = BinderSymbol::classify(&name)
                    .expect("only a bindable name is ever claimed")
                    .bind_kind();
                (name, kind, claim.producer)
            })
            .collect()
    }

    /// Every standing claim on one dispatch bucket, in install order.
    #[cfg(test)]
    pub fn pending_overload_entries(&self, bucket: &UntypedKey) -> Vec<Claim> {
        self.tables.borrow().claims.bucket_claims(bucket)
    }

    #[cfg(test)]
    pub fn expect_type(&self, name: TypeSymbol) -> KType {
        self.tables
            .borrow()
            .types
            .get(&name)
            .map(|(kt, _site)| *kt)
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
    /// No cross-kind probe stands here: a [`TypeSymbol`] and a [`ValueSymbol`] classify disjoint
    /// text, so a type name cannot collide with a committed value. On success the write
    /// **retires its own claim**: it already
    /// carries the name and the [`BindingIndex`] it is writing at, through `site`, so removing the
    /// claim is one hash removal and one bit with nothing searched for.
    pub(crate) fn write_type(
        &self,
        name: TypeSymbol,
        kt: KType,
        site: DeclarationSite,
        policy: TypeWritePolicy,
        registries: &RunRegistries,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let mut tables = self.tables.borrow_mut();
        let rebind = || {
            KError::new(KErrorKind::Rebind {
                name: render_label(name.symbol(), registries),
            })
        };
        match (
            policy,
            tables.types.get(&name).map(|(_, existing)| *existing),
        ) {
            (TypeWritePolicy::Insert, Some(_)) => return Err(rebind()),
            (TypeWritePolicy::UpsertEqual, Some(existing))
                if existing.installer != site.installer =>
            {
                return Err(rebind());
            }
            // Absent, or the same declaration re-entering: write the identity.
            _ => {}
        }
        // A same-declaration re-entry overwrites where it sits; an absent name inserts its `Copy`
        // digest key.
        tables.types.insert(name, (kt, site));
        tables.claims.retire_name(name.symbol(), site.index);
        Ok(())
    }

    /// Size this scope's claim run for a block of `statements` statements fanning out into it — the
    /// one act that builds a claim store. The statement-at-a-time door builds none: a driver using
    /// it runs each statement to completion, so every visible binder has already committed and no
    /// claim is ever consulted.
    pub fn begin_block(&self, statements: usize, _gate: &mut WriteGate) {
        self.tables.borrow_mut().claims.begin_block(statements);
    }

    /// Claim `name` for the binder edge `producer` — the dispatch-time forward-reference stamp.
    /// `producer` names the slot's own installed edge, destined at this scope's region, so a
    /// consumer parking on the claim inherits that destination.
    ///
    /// Errors `Rebind` if the claim collides: a committed `data[name]` (bindings are bind-once), or
    /// a standing claim naming a different edge. Idempotent on same-edge re-entry. A `types` entry
    /// already carrying a bound identity does **not** block: a parallel nominal finalize
    /// pre-installs the external identity while its binder is still in flight, and that coexistence
    /// is a bound entry plus a live claim rather than anything either structure represents.
    ///
    /// The eventual [`Self::write_value`] / [`Self::write_type`] call must carry the
    /// same `index` so the consumer's visibility test stays consistent across
    /// the claimed → committed transition, and so the commit retires this very claim. `name`'s own
    /// class picks the destination table, so a value bind never satisfies a type claim — see
    /// [`Bindings::lookup_value`] / [`Bindings::lookup_type`], each of which probes only its own
    /// table before the store.
    pub fn install_placeholder(
        &self,
        name: BinderSymbol,
        producer: ProducerId,
        index: BindingIndex,
        registries: &RunRegistries,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let mut tables = self.tables.borrow_mut();
        let rebind = || {
            KError::new(KErrorKind::Rebind {
                name: render_label(name.symbol(), registries),
            })
        };
        if let BinderSymbol::Value(name) = name
            && tables.data.contains_key(&name)
        {
            return Err(rebind());
        }
        match tables
            .claims
            .claim_name(name.symbol(), Claim { producer, index })
        {
            Ok(()) => Ok(()),
            // A same-producer re-entry is the same stamp arriving twice, not a second declaration.
            Err(standing) if standing.producer == producer => Ok(()),
            Err(_) => Err(rebind()),
        }
    }

    /// Install a dispatch-time bucket claim: `bucket → producer`.
    /// The bucket key MUST equal what `KExpression::untyped_key` would compute
    /// for a *call* to the eventual overload (not the binder call itself), which is what lets one
    /// key reach both the claim and the overload it becomes.
    ///
    /// **Append, never deduplicate**: sibling FN binders sharing one
    /// inner-call bucket key — `FN (PICK xs :A) -> ...` then
    /// `FN (PICK xs :B) -> ...` — each claim at their own
    /// [`BindingIndex`]. [`Bindings::write_overload`] retires only the sealing binder's own claim;
    /// the other siblings stay as wake sources.
    pub fn install_pending_overload(
        &self,
        bucket: &[KeyElement],
        producer: ProducerId,
        index: BindingIndex,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let brand = self.brand;
        self.tables
            .borrow_mut()
            .claims
            .claim_bucket(brand, bucket, Claim { producer, index });
        Ok(())
    }

    /// Replay another `Bindings`'s `data` entries through [`Self::write_value`] on self, and
    /// its `functions` entries by direct duplication — a view of a module preserves the
    /// module's keyworded dispatch surface as-is (keyword → keyword), it does not re-derive it from
    /// the value bindings. There is nothing to filter: the source's maps hold committed bindings
    /// only, and its claims live in a store this never reads. That is the point — a claim names an
    /// edge of the source's own scheduler run, so a copied one would hand the target a park on a
    /// node that will never wake it, and keeping claims out of the maps makes that unrepresentable
    /// rather than filtered. The `types` table is not replayed: a view's type interface is its own, seeded by
    /// [`Scope::alloc_module_view`](crate::machine::core::Scope) from the ascribed signature rather
    /// than inherited from the source. Snapshots the source maps and releases the source `Ref`
    /// before the replay so re-entrant ascription cannot deadlock.
    pub(crate) fn bulk_install_from(
        &self,
        src: &Bindings<'a>,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        // Duplicate each entry into the snapshot: each seal is a bit-copy naming the source's own
        // minted description, so the replayed entry replays that same claim. The reached regions
        // stay owned by the *source* scope's region union — the replay target's own region must
        // already outlive it (a bulk install is same-run re-entrant ascription). The snapshot's
        // keys are `Copy` borrows into the source's region and outlive the `Ref` they were read
        // under, so nothing is cloned to release the borrow. The `data` keys are `Copy` digests
        // and borrow nothing at all.
        let (data, functions) = {
            let tables = src.tables.borrow();
            let data: Vec<(ValueSymbol, DataEntry)> = tables
                .data
                .iter()
                .map(|(k, entry)| (*k, entry.duplicate()))
                .collect();
            let functions: Vec<(&[KeyElement], Vec<FunctionBucketEntry<'a>>)> = tables
                .functions
                .iter()
                .filter_map(|(key, bucket)| {
                    let sealed: Vec<FunctionBucketEntry<'a>> =
                        bucket.iter().map(FunctionBucketEntry::duplicate).collect();
                    (!sealed.is_empty()).then_some((*key, sealed))
                })
                .collect();
            (data, functions)
        };
        for (name, entry) in data {
            self.write_value(name, entry.index, entry.sealed, registries, gate)?;
        }
        let mut tables = self.tables.borrow_mut();
        for (key, slots) in functions {
            // The key is re-homed into *this* table's region. It buys no independence from the
            // source region and is not trying to: everything else a replayed entry carries — the
            // sealed carrier, the dispatch token — stays a borrow into the source, which is sound
            // for the reason stated at the snapshot above, and is why re-homing the key is symmetry
            // rather than a guard. The relation is held by **retention, not by `'a`**: `'a` covers both
            // regions and orders neither, but the view module escaping
            // [`Scope::alloc_module_view`](crate::machine::core::Scope) composes the source
            // module's reach into its own region, so the source is pinned for as long as the view
            // is reachable at all. A read of the source's bytes past its death is therefore
            // unrepresentable rather than merely unobserved.
            match tables.functions.get_mut(key) {
                Some(bucket) => bucket.extend(slots),
                None => {
                    let mut bucket = bump_bucket(self.brand);
                    bucket.extend(slots);
                    tables
                        .functions
                        .insert(self.brand.allocator().slice(key), bucket);
                }
            }
        }
        Ok(())
    }

    /// The `data` write path: commit `name` → `sealed` as a bind-once value binding. Probes for a
    /// standing binding, writes the entry, and **retires its own claim** — it carries the name and
    /// the [`BindingIndex`] it is writing at, so the removal is one hash removal and one bit with
    /// nothing searched for. All under one borrow.
    ///
    /// No cross-kind probe: a [`ValueSymbol`] and a [`TypeSymbol`] classify disjoint text, so a
    /// name committed to `data` cannot name a `types` entry.
    pub(crate) fn write_value(
        &self,
        name: ValueSymbol,
        index: BindingIndex,
        sealed: SealedValue<'a>,
        registries: &RunRegistries,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let mut tables = self.tables.borrow_mut();
        if tables.data.contains_key(&name) {
            return Err(KError::new(KErrorKind::Rebind {
                name: render_label(name.symbol(), registries),
            }));
        }
        // The weight is read off the seal before it is stored, where the value is open: a bind is
        // already doing this work's worth of table mutation, so the memo costs the definition site
        // nothing beyond one `object_copy_cost` read of a value the seal hands back.
        let weight = sealed
            .open_at()
            .value()
            .as_object()
            .map_or(0, object_copy_cost);
        self.copy_cost
            .set(self.copy_cost.get().saturating_add(weight));
        tables.data.insert(name, DataEntry { index, sealed });
        tables.claims.retire_name(name.symbol(), index);
        Ok(())
    }

    /// Whether this table holds an operator-registry entry — a `GROUP` declaration's powerset keys,
    /// or the flattened copies a `CLOSE OVER` block installs. The environment copy does not model
    /// them (the record they resolve to is region-resident and would have to be rebuilt with the
    /// scope), so their presence is part of the readiness gate rather than a case inside the engine.
    pub(crate) fn has_operators(&self) -> bool {
        !self.tables.borrow().operators.is_empty()
    }

    /// Every type binding in this table, as plain `Copy` data — the environment copy's `types`
    /// read, and the one table [`Self::visible_for_capture`] deliberately leaves out (a `CLOSE OVER`
    /// block reaches a nominal only through an explicit capture, where a copied *environment* must
    /// carry whatever the source scope bound).
    ///
    /// The whole entry copies by value: a [`KType`] is a lifetime-free handle into the run's
    /// registry and a [`DeclarationSite`] is plain data, so the fresh [`ScopeId`] a copied scope
    /// takes changes no identity here — a nominal's identity is registry state, not scope state.
    ///
    /// [`ScopeId`]: crate::machine::core::scope_id::ScopeId
    pub(crate) fn copied_types(&self) -> Vec<(TypeSymbol, KType, DeclarationSite)> {
        self.tables
            .borrow()
            .types
            .iter()
            .map(|(name, (kt, site))| (*name, *kt, *site))
            .collect()
    }

    /// The environment copy's `types` write — [`Self::insert_copied_value`]'s type-channel twin,
    /// registry-free for the same reason: the copy fills an empty table, one entry per source name.
    pub(crate) fn insert_copied_type(&self, name: TypeSymbol, kt: KType, site: DeclarationSite) {
        let mut tables = self.tables.borrow_mut();
        debug_assert!(
            !tables.types.contains_key(&name),
            "an environment copy fills an empty table, one entry per source name",
        );
        tables.types.insert(name, (kt, site));
    }

    /// The environment copy's `data` write: install a rebuilt binding into a freshly built copied
    /// scope. Deliberately **registry-free**, which is what lets it run from inside a relocation
    /// fold: the only thing `write_value` needs registries for is rendering a `Rebind`, and a copy
    /// fills an empty table with one entry per source name, so a collision is a construction bug
    /// rather than a program error. It is asserted here rather than reported.
    ///
    /// The cost memo is bumped exactly as an ordinary bind bumps it, so a copied scope prices its
    /// own re-consolidation on the same terms the source did.
    pub(crate) fn insert_copied_value(
        &self,
        name: ValueSymbol,
        index: BindingIndex,
        sealed: SealedValue<'a>,
    ) {
        let mut tables = self.tables.borrow_mut();
        debug_assert!(
            !tables.data.contains_key(&name),
            "an environment copy fills an empty table, one entry per source name",
        );
        let weight = sealed
            .open_at()
            .value()
            .as_object()
            .map_or(0, object_copy_cost);
        self.copy_cost
            .set(self.copy_cost.get().saturating_add(weight));
        tables.data.insert(name, DataEntry { index, sealed });
    }

    /// The environment copy's `functions` write — [`Self::insert_copied_value`]'s dispatch-bucket
    /// twin, registry-free for the same reason: the copy fills an empty table, so the duplicate
    /// token `write_overload` would render a `DuplicateOverload` for cannot arise. Each overload's
    /// key and token are re-derived from the rebuilt callable's own signature at seal time, so the
    /// copy lands in the same bucket the source sat in with nothing threaded alongside it.
    pub(crate) fn insert_copied_overload(&self, index: BindingIndex, seal: OverloadSeal<'a>) {
        let mut tables = self.tables.borrow_mut();
        if !tables.functions.contains_key(seal.key.as_slice()) {
            let key = self.brand.allocator().slice(&seal.key);
            tables.functions.insert(key, bump_bucket(self.brand));
        }
        let bucket = tables
            .functions
            .get_mut(seal.key.as_slice())
            .expect("the bucket was just seeded if it was missing");
        debug_assert!(
            !bucket
                .iter()
                .any(|existing| seal.token.elements() == existing.token),
            "an environment copy fills an empty bucket, one entry per source overload",
        );
        bucket.push(FunctionBucketEntry {
            index,
            token: seal.token.store_in(self.brand),
            sealed: seal.sealed,
        });
    }

    /// The `functions` write path: add `seal`'s callable to its dispatch bucket. The bucket key and
    /// dedupe token were both computed at seal time, where the callable was open — the write is
    /// pure table mutation, no carrier is opened and no bare reference crosses the door. Token
    /// equality against a bucket sibling raises `DuplicateOverload`, whose text renders on that arm
    /// alone from the seal's own key and the standing entry's stored token — so the success path
    /// resolves no label and the error arm opens nothing either; claims are in
    /// the store and don't participate in the dedupe. The write then **retires its own claim** on
    /// the same key at the same index — the sibling binders' claims stand as wake sources. Bucket
    /// order is not observable: the picker returns a unique winner or a tie that surfaces as
    /// deferred/ambiguous either way.
    pub(crate) fn write_overload(
        &self,
        index: BindingIndex,
        seal: OverloadSeal<'a>,
        registries: &RunRegistries,
        _gate: &mut WriteGate,
    ) -> Result<(), KError> {
        let mut tables = self.tables.borrow_mut();
        // Probe-then-insert rather than an `entry` call: the key a miss inserts has to be re-homed
        // through the brand, which the entry API has no way to defer.
        if !tables.functions.contains_key(seal.key.as_slice()) {
            let key = self.brand.allocator().slice(&seal.key);
            tables.functions.insert(key, bump_bucket(self.brand));
        }
        let bucket = tables
            .functions
            .get_mut(seal.key.as_slice())
            .expect("the bucket was just seeded if it was missing");
        // Dedupe against the stored runs where they sit — no allocation to decide it, and the
        // incoming token is re-homed only once it has passed.
        if let Some(existing) = bucket
            .iter()
            .find(|existing| seal.token.elements() == existing.token)
        {
            return Err(KError::new(KErrorKind::DuplicateOverload {
                name: render_untyped_key(seal.key.as_slice(), registries),
                signature: summarize_dispatch(existing.token, registries),
            }));
        }
        bucket.push(FunctionBucketEntry {
            index,
            token: seal.token.store_in(self.brand),
            sealed: seal.sealed,
        });
        // A builtin seed, a direct registration or a bulk install claimed nothing, so this is a
        // no-op for them.
        tables.claims.retire_bucket(seal.key.as_slice(), index);
        Ok(())
    }

    /// Retire every claim the statement at `index` still holds — the retirement companion to the
    /// installs, run when the claiming slot terminalizes. The write paths retire a binder's own
    /// claim as they commit, so this normally reads a zero live mask and returns; running it on
    /// every terminal is what guarantees no claim survives naming a [`ProducerId`] whose edge its
    /// owner is about to release.
    ///
    /// Keyed on the one address the retiring slot knows about itself. It is an array index and a
    /// zero test on the success path, and at most three direct removals otherwise — nothing is
    /// searched in either direction, not the binding tables by producer and not the store by name.
    ///
    /// Strands bump bytes: a removed bucket key's stored run is abandoned rather than freed. Name
    /// claims key by a `Copy` digest and strand nothing. Bounded by the binders that fail, so a
    /// scope's peak occupancy stays its final binding count plus that error tail.
    pub fn retire_claims(&self, index: BindingIndex, _gate: &mut WriteGate) {
        self.tables.borrow_mut().claims.retire_statement(index);
    }
}

#[cfg(test)]
mod tests;
