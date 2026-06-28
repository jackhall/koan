//! The Koan instantiation of the generic [`Region`](crate::witnessed::Region)
//! storage substrate: `KoanRegion = Region<KoanStorageProfile>`, the per-family
//! [`Stored`](crate::witnessed::Stored) impls (which sub-arena a family lands in), and the
//! Koan-typed `alloc_*` wrappers. `CallFrame`
//! — the per-call frame shell over a refcounted `FrameStorage` (the `KoanRegion` plus the ancestor
//! chain), holding the child `Scope` and resetting in place for TCO — also lives here.
//!
//! The generic erase-store engine lives in [`crate::witnessed::region`]; this file supplies the
//! Koan policy it runs.
//!
//! See [per-call-region/README.md](../../../design/per-call-region/README.md) for the carrier
//! set, escaping-value retention, ancestor chain, and TCO frame reuse;
//! [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
//! for the heap-pinning / drop-order invariants.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use typed_arena::Arena;

use super::scope::Scope;
use super::scope_ptr::ScopeRefFamily;
use crate::machine::core::kfunction::{KFunction, NodeId};
use crate::machine::model::ast::KExpression;
use crate::machine::model::operators::OperatorGroup;
use crate::machine::model::types::KType;
use crate::machine::model::values::{CarriedFamily, KObject, Module, ModuleSignature};
use crate::witnessed::reattachable;
use crate::witnessed::SealedExtern;
use crate::witnessed::{
    reattach_with, MergeWitness, Reattachable, Region, StorageProfile, Stored, Witness,
    WitnessRegion, Witnessed,
};

/// The Koan storage bundle: one typed sub-arena per stored family. Each sub-arena stores the
/// family's `'static` form (phantom); the [`Region`] engine re-anchors to the caller's `'a`
/// on the way out. The `KType` region backs per-type identity binding storage (`Bindings::types`);
/// the `OperatorGroup` region backs the per-scope operator registry (`Bindings::operators`).
#[derive(Default)]
pub struct KoanStorage {
    objects: Arena<KObject<'static>>,
    functions: Arena<KFunction<'static>>,
    scopes: Arena<Scope<'static>>,
    modules: Arena<Module<'static>>,
    signatures: Arena<ModuleSignature<'static>>,
    ktypes: Arena<KType<'static>>,
    operator_groups: Arena<OperatorGroup>,
}

/// The Koan workload: binds the generic [`Region`] to the Koan family set.
pub struct KoanStorageProfile;

impl StorageProfile for KoanStorageProfile {
    type Storage = KoanStorage;
}

/// Run-lifetime allocator. A [`Region`] carrying the Koan family set; lives for one program
/// run. The `KoanRegion` references across the tree and the `Rc<CallFrame>` back-edge ride this
/// alias unchanged.
pub type KoanRegion = Region<KoanStorageProfile>;

// SAFETY: a `Region`'s values live in a `typed_arena`, whose backing pages never move while the
// region is borrowed, so a held `&Region` keeps any pointee alloc'd in it (or a strict ancestor it
// roots) at a fixed address — the bound the consumer-pull lift's frameless re-anchor relies on to
// witness the destination lifetime.
unsafe impl<W: crate::witnessed::StorageProfile> crate::witnessed::Witness for Region<W> {}

// The lifetime family of each stored type, keyed on its `'static` form — the GAT the
// `Region` engine erases to `'static` for storage and re-anchors to the caller's `'a` on read.
// Each family is one type generic only in a single lifetime, so its layout is identical for every
// choice of that lifetime; `OperatorGroup` is lifetime-free, trivially invariant. The shared
// `reattachable!` macro discharges the layout-invariance `unsafe` obligation once (see its docs).
reattachable! {
    KObject<'static> => KObject<'r>,
    KType<'static> => KType<'r>,
    KFunction<'static> => KFunction<'r>,
    Scope<'static> => Scope<'r>,
    Module<'static> => Module<'r>,
    ModuleSignature<'static> => ModuleSignature<'r>,
    OperatorGroup => OperatorGroup,
}

/// A witnessed-construction operand bundling a destination region with a type-channel identity (a
/// `SetRef` / declared type) that must cross the build brand. A value-embedding construction
/// `transfer_into`/`merge`s its object carrier with this operand so the wrapped value lands in
/// `region` tagged by the identity, both re-anchored to the brand under the same witness — the dest
/// frame's `outer` chain pins the identity's (ancestor) region. Used by the newtype / tagged-union
/// constructors and the `CATCH` `Result` build. Layout-invariant: two thin pointers, representation
/// independent of `'r`.
pub struct RegionTypeFamily;
reattachable!(RegionTypeFamily => (&'r KoanRegion, &'r KType<'r>));

// Per-family `Stored` policy: which sub-arena each family lands in, plus `KObject`'s allocation
// address side-table hook. No stored family carries a self-targeting `Rc<FrameStorage>` — a stored
// closure / future / module is a bare borrow into its defining region, kept alive by its carrier's
// witness set rather than an owned anchor — so no allocation can self-cycle and the engine needs no
// cycle gate.

impl Stored<KoanStorageProfile> for KObject<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<KObject<'static>> {
        &s.objects
    }
    fn record_local(frame: &KoanRegion, stored: &KObject<'static>) {
        frame.record_addr(stored as *const _ as usize);
    }
}

impl Stored<KoanStorageProfile> for KType<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<KType<'static>> {
        &s.ktypes
    }
}

impl Stored<KoanStorageProfile> for KFunction<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<KFunction<'static>> {
        &s.functions
    }
}

impl Stored<KoanStorageProfile> for Scope<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<Scope<'static>> {
        &s.scopes
    }
}

impl Stored<KoanStorageProfile> for Module<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<Module<'static>> {
        &s.modules
    }
}

impl Stored<KoanStorageProfile> for ModuleSignature<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<ModuleSignature<'static>> {
        &s.signatures
    }
}

impl Stored<KoanStorageProfile> for OperatorGroup {
    fn sub_arena(s: &KoanStorage) -> &Arena<OperatorGroup> {
        &s.operator_groups
    }
}

/// Koan-typed allocation surface on the run-lifetime region. Each wrapper routes the single
/// [`Region::alloc`] engine; these named wrappers are the public entry points.
impl Region<KoanStorageProfile> {
    /// Store a [`KObject`] into the run-lifetime region; the value lands in this region (no value
    /// holds an owning `Rc` back to a region, so the store forms no back-edge).
    pub fn alloc_object<'a>(&'a self, o: KObject<'a>) -> &'a KObject<'a> {
        self.alloc::<KObject<'static>>(o)
    }

    /// Store a [`KType`] into the run-lifetime region (a `Module` rides a bare borrow into its
    /// child scope's region — held alive by the carrier's witness set, not an embedded anchor).
    pub fn alloc_ktype<'a>(&'a self, t: KType<'a>) -> &'a KType<'a> {
        self.alloc::<KType<'static>>(t)
    }

    /// INVARIANT: a `KFunction` must be allocated into the same `KoanRegion` that owns its
    /// captured scope. The `functions_is_empty` fast path relies on this — without the
    /// invariant, "no KFunction allocated here" no longer implies "no KFunction has
    /// `captured_scope` in this region," and the path silently drops regions out from under
    /// live `&KFunction` references. The `debug_assert!` catches violations at the
    /// allocation site rather than later as use-after-free.
    pub fn alloc_function<'a>(&'a self, f: KFunction<'a>) -> &'a KFunction<'a> {
        debug_assert!(
            std::ptr::eq(
                self as *const KoanRegion,
                f.captured_scope().region as *const KoanRegion
            ),
            "alloc_function invariant :KFunction must be allocated into the same KoanRegion \
             that owns its captured scope"
        );
        self.alloc::<KFunction<'static>>(f)
    }

    /// The alloc-witnessed construction inversion's region-pure primitive: build a value into the
    /// witness frame's region *inside* the `yoke` closure, returning it bundled with `witness` (the set
    /// of regions it reaches) so it is co-located by construction rather than paired with an asserted
    /// witness. One primitive for both value families — the closure returns a `Carried::Object` (an
    /// [`alloc_object`](Self::alloc_object)) or a `Carried::Type` (an
    /// [`alloc_ktype`](Self::alloc_ktype)). A value that *references* another region's resident value
    /// folds that in with [`Witnessed::merge`] instead, unioning its reach; this primitive covers the
    /// case whose references are all region-derived or owned, so the `for<'b>` brand admits them.
    ///
    /// `build`'s return is spelled `<CarriedFamily as Reattachable>::At<'b>`, not the concrete
    /// `Carried<'b>`: the two are equal by the family's definition, but under the `for<'b>` binder the
    /// compiler does not normalize the projection lazily, so a `build` typed `-> Carried<'b>` fails to
    /// satisfy `yoke`'s `-> T::At<'b>` bound. Naming the projection makes the bounds syntactically
    /// identical. An inline closure returning a `Carried` still unifies fine at the call site.
    // Drives the object-family construction inversion
    // (design/per-node-memory.md): a region-pure leaf builds its `KObject` inside this closure.
    pub(crate) fn alloc_witnessed(
        witness: FrameSet,
        build: impl for<'b> FnOnce(&'b KoanRegion) -> <CarriedFamily as Reattachable>::At<'b>,
    ) -> Witnessed<CarriedFamily, FrameSet> {
        // Turbofish `T` explicitly: inference does not drive `yoke`'s `T` from the return type early
        // enough to check `build`'s `-> T::At<'b>` bound, so it sees `<_ as Reattachable>::At` and fails
        // to match the projection.
        Witnessed::<CarriedFamily, FrameSet>::yoke(witness, build)
    }

    /// [`alloc_witnessed`](Self::alloc_witnessed) for a construction that **embeds one owned,
    /// splice-free [`KExpression`]** — a quoted expression or an FN body. The owned `embed` is moved
    /// into the closure (its phantom lifetime forgotten for storage) and re-anchored to the yoke
    /// brand, then handed to `build`, which allocs the object **into the witness region natively** at
    /// the brand. So the object's region is co-located *by construction* — the same `for<'b>`
    /// enforcement [`alloc_witnessed`](Self::alloc_witnessed) gives — rather than asserted over an
    /// already-built value via [`Witnessed::new`]; the embedded expression contributes no region of
    /// its own.
    ///
    /// The embed must be **splice-free** ([`KExpression::is_splice_free`]): a `Spliced(Carried)` part
    /// carries a live `'a` region reference that re-anchoring to the brand would dangle. A value that
    /// references another region composes through [`Witnessed::merge`] of that region's carrier
    /// instead. The `debug_assert` pins the precondition at the call site. **No `unsafe` here** — the
    /// re-anchor routes the safe-signature `reattach_with`, whose audited retype lives once in
    /// `witnessed.rs`.
    pub(crate) fn alloc_witnessed_embedding(
        witness: FrameSet,
        embed: KExpression<'_>,
        build: impl for<'b> FnOnce(
            &'b KoanRegion,
            KExpression<'b>,
        ) -> <CarriedFamily as Reattachable>::At<'b>,
    ) -> Witnessed<CarriedFamily, FrameSet> {
        debug_assert!(
            embed.is_splice_free(),
            "alloc_witnessed_embedding requires a splice-free KExpression: a Spliced(Carried) part \
             holds a live region reference that re-anchoring to the yoke brand would dangle"
        );
        Witnessed::<CarriedFamily, FrameSet>::yoke(witness, move |region| {
            // The owned, splice-free `embed` carries no live borrow, so re-anchoring its phantom
            // lifetime to the yoke brand is the soundest case of the **safe-signature** `reattach_with`
            // — the yoke region itself the witness, bounding the result to the brand `'b` — after which
            // `build` moves it natively into `region`. The object's region is co-located by the yoke's
            // `for<'b>` brand; the only obligation is splice-freeness, asserted above.
            let embed_at = reattach_with::<KExpression<'static>, _>(embed, region);
            build(region, embed_at)
        })
    }

    pub fn alloc_scope<'a>(&'a self, s: Scope<'a>) -> &'a Scope<'a> {
        self.alloc::<Scope<'static>>(s)
    }

    pub fn alloc_module<'a>(&'a self, m: Module<'a>) -> &'a Module<'a> {
        self.alloc::<Module<'static>>(m)
    }

    pub fn alloc_signature<'a>(&'a self, s: ModuleSignature<'a>) -> &'a ModuleSignature<'a> {
        self.alloc::<ModuleSignature<'static>>(s)
    }

    /// Allocate an [`OperatorGroup`]. Lifetime-free and anchor-free, so the gate is a no-op, but it
    /// routes the same engine for a single uniform allocation path.
    pub fn alloc_operator_group(&self, g: OperatorGroup) -> &OperatorGroup {
        self.alloc::<OperatorGroup>(g)
    }

    /// Whether `ptr` was returned by a prior `alloc_object` on this region.
    pub fn owns_object<'a>(&self, ptr: *const KObject<'a>) -> bool {
        // `KObject` is invariant in `'a`, so the through-`'static` cast is required despite
        // clippy's complaint.
        #[allow(clippy::unnecessary_cast)]
        let target = ptr as *const KObject<'static> as usize;
        self.owns_addr(target)
    }

    /// When true, no value can hold a `&KFunction` pointing into this region — see the
    /// `alloc_function` invariant.
    pub fn functions_is_empty(&self) -> bool {
        self.family_len::<KFunction<'static>>() == 0
    }
}

#[cfg(test)]
impl Region<KoanStorageProfile> {
    /// Total number of values stored across all seven sub-arenas (test-only). Each `alloc_*`
    /// writes to exactly one sub-arena, so this is the precise allocation count without
    /// double-counting.
    pub fn alloc_count(&self) -> usize {
        self.family_len::<KObject<'static>>()
            + self.family_len::<KFunction<'static>>()
            + self.family_len::<Scope<'static>>()
            + self.family_len::<Module<'static>>()
            + self.family_len::<ModuleSignature<'static>>()
            + self.family_len::<KType<'static>>()
            + self.family_len::<OperatorGroup>()
    }
}

#[cfg(test)]
impl CallFrame {
    /// Test alias for [`CallFrame::new`], kept so the many frame-construction tests share one
    /// construction name distinct from production call sites.
    pub(crate) fn new_test(
        outer: &Scope<'_>,
        outer_frame: Option<Rc<FrameStorage>>,
    ) -> Rc<CallFrame> {
        CallFrame::new(outer, outer_frame)
    }

    /// Test alias for [`CallFrame::try_reset_for_tail`].
    pub(crate) fn try_reset_for_tail_test(self: &mut Rc<Self>, new_outer: &Scope<'_>) -> bool {
        self.try_reset_for_tail(new_outer)
    }
}

/// A frame's refcounted storage: the per-call `KoanRegion` plus the `outer` link that keeps
/// the lexical-ancestor frames' storage alive. An escaping value (a returned closure, a module
/// frame) pins *this* — not the [`CallFrame`] shell — so the shell stays uniquely owned and the
/// scheduler can reuse it for the next tail iteration while the escapee's captured environment
/// rides the old `FrameStorage` it still holds. Field order is load-bearing: `region` drops
/// before `outer`, so inner pointers die before the outer storage they may reference.
pub struct FrameStorage {
    region: KoanRegion,
    /// The parent per-call frame's storage: both a liveness pin — held so the ancestor frames'
    /// storage outlives this child's `outer` scope pointer — and the link [`FrameStorage::pins_region`]
    /// walks for [`FrameSet`] subsumption. Drop tears down the chain in order.
    outer: Option<Rc<FrameStorage>>,
    /// Per-call regions a value *bound into this frame's region* still borrows into. A returned
    /// closure / module rides a bare borrow into its defining (descendant) frame, whose `Rc` is held
    /// only by the producing scheduler's nodes and would drop when that scheduler tears down. Binding
    /// the value retains that frame here — the persistent pin a region-referencing value needs once
    /// its producer is gone, recovered from the value's scope `region_owner` at bind time. No cycle
    /// forms: a dispatched frame's `outer` is `None`, so a retained descendant never strong-refs back
    /// up the chain. Declared after `region` so this frame's borrowers drop before the regions they
    /// borrow into; `RefCell` because binds accrue after construction.
    retained: RefCell<FrameSet>,
}

impl FrameStorage {
    /// The run-root storage: a fresh run region with no `outer` link. Held by `run_program` (and the
    /// test harness) so the run-root scope's region has an owning Rc; [`CallFrame::adopting`] reuses
    /// it as the run frame's storage and the run-root scope records a `Weak` to it as its
    /// `region_owner`.
    pub fn run_root() -> Rc<FrameStorage> {
        Rc::new(FrameStorage {
            region: KoanRegion::new(),
            outer: None,
            retained: RefCell::new(FrameSet::empty()),
        })
    }

    /// The backing `KoanRegion`. Used for region-identity comparisons (e.g. [`FrameSet`]
    /// subsumption) by holders that pin storage but never name a `CallFrame`.
    pub(crate) fn region(&self) -> &KoanRegion {
        &self.region
    }

    /// True iff holding `self`'s `Rc` keeps the region at `region_ptr` alive — `self`'s own region or
    /// any of its `outer` ancestors (each pinned by the chain). The subsumption test [`FrameSet`]'s
    /// [`MergeWitness::merge`] uses: a member whose region another member already pins is redundant.
    pub(crate) fn pins_region(&self, region_ptr: *const KoanRegion) -> bool {
        let mut node = self;
        loop {
            // `node.region()` coerces `&KoanRegion → *const` for the address compare (as `rc_targets`).
            if std::ptr::eq(node.region(), region_ptr) {
                return true;
            }
            match &node.outer {
                Some(outer) => node = outer,
                None => return false,
            }
        }
    }

    /// Pin `frame`'s region under this frame so a value bound here that still borrows into it
    /// outlives `frame`'s producing scheduler. A no-op when this frame's own `outer` chain already
    /// keeps the region alive (self or an ancestor); the [`FrameSet`] dedups by region, so repeated
    /// binds reaching one region pin it once.
    pub(crate) fn retain(&self, frame: Rc<FrameStorage>) {
        if self.pins_region(frame.region() as *const KoanRegion) {
            return;
        }
        self.retained.borrow_mut().insert(frame);
    }
}

/// The unified region-owner witness: the set of `Rc<FrameStorage>` whose regions a carrier's value
/// reaches. A singleton for a single-region value (a scope, a same-region value, a producer frame) —
/// the common case — and larger for a multi-region value (a lifted closure reaching several source
/// regions, once [`transfer_into`](crate::witnessed) lands). Holding it pins every member region; the
/// empty set pins nothing — a frameless / run-region terminal is backed by a region that outlives the
/// carrier, so no held pin is required (the role the result slot's `None` played).
///
/// Composition ([`MergeWitness::merge`]) is set **union** with `outer`-chain subsumption: a member is
/// dropped when another member's [`FrameStorage::pins_region`] chain already keeps its region alive, so
/// the set stays an antichain of the deepest frames (a singleton whenever the members are co-lineal).
///
/// Backed by a `Vec` (a singleton in the common case); the inline `SmallVec` representation is the
/// open optimization [`transfer_into`](crate::witnessed)'s item owns.
#[derive(Clone, Default)]
pub struct FrameSet {
    frames: Vec<Rc<FrameStorage>>,
}

impl FrameSet {
    /// The empty witness — a frameless / run-region terminal that needs no held pin.
    pub fn empty() -> Self {
        FrameSet { frames: Vec::new() }
    }

    /// A single region owner — the common case (a scope, a same-region value, a producer frame).
    pub fn singleton(owner: Rc<FrameStorage>) -> Self {
        FrameSet {
            frames: vec![owner],
        }
    }

    /// Whether this set holds no region owner (the frameless / run-region terminal).
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// The sole region owner of a singleton set, or `None` for empty / multi-member sets — the hook
    /// the consumer-pull lift uses to recover the producer `FrameStorage` from a finalized terminal's
    /// witness (a finalized value is produced in exactly one frame, so its witness is a singleton).
    pub fn sole(&self) -> Option<&Rc<FrameStorage>> {
        match self.frames.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }

    /// Insert `owner` under `outer`-chain subsumption: skip it when an existing member already pins
    /// its region (dedup + the newcomer-is-an-ancestor case), else drop every existing member the
    /// newcomer subsumes and add it. Keeps the set an antichain of the deepest frames.
    fn insert(&mut self, owner: Rc<FrameStorage>) {
        let owner_region = owner.region() as *const KoanRegion;
        if self.frames.iter().any(|f| f.pins_region(owner_region)) {
            return;
        }
        self.frames
            .retain(|f| !owner.pins_region(f.region() as *const KoanRegion));
        self.frames.push(owner);
    }

    /// Fold every member of `other` into `self` under the same `outer`-chain subsumption as
    /// [`Self::insert`], **omitting** any member whose region `home` already pins (its own region or
    /// an ancestor on the `outer` chain). The per-scope reach-set folds a bound value's carrier
    /// witness through this: a scope must not witness its own home frame — the `region → scope → set →
    /// frame` cycle — and the home frame's `outer` chain already keeps every ancestor region alive, so
    /// only genuinely foreign reach lands in the set. A same-region value's singleton witness drops to
    /// nothing. `home` is `None` for a frameless scope owning no escapable region (test-only), where
    /// there is no home frame to omit. This is the structural form of [`FrameStorage::retain`]'s
    /// self-no-op, redirected from the per-frame accumulator into a scope-owned builder.
    pub(crate) fn fold_foreign(&mut self, other: &FrameSet, home: Option<&Rc<FrameStorage>>) {
        for owner in &other.frames {
            if home.is_some_and(|h| h.pins_region(owner.region() as *const KoanRegion)) {
                continue;
            }
            self.insert(Rc::clone(owner));
        }
    }

    /// Fold every member of `other` into `self`, skipping any whose region `omit` reports as already
    /// kept alive — the predicate form of [`Self::fold_foreign`] for the per-scope reach-set, which
    /// must omit a per-call frame's **lexical** ancestors (its `outer`-chain scopes) that
    /// [`FrameStorage::pins_region`] cannot see: a per-call frame carries no storage `outer` link
    /// under TCO, so the storage chain stops at its own region while the closure still holds its
    /// captured (ancestor) scope alive. Re-pinning such an ancestor in the reach-set, paired with a
    /// sibling bind of the call's result, would close a region cycle.
    pub(crate) fn fold_foreign_omitting(
        &mut self,
        other: &FrameSet,
        omit: impl Fn(*const KoanRegion) -> bool,
    ) {
        for owner in &other.frames {
            if omit(owner.region() as *const KoanRegion) {
                continue;
            }
            self.insert(Rc::clone(owner));
        }
    }

    /// Retain every member's region onto `home` — the persistent-frame analog of [`Self::fold_foreign`]
    /// for a drained top-level result, so a closure read out of the scheduler outlives its producer
    /// frames' teardown. Each member rides [`FrameStorage::retain`]'s region-subsumption dedup (a member
    /// `home` or an ancestor already pins is a no-op), so a multi-region result keeps every region it
    /// reaches read straight off the carrier's witness set.
    pub(crate) fn retain_onto(&self, home: &FrameStorage) {
        for owner in &self.frames {
            home.retain(Rc::clone(owner));
        }
    }
}

// SAFETY: each member `Rc<FrameStorage>` keeps its `KoanRegion` — and the arena pages a value lives in
// — at a fixed heap address for the whole life of the `Rc` (`Rc` is `StableDeref`), so holding the set
// pins every member region. The empty set carries no pin: a frameless value is backed by a region (the
// run region) that outlives the carrier, so no held pin is required — the `Option<W>::None` role.
unsafe impl Witness for FrameSet {}

// SAFETY: `region()` returns the first member's `KoanRegion`, a reference into storage this set's
// `Witness` impl pins (the member's `Rc` keeps it live and fixed-address), so a value built solely from
// that region is pinned by the set. `yoke` calls this on a singleton (a single-region construction) in
// this item's pilot; an empty set has no region to expose and panics — a `yoke` needs a region owner.
unsafe impl WitnessRegion for FrameSet {
    type Region = KoanRegion;
    fn region(&self) -> &KoanRegion {
        self.frames
            .first()
            .expect("WitnessRegion::region on an empty FrameSet — yoke needs a region owner")
            .region()
    }
}

// SAFETY: `merge` returns the set union (deduplicated by region pointer, a member dropped only when
// another member's `outer` chain already pins its region), so holding the result keeps every region
// either input pinned alive. Always `Some` — a set can always represent the union.
unsafe impl MergeWitness for FrameSet {
    fn merge(left: &Self, right: &Self) -> Option<Self> {
        let mut result = left.clone();
        for owner in &right.frames {
            result.insert(Rc::clone(owner));
        }
        Some(result)
    }
}

/// One user-fn call's allocation frame: a thin shell over a refcounted [`FrameStorage`]. `Rc`-pinned
/// so the scheduler manages the frame by `Rc<CallFrame>`; an escaping closure extends only the
/// *storage* (via [`Self::storage_rc`]), not the shell, so tail reuse can reset the shell's storage
/// without foreclosing on the escapee. Field order is load-bearing: `storage` drops before
/// `scope_carrier`, so the region tears down before the now-dangling child reference.
///
/// See [per-call-region/README.md](../../../design/per-call-region/README.md) for the
/// carrier set, escaping-value retention, ancestor chain, and TCO
/// frame reuse; [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
/// for the heap-pinning / drop-order invariants.
pub struct CallFrame {
    storage: Rc<FrameStorage>,
    /// The per-call child scope on the substrate's externally-witnessed [`SealedExtern`] carrier
    /// (a `&'static Scope`); read back through [`SealedExtern::attach`] against `storage` as the pin.
    scope_carrier: Option<SealedExtern<ScopeRefFamily>>,
    /// True only for the scheduler-owned run frame, which carries the top-level run scope and
    /// never drops mid-run. Its `region` is empty (top-level values live in the externally-owned
    /// run region, reached via `scope.region`), so there is nothing to lift out of it: the Done
    /// boundary skips the lift for a non-dying frame (lift exists to rescue values from a *dying*
    /// per-call region). Every per-call frame is `false`.
    non_dying: bool,
    /// The slot this frame was installed for — the body that finalizes it. Set at install; checked at
    /// that slot's `Done` / tail-`Continue` to close the frame's scope exactly when its body completes.
    /// A `Yoked` sub-expression slot sharing the frame is not the owner, so its `Done` does not close.
    owner: Cell<Option<NodeId>>,
}

impl CallFrame {
    /// Build a fresh per-call frame whose child `Scope` uses `outer` as its `outer` link.
    /// `outer_frame` must hold the parent frame's `FrameStorage` Rc when the parent is per-call;
    /// `None` when the parent is run-root — a dispatched frame strong-owns no ancestor, so an
    /// escaping value retained on a consumer frame forms no back-edge.
    pub fn new(outer: &Scope<'_>, outer_frame: Option<Rc<FrameStorage>>) -> Rc<CallFrame> {
        // The region is born inside its own `Rc<FrameStorage>`, heap-pinned from this point on, so
        // the child-scope pointer below stays valid as the storage Rc moves into the shell.
        let storage = Rc::new(FrameStorage {
            region: KoanRegion::new(),
            outer: outer_frame,
            retained: RefCell::new(FrameSet::empty()),
        });
        // The child is built from the heap-pinned `storage` handle — no `'static` claim and no
        // pointer fabrication. It derives both the region borrow and the owning `Weak` from
        // `storage`; `outer` (a longer-lived ancestor) is brand-shortened by `child_for_frame`, so
        // the two need no common lifetime and the outer link needs no `reattach_ref`.
        let child = Scope::child_for_frame(outer, &storage);
        // Stored at the region's real lifetime, then erased once through the safe `SealedExtern::erase`.
        // The local borrow of `storage` ends here (the carrier holds a `&'static` reference, not a
        // borrow of `storage`), so `storage` moves into the shell below; the `KoanRegion` stays at a
        // fixed heap address behind the Rc, keeping the erased reference valid.
        let scope_carrier = SealedExtern::erase(storage.region().alloc_scope(child));
        Rc::new(CallFrame {
            storage,
            scope_carrier: Some(scope_carrier),
            non_dying: false,
            owner: Cell::new(None),
        })
    }

    /// The scheduler-owned **run frame**: a frame that *carries an already-built run scope*
    /// rather than minting a child. Top-level execution runs against this frame so `active_frame`
    /// is never `None`, which makes a body's re-dispatch-against-its-own-scope uniformly framed
    /// (Yoked) at every depth — top level included. Marked `non_dying` so the Done boundary skips
    /// the (pointless) self-lift of top-level results.
    ///
    /// `run_storage` is the `Rc<FrameStorage>` that owns the run region — the same storage `scope`
    /// (the run root) lives in. Adopting it (rather than minting an empty region) makes this frame's
    /// `region()` equal the run-root region, so a top-level-defined FN's captured-region owner
    /// resolves to this frame's storage. The adopted run scope's borrow is erased into
    /// `scope_carrier` exactly as every per-call child scope is — the fabrication hazard is deferred
    /// to the witness-bounded re-attach.
    pub fn adopting<'a>(scope: &'a Scope<'a>, run_storage: Rc<FrameStorage>) -> Rc<CallFrame> {
        debug_assert!(
            std::ptr::eq(run_storage.region(), scope.region as *const KoanRegion),
            "adopting run_storage must own the run-root scope's region"
        );
        Rc::new(CallFrame {
            storage: run_storage,
            scope_carrier: Some(SealedExtern::erase(scope)),
            non_dying: true,
            owner: Cell::new(None),
        })
    }

    /// True only for the scheduler-owned run frame (see [`Self::adopting`]). The Done boundary
    /// reads this to skip the self-lift that a never-dying frame would otherwise perform.
    pub fn non_dying(&self) -> bool {
        self.non_dying
    }

    /// Record the slot that finalizes this frame's scope (the body installed into it). Read by the
    /// finalize-time close so it seals exactly the scope whose body just completed.
    pub fn set_owner(&self, slot: NodeId) {
        self.owner.set(Some(slot));
    }

    /// The slot that finalizes this frame's scope, if installed.
    pub fn owner(&self) -> Option<NodeId> {
        self.owner.get()
    }

    pub fn scope<'a>(&'a self) -> &'a Scope<'a> {
        // Borrow and content collapse to the receiver's `'a` (`'b = 's = 'a`).
        self.reattach_scope()
    }

    /// Scope handle bounded by `&'step Rc<Self>` — strictly shorter than the `&'a Scope<'a>`
    /// claim of [`CallFrame::scope`]. Use this for local-bind plumbing (e.g.
    /// [`Scope::bind_value`]) that does not need to escape the `Rc`'s borrow, so the caller
    /// avoids an `unsafe` `'a`-anchoring transmute on the receiving end. Borrow and content collapse
    /// to the receiver's `'step`.
    pub fn scope_for_bind<'step>(self: &'step Rc<Self>) -> &'step Scope<'step> {
        (**self).reattach_scope()
    }

    /// The child scope re-handed with a **witness-bounded** borrow: the borrow `'step` is bounded by
    /// the `&'step Rc<Self>` receiver (the frame `Rc` witness), while the scope content `'a` is free
    /// (`'a: 'step`). This is the read boundary: it hands back a reference that *cannot outlive the
    /// `Rc` it borrows from*, so storing it past the frame is a compile error rather than a
    /// fabrication. Invariance in `'a` rides structurally on the returned `Scope<'a>` (`Scope` is
    /// invariant), so this ephemeral form needs no separate brand struct. Reached through the
    /// scheduler's workload-side scope re-anchor (`reattach_node_scope`, `Yoked` slots) and
    /// [`Self::with_frame_interior`] (the seed binds).
    pub fn scope_bounded<'step, 'a: 'step>(self: &'step Rc<Self>) -> &'step Scope<'a> {
        (**self).reattach_scope()
    }

    /// The sole re-attach of the frame's child scope: borrow bounded by the `&'s self` receiver,
    /// content `'b` free (`'b: 's`). The three public accessors above are safe wrappers that only
    /// pick the lifetimes. Carries **no `unsafe`** of its own — it re-anchors through the carrier's
    /// witness-bounded [`SealedExtern::attach`], passing this frame's own storage `Rc` as the pin, so
    /// the returned borrow cannot outlive the region that `Rc` keeps alive.
    fn reattach_scope<'s, 'b: 's>(&'s self) -> &'s Scope<'b> {
        self.scope_carrier_set().attach(&self.storage)
    }

    /// Run `f` with this frame's child scope handed in at a **rank-2 (`for<'b>`)** brand, so the
    /// borrow cannot escape the closure. The dispatch handlers that consume their scope in place
    /// (e.g. `fn_value::initial`, `single_poll::type_call`) read it through this instead of cashing a
    /// free `current_scope()`, so the re-anchored borrow lives only inside `f`.
    pub fn with_scope<R>(&self, f: impl for<'b> FnOnce(&'b Scope<'b>) -> R) -> R {
        f(self.reattach_scope())
    }

    /// The child scope's externally-witnessed [`SealedExtern`] carrier, which is `Some` for the whole
    /// life of a constructed frame (`None` only transiently inside `new` / `try_reset_for_tail`
    /// before the child scope is allocated).
    fn scope_carrier_set(&self) -> &SealedExtern<ScopeRefFamily> {
        self.scope_carrier
            .as_ref()
            .expect("scope_carrier is set after construction")
    }

    /// Run `f` with this frame's per-call region and its child scope. The seed-side re-anchor: the
    /// MATCH / TRY arm and `KFunction::invoke` body seeds bind their `it` / parameters — values
    /// whose type carries the caller's `'a`, deep-cloned into this frame's region — into the child
    /// scope inside `f`.
    ///
    /// The **region** is reached through the child scope's own `region` field (`&'a KoanRegion`, a
    /// `Copy` reference), so reading it back at the scope's content `'a` needs no separate re-borrow:
    /// the same heap pin (this frame's `Rc`) that keeps the scope alive keeps the region it names
    /// alive. The **child scope** rides the bounded `scope_bounded` brand — borrow capped at the
    /// `&Rc` receiver, content `'a` — so it is *not* fabricated free; `bind_value` matches on the
    /// `'a` content. Carries **no `unsafe`**.
    pub fn with_frame_interior<'a, R>(
        self: &Rc<Self>,
        f: impl FnOnce(&'a KoanRegion, &Scope<'a>) -> R,
    ) -> R {
        let scope: &Scope<'a> = self.scope_bounded();
        f(scope.region, scope)
    }

    pub fn region(&self) -> &KoanRegion {
        &self.storage.region
    }

    /// Clone this frame's `FrameStorage` Rc — the handle an escaping value (a returned closure, a
    /// module frame) pins to keep its captured environment alive *without* pinning the shell, so
    /// tail reuse stays free to reset the shell.
    pub fn storage_rc(&self) -> Rc<FrameStorage> {
        Rc::clone(&self.storage)
    }

    /// Reset this frame for a tail-call iteration: install a fresh `FrameStorage` (a new
    /// `KoanRegion` escaping into `new_outer.region`, no `outer` link) and re-allocate the child
    /// `Scope` under `new_outer`. The old `FrameStorage` is dropped here — and its region with it —
    /// *unless* an escaped value still holds it, in which case that snapshot lives on independently
    /// while the shell reuses. Returns `false` (untouched) only when `Rc::get_mut` fails — another
    /// live `Rc<CallFrame>` (a shell clone, never an escape) foreclosing in-place reuse. See
    /// [per-call-region/frames.md § TCO frame reuse](../../../design/per-call-region/frames.md#tco-frame-reuse).
    pub fn try_reset_for_tail(self: &mut Rc<Self>, new_outer: &Scope<'_>) -> bool {
        if Rc::get_mut(self).is_none() {
            return false;
        }
        // Build the fresh storage and its child scope before touching the shell, so the region is
        // heap-pinned by the new storage Rc when it lands in the shell.
        let storage = Rc::new(FrameStorage {
            region: KoanRegion::new(),
            outer: None,
            retained: RefCell::new(FrameSet::empty()),
        });
        // The child is built from the heap-pinned `storage` handle (region borrow + owning `Weak`),
        // with `new_outer` brand-shortened by `child_for_frame` (no `reattach_ref` on the outer link).
        let child = Scope::child_for_frame(new_outer, &storage);
        let scope_carrier = SealedExtern::erase(storage.region().alloc_scope(child));
        // The local borrow of `storage` ends above, so it can move into the shell.
        let this = Rc::get_mut(self).expect("just-verified unique above");
        // Drops the old storage (and its region) unless an escapee still holds it.
        this.storage = storage;
        this.scope_carrier = Some(scope_carrier);
        true
    }
}

#[cfg(test)]
mod tests;
