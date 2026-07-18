//! NewType + tagged-union construction dispatch. Both the `TypeCall` fast lane (single_poll)
//! and the `FunctionValueCall` fast lane (fn_value) route a resolved verb-carrier here. Args
//! resolve through per-value eager sub-Dispatches; when all are bound, `finish` validates
//! types and emits the `KObject::Wrapped` / `KObject::Tagged` directly — no bucket lookup, no
//! re-dispatch. Reusing the eager-subs `AwaitDeps` machinery (rather than a
//! standalone `AwaitDeps`) is load-bearing: it stages an already-ready value in place and parks
//! a deferred one on the construction node itself, so a newtype built from a still-pending
//! reference (`(Boxed (p))` where `p` is a sibling construction) finalizes correctly.

use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::DepPlacement;
use crate::machine::core::{
    FoldingBrand, FrameStorage, KoanRegionExt, KoanStorageProfile, Scope, StoredReach,
};
use crate::machine::model::TypeRegistry;
use crate::machine::model::{Carried, KObject, Record};
use crate::machine::model::{CarriedFamily, WrappedPayload};
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::model::{KType, ProjectedSchema, RecursiveSet};
use crate::machine::{
    CarrierWitness, DeliveredCarried, FrameSet, KError, KErrorKind, KoanRegion, RegionTypeFamily,
};
use crate::source::Spanned;
use crate::witnessed::Residence;
use crate::witnessed::{reattachable, RegionHandle, Witnessed};

use super::super::outcome::DepTerminal;
use super::super::run_loop::{dest_brand, DestHandleFamily};
use super::super::{StepCarried, WitnessedDepFinish};
use super::ctx::SchedulerView;
use super::single_poll::CtorKind;
use super::{Await, DepRequest, Outcome};
use crate::scheduler::{DepResults, Deps};

/// Fold accumulator for a record-repr newtype: the destination region plus the field values
/// gathered from the value deps, each `transfer_into`-folded so the accumulator's witness composes
/// by minting into that region (the [`HasRegionHandle`](crate::witnessed::HasRegionHandle) seam).
/// The final `merge` with [`RegionTypeFamily`] builds the `Record` and wraps it with the identity.
/// Layout-invariant: a thin region pointer and a `Vec` of layout-invariant `(String, KObject)` cells
/// — the same shape as [`dispatch::literal`](super::literal)'s `AggBuildFamily`.
struct RecordFieldsFamily;
reattachable!(RecordFieldsFamily => (RegionHandle<'r, KoanStorageProfile>, Vec<(String, KObject<'r>)>));

/// Validate a tagged-union call site's args shape: exactly two parts, the first a
/// `Type`-token tag (tags are capitalized variant types). The value part rides through
/// unchanged so the dispatcher can sub-Dispatch it before construction sees its resolved
/// value — the tag/value-type checks and the witnessed `KObject::Tagged` build live in
/// [`finish_witnessed`], which folds the value carrier's reach onto the result.
pub(in crate::machine::execute) fn prepare_args<'step>(
    args_parts: Vec<Spanned<ExpressionPart<'step>>>,
) -> Result<(String, ExpressionPart<'step>), KError> {
    if args_parts.len() != 2 {
        return Err(KError::new(KErrorKind::ArityMismatch {
            expected: 2,
            got: args_parts.len(),
        }));
    }
    let mut iter = args_parts.into_iter();
    let tag_part = iter.next().unwrap();
    let value_part = iter.next().unwrap();
    let tag = match tag_part.value {
        ExpressionPart::Type(t) => t.render(),
        other => {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "tagged-union construction = first arg must be a capitalized variant tag, got {}",
                other.summarize()
            ))));
        }
    };
    Ok((tag, value_part.value))
}

#[cfg(test)]
mod tests;

/// Construct a newtype value (record-repr or scalar). `value_parts` is the whole value
/// expression (`expr.parts[1..]`); a single redundant `(...)` paren group unwraps so
/// `(Distance 3.0)` / `Distance (3.0)` construct identically and `Distance ()` is arity-zero.
/// The parts are launched as one value cell whose finish type-checks against the member's
/// `repr` and wraps with `identity`.
pub(in crate::machine::execute) fn dispatch_construct_newtype<'step>(
    identity: &'step KType<'step>,
    reach: StoredReach<'step>,
    mut value_parts: Vec<Spanned<ExpressionPart<'step>>>,
) -> Outcome<'step> {
    if let [Spanned {
        value: ExpressionPart::Expression(inner),
        ..
    }] = value_parts.as_slice()
    {
        value_parts = inner.parts.clone();
    }
    if value_parts.is_empty() {
        return Outcome::Done(Err(KError::new(KErrorKind::ArityMismatch {
            expected: 1,
            got: 0,
        })));
    }
    // One value cell. A single part dispatches directly (a bare `(p)` reference resolves
    // in place when ready, the way tagged construction dispatches its lone value); a
    // multi-part value (`Bar (Foo 3.0)`) is wrapped so `launch` dispatches it as one unit.
    let value_cell = if value_parts.len() == 1 {
        value_parts.into_iter().next().expect("len == 1").value
    } else {
        ExpressionPart::Expression(Box::new(KExpression::new(value_parts)))
    };
    launch(vec![value_cell], CtorKind::NewType { identity, reach })
}

/// Direct-construct a record-repr newtype from a named record-literal body. Launches one
/// value cell per field — a literal field stages in place, so a record over literal fields
/// binds synchronously (the property the retired struct path relied on, and which a chained
/// `(Boxed (p))` depends on). The finish builds the `KObject::Record` and wraps it.
pub(in crate::machine::execute) fn dispatch_construct_record_newtype<'step>(
    identity: &'step KType<'step>,
    reach: StoredReach<'step>,
    record_fields: Vec<(String, ExpressionPart<'step>)>,
) -> Outcome<'step> {
    let field_names: Vec<String> = record_fields.iter().map(|(n, _)| n.clone()).collect();
    let value_parts: Vec<ExpressionPart<'step>> =
        record_fields.into_iter().map(|(_, p)| p).collect();
    launch(
        value_parts,
        CtorKind::RecordNewType {
            identity,
            field_names,
            reach,
        },
    )
}

/// Type-check `value` against the newtype member's projected `repr` and decide how the wrap folds
/// its payload. The check runs **before** the witness-closure build (read out of the value carrier),
/// so the build inside the brand is infallible. Returns whether to **collapse** one wrapper layer:
/// a transparent re-tag (`NEWTYPE Bar = Foo` over a `Foo` value — the payload's identity is exactly
/// this repr) collapses so identities never stack; a union-repr variant (`Succ :Nat`) wraps a
/// *member* of the union, whose identity differs from the union repr, so it preserves the nested
/// payload — the recursion the dissolved-union model needs.
fn check_newtype_repr<'a>(
    identity: &KType<'a>,
    value: &KObject<'a>,
    types: &TypeRegistry,
) -> Result<bool, KError> {
    let (set, index) = match identity {
        KType::SetRef { set, index } => (set, *index),
        _ => unreachable!("TypeCall fast lane routed a non-SetRef identity into newtype construct"),
    };
    let repr = match RecursiveSet::projected_schema(set, index) {
        ProjectedSchema::NewType(repr) => repr,
        _ => unreachable!("newtype construct ran on a non-NewType member"),
    };
    if !repr.matches_value(value, types) {
        return Err(KError::new(KErrorKind::TypeMismatch {
            arg: "value".to_string(),
            expected: repr.name(),
            got: value.ktype().name(),
        }));
    }
    let collapse = matches!(value, KObject::Wrapped { .. }) && repr == value.ktype();
    Ok(collapse)
}

/// Construct an identity-wrapper value over a `NEWTYPE (Type AS Wrapper)`-declared constructor
/// family. Mirrors [`dispatch_construct_newtype`]'s body shape: a single redundant `(...)` paren
/// group unwraps so `(Wrapper 3.0)` / `Wrapper (3.0)` construct identically and `Wrapper ()` is
/// arity-zero; a single part dispatches directly, a multi-part value wraps as one value cell. The
/// finish ([`finish_witnessed`]'s `ApplyConstructor` arm) stamps the value's type as the applied
/// arg and wraps it with a `ConstructorApply(<ctor SetRef>, [arg])` identity.
pub(in crate::machine::execute) fn dispatch_construct_apply<'step>(
    set: Rc<RecursiveSet<'step>>,
    index: usize,
    reach: StoredReach<'step>,
    mut value_parts: Vec<Spanned<ExpressionPart<'step>>>,
) -> Outcome<'step> {
    if let [Spanned {
        value: ExpressionPart::Expression(inner),
        ..
    }] = value_parts.as_slice()
    {
        value_parts = inner.parts.clone();
    }
    if value_parts.is_empty() {
        return Outcome::Done(Err(KError::new(KErrorKind::ArityMismatch {
            expected: 1,
            got: 0,
        })));
    }
    let value_cell = if value_parts.len() == 1 {
        value_parts.into_iter().next().expect("len == 1").value
    } else {
        ExpressionPart::Expression(Box::new(KExpression::new(value_parts)))
    };
    launch(
        vec![value_cell],
        CtorKind::ApplyConstructor { set, index, reach },
    )
}

/// Direct-construct a tagged-union value from the projected schema of its sealed
/// `RecursiveSet` member. Shared by named UNIONs (`Tagged` kind) and the builtin `Result`
/// constructor (`TypeConstructor` kind) — both reference a sealed member.
#[allow(clippy::too_many_arguments)]
pub(in crate::machine::execute) fn dispatch_construct_tagged<'step>(
    set: Rc<RecursiveSet<'step>>,
    index: usize,
    schema: Rc<HashMap<String, KType<'step>>>,
    reach: StoredReach<'step>,
    args_parts: Vec<Spanned<ExpressionPart<'step>>>,
) -> Outcome<'step> {
    let (tag, value_part) = match prepare_args(args_parts) {
        Ok(v) => v,
        Err(e) => return Outcome::Done(Err(e)),
    };
    launch(
        vec![value_part],
        CtorKind::Tagged {
            schema,
            set,
            index,
            tag,
            reach,
        },
    )
}

/// Decide a constructor park: every value part is a fresh sub-Dispatch dep (a single-part
/// `Expression` wrapping routes through normal classification), and a freshly-minted sub is never
/// terminal in the same step (submission is enqueue-then-drain), so there is no inline-ready case —
/// the slot always parks as a [`Outcome::ParkThenContinue`]. The finish folds the resolved value
/// carriers into the wrapped value **inside the witness closure** ([`finish_witnessed`]) so it names
/// every region it reaches; dep errors propagate frameless.
fn launch<'step>(value_parts: Vec<ExpressionPart<'step>>, kind: CtorKind<'step>) -> Outcome<'step> {
    debug_assert!(
        !value_parts.is_empty(),
        "launch requires at least one value part (arity-zero is rejected upstream)"
    );
    let deps: Vec<DepRequest<'step>> = value_parts
        .into_iter()
        .map(|part| DepRequest::Dispatch {
            expr: KExpression::new(vec![Spanned::bare(part)]),
            placement: DepPlacement::OwnScope,
        })
        .collect();
    let combine_finish: WitnessedDepFinish<'step> = Box::new(move |view, terminals| {
        finish_witnessed(view, &kind, terminals).map(StepCarried::born)
    });
    Await::on(Deps::from_owned(deps)).finish_witnessed(combine_finish)
}

/// Build the construction operand carrying `(dest brand, nominal identity)` across the build brand.
/// `dest_frame`'s brand is `yoke`d into that frame's own region — witnessed by it — and `merge`d with
/// the identity wrapped by [`Scope::resident_type_carrier`] under its `stored` per-binding token, so
/// the operand's witness is the dest region's pin ∪ the identity's own reach — folded, never paired
/// with an asserted witness. The token's foreign reach is empty while `RecursiveSet` is heap-`Rc`'d
/// (the identity points into no region) and names the set's region once it is region-allocated; the
/// token's home-borrow bit is replayed from its derivation, never asserted here.
pub(crate) fn build_type_operand<'step>(
    scope: &'step Scope<'step>,
    dest_frame: Rc<FrameStorage>,
    identity: &'step KType<'step>,
    stored: StoredReach<'step>,
) -> Witnessed<RegionTypeFamily, CarrierWitness> {
    let dest_brand = dest_brand(Rc::clone(&dest_frame));
    let identity_carrier = scope.resident_type_carrier(identity, stored);
    // The dest brand is the *destination* operand (its `DestHandleFamily` live form is the
    // `HasRegionHandle` mint target the pinned merge's composition seam needs), so it rides as
    // `other` — `identity_carrier`'s own reach is what gets minted into the dest frame's arena.
    // The pin: the identity's home region owner when live (the identity and its reach set live
    // there), else the empty set — the identity is then covered by the live `scope` borrow itself.
    let pin: FrameSet = scope
        .region_owner()
        .upgrade()
        .map_or_else(FrameSet::empty, FrameSet::singleton);
    identity_carrier.merge_pinned::<DestHandleFamily, RegionTypeFamily, _>(
        dest_brand,
        &pin,
        |carried, brand, _b| {
            let kt = match carried {
                Carried::Type(t) => t,
                _ => unreachable!("the identity carrier is always a Type"),
            };
            (brand, kt)
        },
    )
}

/// Seal a declaration's nominal identity as a `Carried::Type` terminal, folding each carrier dep's
/// reach — and its residence host, at [`Residence::Kept`] — onto the placement's witness. The
/// finalize-path twin of [`finish_witnessed`]'s construction fold: the identity crosses the build
/// brand as a [`RegionTypeFamily`] operand ([`build_type_operand`]) rather than captured into a fold
/// closure, so the placement's witness folds the identity's own reach as a declared operand. Reach =
/// the dest region ∪ the identity's own reach ∪ every carrier's reach-and-host — the coverage
/// [`alloc_carried_with`](crate::machine::core::StepAllocator::alloc_carried_with) produces for
/// the same `(identity, carriers)`, with the identity operand-crossed rather than closure-captured.
///
/// `stored` is the identity's own token (its home-omitted foreign reach is empty while a
/// `RecursiveSet` is heap-`Rc`'d and its members reach no region); `dest_frame` owns the destination
/// scope's region and pins the operand's backing for the synchronous re-anchor.
pub(crate) fn seal_type_operand<'a>(
    scope: &'a Scope<'a>,
    dest_frame: Rc<FrameStorage>,
    identity: &'a KType<'a>,
    stored: StoredReach<'a>,
    carriers: &[&DeliveredCarried],
) -> StepCarried<'a> {
    let operand = build_type_operand(scope, Rc::clone(&dest_frame), identity, stored);
    // Fold each carrier's reach onto the identity operand at `Residence::Kept` — the dep keeps living
    // in its producer region, so its host materializes as a member — the per-dep envelope fold
    // `alloc_carried_with` runs. The relocate closure discards the carrier's value and threads the
    // identity operand through unchanged; only its reach folds onto the composed witness.
    let operand = carriers.iter().fold(operand, |operand, carrier| {
        carrier.transfer_into::<RegionTypeFamily, RegionTypeFamily, KoanStorageProfile>(
            operand,
            Residence::Kept,
            |_value, operand, _brand| operand,
        )
    });
    // Drop the destination handle and re-seal the identity as a `Carried::Type` under the composed
    // witness; `dest_frame` pins the operand's backing for the transient re-anchor.
    StepCarried::born(
        operand.map_pinned::<CarriedFamily, _>(&dest_frame, |(_region, identity_ty), _brand| {
            Carried::Type(identity_ty)
        }),
    )
}

/// All value subs have resolved. Build the wrapped value **inside the witness closure**, folding the
/// value carriers' reach onto the result so the constructed object names every region it reaches by
/// construction. The nominal type identity crosses the brand as a non-object operand
/// ([`RegionTypeFamily`]), `merge`d in via [`build_type_operand`] so it rides the brand witnessed by
/// its own reach rather than an asserted co-location. Type-checks run before the build (read out of
/// the carrier), so the closure is infallible.
fn finish_witnessed<'step>(
    view: &SchedulerView<'step, '_>,
    kind: &CtorKind<'step>,
    terminals: DepResults<'_, &DepTerminal<'step>>,
) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
    // A constructor parks on its value subs only (all owned, no park producers), so its results are
    // exactly the owned suffix — read them as one slice.
    let terminals = terminals.owned_slice();
    let scope = view.current_scope();
    let region = scope.brand();
    match kind {
        CtorKind::NewType { identity, reach } => {
            debug_assert_eq!(terminals.len(), 1);
            let collapse = check_newtype_repr(identity, terminals[0].value.object(), view.types())?;
            let home = build_type_operand(scope, view.dest_frame(), identity, *reach);
            Ok(terminals[0]
                .delivered
                .transfer_into_placing::<RegionTypeFamily, CarriedFamily, _>(
                    home,
                    Residence::Copied,
                    move |value, (_region, identity_ty), placement| {
                        let region = FoldingBrand::in_fold_closure(placement);
                        let inner = if collapse {
                            WrappedPayload::peel(value.object())
                        } else {
                            WrappedPayload::hold(value.object())
                        };
                        Carried::Object(region.alloc_object_folded(KObject::Wrapped {
                            inner,
                            type_id: identity_ty,
                        }))
                    },
                ))
        }
        CtorKind::RecordNewType {
            identity,
            field_names,
            reach,
        } => {
            // Check the assembled record against the newtype repr first (read out of the carriers),
            // then fold the field carriers into the witnessed record and wrap it.
            let probe = Record::from_pairs(
                field_names
                    .iter()
                    .cloned()
                    .zip(terminals.iter().map(|t| t.value.object().deep_clone())),
            );
            // A record probe is never a `Wrapped`, so the repr check never asks to collapse.
            check_newtype_repr(identity, &KObject::record(probe), view.types())?;
            // The fold accumulator is yoked into the dest frame's own region up front (mirroring
            // `dispatch::literal`'s `AggBuildFamily`), so each field's `transfer_into` composes by
            // minting that field's reach into the accumulator's own arena rather than by plain union.
            let acc0 =
                KoanRegion::yoke_branded::<RecordFieldsFamily, _>(view.dest_frame(), |region| {
                    (region.handle(), Vec::with_capacity(field_names.len()))
                });
            let fields = terminals
                .iter()
                .zip(field_names)
                .fold(acc0, |acc, (term, name)| {
                    let name = name.clone();
                    term.delivered
                        .transfer_into::<RecordFieldsFamily, RecordFieldsFamily, _>(
                            acc,
                            Residence::Copied,
                            move |value, (region, mut fields), _brand| {
                                fields.push((name, value.object().deep_clone()));
                                (region, fields)
                            },
                        )
                });
            let home = build_type_operand(scope, view.dest_frame(), identity, *reach);
            // The pin: the destination frame, whose arena holds the sets the field folds minted.
            let dest_frame = view.dest_frame();
            Ok(fields
                .merge_pinned_placing::<RegionTypeFamily, CarriedFamily, KoanStorageProfile, _>(
                    home,
                    &dest_frame,
                    |(_region, fields), (_identity_region, identity_ty), placement| {
                        let region = FoldingBrand::in_fold_closure(placement);
                        let record = Record::from_pairs(fields);
                        Carried::Object(region.alloc_object_folded(KObject::Wrapped {
                            inner: WrappedPayload::hold(&KObject::record(record)),
                            type_id: identity_ty,
                        }))
                    },
                ))
        }
        CtorKind::Tagged {
            schema,
            set,
            index,
            tag,
            reach,
        } => {
            debug_assert_eq!(terminals.len(), 1);
            let expected = schema.get(tag).ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "tag `{}` not in union (known: {})",
                    tag,
                    schema.keys().cloned().collect::<Vec<_>>().join(", ")
                )))
            })?;
            if !expected.matches_value(terminals[0].value.object(), view.types()) {
                return Err(KError::new(KErrorKind::TypeMismatch {
                    arg: "value".to_string(),
                    expected: expected.name().to_string(),
                    got: terminals[0].value.object().ktype().name().to_string(),
                }));
            }
            // The tag's `SetRef` identity crosses the brand as a `&KType` so the built `Tagged` names
            // its set/index at the brand. Freshly minted in the dest region, so `reach` is empty
            // today; the operand `merge`s it under the dest frame's yoke plus that reach.
            let identity: &KType<'step> = region.alloc_ktype_checked(KType::SetRef {
                set: Rc::clone(set),
                index: *index,
            })?;
            let home = build_type_operand(scope, view.dest_frame(), identity, *reach);
            let tag = tag.clone();
            Ok(terminals[0]
                .delivered
                .transfer_into_placing::<RegionTypeFamily, CarriedFamily, _>(
                    home,
                    Residence::Copied,
                    move |value, (_region, identity_ty), placement| {
                        let region = FoldingBrand::in_fold_closure(placement);
                        let (set, index) = match identity_ty {
                            KType::SetRef { set, index } => (Rc::clone(set), *index),
                            _ => unreachable!("a Tagged identity is always a SetRef"),
                        };
                        Carried::Object(region.alloc_object_folded(KObject::Tagged {
                            tag,
                            value: Rc::new(value.object().deep_clone()),
                            set,
                            index,
                            type_args: Rc::new(vec![]),
                        }))
                    },
                ))
        }
        CtorKind::ApplyConstructor { set, index, reach } => {
            debug_assert_eq!(terminals.len(), 1);
            // The ctor `SetRef` identity crosses the brand as a `&KType` so the built value's
            // `ConstructorApply` type id names the ctor's set/index at the brand. Freshly minted
            // in the dest region, so `reach` is empty today; the operand `merge`s it under the
            // dest frame's yoke plus that reach.
            let identity: &KType<'step> = region.alloc_ktype_checked(KType::SetRef {
                set: Rc::clone(set),
                index: *index,
            })?;
            let home = build_type_operand(scope, view.dest_frame(), identity, *reach);
            Ok(terminals[0]
                .delivered
                .transfer_into_placing::<RegionTypeFamily, CarriedFamily, _>(
                    home,
                    Residence::Copied,
                    move |value, (_region, identity_ty), placement| {
                        let region = FoldingBrand::in_fold_closure(placement);
                        // Stamp the value's FULL type — including a `Wrapped` payload's own
                        // nominal identity — as the sole applied arg before collapsing.
                        let arg = value.object().ktype();
                        // Collapse: peel any single `Wrapped` layer so `Wrapped.inner` is never
                        // itself `Wrapped` (the single-layer invariant); the peeled identity is
                        // not lost — it lives in `arg`.
                        let inner = if matches!(value.object(), KObject::Wrapped { .. }) {
                            WrappedPayload::peel(value.object())
                        } else {
                            WrappedPayload::hold(value.object())
                        };
                        // The type id is a fresh `ConstructorApply(<ctor SetRef>, [arg])`, allocated
                        // at the fold brand — `arg` may borrow the value's region, folded in here.
                        let type_id = region.alloc_ktype_folded(KType::constructor_apply(
                            Box::new(identity_ty.clone()),
                            vec![arg],
                        ));
                        Carried::Object(
                            region.alloc_object_folded(KObject::Wrapped { inner, type_id }),
                        )
                    },
                ))
        }
    }
}
