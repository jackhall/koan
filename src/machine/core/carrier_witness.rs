//! Koan's instantiation of the library's reference-only carrier witness
//! ([`crate::witnessed::Carrier`]) over `F = FrameStorage` (the per-call frame owner), and the
//! delivery envelope that carries a value's retained frame pin in transit. See
//! [workgraph/design/reach.md § The carrier states](../../../workgraph/design/reach.md#the-carrier-states).

use crate::machine::model::{
    Carried, CarriedFamily, DispatchToken, KObject, OperatorGroupFamily, UntypedKey, retains_home,
};

use super::arena::{FrameStorage, KoanRegion};
use super::kfunction::KFunctionFamily;
use super::scope::Scope;
use crate::machine::model::RunRegistries;

/// Koan's value-carrier witness: the library [`Carrier`](crate::witnessed::Carrier) over koan's
/// frame owner — a reference to the value's hosted reach description and nothing else. The
/// description carries both of the value's region facts: its *host* is the region the value lives
/// in, its *members* are the regions the value's borrows reach, home among them exactly when the
/// value genuinely borrows into its own region. The carrier pins nothing; liveness is always the
/// containing region — the producer's own while the delivery walk carries the terminal, the
/// destination's the moment the walk adopts it in. Every site that only
/// *threads* this type as the `W` witness parameter of `Witnessed<T, W>` / `Sealed<T, W>` is
/// unaffected by this alias; a site that constructs or inspects a carrier routes the library's
/// `Carrier` surface directly.
pub type CarrierWitness = crate::witnessed::Carrier<FrameStorage>;

/// Koan's **delivery envelope**: the library [`Delivered`](crate::witnessed::Delivered) carrying a
/// [`CarrierWitness`]-witnessed value carrier paired with its retained [`FrameStorage`] owner. The
/// in-transit form of a value's liveness — from a scheduler pull (or a resident seal) to its
/// adoption. The retained frame is private to the envelope and materializes into a minted reach set
/// only through the envelope's own verbs (`adopt_into` / `open_adopted` / `transfer_into`), so koan
/// never holds a bare frame pin at a consumer site. The
/// envelope's member set pins the value's home region alongside everything else it reaches, and the
/// residence itself is the host of the description the carrier references — so a site that needs the
/// home back reads it off the value's own record rather than off a side channel on the envelope, and
/// a relocation derives what it still reaches from the product it built ([`product_reaches_region`])
/// rather than choosing a bundle up front.
pub type DeliveredCarried =
    crate::witnessed::Delivered<CarriedFamily, CarrierWitness, FrameStorage>;

/// A callable **in transit from its birth**: the merge-born `KFunction` carrier paired with the home
/// pin its birth composed. What [`KFunction::alloc_captured`](crate::machine::core::KFunction::alloc_captured) hands back and what every registration
/// door composes from — the seal ([`OverloadSeal::of_delivered`]) rests it, the `KObject` wrapper
/// ([`Scope::store_function_cell`](crate::machine::core::Scope)) merges it — so no door re-states the
/// callable's reach on its own authority.
pub type DeliveredFunction =
    crate::witnessed::Delivered<KFunctionFamily, CarrierWitness, FrameStorage>;

/// A resolved sub-result **at rest** inside a working expression: the producer's sealed value
/// carrier alone, `Copy` and `Drop`-free, with the pins that keep its backing alive lodged one level
/// down in the region the cell was rested into
/// ([`Delivered::rest_in`](crate::witnessed::Delivered::rest_in), reached through
/// [`Scope::rest_delivered`](crate::machine::core::Scope::rest_delivered)). The resting form of a
/// [`DeliveredCarried`]: same carrier, ownership relocated — which is what lets an
/// [`WorkingPart`](crate::machine::model::WorkingPart) hold one without becoming heap-shaped.
///
/// Reading one names its coverage, as every reference-only carrier does: the reach-carrying route is
/// [`Scope::lift_spliced`](crate::machine::core::Scope::lift_spliced), back to an envelope for an
/// adoption; a verdict-only reader opens the cell at its own brand through [`read_resting`].
pub type SplicedCell<'home> = crate::witnessed::Sealed<'home, CarriedFamily, CarrierWitness>;

/// Read a resting splice cell at a site with **no pin vocabulary** — the registry-free renderers
/// ([`WorkingPart`](crate::machine::model::WorkingPart)'s `Debug` / `summarize`) and the slot
/// classifier `KType::accepts_cell`). Each is a pure
/// probe over a part the caller already holds, reached from signatures that carry no scope and (for
/// `Debug::fmt`) could not be given one.
///
/// The coverage is the step's, not the reader's: a probe runs synchronously inside the step holding
/// the expression, and a cell rests in that step's own cart — the splice and every read of it happen
/// on one side of a tail hop, never across one. So the pointee outlives the read for a reason
/// outside it, which is exactly what [`NoPins`] names. Stated once here so the
/// assertion has one home rather than one per call site. A reader that holds a scope names a pin
/// instead: [`Scope::read_spliced`](crate::machine::core::Scope::read_spliced) for another verdict,
/// [`Scope::lift_spliced`](crate::machine::core::Scope::lift_spliced) when it goes on to *adopt* the
/// value, which owns the reach rather than merely naming it.
pub(crate) fn read_resting<R>(
    cell: &SplicedCell<'_>,
    read: impl for<'b> FnOnce(Carried<'b>) -> R,
) -> R {
    cell.open(read)
}

/// A callable's **dormant** carrier: the `KFunction` fused to the exact reach description its birth
/// composed for it, over the [`KFunctionFamily`] the library dispatches on. This is what a `functions` dispatch
/// bucket stores and what a [`ReturnContract`](crate::machine::core::ReturnContract) carries across
/// a tail chain: the seal fuses the callable with its reach claim, where a bare `&KFunction` would
/// state no reach at all.
pub type SealedFunction<'home> = crate::witnessed::Sealed<'home, KFunctionFamily, CarrierWitness>;

/// An operator group's **dormant** carrier: the region-hosted [`OperatorGroup`](crate::machine::model::OperatorGroup) record fused to the
/// reach description its yoked birth composed for it, over the [`OperatorGroupFamily`]. This is what an `operators`
/// registry entry stores — the same entry shape the `data` and `functions` tables use, so a
/// [`Bindings`](crate::machine::core::Bindings) table stays lifetime-free. Every powerset key of one
/// `GROUP` declaration holds a duplicate of the same seal over the same pointee, so sharing is
/// address identity.
pub type SealedOperatorGroup<'home> =
    crate::witnessed::Sealed<'home, OperatorGroupFamily, CarrierWitness>;

/// An operator group **in transit**: [`SealedOperatorGroup`] lifted at its declaring scope, so the
/// envelope's coverage owns the region hosting the record — which is what lets a chain resolve a
/// group declared in an ancestor scope and read it under pins of its own.
pub type DeliveredOperatorGroup =
    crate::witnessed::Delivered<OperatorGroupFamily, CarrierWitness, FrameStorage>;

/// A callable **in use**: re-anchored at a region's own lifetime, paired with the reach witness it
/// was opened under. Dispatch resolves on one of these and carries it across argument evaluation
/// (`Resolved<'step>`); the escape into the call chain
/// [`reseal`](crate::witnessed::Opened::reseal)s it back to a [`SealedFunction`].
pub type OpenedFunction<'a> = crate::witnessed::Opened<'a, KFunctionFamily, CarrierWitness>;

/// Everything a dispatch-bucket registration needs about a callable, computed at seal time — the
/// one moment the callable is open under its home pin — so no write verb ever opens a carrier.
/// `sealed` is what the `functions` bucket stores; the rest is plain data with no region lifetime.
///
/// A keyworded expression becomes dispatchable by registering one of these: the dispatch bucket is
/// private to its tables and its write verb (`write_overload`) takes an `OverloadSeal` by value, so
/// a bucket entry cannot exist without one. Binding a function *value* (`LET g = (f)`) publishes
/// nothing here: a value binding is called by name.
pub(crate) struct OverloadSeal<'a> {
    /// The dormant callable carrier the dispatch bucket stores.
    pub sealed: SealedFunction<'a>,
    /// `signature.untyped_key()` — the bucket this callable belongs in.
    pub key: UntypedKey,
    /// `signature.dispatch_token()` — the stored form of the duplicate-overload predicate.
    pub token: DispatchToken,
    /// `KFunction::summarize()`, rendered here so the `DuplicateOverload` diagnostic can name the
    /// colliding overload without re-opening it.
    pub summary: String,
}

impl<'a> OverloadSeal<'a> {
    /// The bundle for a callable **fresh from its witnessed birth** — the `FN` / `OP` registration
    /// doors and the builtin seeds. Nothing is minted here: the description the bucket stores is the
    /// one [`KFunction::alloc_captured`] composed, naming the callable's home region as its host and
    /// its one member, so the reach claim the bucket carries is the birth's derived fact rather than
    /// a restatement at the registration site.
    ///
    /// `scope` must be the defining scope — the region the callable was born into. The envelope's
    /// coverage is lodged there ([`Delivered::rest_in`](crate::witnessed::Delivered::rest_in)),
    /// which the library's self rule makes free for a value already resident in it. Everything the
    /// bucket write keys on is read inside the envelope's own open and travels as plain data.
    pub(crate) fn of_delivered(
        scope: &'a Scope<'a>,
        cell: &DeliveredFunction,
        registries: &RunRegistries,
    ) -> Self {
        let (key, token, summary) = cell.open(|f| {
            (
                f.signature.untyped_key(),
                f.signature.dispatch_token(),
                f.summarize(registries),
            )
        });
        OverloadSeal {
            sealed: cell.rest_in(scope.brand().handle()),
            key,
            token,
            summary,
        }
    }
}

/// Everything an operator-registry registration needs about a group, computed at seal time — the
/// one moment the record is open — so no write verb ever opens a carrier. `sealed` is what the
/// `operators` table stores; the rest is plain data with no region lifetime. The operator-table twin
/// of [`OverloadSeal`].
///
/// One of these backs a whole `GROUP` declaration: every powerset key of the install names the same
/// record, so the write applies this one bundle across all of its probe keys.
#[derive(Clone)]
pub(crate) struct GroupSeal<'a> {
    /// The dormant group carrier the registry entry stores.
    pub sealed: SealedOperatorGroup<'a>,
    /// The record's address — the upsert's **cheap** identity arm. Every powerset key of one
    /// declaration shares one pointee, so re-registering a key that is already installed compares
    /// equal here without ever touching the record.
    pub address: usize,
    /// `OperatorGroup::declaration_key()` — the stored form of the upsert's **structural** arm, for
    /// the two-`OP`-statements case where one declaration allocates two records.
    pub declaration: String,
}

impl<'a> GroupSeal<'a> {
    /// The bundle for a group record **fresh from its yoked birth** — the `GROUP` binder, the `OP`
    /// declaration doors, and the builtin seeds, each of which births its record at the very region
    /// it registers against ([`Scope::birth_operator_group`](crate::machine::core::Scope)). Nothing
    /// is minted here: the yoke brand is the compile-time proof that the record is region-pure —
    /// [`OperatorGroup::alloc`](crate::machine::model::OperatorGroup::alloc) re-homes every byte it stores at the brand it is handed, so no
    /// foreign borrow can inhabit the built value — and the description the birth composed says
    /// exactly that: hosted at the declaring region, with no members. The seal rests that envelope.
    ///
    /// `scope` must be the declaring scope, the region the record was born into; resting there is
    /// free under the library's self rule. The address and the declaration key are both read inside
    /// the envelope's own open and travel as region-free data.
    pub(crate) fn of_delivered(scope: &'a Scope<'a>, cell: &DeliveredOperatorGroup) -> Self {
        let (address, declaration) =
            cell.open(|group| (std::ptr::from_ref(group) as usize, group.declaration_key()));
        GroupSeal {
            sealed: cell.rest_in(scope.brand().handle()),
            address,
            declaration,
        }
    }
}

/// Koan's **retention claim** for a copying relocation of `envelope`
/// ([`Delivered::transfer_into`](crate::witnessed::Delivered::transfer_into),
/// design/witness-hosting.md § Escape): whether `product` — what the fold just built at the
/// destination — still borrows `region`, one of the regions the envelope pins. Answered by
/// [`retains_home`], a read over `product`'s stored reach; no probe walks its shape.
///
/// A copy releases only the value's own home region
/// ([value-substrates.md § Sectioned reach](../../../design/value-substrates.md#sectioned-reach)).
/// `region` is home exactly when the value's own reach description names it as host — read off the
/// carrier through the envelope's open, so residence is answered by identity against the value's own
/// record rather than a side channel on the envelope. A non-home member is kept because it may be
/// reached through structure the product's stored reach does not cover (a `KFunction`'s captured
/// environment reaches on transitively), so releasing it would dangle.
///
/// A `product` of `None` (the fold built no object — a type-channel cell) keeps every member.
/// Releasing home is what frees a tail loop's retiring region once its delivered carrier drops,
/// instead of chaining it into every successor region's arena.
pub(crate) fn product_reaches_region(
    envelope: &DeliveredCarried,
    product: Option<&KObject<'_>>,
    region: &KoanRegion,
) -> bool {
    let is_home = envelope
        .open_at()
        .with_home_region(|home| std::ptr::eq(home, region));
    !is_home || product.is_none_or(|value| retains_home(value, region))
}
