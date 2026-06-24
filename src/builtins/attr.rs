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

use crate::machine::execute::{resolve_type_leaf_carrier, TypeLeafCarrier};
use crate::machine::model::types::AbstractSource;
use crate::machine::model::types::KKind;
use crate::machine::model::values::Carried;
use crate::machine::model::values::{Module, NonWrappedRef};
use crate::machine::model::{Held, KObject, KType};
use crate::machine::{KError, KErrorKind, Resolution, Scope};

use super::{arg, kw, sig};

/// Wrap an `access_*` helper's `Result<Carried, KError>` as a `Done` [`Action`].
fn done<'a>(
    result: Result<Carried<'a>, KError>,
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
    if let Some(target) = ctx.scope.lookup(&s_name) {
        return done(access_field(ctx.scope, target, &field_name));
    }
    if let Some(kt) = ctx.scope.resolve_type(&s_name) {
        match kt {
            KType::Module { module: m, .. } => return done(access_module_member(m, &field_name)),
            KType::AbstractType {
                source: AbstractSource::Module(m),
                ..
            } => {
                return done(access_module_member(m, &field_name));
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
        KType::Unresolved(te) => match resolve_type_leaf_carrier(ctx.scope, te, None) {
            TypeLeafCarrier::Resolved(kt) => done(access_type_member(ctx.scope, kt, &field_name)),
            TypeLeafCarrier::Unbound(name) => {
                Action::Done(Err(KError::new(KErrorKind::UnboundName(name))))
            }
            TypeLeafCarrier::Park(producers) => {
                Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "ATTR lhs type `{}` resolved to a still-finalizing type \
                     (parked on {} producer(s)); the type argument should already be sealed \
                     at body entry",
                    te.render(),
                    producers.len(),
                )))))
            }
        },
        kt => done(access_type_member(ctx.scope, kt, &field_name)),
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
    done(access_field(ctx.scope, target, &field_name))
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
    done(access_module_member(m, &field_name))
}

/// Project `field` off a Type-channel lhs: a module / signature / opaque-abstract identity.
/// A `SetRef` (struct / union name) and every other type has no members and falls through to
/// the same TypeMismatch a static struct field access produces.
fn access_type_member<'a>(
    scope: &Scope<'a>,
    kt: &KType<'a>,
    field: &str,
) -> Result<Carried<'a>, KError> {
    match kt {
        KType::Module { module: m, .. } => access_module_member(m, field),
        // ATTR over a first-class signature value — reverse-lookup against the decl scope.
        KType::Signature { sig: s, .. } => {
            let decl = s.decl_scope();
            if let Some(Resolution::Value(obj)) = decl.bindings().lookup_value(field, None) {
                return Ok(Carried::Object(obj));
            }
            if let Some(kt) = decl.resolve_type(field) {
                return Ok(Carried::Type(scope.region.alloc_ktype(kt.clone())));
            }
            Err(KError::new(KErrorKind::ShapeError(format!(
                "signature `{}` has no member `{}`",
                s.path, field
            ))))
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
    scope: &Scope<'a>,
    target: &KObject<'a>,
    field: &str,
) -> Result<Carried<'a>, KError> {
    match target {
        // NEWTYPE fall-through. A record-repr newtype (an ex-struct) wraps a
        // `KObject::Record`; read the field straight off it, naming the nominal type in the
        // miss diagnostic so `b.z` on a `Point` still reports `Point`. `Wrapped.inner` is
        // invariantly not a `Wrapped` (the construction-time collapse rule peels any
        // `Wrapped` before re-wrapping), so a scalar inner (a NEWTYPE-over-`Number`, which
        // has no fields) falls to the `other` arm.
        KObject::Wrapped { inner, type_id } => match inner.get() {
            KObject::Record(values, _) => match values.get(field) {
                Some(Held::Object(value)) => Ok(Carried::Object(
                    scope.region.alloc_object(value.deep_clone()),
                )),
                Some(Held::Type(kt)) => Ok(Carried::Type(scope.region.alloc_ktype(kt.clone()))),
                None => Err(KError::new(KErrorKind::ShapeError(format!(
                    "`{}` has no field `{}`",
                    type_id.name(),
                    field
                )))),
            },
            inner => access_field(scope, inner, field),
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
fn access_module_member<'a>(m: &'a Module<'a>, field: &str) -> Result<Carried<'a>, KError> {
    let module_scope = m.child_scope();
    if let Some(kt) = m.type_members.borrow().get(field).cloned() {
        return Ok(Carried::Type(module_scope.region.alloc_ktype(kt)));
    }
    if let Some(Resolution::Value(obj)) = module_scope.bindings().lookup_value(field, None) {
        if let Some(tag) = m.slot_type_tags.borrow().get(field).cloned() {
            let type_id = module_scope.region.alloc_ktype(tag);
            return Ok(Carried::Object(module_scope.region.alloc_object(
                KObject::Wrapped {
                    inner: NonWrappedRef::peel(obj),
                    type_id,
                },
            )));
        }
        return Ok(Carried::Object(obj));
    }
    if let Some(kt) = module_scope.resolve_type(field) {
        return Ok(Carried::Type(module_scope.region.alloc_ktype(kt.clone())));
    }
    Err(KError::new(KErrorKind::ShapeError(format!(
        "module `{}` has no member `{}`",
        m.path, field
    ))))
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
