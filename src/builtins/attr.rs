//! `ATTR <s> <field:Identifier>` — newtype (record-repr or scalar), module, or signature
//! field access. Surface syntax is the `.` infix operator. Overloads share the bucket
//! `[Keyword, Slot, Slot]` and pick by lhs shape: [`body_identifier`] for `p.x` where
//! the lhs is still an `Identifier`, [`body_newtype`] for a `Wrapped` lhs (a record-repr
//! newtype's `.x` reads through to the wrapped record), [`body_module`] for chained module
//! access.
//!
//! The lhs is matched by *type*, never by a kind: a module value picks `body_module` through the
//! empty-signature slot every module's self-sig satisfies, a type-token lhs picks `body_type_lhs`
//! through its `OfKind` kind, and any other value-channel lhs is caught by the least-specific
//! `s: Any` slot and validated in [`access_field`]. Specificity (`Any` < `OfKind` < `Identifier`)
//! resolves the overloads: an `Identifier` lhs wins `body_identifier`, a module / type-token lhs
//! wins its own slot, and only a bare runtime value falls through to [`body_newtype`].

use crate::machine::model::KKind;
use crate::machine::model::TypeRegistry;
use crate::machine::model::TypeResolution;
use crate::machine::model::{Carried, Module};
use crate::machine::model::{CarriedFamily, Held, KObject, KType, PartedCell, Record, TypeNode};
use crate::machine::StepAllocator;
use crate::machine::StepCarried;
use crate::machine::WriteGate;
use crate::machine::{KError, KErrorKind, MemberResolution, NameLookup, Scope, Visibility};

use super::{arg, kw, sig};
use crate::machine::DeliveredCarried;

/// Lift an `access_*` result into its terminal [`Action`]: a projected member — object or type —
/// seals as a [`StepCarried`] carrier naming its reach ([`Action::done(Ok)`]), an error as a
/// [`Action::done(Err)`]. Both channels are witnessed: an object value re-projected at the fold
/// brand from the lhs operand's view (its reach folded by construction), a type identity witnessed
/// in place from its stored reach via [`Scope::resident`] (or, for a projected type
/// field, re-projected and sealed under the folded lhs reach).
fn route<'a>(result: Result<StepCarried<'a>, KError>) -> crate::machine::Action<'a> {
    crate::machine::Action::done(result)
}

/// Read the `field` member name from `BodyCtx::args`: the value-channel `Identifier` cell, else the
/// type-channel leaf token (resolved or rendered), else a `MissingArg`. Mirrors [`read_field_name`].
fn read_field_name<'a>(args: &Record<Held<'a>>, types: &TypeRegistry) -> Result<String, KError> {
    use crate::machine::{arg_object, arg_type};
    if let Some(obj) = arg_object(args, "field") {
        return match obj {
            KObject::KString(s) => Ok((*s).to_string()),
            other => Err(KError::new(KErrorKind::TypeMismatch {
                arg: "field".to_string(),
                expected: "Identifier".to_string(),
                got: other.ktype().name(types),
            })),
        };
    }
    if let Some(te) = crate::machine::arg_unresolved_type(args, "field") {
        return Ok(te.render());
    }
    if let Some(kt) = arg_type(args, "field") {
        return Ok(kt.name(types));
    }
    Err(KError::new(KErrorKind::MissingArg("field".to_string())))
}

/// Value-then-type lookup of the `s` identifier against `ctx.scope`, returning the projected
/// member as `Action::Done`. A module-valued parameter binds value-side, so a lowercase
/// (Identifier-classed) parameter member access like `elem.compare` inside a functor body reaches
/// the module through the value arm. The type-side probe serves a name bound to an abstract
/// identity (a SIG value slot's `VAL zero :Carrier` type), which names no receiver to project off.
pub fn body_identifier<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::{arg_object, Action};
    let s_name = match arg_object(ctx.args, "s") {
        Some(KObject::KString(s)) => (*s).to_string(),
        Some(other) => {
            return Action::done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "s".to_string(),
                expected: "Identifier".to_string(),
                got: other.ktype().name(ctx.types),
            })));
        }
        None => return Action::done(Err(KError::new(KErrorKind::MissingArg("s".to_string())))),
    };
    let field_name = crate::try_action!(read_field_name(ctx.args, ctx.types));
    // `s` is a bound name: cross the binding's own carrier as the field read's lhs operand, so the
    // projected field folds every region the bound value reaches. The lift is the only read — the
    // field probe runs under the envelope's own pins rather than off a bare reference.
    if let Some(lhs) = ctx.scope.lookup_value_delivered(&s_name) {
        return route(access_field(&ctx.ctx, &field_name, &lhs, ctx.types));
    }
    if let Some(kt) = ctx.scope.resolve_type(&s_name) {
        if let TypeNode::AbstractType { name, .. } = ctx.types.node(kt) {
            return Action::done(Err(abstract_type_has_no_members(&name)));
        }
    }
    Action::done(Err(KError::new(KErrorKind::UnboundName(s_name))))
}

/// `ATTR <s:ProperType> <field:_>` — entry for a type-channel lhs, e.g. a first-class signature
/// value (see [token classes](../../design/typing/tokens.md) for why such an lhs token is
/// Type-classed). The Type-Type overload shares this body so a chained access whose field is itself
/// a Type token reaches the same projection. Projects a member off the Type-classed `s`, resolving
/// an unlowered name carrier through the memoized bridge first. A module lhs rides the value channel
/// and picks [`body_module`] instead, so `Foo.Carrier` projects off the module value.
pub fn body_type_lhs<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::{arg_object, arg_type, arg_unresolved_type, Action};
    if let Some(te) = arg_unresolved_type(ctx.args, "s") {
        let field_name = crate::try_action!(read_field_name(ctx.args, ctx.types));
        return match ctx.scope.resolve_type_identifier(te, None, ctx.types) {
            TypeResolution::Done(kt) => {
                route(access_type_member(ctx.scope, kt, &field_name, ctx.types))
            }
            TypeResolution::Unbound(name) => {
                Action::done(Err(KError::new(KErrorKind::UnboundName(name))))
            }
            TypeResolution::Park(producers) => {
                Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "ATTR lhs type `{}` resolved to a still-finalizing type \
                     (parked on {} producer(s)); the type argument should already be sealed \
                     at body entry",
                    te.render(),
                    producers.len(),
                )))))
            }
        };
    }
    let s_kt = match arg_type(ctx.args, "s") {
        Some(kt) => kt,
        None => {
            return Action::done(Err(match arg_object(ctx.args, "s") {
                Some(other) => KError::new(KErrorKind::TypeMismatch {
                    arg: "s".to_string(),
                    expected: "ProperType".to_string(),
                    got: other.ktype().name(ctx.types),
                }),
                None => KError::new(KErrorKind::MissingArg("s".to_string())),
            }));
        }
    };
    let field_name = crate::try_action!(read_field_name(ctx.args, ctx.types));
    route(access_type_member(ctx.scope, s_kt, &field_name, ctx.types))
}

/// Reads the `Wrapped` runtime lhs and projects the field through [`access_field`].
pub fn body_newtype<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::{arg_object, Action};
    let target = match arg_object(ctx.args, "s") {
        Some(obj) => obj,
        None => return Action::done(Err(KError::new(KErrorKind::MissingArg("s".to_string())))),
    };
    let field_name = crate::try_action!(read_field_name(ctx.args, ctx.types));
    // The lhs `s` is a computed `Wrapped` value delivered to this call (e.g. `seg.finish.x`), so its
    // carrier names regions the read-site frame may not pin; cross the lhs carrier as the field
    // read's operand so the projected field outlives every region the lhs reaches. A carrier-less
    // `s` is region-pure by the `arg_carriers` contract, so it is placed into the read-site region
    // through the shape-split pure door and enveloped there — coverage-equivalent to an empty-reach
    // seal. No region-pure shape is a `Wrapped`, so that arm's diagnostic is what a construction
    // bug would surface here.
    match ctx.arg_carrier("s") {
        Some(lhs) => route(access_field(&ctx.ctx, &field_name, lhs, ctx.types)),
        None => {
            let resident = match ctx.scope.deliver_pure_value(target) {
                Ok(resident) => resident,
                Err(e) => return Action::done(Err(e)),
            };
            route(access_field(&ctx.ctx, &field_name, &resident, ctx.types))
        }
    }
}

/// Projects the field off a module lhs riding the value channel's Object arm.
pub fn body_module<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::{arg_object, Action};
    let m = match arg_object(ctx.args, "s") {
        Some(KObject::Module(module)) => *module,
        Some(other) => {
            return Action::done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "s".to_string(),
                expected: "Module".to_string(),
                got: other.ktype().name(ctx.types),
            })));
        }
        None => {
            return Action::done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "s".to_string(),
                expected: "Module".to_string(),
                got: "Type".to_string(),
            })));
        }
    };
    let field_name = crate::try_action!(read_field_name(ctx.args, ctx.types));
    route(access_module_member(m, &field_name))
}

/// Project `field` off a Type-channel lhs. A signature answers directly from its owned schema —
/// a manifest or abstract type member first, then a declared value slot's type — with no
/// decl-scope reverse-lookup; an abstract identity carries no receiver and errors. A module rides
/// the value channel, so a module lhs lands in [`body_module`] instead. A nominal type handle
/// (struct / union name) and every other type has no members and falls through to the same TypeMismatch a
/// static struct field access produces.
fn access_type_member<'a>(
    scope: &Scope<'a>,
    kt: KType,
    field: &str,
    types: &TypeRegistry,
) -> Result<StepCarried<'a>, KError> {
    match types.node(kt) {
        // ATTR over a first-class signature value — answered from the owned schema. The
        // projected member is a clone out of that schema, allocated fresh into the read-site
        // scope's own region.
        TypeNode::Signature { schema, .. } => {
            let member = schema
                .manifest_members
                .get(field)
                .or_else(|| schema.abstract_members.get(field))
                .or_else(|| schema.value_slots.get(field));
            match member {
                Some(member) => Ok(StepCarried::born(scope.resident(Carried::Type(*member)))),
                None => Err(KError::new(KErrorKind::ShapeError(format!(
                    "signature `{}` has no member `{}`",
                    kt.name(types),
                    field
                )))),
            }
        }
        TypeNode::AbstractType { name, .. } => Err(abstract_type_has_no_members(&name)),
        _ => Err(KError::new(KErrorKind::TypeMismatch {
            arg: "s".to_string(),
            expected: "a type with members".to_string(),
            got: kt.name(types),
        })),
    }
}

/// An abstract type ([`TypeNode::AbstractType`]) is an identity — a binder scope, a name, and a generativity nonce —
/// not a receiver. The
/// module it names rides the value channel, and further members project off *that* value, so a
/// member access whose lhs is a bare abstract identity has nowhere to look.
fn abstract_type_has_no_members(name: &str) -> KError {
    KError::new(KErrorKind::ShapeError(format!(
        "abstract type `{name}` has no projectable members here — project off the module value"
    )))
}

/// Walk nested `Wrapped` layers to the record member named `field` and **part it from its
/// container**: the cell arrives bundled with exactly its own run's stored reach, read off the run,
/// never derived by a subset walk over the container. Lifetime-generic in the container's own region,
/// so the parted cell is confined there until a relocation seam lifts it out.
///
/// A record-repr newtype (an ex-struct) wraps a `KObject::Record`; the member reads straight off
/// it, naming the nominal type in the miss diagnostic so `b.z` on a `Point` still reports `Point`.
/// `Wrapped.inner` is invariantly not a `Wrapped` (the construction-time collapse rule peels any
/// `Wrapped` before re-wrapping), so a scalar inner (a NEWTYPE-over-`Number`, which has no fields)
/// falls to the `other` arm.
fn wrapped_field_cell<'w>(
    target: &'w KObject<'w>,
    field: &str,
    types: &TypeRegistry,
) -> Result<PartedCell<'w>, KError> {
    match target {
        KObject::Wrapped { inner, type_id } => match inner.payload() {
            KObject::Record(substrate, _) => match substrate.field_index(field) {
                Some(at) => Ok(substrate
                    .project(at)
                    .expect("the index came from this substrate's own layout")),
                None => Err(KError::new(KErrorKind::ShapeError(format!(
                    "`{}` has no field `{}`",
                    type_id.name(types),
                    field
                )))),
            },
            payload => wrapped_field_cell(payload, field, types),
        },
        other => Err(KError::new(KErrorKind::TypeMismatch {
            arg: "s".to_string(),
            expected: "a value with fields".to_string(),
            got: other.ktype().name(types),
        })),
    }
}

/// Project `field` off the `Wrapped` runtime lhs whose carrier is the declared operand `lhs`.
///
/// The member is **parted** from its container ([`wrapped_field_cell`]) under the envelope's own
/// pins, so it arrives paired with exactly its own run's stored reach rather than the whole
/// container's. [`Opened::lift_out`](crate::witnessed::Opened::lift_out) is the relocation seam that
/// turns that run into owned coverage — the run's members plus the region the cell lives in, and
/// nothing else — and the field carrier is then folded from *that* envelope, so the product names
/// exactly what the member reaches instead of everything the container did.
///
/// A shallow-scalar member embeds no borrow, so it seals with an empty reach through the no-fold
/// door; a type member is owned data that clones into the read site's own region.
fn access_field<'a>(
    step: &StepAllocator<'a>,
    field: &str,
    lhs: &DeliveredCarried,
    types: &TypeRegistry,
) -> Result<StepCarried<'a>, KError> {
    // The open borrows the envelope's own pins for the whole read, which covers both the walk and
    // the `lift_out` upgrade below.
    let opened = lhs.open_at();
    let parted = wrapped_field_cell(opened.value().object(), field, types)?;
    match parted.value() {
        // A type member is owned data: it clones out of the container and allocates into the read
        // site's own region, so the read carries no dependence on the lhs carrier.
        Held::Type(kt) => return Ok(step.type_carried(*kt)),
        // A record field cell is a value or a resolved type; the bind seam's unlowered carrier
        // never lands in one.
        Held::UnresolvedType(_) => unreachable!("a record field is never an unlowered type name"),
        Held::Object(value) => {
            // A shallow scalar embeds no borrow at all, so it rebuilds owned and seals empty rather
            // than naming the member's run.
            if let Some(sealed) = step.alloc_object_scalar(value) {
                return Ok(sealed);
            }
        }
    }
    // Re-family the lifted cell onto the value channel in place: the envelope keeps the coverage the
    // lift derived, and the projection selects a part of the cell it already covers.
    let member = parted.lift_out().project::<CarriedFamily>(|cell, _token| {
        Carried::Object(
            cell.as_object()
                .expect("classified above: the member is an object cell"),
        )
    });
    Ok(step.alloc_carried_with(&[&member], |b, views| {
        Carried::Object(b.alloc_object_folded(views[0].object().deep_clone()))
    }))
}

/// Look `field` up inside a [`Module`]'s child scope: opaque-ascription `type_members`,
/// then the classified `data`-then-`types` member lookup ([`Bindings::lookup_member`]).
///
/// A nested `MODULE sub = (...)` is a value member, so chained access `Outer.Inner.X` reads the
/// inner module value from `data` and the next ATTR step recurses into its child scope.
///
/// On a value-side hit, an opaque-ascription `slot_type_tags` entry re-tags the read: the
/// raw value is rewrapped in a `KObject::Wrapped` carrier whose `ktype()` is the per-call
/// abstract identity the SIG named (so `(int_ord.zero)` reads as the view's nonced
/// `AbstractType` for `Type`, not the underlying `Number`). Transparent `:!` leaves `slot_type_tags` empty,
/// so transparent reads stay concrete.
///
/// The re-tag carrier is alloc'd in the *module*'s region, not the read-site `scope`'s:
/// `inner` is a pre-existing reference into the module region, so the wrapper is built
/// beside it under the module's home frame, which transitively pins the module's
/// reach-set for the read value. (`type_id` is a Copy handle and imposes no placement
/// constraint.)
fn access_module_member<'a>(m: &'a Module<'a>, field: &str) -> Result<StepCarried<'a>, KError> {
    let module_scope = m.child_scope();
    if let Some(minted) = m.type_members.get(&field).copied() {
        // Prefer the child scope's own binding; a member present only in the mirror is an
        // `:|`-minted abstract type.
        return Ok(StepCarried::born(
            match module_scope
                .bindings()
                .lookup_type(field, Visibility::UNFILTERED)
            {
                Some(NameLookup::Bound(kt)) => module_scope.resident(Carried::Type(kt)),
                _ => module_scope.resident(Carried::Type(minted)),
            },
        ));
    }
    // One classified lookup over the module's own bindings — the cross-kind exclusion makes a
    // name value-xor-type, so a single read decides the arm instead of probing `data` then
    // `types` by hand. A value member lives in the module's region; it seals under the module
    // scope's home frame, which transitively pins the module's reach-set — so the read value
    // (or its re-tag carrier) names the full reach without an embedded lhs to fold (the module
    // identity is the lhs).
    match module_scope
        .bindings()
        .lookup_member(field, Visibility::UNFILTERED)
    {
        Some(MemberResolution::Value(sealed)) => {
            if let Some(tag) = m.slot_type_tags.get(&field).copied() {
                // The re-tag allocates in the module region (not the read site's): both the value
                // member and the re-tag identity `tag` cross as declared fold operands. The member
                // is a binding seal lifted into an envelope pinned by the module scope's own region
                // owner; `tag` is a Copy handle sealed resident via `Scope::resident`. Both
                // carriers union into the wrapped result's witness via `alloc_carried_with`.
                let obj_carrier = module_scope.lift_resident(sealed);
                let tag_carrier = module_scope.seal_resident_delivered(
                    module_scope.resident(Carried::Type(tag)),
                    crate::machine::core::FrameCoverage::empty(),
                );
                let ctx = StepAllocator::for_scope(module_scope);
                // The peel keeps the member's payload verbatim, so a payload substrate that stays
                // in the module's region rides as the payload cell's own stored run; the member
                // carrier's coverage is the holder-rule proof for reading it at the door.
                let holder = obj_carrier.coverage().clone();
                return Ok(ctx.alloc_carried_with(
                    &[&obj_carrier, &tag_carrier],
                    |b, views| match (views[0], views[1]) {
                        (Carried::Object(o), Carried::Type(tag)) => {
                            Carried::Object(b.alloc_object_folded(KObject::wrapped_peel(
                                b.with_holder(&holder),
                                o,
                                tag,
                            )))
                        }
                        _ => unreachable!("operand order: [value member, re-tag identity]"),
                    },
                ));
            }
            // A value member read reaches into the module's region; the lift upgrades the member's
            // exact reach into the owned pins that ride the step.
            Ok(StepCarried::born_delivered(
                module_scope.lift_resident(sealed),
            ))
        }
        Some(MemberResolution::Type { kt }) => {
            Ok(StepCarried::born(module_scope.resident(Carried::Type(kt))))
        }
        None => Err(KError::new(KErrorKind::ShapeError(format!(
            "module `{}` has no member `{}`",
            m.path, field
        )))),
    }
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry, gate: &mut WriteGate) {
    let identifier_sig = || {
        sig(
            KType::ANY,
            vec![
                kw("ATTR"),
                arg("s", KType::IDENTIFIER),
                arg("field", KType::IDENTIFIER),
            ],
        )
    };
    let module_field_sig = || {
        sig(
            KType::ANY,
            vec![
                kw("ATTR"),
                arg("s", KType::EMPTY_SIGNATURE),
                arg("field", KType::IDENTIFIER),
            ],
        )
    };
    // NEWTYPE fall-through, including ex-structs. A computed `Wrapped` lhs (e.g.
    // `seg.finish.x`) arrives in the Object channel; the `s: Any` slot matches the *value* by
    // a type (never by a kind — `OfKind` is type-channel-only), and `access_field`'s `Wrapped`
    // arm validates the shape, reading a record repr's field directly and recursing one level
    // for any other inner (a non-`Wrapped` value errors "a value with fields"). This stays
    // unambiguous with the sibling overloads: `Any` is the least specific, so an `Identifier`
    // lhs picks `body_identifier`, a module / type-token lhs picks `body_module` /
    // `body_type_lhs`, and only a bare runtime value falls through to here.
    let newtype_sig = || {
        sig(
            KType::ANY,
            vec![
                kw("ATTR"),
                arg("s", KType::ANY),
                arg("field", KType::IDENTIFIER),
            ],
        )
    };
    let type_identifier_field_sig = || {
        sig(
            KType::ANY,
            vec![
                kw("ATTR"),
                arg("s", KType::of_kind(KKind::ProperType)),
                arg("field", KType::IDENTIFIER),
            ],
        )
    };
    let type_type_field_sig = || {
        sig(
            KType::ANY,
            vec![
                kw("ATTR"),
                arg("s", KType::of_kind(KKind::ProperType)),
                arg("field", KType::of_kind(KKind::ProperType)),
            ],
        )
    };
    // Module lhs with a Type-classed field (e.g. the `Outer.Inner` step in `Outer.Inner.x`).
    let module_type_field_sig = || {
        sig(
            KType::ANY,
            vec![
                kw("ATTR"),
                arg("s", KType::EMPTY_SIGNATURE),
                arg("field", KType::of_kind(KKind::ProperType)),
            ],
        )
    };

    use crate::builtins::register_builtin;
    register_builtin(
        scope,
        "ATTR",
        identifier_sig(),
        body_identifier,
        types,
        gate,
    );
    register_builtin(scope, "ATTR", module_field_sig(), body_module, types, gate);
    register_builtin(scope, "ATTR", newtype_sig(), body_newtype, types, gate);
    register_builtin(
        scope,
        "ATTR",
        type_identifier_field_sig(),
        body_type_lhs,
        types,
        gate,
    );
    register_builtin(
        scope,
        "ATTR",
        type_type_field_sig(),
        body_type_lhs,
        types,
        gate,
    );
    register_builtin(
        scope,
        "ATTR",
        module_type_field_sig(),
        body_module,
        types,
        gate,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, TestRun};
    use crate::machine::model::KObject;
    use crate::machine::model::KType;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    use crate::machine::KErrorKind;

    #[test]
    fn attr_reads_field_from_named_struct() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Point = :{x :Number, y :Number}\nLET p = (Point {x = 3, y = 4})");
        let result = test_run.run_one(parse_one(&program, "p.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 3.0));
    }

    #[test]
    fn attr_reads_each_field_independently() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Point = :{x :Number, y :Number}\nLET p = (Point {x = 3, y = 4})");
        assert!(
            matches!(test_run.run_one(parse_one(&program, "p.x")), KObject::Number(n) if *n == 3.0)
        );
        assert!(
            matches!(test_run.run_one(parse_one(&program, "p.y")), KObject::Number(n) if *n == 4.0)
        );
    }

    #[test]
    fn attr_chained_through_nested_struct() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "NEWTYPE Point = :{x :Number, y :Number}\n\
             NEWTYPE Line = :{start :Point, finish :Point}\n\
             LET origin = (Point {x = 0, y = 0})\n\
             LET tip = (Point {x = 3, y = 4})\n\
             LET seg = (Line {start = origin, finish = tip})",
        );
        let result = test_run.run_one(parse_one(&program, "seg.finish.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 3.0));
    }

    #[test]
    fn attr_unbound_name_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run.run_one_err(parse_one(&program, "ghost.x"));
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(name) if name == "ghost"),
            "expected UnboundName(\"ghost\"), got {err}",
        );
    }

    #[test]
    fn attr_on_non_struct_value_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("LET n = 5");
        let err = test_run.run_one_err(parse_one(&program, "n.x"));
        match &err.kind {
            KErrorKind::TypeMismatch { arg, expected, got } => {
                assert_eq!(arg, "s");
                assert_eq!(expected, "a value with fields");
                assert_eq!(got, "Number");
            }
            _ => panic!("expected TypeMismatch on non-struct lhs, got {err}"),
        }
    }

    #[test]
    fn attr_unknown_field_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Point = :{x :Number, y :Number}\nLET p = (Point {x = 3, y = 4})");
        let err = test_run.run_one_err(parse_one(&program, "p.z"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Point") && msg.contains("`z`")),
            "expected ShapeError naming Point and z, got {err}",
        );
    }

    #[test]
    fn attr_chained_unknown_field_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "NEWTYPE Point = :{x :Number, y :Number}\n\
             NEWTYPE Line = :{start :Point, finish :Point}\n\
             LET origin = (Point {x = 0, y = 0})\n\
             LET tip = (Point {x = 3, y = 4})\n\
             LET seg = (Line {start = origin, finish = tip})",
        );
        let err = test_run.run_one_err(parse_one(&program, "seg.start.bogus"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Point") && msg.contains("`bogus`")),
            "expected ShapeError naming Point and bogus on chained access, got {err}",
        );
    }

    /// `b.x` on a NEWTYPE-wrapped record-newtype reads through to the underlying field.
    #[test]
    fn access_field_falls_through_wrapped_record_newtype() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "NEWTYPE Point = :{x :Number, y :Number}\n\
             NEWTYPE Boxed = Point\n\
             LET p = (Point {x = 1, y = 2})\n\
             LET b = (Boxed (p))",
        );
        assert!(
            matches!(test_run.run_one(parse_one(&program, "b.x")), KObject::Number(n) if *n == 1.0)
        );
        assert!(
            matches!(test_run.run_one(parse_one(&program, "b.y")), KObject::Number(n) if *n == 2.0)
        );
    }

    /// Wrapping a scalar doesn't grow fields: `d.x` on a NEWTYPE-over-Number errors.
    #[test]
    fn access_field_rejects_wrapped_non_struct() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "NEWTYPE Distance = Number\n\
             LET d = (Distance (3.0))",
        );
        let err = test_run.run_one_err(parse_one(&program, "d.x"));
        match &err.kind {
            KErrorKind::TypeMismatch { arg, expected, got } => {
                assert_eq!(arg, "s");
                assert_eq!(expected, "a value with fields");
                assert_eq!(got, "Number");
            }
            _ => panic!("expected TypeMismatch on NEWTYPE-over-Number field access, got {err}"),
        }
    }

    /// An opaque (`:|`) view re-tags a VAL-slot read with the per-call abstract identity:
    /// `int_ord_view.zero` reads as the abstract `Carrier` (`ktype().name() == "Carrier"`), not the
    /// underlying `Number`, so a deferred return `er.Carrier` accepts the body.
    #[test]
    fn opaque_view_slot_read_re_tags_with_abstract_type() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "SIG WithZero = ((TYPE Carrier) (VAL zero :Carrier))\n\
             MODULE int_ord = ((LET Carrier = Number) (LET zero = 0))\n\
             LET int_ord_view = (int_ord :| WithZero)",
        );
        let types = test_run.types.clone();
        let result = test_run.run_one(parse_one(&program, "int_ord_view.zero"));
        assert_eq!(
            result.ktype().name(&types),
            "Carrier",
            "opaque-view slot read must carry the abstract `Carrier` identity, got {:?}",
            result.ktype(),
        );
    }

    /// Transparent (`:!`) views leave `slot_type_tags` empty, so the slot read stays
    /// concrete: `int_ord_view.zero` reads as the underlying `Number`, not the abstract `Type`.
    #[test]
    fn transparent_view_slot_read_stays_concrete() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "SIG WithZero = ((TYPE Carrier) (VAL zero :Carrier))\n\
             MODULE int_ord = ((LET Carrier = Number) (LET zero = 0))\n\
             LET int_ord_view = (int_ord :! WithZero)",
        );
        let result = test_run.run_one(parse_one(&program, "int_ord_view.zero"));
        assert!(
            matches!(result, KObject::Number(n) if *n == 0.0),
            "transparent-view slot read must stay the underlying Number, got {:?}",
            result.ktype(),
        );
    }

    /// ATTR on a bare signature type value — not a module/view instance — reads a `VAL` slot's
    /// declared type straight out of the decl scope's slot collector (the `sig_slot` fallback in
    /// `access_type_member`): `Ordered.compare` yields the slot's declared `Number`, as a type-side
    /// result (a `VAL` slot is a specification, never a value).
    #[test]
    fn attr_on_signature_type_reads_val_slot_declared_type() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("SIG Ordered = (VAL compare :Number)");
        let kt = test_run.run_one_type(parse_one(&program, "Ordered.compare"));
        assert_eq!(kt, KType::NUMBER);
    }

    /// A missing field on the wrapped record names the carrier's nominal type in the
    /// `ShapeError`. The newtype-over-newtype collapse peels the inner `Point` identity, so
    /// `b = Boxed(p)` wraps the bare record tagged `Boxed`; the diagnostic names `Boxed`.
    #[test]
    fn access_field_falls_through_wrapped_with_missing_field() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "NEWTYPE Point = :{x :Number, y :Number}\n\
             NEWTYPE Boxed = Point\n\
             LET p = (Point {x = 1, y = 2})\n\
             LET b = (Boxed (p))",
        );
        let err = test_run.run_one_err(parse_one(&program, "b.z"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Boxed") && msg.contains("`z`")),
            "expected ShapeError naming Boxed and z on Wrapped fall-through, got {err}",
        );
    }
}
