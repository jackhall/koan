//! The **step-brand layer**: the one seam outside [`memory`](crate::memory) that spells the region
//! substrate directly. [`StepCarried`] is the Done-arm value carrier confined to the scheduler step
//! that built it, [`StepAllocator`] the construction context over that step's destination frame, and
//! the [`RegionBrand`] doors below are the two that mint a step-branded product.
//!
//! A member-less resident carrier (a region-pure value under a description hosted in its own
//! region) pins nothing: it is sound only as a within-step transient, covered by
//! [the step's coverage](../../../design/per-node-memory.md#the-steps-coverage) until
//! [`StepCarried::seal_at_step`] folds the producing frame into its reach. [`StepCarried`] makes
//! that transient a type: the carrier crossing the Done arm rides a brand lifetime `'step` that is
//! the step's rank-2 open lifetime ([`Host::step`](super::harness::Host)), unnameable outside that
//! closure, so the borrow checker rejects any attempt to stash it past its construction step.
//!
//! The brand doors live here rather than in `memory` for the same reason: `StepCarried`'s only exit
//! is `pub(super)` inside `execute`, so a door that returns one cannot sit below it.

use std::marker::PhantomData;
use std::rc::Rc;

use crate::machine::core::Scope;
#[cfg(any(test, feature = "region-audit"))]
use crate::machine::execute::reach_audit;
use crate::machine::model::{Carried, CarriedFamily, DeliveredCarried};
use crate::machine::model::{KObject, KType, Scalar};
use crate::memory::{
    Delivered, DropFree, FoldingBrand, FrameStorage, KoanStorageProfile, Reattachable, RegionBrand,
    StepContext, Unhosted, Witnessed, run_root_storage,
};
use crate::parse::ProgramExpression;

/// A value carrier confined to the scheduler step that built it, so it cannot be stored past that
/// step.
///
/// The `inner` pair is **private to this module** — that privacy is the mechanism. The sole exit
/// is [`Self::seal_at_step`], which consumes the wrapper into a delivery envelope; no accessor hands
/// back the lifetime-free [`Witnessed`] for a holder to stash. `PhantomData<&'step ()>` is
/// covariant, matching [`FoldToken`](crate::memory::FoldToken): escaping the brand would require
/// *lengthening* `'step`, which covariance forbids and unnameability prevents.
///
/// The type is `pub` only so the `#[doc(hidden)]` `step_fixture` can drive it from a `compile_fail`
/// external crate (the `machine::execute` module is `pub(crate)`, so it is not part of koan's real
/// API). The confinement rests on [`born`](Self::born) (`pub(crate)`) and
/// [`seal_at_step`](Self::seal_at_step) (`pub(super)`) being unreachable outside the crate, plus the
/// brand lifetime — never on the type being unnameable.
pub struct StepCarried<'step, T: Reattachable + DropFree = CarriedFamily> {
    /// The host is the *finalizing* node's anchor owner, not known at any door that builds a
    /// Done-arm value, so the carrier rides fused to its owned coverage minus that pin.
    /// [`Self::seal_at_step`] supplies the host and the pair becomes the delivery envelope, so the
    /// terminal's reach is threaded from here — never re-derived from the carrier's description.
    inner: Unhosted<T>,
    step: PhantomData<&'step ()>,
}

impl<'step, T: Reattachable + DropFree> StepCarried<'step, T> {
    /// Wrap a **no-foreign-reach** carrier, whose owned pin bundle is therefore empty. Unrestricted
    /// in-crate: wrapping only ever *adds* confinement.
    ///
    /// The premise is established at the doors that build such a carrier, not checked here: a
    /// carrier eligible for this wrapper comes from
    /// [`Scope::resident`](crate::machine::core::Scope) / [`RegionBrand::seal_resident`](crate::memory::RegionBrand),
    /// whose mint composes no source and so yields a description with no members at all, or from the
    /// checked-alloc door, whose only possible member is the birth region itself. Neither can name a
    /// foreign region, so neither has owned pins to thread. A producer whose value *does* reach
    /// elsewhere takes [`Self::born_delivered`], which carries the bundle. (Re-checking it here is
    /// not available anyway: the membership queries live on the opened carrier, and this door holds
    /// no pin to open one under.)
    pub(crate) fn born(inner: Witnessed<T>) -> Self {
        StepCarried {
            inner: Unhosted::born(inner),
            step: PhantomData,
        }
    }

    /// Wrap a **reach-carrying** carrier by consuming its whole delivery envelope. The envelope is
    /// the only source, and it is never taken apart: [`Delivered::unhost`] drops the home pin and
    /// keeps the carrier fused to its coverage, so the two cannot arrive from different values and a
    /// caller cannot drop the pins and brand the carrier alone. The coverage travels whole,
    /// residence included — [`Self::seal_at_step`] pins a host back on, so the terminal's reach is
    /// owned end-to-end rather than re-derived.
    pub(crate) fn born_delivered(envelope: Delivered<T>) -> Self {
        StepCarried {
            inner: envelope.unhost(),
            step: PhantomData,
        }
    }

    /// The only exit from the step brand. `pub(super)` so the seal/finalize sites in [`super`] can
    /// call it while `crate::builtins` cannot — a builtin holding a `StepCarried` cannot strip the
    /// brand.
    ///
    /// This door trusts its caller to pass the *right* host (the anchor's owner); binding that free
    /// parameter is a separate concern. The door's contract here is only that it is the unique way a
    /// `StepCarried` reaches node storage.
    pub(super) fn seal_at_step(self, host: Rc<FrameStorage>) -> Delivered<T> {
        self.inner.host(host)
    }

    /// Seal over the anchor's owner and read the delivered value by reference, without ever
    /// extracting the carrier: `read` sees a `for<'b>` re-anchored view and returns owned data, so
    /// the lifetime-free [`Witnessed`] never leaves the wrapper. `host` is the anchor's owner, the
    /// same trust [`Self::seal_at_step`] places. `#[cfg(test)]`-gated, so the no-stash compile
    /// guarantee holds for every production build; it exists because a `machine::core` door test
    /// cannot reach the `pub(super)` [`Self::seal_at_step`] exit.
    #[cfg(test)]
    pub(crate) fn inspect_at<R>(
        self,
        host: Rc<FrameStorage>,
        read: impl for<'b> FnOnce(&'b <T as Reattachable>::At<'b>) -> R,
    ) -> R {
        self.inner.host(host).open_ref(read)
    }
}

/// The step-branded construction context: the library [`StepContext`] over the step's destination
/// frame, confined to the scheduler step that minted it by the brand lifetime `'step`. The koan
/// construction doors live here — not on `StepContext` itself — because [`RegionBrand`]'s
/// constructor is private to the `arena` module (see [`FrameStorage::brand`]): each door's closure
/// receives a [`RegionBrand`] / [`FoldingBrand`] (the koan allocation capability) rather than the
/// bare `&KoanRegion` the library-level context hands out. Named with full words (`alloc_carried`,
/// not `alloc`) to avoid colliding with the generic verb each wraps.
///
/// Every door returns its carrier as a [`StepCarried`] at `'step` — in production the step tail's
/// rank-2 open lifetime, so a door product cannot be stashed past its construction step (the
/// within-step transient invariant, borrow-checker-enforced). [`Self::alloc_carried_with`] is how a
/// finish folds a dep's reach into a carrier it builds from that dep's value: the dep views only
/// exist inside the shared brand, so a caller cannot smuggle one out and seal it under a narrower
/// reach than the fold produces.
///
/// The type is `pub` and [`Self::alloc_object_scalar`] is `pub`, so the `#[doc(hidden)]`
/// `step_fixture` can hand an external `compile_fail` guard an allocator and have it door-allocate;
/// the remaining doors are crate-visible. The constructors
/// are crate-confined, so no external caller can mint one. The confinement rests on the brand
/// lifetime and on the constructors' visibility — builtins receive an allocator already branded at
/// their step (`BodyCtx.ctx` / `FinishCtx.ctx`) and cannot mint one at a lifetime of their choosing.
///
/// Every door here yields a *carrier*, whose value is erased and re-anchored on read, so each takes
/// a fresh `for<'b>` brand. A [`WorkingExpression`](crate::machine::model::WorkingExpression) is not
/// a carrier — it is a plain borrowing node that must live at `'step` — and this context holds its
/// destination frame as an owned `Rc`, which no `'step` brand can be derived from. Its doors
/// (`BodyCtx::working` and its `FinishCtx` peer) hang off the step's own scope instead, whose
/// [`RegionBrand`] is already at `'step` and names this same destination region.
#[derive(Clone)]
pub struct StepAllocator<'step> {
    context: StepContext,
    step: PhantomData<&'step ()>,
}

impl<'step> StepAllocator<'step> {
    /// Mint over the step's destination frame at a caller-chosen brand — the harness door (the
    /// scheduler view's step door mints at the step's own `'step`). `pub(in crate::machine)` keeps
    /// the free-brand mint out of builtins' reach.
    pub(in crate::machine) fn over_frame(frame: Rc<FrameStorage>) -> Self {
        StepAllocator {
            context: StepContext::new(frame),
            step: PhantomData,
        }
    }

    /// Mint over `scope`'s own frame, branded at the scope reference's lifetime. Sound to expose
    /// in-crate: every production `&Scope` is minted at a step's rank-2 open (the step-brand
    /// design's verified premise), so the allocator inherits a genuinely step-confined brand.
    pub(crate) fn for_scope(scope: &'step Scope<'step>) -> Self {
        Self::over_frame(scope.frame())
    }

    /// The held destination-frame `Rc` itself — for callers that pin or compare the frame.
    pub(crate) fn frame(&self) -> Rc<FrameStorage> {
        self.context.frame()
    }

    /// [`StepContext::alloc`] with the closure receiving a [`RegionBrand`]: reach = own region only.
    pub(crate) fn alloc_carried(
        &self,
        build: impl for<'b> FnOnce(RegionBrand<'b>) -> <CarriedFamily as Reattachable>::At<'b>,
    ) -> StepCarried<'step> {
        StepCarried::born(
            self.context
                .alloc::<KoanStorageProfile, CarriedFamily>(|handle| build(RegionBrand(handle))),
        )
    }

    /// [`StepContext::alloc_with`] with the closure receiving a [`FoldingBrand`] and the deps'
    /// views: the built carrier names every listed dep's reach **and residence host** (each dep
    /// arrives as its delivery envelope and relocates at `Residence::Kept`), by construction — so a
    /// value the closure builds from those deps' operands is covered by that composition, and
    /// [`FoldingBrand`]'s folded-placement methods store it without a per-value audit. Plain
    /// [`RegionBrand`] methods stay reachable through `Deref`, so a closure building an unrelated
    /// `'static` value is unaffected.
    pub(crate) fn alloc_carried_with(
        &self,
        deps: &[&DeliveredCarried],
        build: impl for<'b> FnOnce(FoldingBrand<'b>, &'b [Carried<'b>]) -> Carried<'b>,
    ) -> StepCarried<'step> {
        // The fold composes the deps' reach into the built carrier and hands back the product as a
        // delivery envelope homed in this context's own frame; `born_delivered` releases that home
        // (the seal re-pins it) so the product's foreign coverage rides the step to the seal.
        //
        // The operand views and the product are nameable only inside the brand, so the tightness
        // audit's address walk runs there and only `usize` addresses cross back out to the
        // comparison below.
        #[cfg(any(test, feature = "region-audit"))]
        let audit = reach_audit::FoldAudit::begin("StepAllocator::alloc_carried_with");
        let delivered = self
            .context
            .alloc_with::<KoanStorageProfile, CarriedFamily, CarriedFamily>(
                deps,
                |placement, views| {
                    #[cfg(any(test, feature = "region-audit"))]
                    for view in views {
                        audit.note_operand(view);
                    }
                    let product = build(FoldingBrand::in_fold_closure(placement), views);
                    #[cfg(any(test, feature = "region-audit"))]
                    audit.note_product(&product);
                    product
                },
            );
        #[cfg(any(test, feature = "region-audit"))]
        audit.finish(deps, &delivered);
        StepCarried::born_delivered(delivered)
    }

    /// Wrap a `Copy` [`KType`] handle in a step terminal: reach = own region only. A handle carries
    /// no region content, so it rides `Carried::Type` by value with no storage door.
    pub(crate) fn type_carried(&self, kt: KType) -> StepCarried<'step> {
        self.alloc_carried(|_| Carried::Type(kt))
    }

    /// The no-fold arm for a shallow scalar (Number / Bool / Null): such a value embeds no borrow, so
    /// it rebuilds owned and seals with an empty reach instead of over-retaining a producer arena.
    /// `None` when the value is not one and the caller takes a fold door instead — a `KString` is the
    /// notable one: its bytes live in a region's bump, so the rebuild would have to re-home them and
    /// a string producer folds.
    ///
    /// The test and the rebuild are the same verb ([`KObject::as_scalar`]), so the `Scalar` handed to
    /// the store door is the evidence the value was one — there is no residual arm to declare
    /// unreachable.
    pub fn alloc_object_scalar(&self, value: &KObject<'_>) -> Option<StepCarried<'step>> {
        let scalar = value.as_scalar()?;
        Some(self.alloc_carried(|b| Carried::Object(b.alloc_scalar(scalar))))
    }
}

/// The two brand doors whose product is step-branded — the witnessed-allocation surface for an
/// owned leaf built fresh inside the brand, and the store for a `#(...)` quote's body as data.
///
/// An inherent block here rather than in [`memory`](crate::memory) because each returns a
/// [`StepCarried`], whose sole exit is `pub(super)` inside this module: a door minting one cannot
/// live below the confinement it feeds.
impl<'a> RegionBrand<'a> {
    /// Born under a description hosted in this region with **no members**:
    /// [`RegionBrand::alloc_scalar`] stores the value and [`RegionBrand::seal_resident`] names the
    /// region-pure obligation, so the active frame is deliberately excluded from the pins. The
    /// producing frame is folded in only at finalize/close (the scope-reach seal), so a
    /// region-resident value never strong-owns its own frame (the `region → object → frame` cycle
    /// that would keep the frame's `Rc` alive forever and defeat the refcount-driven region free).
    ///
    /// The within-step transient invariant is typed: the member-less carrier pins nothing, so it
    /// returns as a [`StepCarried`] branded at this brand's own `'a` — in production a step's
    /// rank-2 open lifetime — and the borrow checker rejects any use past the step. The active
    /// frame pins the region across the step; the seal that moves the product into node storage is
    /// where finalize's fold names the producer in the carrier's own reach.
    ///
    /// [`Scalar`] is region-purity as a signature: a value that references another region cannot
    /// spell itself as one, and takes the `yoke` / `merge` path or
    /// [`Self::alloc_expression_witnessed`] instead.
    pub(crate) fn alloc_scalar_witnessed(self, scalar: Scalar) -> StepCarried<'a> {
        StepCarried::born(
            self.seal_resident::<CarriedFamily>(Carried::Object(self.alloc_scalar(scalar))),
        )
    }

    /// [`RegionBrand::alloc_expression`] bundled as the resident carrier, sealed under the same
    /// member-less own-region description [`Self::alloc_scalar_witnessed`] mints.
    pub(crate) fn alloc_expression_witnessed(
        self,
        expression: ProgramExpression<'a>,
    ) -> StepCarried<'a> {
        StepCarried::born(
            self.seal_resident::<CarriedFamily>(Carried::Object(self.alloc_expression(expression))),
        )
    }
}

/// Hand a step allocator to `guard` at a `for<'b>` rank-2 brand — the step tail's confinement shape.
/// `'b` is universally quantified over `guard`, so a guard body can allocate through the allocator's
/// doors but cannot store a door product past the closure (doing so makes `'b` escape — the
/// `compile_fail` pin for the door half of the brand). Lives here rather than in the crate-root
/// `step_fixture` because [`StepAllocator::over_frame`] is `pub(in crate::machine)`.
/// `#[doc(hidden)]` fixture surface, not real API.
#[doc(hidden)]
pub fn drive_step_allocator(guard: impl for<'b> FnOnce(StepAllocator<'b>)) {
    guard(StepAllocator::over_frame(run_root_storage()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::CallFrame;
    use crate::machine::model::RecordSubstrate;
    use crate::machine::model::{DeliveredCarried, Held};
    use crate::machine::model::{KObject, Record};
    use crate::memory::{FoldedPlacement, FrameStorageExt, program_storage, run_root_storage};

    /// The legal shape: born a region-pure carrier, then exit through the sole seal door into a
    /// delivery envelope pinned by its own storage.
    #[test]
    fn born_then_seal_at_step_round_trips() {
        let storage = run_root_storage();
        let step_carried: StepCarried = storage.brand().alloc_scalar_witnessed(Scalar::Number(7.0));
        let envelope = step_carried.seal_at_step(Rc::clone(&storage));
        let value = envelope.open(|c| match c {
            Carried::Object(KObject::Number(n)) => *n,
            _ => panic!("expected a Number object"),
        });
        assert_eq!(value, 7.0);
    }

    /// FROM's own construction shape — [`record_projection::body`](crate::builtins::record_projection)
    /// narrows a record's carried type by sharing its substrate borrow whole, built at the fold brand
    /// from the delivered `record` operand's view (`alloc_carried_with`). The combinator's
    /// by-construction dep-run relocation is pinned library-side
    /// (`alloc_with_folds_dep_reach_before_result_read` in the workgraph slate); this exercises it over
    /// the `Record` substrate specifically: the substrate stays in the *producer's* region (never
    /// copied — `record_with_type` swaps only the type handle), and the composed reach is what keeps
    /// that region alive once every producer handle drops. A regression that copied the substrate instead of sharing it would still pass
    /// (a copy is also readable); the pointer-identity assertion is what actually pins "shares, never
    /// copies," while Miri is what catches a dangling read if the reach fold is skipped.
    #[test]
    fn record_retype_shares_substrate_across_producer_frame_free() {
        let program = program_storage();
        let root = run_root_storage();
        let test_run = TestRun::silent(&program, &root);
        let scope = test_run.scope;

        // Producer: a plain-data record resident in its own frame's region, born through the fold
        // door — the exact shape FROM's `record` operand arrives as. Allocated through the frame's own
        // brand (not a transient `with_resident` sub-brand), so the reference escapes at the frame's own
        // lifetime.
        let producer_frame: Rc<CallFrame> = scope.open_frame();
        let owned_cells = crate::memory::FrameCoverage::empty();
        let door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(
            producer_frame.brand().handle(),
        ))
        .with_holder(&owned_cells);
        let fields = Vec::from([
            (
                crate::builtins::test_support::binder_token("x"),
                Held::Object(KObject::Number(1.0)),
            ),
            (
                crate::builtins::test_support::binder_token("y"),
                Held::Object(KObject::Number(2.0)),
            ),
        ]);
        let obj: &KObject<'_> = door.alloc_object_folded(KObject::record_of_held(
            door,
            fields.as_slice(),
            test_run.types(),
        ));
        // `RecordSubstrate` is invariant in its lifetime, so the comparison casts through `usize`
        // rather than keeping a lifetime-parameterized raw pointer type alive across the fold below.
        let expected_addr = match obj {
            KObject::Record(substrate, _) => *substrate as *const RecordSubstrate<'_> as usize,
            other => panic!(
                "expected a Record, got {}",
                other.ktype().name(test_run.registries())
            ),
        };
        let dep: DeliveredCarried = producer_frame
            .brand()
            .deliver_resident::<CarriedFamily>(Carried::Object(obj));

        // Consumer: a different frame — FROM's own step surface, narrowing to just `{x}`.
        let consumer_frame: Rc<CallFrame> = scope.open_frame();
        let ctx = StepAllocator::over_frame(consumer_frame.storage_rc());
        let narrowed_type = test_run.types().record(Record::from_pairs([(
            crate::builtins::test_support::binder_token("x"),
            KType::NUMBER,
        )]));
        let sealed: StepCarried = ctx.alloc_carried_with(&[&dep], move |b, views| {
            let substrate = match views[0] {
                Carried::Object(KObject::Record(substrate, _)) => substrate,
                _ => panic!("expected a Record dep view"),
            };
            Carried::Object(
                b.alloc_object_folded(KObject::record_with_type(substrate, narrowed_type)),
            )
        });

        // Drop the dep envelope and every frame shell: only the fold's minted reach (through the
        // retained consumer storage) keeps the producer's region alive.
        let consumer_storage = consumer_frame.storage_rc();
        drop(dep);
        drop(producer_frame);
        drop(consumer_frame);

        let read_addr = sealed.inspect_at(Rc::clone(&consumer_storage), |c| match c.object() {
            KObject::Record(substrate, record_type) => {
                assert_eq!(
                    *record_type, narrowed_type,
                    "narrowed to the FROM-selected type"
                );
                *substrate as *const RecordSubstrate<'_> as usize
            }
            other => panic!(
                "expected a Record, got {}",
                other.ktype().name(test_run.registries())
            ),
        });
        assert_eq!(
            read_addr, expected_addr,
            "the narrowed record shares the exact same substrate borrow — never copies — read back \
             after the producer frame freed"
        );
    }
}
