//! `ATTR <s> <field:Identifier>` — newtype (record-repr or scalar), module, or signature
//! field access. Surface syntax is the `.` infix operator. Overloads share the bucket
//! `[Keyword, Slot, Slot]` and pick by lhs shape: [`body_identifier`] for `p.x` where
//! the lhs is still an `Identifier`, [`body_newtype`] for a `Wrapped` lhs (a record-repr
//! newtype's `.x` reads through to the wrapped record), [`body_module`] for chained module
//! access.
//!
//! The lhs is matched by *type*, never by a kind: a type-channel lhs (a module / type token)
//! picks `body_module` / `body_type_lhs` through its `OfKind` kind, while a
//! value-channel lhs is caught by the least-specific `s: Any` slot and validated in
//! [`access_field`]. Specificity (`Any` < `OfKind` < `Identifier`) resolves the overloads: an
//! `Identifier` lhs wins `body_identifier`, a module / type-token lhs wins its `OfKind`
//! overload, and only a bare runtime value falls through to [`body_newtype`].

use crate::machine::core::kfunction::action::scope_frame;
use crate::machine::core::KoanStepContextExt;
use crate::machine::model::types::AbstractSource;
use crate::machine::model::types::KKind;
use crate::machine::model::types::TypeResolution;
use crate::machine::model::values::{Carried, CarriedFamily, Module, NonWrappedRef};
use crate::machine::model::{Held, KObject, KType};
use crate::machine::{
    CarrierWitness, FrameSet, FrameStorage, KError, KErrorKind, MemberResolution, NameLookup, Scope,
};
use crate::witnessed::{Sealed, StepContext, Witnessed};

use super::{arg, kw, sig};

/// Lift an `access_*` result into its terminal [`Action`]: a projected member — object or type —
/// seals as a [`Witnessed`] carrier naming its reach ([`Action::Done(Ok)`]), an error as a
/// [`Action::Done(Err)`]. Both channels are witnessed: an object value sealed by the step
/// context's `alloc_object_with`, its dep carriers' reach folded by construction, a type
/// identity witnessed in place from its stored reach via [`Scope::resident_type_carrier`] (or,
/// for a projected type field, sealed under the folded dep reach).
fn route<'a>(
    result: Result<Witnessed<CarriedFamily, CarrierWitness>, KError>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    crate::machine::core::kfunction::action::Action::Done(result)
}

/// Read the `field` member name from `BodyCtx::args`: the value-channel `Identifier` cell, else the
/// type-channel leaf token (resolved or rendered), else a `MissingArg`. Mirrors [`read_field_name`].
fn read_field_name<'a>(args: &KObject<'a>) -> Result<String, KError> {
    use crate::machine::core::kfunction::action::{arg_object, arg_type};
    if let Some(obj) = arg_object(args, "field") {
        return match obj {
            KObject::KString(s) => Ok(s.clone()),
            other => Err(KError::new(KErrorKind::TypeMismatch {
                arg: "field".to_string(),
                expected: "Identifier".to_string(),
                got: other.ktype().name().to_string(),
            })),
        };
    }
    if let Some(kt) = arg_type(args, "field") {
        return Ok(match kt {
            KType::Unresolved(te) => te.render(),
            other => other.name(),
        });
    }
    Err(KError::new(KErrorKind::MissingArg("field".to_string())))
}

/// Value-then-type lookup of the `s` identifier against `ctx.scope`, returning the projected
/// member as `Action::Done`. A FUNCTOR's signature-typed parameter is bound only into
/// `bindings.types`, so `Er.pure(x)` inside the functor body must reach the carried `&Module`
/// through `resolve_type` — hence value-side first, then type-side.
pub fn body_identifier<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{arg_object, Action};
    let s_name = match arg_object(ctx.args, "s") {
        Some(KObject::KString(s)) => s.clone(),
        Some(other) => {
            return Action::Done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "s".to_string(),
                expected: "Identifier".to_string(),
                got: other.ktype().name().to_string(),
            })));
        }
        None => return Action::Done(Err(KError::new(KErrorKind::MissingArg("s".to_string())))),
    };
    let field_name = crate::try_action!(read_field_name(ctx.args));
    // `s` is a bound name: the target lives in its (ancestor) binding scope, transitively pinned by
    // the read-site scope's home frame, so the field read folds no dep carrier (`&[]`).
    if let Some(target) = ctx.scope.lookup(&s_name) {
        return route(access_field(&ctx.ctx, target, &field_name, &[]));
    }
    if let Some(kt) = ctx.scope.resolve_type(&s_name) {
        match kt {
            KType::Module { module: m, .. } => return route(access_module_member(m, &field_name)),
            KType::AbstractType {
                source: AbstractSource::Module(m),
                ..
            } => {
                return route(access_module_member(m, &field_name));
            }
            KType::AbstractType { .. } => {}
            _ => {}
        }
    }
    Action::Done(Err(KError::new(KErrorKind::UnboundName(s_name))))
}

/// `ATTR <s:ProperType> <field:_>` — entry for a Type-classed lhs. Module names are Type-classed
/// tokens (see [token classes](../../design/typing/tokens.md)), so `Foo.x` parses as
/// `[ATTR Type(Foo) Identifier(x)]` instead of the `Identifier`-lhs the struct path uses. The
/// Type-Type overload shares this body so chained module access (`Outer.Inner.x`) works regardless
/// of whether each step's field is a module name or a regular member. Projects a member off the
/// Type-classed `s`, resolving an `Unresolved` leaf through the memoized bridge first.
pub fn body_type_lhs<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{arg_object, arg_type, Action};
    let s_kt = match arg_type(ctx.args, "s") {
        Some(kt) => kt,
        None => {
            return Action::Done(Err(match arg_object(ctx.args, "s") {
                Some(other) => KError::new(KErrorKind::TypeMismatch {
                    arg: "s".to_string(),
                    expected: "ProperType".to_string(),
                    got: other.ktype().name(),
                }),
                None => KError::new(KErrorKind::MissingArg("s".to_string())),
            }));
        }
    };
    let field_name = crate::try_action!(read_field_name(ctx.args));
    match s_kt {
        KType::Unresolved(te) => match ctx.scope.resolve_type_identifier(te, None) {
            // The lhs type's own reach is irrelevant here — the member's carrier is built from the
            // *member's* stored reach inside `access_type_member`.
            TypeResolution::Done(resolved) => route(access_type_member(resolved.kt, &field_name)),
            TypeResolution::Unbound(name) => {
                Action::Done(Err(KError::new(KErrorKind::UnboundName(name))))
            }
            TypeResolution::Park(producers) => {
                Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "ATTR lhs type `{}` resolved to a still-finalizing type \
                     (parked on {} producer(s)); the type argument should already be sealed \
                     at body entry",
                    te.render(),
                    producers.len(),
                )))))
            }
        },
        kt => route(access_type_member(kt, &field_name)),
    }
}

/// Reads the `Wrapped` runtime lhs and projects the field through [`access_field`].
pub fn body_newtype<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{arg_object, Action};
    let target = match arg_object(ctx.args, "s") {
        Some(obj) => obj,
        None => return Action::Done(Err(KError::new(KErrorKind::MissingArg("s".to_string())))),
    };
    let field_name = crate::try_action!(read_field_name(ctx.args));
    // The lhs `s` is a computed `Wrapped` value delivered to this call (e.g. `seg.finish.x`), so its
    // carrier names regions the read-site frame may not pin; fold the lhs carrier as the field
    // read's dep so the projected field outlives every region the lhs reaches.
    let deps: Vec<_> = ctx.arg_carrier("s").into_iter().collect();
    route(access_field(&ctx.ctx, target, &field_name, &deps))
}

/// Projects the field off a module identity riding the type channel (the lhs is the `Type` arm).
pub fn body_module<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{arg_object, arg_type, Action};
    let m = match arg_type(ctx.args, "s") {
        Some(KType::Module { module, .. }) => *module,
        _ => {
            return Action::Done(Err(match arg_object(ctx.args, "s") {
                Some(other) => KError::new(KErrorKind::TypeMismatch {
                    arg: "s".to_string(),
                    expected: "Module".to_string(),
                    got: other.ktype().name(),
                }),
                None => KError::new(KErrorKind::TypeMismatch {
                    arg: "s".to_string(),
                    expected: "Module".to_string(),
                    got: "Type".to_string(),
                }),
            }));
        }
    };
    let field_name = crate::try_action!(read_field_name(ctx.args));
    route(access_module_member(m, &field_name))
}

/// Project `field` off a Type-channel lhs: a module / signature / opaque-abstract identity.
/// A `SetRef` (struct / union name) and every other type has no members and falls through to
/// the same TypeMismatch a static struct field access produces.
fn access_type_member<'a>(
    kt: &KType<'a>,
    field: &str,
) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
    match kt {
        KType::Module { module: m, .. } => access_module_member(m, field),
        // ATTR over a first-class signature value — reverse-lookup against the decl scope. A value
        // member lives in that decl region, so it seals under the decl scope's home frame.
        KType::Signature { sig: s, .. } => {
            let decl = s.decl_scope();
            match decl.bindings().lookup_member(field, None) {
                Some(MemberResolution::Value {
                    obj,
                    reach,
                    borrows_into_home: _,
                }) => Ok(decl.resident_value_carrier(obj, &reach)),
                Some(MemberResolution::Type {
                    kt,
                    reach,
                    borrows_into_home: _,
                }) => Ok(decl.resident_type_carrier(kt, &reach)),
                None => Err(KError::new(KErrorKind::ShapeError(format!(
                    "signature `{}` has no member `{}`",
                    s.path, field
                )))),
            }
        }
        // ATTR over an opaque-ascription abstract type — project against the source module.
        // A `Sig`-rooted abstract type has no module to project off, so it falls through.
        KType::AbstractType {
            source: AbstractSource::Module(m),
            ..
        } => access_module_member(m, field),
        other => Err(KError::new(KErrorKind::TypeMismatch {
            arg: "s".to_string(),
            expected: "a type with members".to_string(),
            got: other.name(),
        })),
    }
}

fn access_field<'a>(
    step: &StepContext<FrameStorage>,
    target: &KObject<'a>,
    field: &str,
    deps: &[&Sealed<CarriedFamily, CarrierWitness>],
) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
    match target {
        // NEWTYPE fall-through. A record-repr newtype (an ex-struct) wraps a
        // `KObject::Record`; read the field straight off it, naming the nominal type in the
        // miss diagnostic so `b.z` on a `Point` still reports `Point`. `Wrapped.inner` is
        // invariantly not a `Wrapped` (the construction-time collapse rule peels any
        // `Wrapped` before re-wrapping), so a scalar inner (a NEWTYPE-over-`Number`, which
        // has no fields) falls to the `other` arm. The field value is deep-cloned into the
        // read-site region and sealed under that region's home frame; the `deps` slice — the
        // computed lhs carrier, when the lhs was a delivered value — folds its reach so a field
        // of a multi-region value keeps every region it borrows into alive.
        KObject::Wrapped { inner, type_id } => match inner.get() {
            KObject::Record(values, _) => match values.get(field) {
                Some(Held::Object(value)) => Ok(step.alloc_object_with(deps, value.deep_clone())),
                // A projected type field of the `Wrapped` record — born region-pure and sealed under
                // the home frame plus the `deps` slice's folded reach (which pins a nested module's
                // region, since the `Wrapped` physically contains it), mirroring the Object arm above.
                Some(Held::Type(kt)) => Ok(step.alloc_type_with(deps, kt.clone())),
                None => Err(KError::new(KErrorKind::ShapeError(format!(
                    "`{}` has no field `{}`",
                    type_id.name(),
                    field
                )))),
            },
            inner => access_field(step, inner, field, deps),
        },
        other => Err(KError::new(KErrorKind::TypeMismatch {
            arg: "s".to_string(),
            expected: "a value with fields".to_string(),
            got: other.ktype().name().to_string(),
        })),
    }
}

/// Look `field` up inside a [`Module`]'s child scope: opaque-ascription `type_members`,
/// then value-side `data`, then type-side `bindings.types`.
///
/// Preferring `data` over `bindings.types` matters for nominal binders like
/// `MODULE Sub = (...)` and `NEWTYPE P = :{...}`, which install into both: chained access
/// `Outer.Inner.X` needs the inner *module value* from `data`, not its type identity,
/// so the next ATTR step can recurse into the inner module's child scope.
///
/// On a value-side hit, an opaque-ascription `slot_type_tags` entry re-tags the read: the
/// raw value is rewrapped in a `KObject::Wrapped` carrier whose `ktype()` is the per-call
/// abstract identity the SIG named (so `(int_ord.zero)` reads as `AbstractType{int_ord,
/// "Type"}`, not the underlying `Number`). Transparent `:!` leaves `slot_type_tags` empty,
/// so transparent reads stay concrete.
///
/// The re-tag carrier (and its `type_id`) is alloc'd in the *module*'s region, not the
/// read-site `scope`'s: `Wrapped::deep_clone` is shallow (the NEWTYPE invariant that
/// `type_id` is a declaration-stable `&'a KType`), so the `type_id` must outlive any
/// lift/deep-clone of the read value — e.g. a functor body's `(Er.zero)` whose read-site
/// scope is a per-call region. The module and its `slot_type_tags` are declaration-stable,
/// so the module region is the right home; both `inner` (the slot value) and `type_id`
/// (the abstract tag, which references the module) then live there together.
fn access_module_member<'a>(
    m: &'a Module<'a>,
    field: &str,
) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
    let module_scope = m.child_scope();
    if let Some(minted) = m.type_members.borrow().get(field).cloned() {
        // Prefer the child scope's own binding — witness its `&KType` in place from the stored reach
        // (non-empty for a nested module). A member present only in the mirror is an `:|`-minted
        // abstract type reaching nothing foreign, so it is alloc'd fresh under the empty reach.
        return Ok(
            match module_scope.bindings().lookup_type_carrier(field, None) {
                Some(NameLookup::Bound(hit)) => {
                    module_scope.resident_type_carrier(hit.kt, &hit.reach)
                }
                _ => module_scope.resident_type_carrier(
                    module_scope.brand().alloc_ktype(minted),
                    &FrameSet::empty(),
                ),
            },
        );
    }
    // One classified lookup over the module's own bindings — the cross-kind exclusion makes a
    // name value-xor-type, so a single read decides the arm instead of probing `data` then
    // `types` by hand. A value member lives in the module's region; it seals under the module
    // scope's home frame, which transitively pins the module's reach-set — so the read value
    // (or its re-tag carrier) names the full reach without an embedded lhs to fold (the module
    // identity is the lhs).
    match module_scope.bindings().lookup_member(field, None) {
        Some(MemberResolution::Value {
            obj,
            reach,
            borrows_into_home: _,
        }) => {
            if let Some(tag) = m.slot_type_tags.borrow().get(field).cloned() {
                // The re-tag allocates in the module region (not the read site's): `obj` is a
                // pre-existing reference into that region, so it crosses as a fold operand — its
                // carrier (named by the member's own `reach`) unions into the wrapped result's
                // witness via `alloc_carried_with`.
                let obj_carrier = Sealed::seal(module_scope.resident_value_carrier(obj, &reach));
                let ctx = StepContext::new(scope_frame(module_scope));
                return Ok(
                    ctx.alloc_carried_with(&[&obj_carrier], |b, views| match views[0] {
                        Carried::Object(o) => Carried::Object(b.alloc_object(KObject::Wrapped {
                            inner: NonWrappedRef::peel(o),
                            type_id: b.alloc_ktype(tag),
                        })),
                        Carried::Type(_) => unreachable!("a module value member is always Object"),
                    }),
                );
            }
            Ok(module_scope.resident_value_carrier(obj, &reach))
        }
        Some(MemberResolution::Type {
            kt,
            reach,
            borrows_into_home: _,
        }) => Ok(module_scope.resident_type_carrier(kt, &reach)),
        None => Err(KError::new(KErrorKind::ShapeError(format!(
            "module `{}` has no member `{}`",
            m.path, field
        )))),
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let identifier_sig = || {
        sig(
            KType::Any,
            vec![
                kw("ATTR"),
                arg("s", KType::Identifier),
                arg("field", KType::Identifier),
            ],
        )
    };
    let module_field_sig = || {
        sig(
            KType::Any,
            vec![
                kw("ATTR"),
                arg("s", KType::OfKind(KKind::Module)),
                arg("field", KType::Identifier),
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
            KType::Any,
            vec![
                kw("ATTR"),
                arg("s", KType::Any),
                arg("field", KType::Identifier),
            ],
        )
    };
    let type_identifier_field_sig = || {
        sig(
            KType::Any,
            vec![
                kw("ATTR"),
                arg("s", KType::OfKind(KKind::ProperType)),
                arg("field", KType::Identifier),
            ],
        )
    };
    let type_type_field_sig = || {
        sig(
            KType::Any,
            vec![
                kw("ATTR"),
                arg("s", KType::OfKind(KKind::ProperType)),
                arg("field", KType::OfKind(KKind::ProperType)),
            ],
        )
    };
    // Module lhs with a Type-classed field (e.g. the `Outer.Inner` step in `Outer.Inner.x`).
    let module_type_field_sig = || {
        sig(
            KType::Any,
            vec![
                kw("ATTR"),
                arg("s", KType::OfKind(KKind::Module)),
                arg("field", KType::OfKind(KKind::ProperType)),
            ],
        )
    };

    use crate::builtins::register_builtin;
    register_builtin(scope, "ATTR", identifier_sig(), body_identifier);
    register_builtin(scope, "ATTR", module_field_sig(), body_module);
    register_builtin(scope, "ATTR", newtype_sig(), body_newtype);
    register_builtin(scope, "ATTR", type_identifier_field_sig(), body_type_lhs);
    register_builtin(scope, "ATTR", type_type_field_sig(), body_type_lhs);
    register_builtin(scope, "ATTR", module_type_field_sig(), body_module);
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::machine::core::FrameStorage;
    use crate::machine::model::KObject;
    use crate::machine::KErrorKind;

    #[test]
    fn attr_reads_field_from_named_struct() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(
            scope,
            "NEWTYPE Point = :{x :Number, y :Number}\nLET p = (Point {x = 3, y = 4})",
        );
        let result = run_one(scope, parse_one("p.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 3.0));
    }

    #[test]
    fn attr_reads_each_field_independently() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(
            scope,
            "NEWTYPE Point = :{x :Number, y :Number}\nLET p = (Point {x = 3, y = 4})",
        );
        assert!(matches!(run_one(scope, parse_one("p.x")), KObject::Number(n) if *n == 3.0));
        assert!(matches!(run_one(scope, parse_one("p.y")), KObject::Number(n) if *n == 4.0));
    }

    #[test]
    fn attr_chained_through_nested_struct() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(
            scope,
            "NEWTYPE Point = :{x :Number, y :Number}\n\
             NEWTYPE Line = :{start :Point, finish :Point}\n\
             LET origin = (Point {x = 0, y = 0})\n\
             LET tip = (Point {x = 3, y = 4})\n\
             LET seg = (Line {start = origin, finish = tip})",
        );
        let result = run_one(scope, parse_one("seg.finish.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 3.0));
    }

    #[test]
    fn attr_unbound_name_errors() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        let err = run_one_err(scope, parse_one("ghost.x"));
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(name) if name == "ghost"),
            "expected UnboundName(\"ghost\"), got {err}",
        );
    }

    #[test]
    fn attr_on_non_struct_value_errors() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(scope, "LET n = 5");
        let err = run_one_err(scope, parse_one("n.x"));
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
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(
            scope,
            "NEWTYPE Point = :{x :Number, y :Number}\nLET p = (Point {x = 3, y = 4})",
        );
        let err = run_one_err(scope, parse_one("p.z"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Point") && msg.contains("`z`")),
            "expected ShapeError naming Point and z, got {err}",
        );
    }

    #[test]
    fn attr_chained_unknown_field_errors() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(
            scope,
            "NEWTYPE Point = :{x :Number, y :Number}\n\
             NEWTYPE Line = :{start :Point, finish :Point}\n\
             LET origin = (Point {x = 0, y = 0})\n\
             LET tip = (Point {x = 3, y = 4})\n\
             LET seg = (Line {start = origin, finish = tip})",
        );
        let err = run_one_err(scope, parse_one("seg.start.bogus"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Point") && msg.contains("`bogus`")),
            "expected ShapeError naming Point and bogus on chained access, got {err}",
        );
    }

    /// `b.x` on a NEWTYPE-wrapped record-newtype reads through to the underlying field.
    #[test]
    fn access_field_falls_through_wrapped_record_newtype() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(
            scope,
            "NEWTYPE Point = :{x :Number, y :Number}\n\
             NEWTYPE Boxed = Point\n\
             LET p = (Point {x = 1, y = 2})\n\
             LET b = (Boxed (p))",
        );
        assert!(matches!(run_one(scope, parse_one("b.x")), KObject::Number(n) if *n == 1.0));
        assert!(matches!(run_one(scope, parse_one("b.y")), KObject::Number(n) if *n == 2.0));
    }

    /// Wrapping a scalar doesn't grow fields: `d.x` on a NEWTYPE-over-Number errors.
    #[test]
    fn access_field_rejects_wrapped_non_struct() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(
            scope,
            "NEWTYPE Distance = Number\n\
             LET d = (Distance (3.0))",
        );
        let err = run_one_err(scope, parse_one("d.x"));
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
    /// `IntOrdView.zero` reads as the abstract `Carrier` (`ktype().name() == "Carrier"`), not the
    /// underlying `Number`, so a deferred return `Er.Carrier` accepts the body.
    #[test]
    fn opaque_view_slot_read_re_tags_with_abstract_type() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(
            scope,
            "SIG WithZero = ((LET Carrier = Number) (VAL zero :Carrier))\n\
             MODULE IntOrd = ((LET Carrier = Number) (LET zero = 0))\n\
             LET IntOrdView = (IntOrd :| WithZero)",
        );
        let result = run_one(scope, parse_one("IntOrdView.zero"));
        assert_eq!(
            result.ktype().name(),
            "Carrier",
            "opaque-view slot read must carry the abstract `Carrier` identity, got {:?}",
            result.ktype(),
        );
    }

    /// Transparent (`:!`) views leave `slot_type_tags` empty, so the slot read stays
    /// concrete: `IntOrdView.zero` reads as the underlying `Number`, not the abstract `Type`.
    #[test]
    fn transparent_view_slot_read_stays_concrete() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(
            scope,
            "SIG WithZero = ((LET Carrier = Number) (VAL zero :Carrier))\n\
             MODULE IntOrd = ((LET Carrier = Number) (LET zero = 0))\n\
             LET IntOrdView = (IntOrd :! WithZero)",
        );
        let result = run_one(scope, parse_one("IntOrdView.zero"));
        assert!(
            matches!(result, KObject::Number(n) if *n == 0.0),
            "transparent-view slot read must stay the underlying Number, got {:?}",
            result.ktype(),
        );
    }

    /// A missing field on the wrapped record names the carrier's nominal type in the
    /// `ShapeError`. The newtype-over-newtype collapse peels the inner `Point` identity, so
    /// `b = Boxed(p)` wraps the bare record tagged `Boxed`; the diagnostic names `Boxed`.
    #[test]
    fn access_field_falls_through_wrapped_with_missing_field() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(
            scope,
            "NEWTYPE Point = :{x :Number, y :Number}\n\
             NEWTYPE Boxed = Point\n\
             LET p = (Point {x = 1, y = 2})\n\
             LET b = (Boxed (p))",
        );
        let err = run_one_err(scope, parse_one("b.z"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Boxed") && msg.contains("`z`")),
            "expected ShapeError naming Boxed and z on Wrapped fall-through, got {err}",
        );
    }
}
