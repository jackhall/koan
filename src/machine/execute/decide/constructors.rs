//! NewType + tagged-union construction dispatch. Args resolve through one sub-Dispatch per value
//! cell; once every cell is bound, [`finish_witnessed`] type-checks them and emits the
//! `KObject::Wrapped` / `KObject::Tagged` directly — no bucket lookup, no re-dispatch.

use std::collections::HashMap;
use std::rc::Rc;

use smallvec::SmallVec;

use crate::machine::core::DepPlacement;
use crate::machine::core::{
    FoldingBrand, FrameStorage, KoanRegionExt, KoanStorageProfile, RegionBrand, Scope,
};
use crate::machine::model::CarriedFamily;
use crate::machine::model::types::record_field;
use crate::machine::model::{Carried, KObject, Record};
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::machine::model::{KType, NodeSchema, TypeNode};
use crate::machine::{
    CarrierWitness, DeliveredCarried, KError, KErrorKind, KoanRegion, RegionTypeFamily,
};
use crate::source::Spanned;
use crate::witnessed::{
    BumpAllocator, BumpVec, Delivered, RegionHandle, RegionHandleFamily, reattachable,
};

use super::super::outcome::DepTerminal;
use super::super::{StepCarried, WitnessedDepFinish};
use super::ctx::DecideCtx;
use super::{Await, DepRequest, Outcome};
use crate::machine::model::RunRegistries;
use crate::machine::model::Symbol;
use crate::scheduler::Deps;

/// Which construction shape the resolved value subs feed. The carried `KType` is the sealed
/// member's own handle, stamped onto the produced `KObject` as its identity.
pub(in crate::machine::execute) enum CtorKind {
    /// NewType construction (record-repr or scalar) from a single positional value, checked
    /// against the member's `repr`.
    NewType { identity: KType },
    /// Record-repr newtype construction from a named record-literal body (`Point {x = 1, y =
    /// 2}`), with one value cell per field.
    RecordNewType {
        identity: KType,
        field_names: Vec<Symbol>,
    },
    Tagged {
        schema: Rc<HashMap<String, KType>>,
        member: KType,
        tag: String,
    },
    /// Identity-wrapper construction over a `NEWTYPE (Type AS Wrapper)`-declared constructor
    /// family (empty-schema `TypeConstructor` member). The value's own full type becomes the sole
    /// applied arg, so the built value inhabits `:(<value's type> AS Wrapper)`.
    ApplyConstructor { constructor: KType },
}

/// Relocation product for a record-repr newtype: the destination region plus the field values
/// gathered from the value deps, relocated as one run so the product's witness composes by minting
/// into that region (the [`HasRegionHandle`](crate::witnessed::HasRegionHandle) seam).
/// Layout-invariant, and bumped exactly once: the run rides the Copy tier, whose dormant slot runs
/// no drop glue, so an owned buffer could not live there.
///
/// Only the *values* ride the carrier. The field **names** are owned data with no region lifetime,
/// so they stay beside the relocation and pair back with the relocated values at the final merge.
struct RecordFieldsFamily;
reattachable!(RecordFieldsFamily => (RegionHandle<'r, KoanStorageProfile>, &'r [KObject<'r>]));

/// The per-source cell family the field relocation hands back: one rebuilt field value per term,
/// each paired with its own source envelope to derive that field's retention claim.
/// Layout-invariant in `'r`: [`KObject`] is one type up to its lifetime.
struct KObjectFamily;
reattachable!(KObjectFamily => KObject<'r>);

/// Validate a tagged-union call site's args shape: exactly two parts, the first a `Type`-token tag
/// (tags are capitalized variant types). The value part rides through unchanged — the tag/value
/// type checks and the witnessed build wait for its resolved value in [`finish_witnessed`].
pub(in crate::machine::execute) fn prepare_args<'step>(
    args_parts: &[Spanned<ExpressionPart<'step>>],
) -> Result<(String, ExpressionPart<'step>), KError> {
    let [tag_part, value_part] = args_parts else {
        return Err(KError::new(KErrorKind::ArityMismatch {
            expected: 2,
            got: args_parts.len(),
        }));
    };
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

/// One dep's worth of construction value, in whichever form it already has: a parsed part straight
/// out of the body, or a synthesized node for a multi-part value — so the multi-part case never has
/// to mint an AST arm.
enum ValueCell<'step> {
    /// A part lifted verbatim out of the construction body.
    Part(ExpressionPart<'step>),
    /// A multi-part value the dispatcher grouped itself, already in working form.
    Synthesized(WorkingExpression<'step>),
}

/// Paren-unwrap a construction's value parts to a single value cell: a redundant `(...)` wrapper
/// group unwraps first, so `(Distance 3.0)` / `Distance (3.0)` construct identically and
/// `Distance ()` is arity-zero (rejected here).
///
/// A multi-part value (`Bar (Foo 3.0)`) groups in **working** form rather than under an AST
/// `Expression` arm: the group exists only to be dispatched and is minted per call, so it never has
/// to claim the eternal-tier residence an AST arm's payload does.
fn single_value_cell<'step>(
    brand: RegionBrand<'step>,
    mut value_parts: &[Spanned<ExpressionPart<'step>>],
) -> Result<ValueCell<'step>, KError> {
    if let [
        Spanned {
            value: ExpressionPart::Expression(inner),
            ..
        },
    ] = value_parts
    {
        value_parts = inner.parts;
    }
    match value_parts {
        [] => Err(KError::new(KErrorKind::ArityMismatch {
            expected: 1,
            got: 0,
        })),
        [only] => Ok(ValueCell::Part(only.value)),
        many => Ok(ValueCell::Synthesized(WorkingExpression::new_from_iter(
            brand,
            many.iter().map(|part| Spanned {
                value: WorkingPart::Ast(part.value),
                span: part.span,
            }),
        ))),
    }
}

/// The construction body's parts as parser parts. Construction reaches its value expression through
/// `expr.parts[1..]`, which is raw syntax on every lane that gets here — a resolved head never
/// splices into the trailing slots.
fn value_parts_of<'step>(
    parts: &[Spanned<WorkingPart<'step>>],
) -> Vec<Spanned<ExpressionPart<'step>>> {
    parts
        .iter()
        .map(|part| Spanned {
            value: part
                .value
                .as_ast()
                .expect("a construction body is parsed syntax"),
            span: part.span,
        })
        .collect()
}

/// Construct a newtype value (record-repr or scalar). `value_parts` is the whole value expression
/// (`expr.parts[1..]`), collapsed to one value cell by [`single_value_cell`].
pub(in crate::machine::execute) fn dispatch_construct_newtype<'step>(
    brand: RegionBrand<'step>,
    identity: KType,
    value_parts: &[Spanned<WorkingPart<'step>>],
    scratch: BumpAllocator<'step>,
) -> Outcome<'step> {
    let value_cell = match single_value_cell(brand, &value_parts_of(value_parts)) {
        Ok(cell) => cell,
        Err(e) => return Outcome::Done(Err(e)),
    };
    launch(
        brand,
        vec![value_cell],
        CtorKind::NewType { identity },
        scratch,
    )
}

/// Direct-construct a record-repr newtype from a named record-literal body, one value cell per
/// field — so a field resolves on its own rather than through a dispatch of the whole literal.
pub(in crate::machine::execute) fn dispatch_construct_record_newtype<'step>(
    brand: RegionBrand<'step>,
    identity: KType,
    record_fields: &[(&'step str, ExpressionPart<'step>)],
    registries: &RunRegistries,
    scratch: BumpAllocator<'step>,
) -> Outcome<'step> {
    // The literal's field labels are syntactic, so they intern here — once per construction site.
    let field_names: Vec<Symbol> = record_fields
        .iter()
        .map(|(name, _)| registries.labels.intern(name))
        .collect();
    let value_parts: Vec<ValueCell<'step>> = record_fields
        .iter()
        .map(|(_, p)| ValueCell::Part(*p))
        .collect();
    launch(
        brand,
        value_parts,
        CtorKind::RecordNewType {
            identity,
            field_names,
        },
        scratch,
    )
}

/// Type-check `value` against the newtype member's `repr`. The check runs **before** the
/// witness-closure build, so the build inside the brand is infallible. Returns whether to
/// **collapse** one wrapper layer: a transparent re-tag (`NEWTYPE Bar = Foo` over a `Foo` value —
/// the payload's identity is exactly this repr) collapses so identities never stack; a
/// self-recursive newtype (`NEWTYPE List = :{head :Number, tail :List}`) wraps a payload whose
/// identity differs from the repr, preserving the nested value a linked structure needs.
fn check_newtype_repr<'a>(
    identity: KType,
    value: &KObject<'a>,
    registries: &RunRegistries,
) -> Result<bool, KError> {
    let types = &registries.types;
    // A sealed member's schema is already absolute — every sibling reference in it is the
    // sibling's own handle — so the repr is a direct node read.
    let repr = match types.node(identity) {
        TypeNode::SetMember {
            schema: NodeSchema::NewType(repr),
            ..
        } => repr,
        _ => unreachable!("newtype construct ran on a non-NewType member"),
    };
    if !repr.matches_value(value, registries) {
        return Err(KError::new(KErrorKind::TypeMismatch {
            arg: "value".to_string(),
            expected: repr.name(registries),
            got: value.ktype().name(registries),
        }));
    }
    let collapse = matches!(value, KObject::Wrapped { .. }) && repr == value.ktype();
    Ok(collapse)
}

/// Record-shaped twin of [`check_newtype_repr`] for [`CtorKind::RecordNewType`]: checks the
/// assembled field values against the declared record repr field by field, never building a probe
/// `KObject::Record` to run [`KType::matches_value`] against — a record's substrate is born only
/// through the fold door, and this check runs before any brand is in hand. A record-repr newtype's
/// collapse question never arises (a bare field record is never itself a `Wrapped`).
fn check_record_newtype_repr(
    identity: KType,
    fields: &[(Symbol, KObject<'_>)],
    registries: &RunRegistries,
) -> Result<(), KError> {
    let types = &registries.types;
    let repr = match types.node(identity) {
        TypeNode::SetMember {
            schema: NodeSchema::NewType(repr),
            ..
        } => repr,
        _ => unreachable!("newtype construct ran on a non-NewType member"),
    };
    let matches = match types.node(repr) {
        TypeNode::Record {
            fields: repr_fields,
        } => repr_fields.iter().all(|(name, field_type)| {
            record_field(fields, name)
                .map(|v| field_type.matches_value(v, registries))
                .unwrap_or(false)
        }),
        _ => false,
    };
    if !matches {
        return Err(KError::new(KErrorKind::TypeMismatch {
            arg: "value".to_string(),
            expected: repr.name(registries),
            got: types
                .record(Record::from_pairs(
                    fields.iter().map(|(name, value)| (*name, value.ktype())),
                ))
                .name(registries),
        }));
    }
    Ok(())
}

/// Construct an identity-wrapper value over a `NEWTYPE (Type AS Wrapper)`-declared constructor
/// family. `value_parts` collapses to one value cell via [`single_value_cell`], the same shape
/// [`dispatch_construct_newtype`] uses.
pub(in crate::machine::execute) fn dispatch_construct_apply<'step>(
    brand: RegionBrand<'step>,
    constructor: KType,
    value_parts: &[Spanned<ExpressionPart<'step>>],
    scratch: BumpAllocator<'step>,
) -> Outcome<'step> {
    let value_cell = match single_value_cell(brand, value_parts) {
        Ok(cell) => cell,
        Err(e) => return Outcome::Done(Err(e)),
    };
    launch(
        brand,
        vec![value_cell],
        CtorKind::ApplyConstructor { constructor },
        scratch,
    )
}

/// Direct-construct a tagged-union value from the variant schema of its sealed member. Shared by
/// named UNIONs (`Tagged` kind) and the builtin `Result` constructor (`TypeConstructor` kind) —
/// both name a sealed member by its handle.
pub(in crate::machine::execute) fn dispatch_construct_tagged<'step>(
    brand: RegionBrand<'step>,
    member: KType,
    schema: Rc<HashMap<String, KType>>,
    args_parts: &[Spanned<ExpressionPart<'step>>],
    scratch: BumpAllocator<'step>,
) -> Outcome<'step> {
    let (tag, value_part) = match prepare_args(args_parts) {
        Ok(v) => v,
        Err(e) => return Outcome::Done(Err(e)),
    };
    construct_tagged(brand, member, schema, tag, value_part, scratch)
}

/// Construct a tagged value from an already-split `(tag, value)` pair. The finish type-checks the
/// value against `schema[tag]` and builds `KObject::Tagged { tag, value, identity: member }`.
pub(in crate::machine::execute) fn construct_tagged<'step>(
    brand: RegionBrand<'step>,
    member: KType,
    schema: Rc<HashMap<String, KType>>,
    tag: String,
    value_part: ExpressionPart<'step>,
    scratch: BumpAllocator<'step>,
) -> Outcome<'step> {
    launch(
        brand,
        vec![ValueCell::Part(value_part)],
        CtorKind::Tagged {
            schema,
            member,
            tag,
        },
        scratch,
    )
}

/// Decide a constructor park. A freshly-minted sub is never terminal in the same step (submission
/// is enqueue-then-drain), so there is no inline-ready case — the slot always parks as an
/// [`Outcome::Park`]. Dep errors propagate frameless.
fn launch<'step>(
    brand: RegionBrand<'step>,
    value_parts: Vec<ValueCell<'step>>,
    kind: CtorKind,
    scratch: BumpAllocator<'step>,
) -> Outcome<'step> {
    debug_assert!(
        !value_parts.is_empty(),
        "launch requires at least one value part (arity-zero is rejected upstream)"
    );
    let deps: Vec<DepRequest<'step>> = value_parts
        .into_iter()
        .map(|cell| DepRequest::Dispatch {
            expr: match cell {
                ValueCell::Part(part) => {
                    WorkingExpression::new(brand, &[Spanned::bare(WorkingPart::Ast(part))])
                }
                ValueCell::Synthesized(expr) => expr,
            },
            placement: DepPlacement::OwnScope,
        })
        .collect();
    let combine_finish: WitnessedDepFinish<'step> = Box::new(move |view, terminals| {
        finish_witnessed(view, &kind, terminals).map(StepCarried::born_delivered)
    });
    Await::on(Deps::from_requests_in(deps, scratch)).finish_witnessed(combine_finish)
}

/// Build the construction operand carrying `(dest brand, nominal identity)` across the build brand.
/// `KType` is a bare interned handle that points into no region, so it is region-pure data the yoke
/// may carry in: nothing is composed, and the operand's reach is exactly the dest region's own,
/// born co-located rather than paired with an asserted witness — [`Delivered::destination`]'s shape
/// exactly, homed in `dest_frame` and covering nothing beyond it.
pub(crate) fn build_type_operand(
    dest_frame: Rc<FrameStorage>,
    identity: KType,
) -> Delivered<RegionTypeFamily, CarrierWitness, FrameStorage> {
    KoanRegion::yoke_branded::<RegionTypeFamily, _>(dest_frame, |b| (b.handle(), identity))
}

/// Seal a declaration's nominal identity as a `Carried::Type` terminal. A `KType` is a `Copy`
/// handle, so the identity reaches no region and the carrier seals under a member-less description
/// hosted in `scope`'s own region — the read travels under the home-frame pin alone.
pub(crate) fn seal_type_identity<'a>(scope: &'a Scope<'a>, identity: KType) -> StepCarried<'a> {
    StepCarried::born(scope.resident(Carried::Type(identity)))
}

/// All value subs have resolved. The wrapped value is built **inside the witness closure**, folding
/// the value carriers' reach onto the result so the constructed object names every region it
/// reaches by construction. The nominal type identity crosses the brand as a non-object operand
/// ([`RegionTypeFamily`]) via [`build_type_operand`], so it rides the brand witnessed by its own
/// reach rather than an asserted co-location. Type-checks run before the build, so the closure is
/// infallible.
fn finish_witnessed<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    kind: &CtorKind,
    terminals: &[DepTerminal<'_>],
) -> Result<DeliveredCarried, KError> {
    match kind {
        CtorKind::NewType { identity } => {
            debug_assert_eq!(terminals.len(), 1);
            // The repr check reads the payload at the term's resident brand — a pin-free read, since
            // the brand is the proof; only the `collapse` verdict escapes the guard's borrow.
            let collapse = {
                let opened = terminals[0].cell.open_at();
                check_newtype_repr(*identity, opened.value().object(), view.registries())?
            };
            let home = build_type_operand(view.dest_frame(), *identity);
            // The wrap keeps the value verbatim, so a payload substrate that stays foreign rides as
            // the payload cell's own stored run; the term's coverage is the holder-rule proof for
            // reading it. `transfer_into` and `.coverage()` need an owned envelope, so the term's
            // resident cell is lifted back to one first.
            let delivered = view.current_scope().lift_spliced(&terminals[0].cell);
            let holder = delivered.coverage().clone();
            // The type operand is empty-reach, so the transfer composes the value's reach alone and
            // hands back the wrapped product as an envelope homed in the dest frame.
            Ok(
                delivered.transfer_into::<RegionTypeFamily, CarriedFamily, _>(
                    home,
                    // The wrap holds the value's borrow verbatim, so nothing is released.
                    |_product, _region| true,
                    move |value, (_region, identity_ty), placement| {
                        let region = FoldingBrand::in_fold_closure(placement);
                        let door = region.with_holder(&holder);
                        let wrapped = if collapse {
                            KObject::wrapped_peel(door, value.object(), identity_ty)
                        } else {
                            KObject::wrapped_hold(door, value.object(), identity_ty)
                        };
                        Carried::Object(region.alloc_object_folded(wrapped))
                    },
                ),
            )
        }
        CtorKind::RecordNewType {
            identity,
            field_names,
        } => {
            // Each field value is read at its own resident brand — a pin-free read; the guards stay
            // bound across the probe build, and the deep clone is owned data that outlives them.
            let opened: Vec<_> = terminals.iter().map(|t| t.cell.open_at()).collect();
            let probe: Vec<(Symbol, KObject<'_>)> = field_names
                .iter()
                .copied()
                .zip(opened.iter().map(|o| o.value().object().deep_clone()))
                .collect();
            check_record_newtype_repr(*identity, &probe, view.registries())?;
            drop(opened);
            // The whole field run relocates in one act against a bare destination handle over the
            // dest frame's region (mirroring `literal`'s `AggBuildFamily`), so every field's reach
            // is minted into that region's own arena rather than unioned by hand, and the fields
            // are bumped once.
            let dest_frame = view.dest_frame();
            // The relocation takes owned envelopes, so each term's resident cell is lifted back to
            // one first.
            let lifted: Vec<DeliveredCarried> = terminals
                .iter()
                .map(|term| view.current_scope().lift_spliced(&term.cell))
                .collect();
            let fields = DeliveredCarried::transfer_all_into::<
                RegionHandleFamily<KoanStorageProfile>,
                RecordFieldsFamily,
                KObjectFamily,
                KoanStorageProfile,
            >(
                &lifted,
                Delivered::destination(Rc::clone(&dest_frame)),
                // Each field cell is a pointer copy of its term's value, so it borrows everything
                // that term did.
                |_source, _cell, _region| true,
                |run, dest_handle, placement| {
                    let rebuilt: SmallVec<[KObject<'_>; 8]> = run
                        .iter()
                        .map(|value| value.object().deep_clone())
                        .collect();
                    // The one bump of the run. Staging order: `rebuilt[i]` is `lifted[i]`'s field
                    // value, so the same slice serves as the product's fields and as the
                    // per-source cells the door pairs back with the envelopes.
                    let slice = placement.allocator().slice(&rebuilt);
                    ((dest_handle, slice), slice)
                },
            );
            let home = build_type_operand(Rc::clone(&dest_frame), *identity);
            let types = view.types();
            // The value born at this door is the record, whose stored run spans every field, so
            // its holder is the union across the run — the relocated envelope's coverage. No
            // per-field narrowing applies: the relocation `deep_clone`s each field's top node
            // rather than rebuilding it through a door of its own.
            let holder = fields.coverage().clone();
            // The type operand is empty-reach; merge the relocated fields into it, yielding the
            // wrapped record homed in the dest frame.
            let product = fields.merge_into::<RegionTypeFamily, CarriedFamily, KoanStorageProfile>(
                home,
                |(_region, fields), (_identity_region, identity_ty), placement| {
                    let region = FoldingBrand::in_fold_closure(placement);
                    let door = region.with_holder(&holder);
                    // The names never rode the carrier — they pair back with the relocated
                    // values here, in the staging order the terminals were visited in.
                    let mut pairs: BumpVec<'_, (Symbol, KObject<'_>)> =
                        BumpVec::with_capacity_in(field_names.len(), door.allocator());
                    pairs.extend(field_names.iter().copied().zip(fields.iter().copied()));
                    let record = KObject::record(door, &pairs, types);
                    Carried::Object(region.alloc_object_folded(KObject::wrapped_hold(
                        door,
                        &record,
                        identity_ty,
                    )))
                },
            );
            // The merge minted the product's description into `dest_frame`'s own region, so that
            // region is the product's host and rides its members: the fresh record's substrate
            // borrows into the very region it was built in, and the relocated envelope's pins
            // named it.
            Ok(product)
        }
        CtorKind::Tagged {
            schema,
            member,
            tag,
        } => {
            debug_assert_eq!(terminals.len(), 1);
            let expected = schema.get(tag).ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "tag `{}` not in union (known: {})",
                    tag,
                    schema.keys().cloned().collect::<Vec<_>>().join(", ")
                )))
            })?;
            // The schema check reads the payload at the term's resident brand — a pin-free read;
            // only the verdict (and, on mismatch, an owned rendered name) escapes the guard's
            // borrow.
            {
                let opened = terminals[0].cell.open_at();
                if !expected.matches_value(opened.value().object(), view.registries()) {
                    return Err(KError::new(KErrorKind::TypeMismatch {
                        arg: "value".to_string(),
                        expected: expected.name(view.registries()).to_string(),
                        got: opened
                            .value()
                            .object()
                            .ktype()
                            .name(view.registries())
                            .to_string(),
                    }));
                }
            }
            let home = build_type_operand(view.dest_frame(), *member);
            let tag = tag.clone();
            // The tag keeps the value verbatim — see the `NewType` arm's holder.
            let delivered = view.current_scope().lift_spliced(&terminals[0].cell);
            let holder = delivered.coverage().clone();
            Ok(
                delivered.transfer_into::<RegionTypeFamily, CarriedFamily, _>(
                    home,
                    // The tag holds the value's borrow verbatim, so nothing is released.
                    |_product, _region| true,
                    move |value, (_region, identity_ty), placement| {
                        let region = FoldingBrand::in_fold_closure(placement);
                        Carried::Object(region.alloc_object_folded(KObject::tagged(
                            region.with_holder(&holder),
                            &tag,
                            value.object(),
                            identity_ty,
                        )))
                    },
                ),
            )
        }
        CtorKind::ApplyConstructor { constructor } => {
            debug_assert_eq!(terminals.len(), 1);
            let identity: KType = *constructor;
            // An identity wrapper takes exactly one type parameter; its name keys the applied
            // arg in the built `ConstructorApply`.
            let param_name = match view.types().node(*constructor) {
                TypeNode::SetMember {
                    schema: NodeSchema::TypeConstructor { param_names, .. },
                    ..
                } => param_names
                    .first()
                    .cloned()
                    .expect("an identity-wrapper family declares one type parameter"),
                _ => unreachable!("a ConstructorApply ctor is a TypeConstructor-kind member"),
            };
            // The parameter name interned where the constructor was declared, so the applied
            // argument's label is the symbol it already carries.
            let param_symbol = param_name.symbol();
            let home = build_type_operand(view.dest_frame(), identity);
            let types = view.types();
            // The wrap keeps the value verbatim — see the `NewType` arm's holder.
            let delivered = view.current_scope().lift_spliced(&terminals[0].cell);
            let holder = delivered.coverage().clone();
            Ok(
                delivered.transfer_into::<RegionTypeFamily, CarriedFamily, _>(
                    home,
                    // The wrap holds the value's borrow verbatim, so nothing is released.
                    |_product, _region| true,
                    move |value, (_region, identity_ty), placement| {
                        let region = FoldingBrand::in_fold_closure(placement);
                        // Stamp the value's FULL type — including a `Wrapped` payload's own
                        // nominal identity — as the sole applied arg before collapsing.
                        let arg = value.object().ktype();
                        let type_id = types.constructor_apply(
                            identity_ty,
                            Record::from_pairs([(param_symbol, arg)]),
                        );
                        // `wrapped_peel` keeps the single-layer invariant (`Wrapped.inner` is never
                        // itself `Wrapped`); the peeled identity is not lost — it lives in `arg`.
                        Carried::Object(region.alloc_object_folded(KObject::wrapped_peel(
                            region.with_holder(&holder),
                            value.object(),
                            type_id,
                        )))
                    },
                ),
            )
        }
    }
}
