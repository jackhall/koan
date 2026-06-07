//! `ATTR <s> <field:Identifier>` — newtype (record-repr or scalar), module, or signature
//! field access. Surface syntax is the `.` infix operator. Overloads share the bucket
//! `[Keyword, Slot, Slot]` and pick by lhs shape: [`body_identifier`] for `p.x` where
//! the lhs is still an `Identifier`, [`body_newtype`] for a `Wrapped` lhs (a record-repr
//! newtype's `.x` reads through to the wrapped record), [`body_module`] for chained module
//! access.
//!
//! The slot types are disjoint (`KType::Identifier` only matches `ExpressionPart::Identifier`;
//! the `AnyUserType { kind: Newtype }` slot admits a `KObject::Wrapped`, and `AnyModule`
//! admits modules), so dispatch picks unambiguously without a specificity tiebreaker.

use crate::machine::execute::{resolve_type_leaf_carrier, TypeLeafCarrier};
use crate::machine::model::types::{AbstractSource, NominalKind};
use crate::machine::model::values::{Module, NonWrappedRef};
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BodyResult, KError, KErrorKind, Resolution, SchedulerHandle, Scope,
};

use super::{arg, err, kw, register_builtin, sig};

pub fn body_identifier<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let s_name = match bundle.get("s") {
        Some(KObject::KString(s)) => s.clone(),
        Some(other) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "s".to_string(),
                expected: "Identifier".to_string(),
                got: other.ktype().name().to_string(),
            }));
        }
        None => return err(KError::new(KErrorKind::MissingArg("s".to_string()))),
    };
    let field_name = read_field_name(&bundle);
    let field_name = match field_name {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    // Value-side first, then type-side: a FUNCTOR's signature-typed parameter is bound
    // only into `bindings.types`, so `Er.pure(x)` inside the functor body must reach the
    // carried `&Module` through `resolve_type`.
    if let Some(target) = scope.lookup(&s_name) {
        return access_field(scope, target, &field_name);
    }
    if let Some(kt) = scope.resolve_type(&s_name) {
        match kt {
            KType::Module { module: m, .. } => return access_module_member(m, &field_name),
            KType::AbstractType {
                source: AbstractSource::Module(m),
                ..
            } => {
                return access_module_member(m, &field_name);
            }
            // A `Sig`-rooted abstract type has no module to project a member off; fall
            // through to UnboundName like the scalar arms below.
            KType::AbstractType { .. } => {}
            // Scalar type-side bindings have no members; fall through to UnboundName so
            // the diagnostic stays attributed to the lhs identifier rather than the
            // type-language side.
            _ => {}
        }
    }
    err(KError::new(KErrorKind::UnboundName(s_name)))
}

/// `ATTR <s:TypeExprRef> <field:_>` — entry for a Type-classed lhs. Module names are
/// Type-classed tokens (see [token classes](../../design/typing/tokens.md)), so `Foo.x`
/// parses as `[ATTR Type(Foo) Identifier(x)]` instead of the `Identifier`-lhs the struct
/// path uses. The Type-Type overload shares this body so chained module access
/// (`Outer.Inner.x`) works regardless of whether each step's field is a module name or
/// a regular member.
pub fn body_type_lhs<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let s_name = match bundle.get("s") {
        Some(KObject::KTypeValue(t)) => t.name(),
        Some(KObject::TypeNameRef(t)) => t.render(),
        Some(other) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "s".to_string(),
                expected: "TypeExprRef".to_string(),
                got: other.ktype().name().to_string(),
            }));
        }
        None => return err(KError::new(KErrorKind::MissingArg("s".to_string()))),
    };
    let field_name = match read_field_name(&bundle) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    // The Type-classed lhs is a nominal type-side binding. A module / signature identity
    // resolves to a `KTypeValue(Module/Signature)` carrier `access_field` projects a member
    // off; a struct / union name resolves to a `KTypeValue(SetRef)` that `access_field`
    // rejects with the same TypeMismatch a static struct field access produces. Dispatch
    // resolves this ATTR type argument before the body runs, so a `Park` outcome is
    // unreachable here and surfaces as a loud structured error rather than a silent stall.
    let leaf = crate::machine::model::ast::TypeName::leaf(s_name);
    let target = match resolve_type_leaf_carrier(scope, &leaf, None) {
        TypeLeafCarrier::Resolved(obj) => obj,
        TypeLeafCarrier::Unbound(name) => return err(KError::new(KErrorKind::UnboundName(name))),
        TypeLeafCarrier::Park(producers) => {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "ATTR lhs type `{}` resolved to a still-finalizing type \
                 (parked on {} producer(s)); the type argument should already be sealed \
                 at body entry",
                leaf.render(),
                producers.len(),
            ))))
        }
    };
    access_field(scope, target, &field_name)
}

pub fn body_newtype<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let target = match bundle.require("s") {
        Ok(obj) => obj,
        Err(e) => return err(e),
    };
    let field_name = match read_field_name(&bundle) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    access_field(scope, target, &field_name)
}

pub fn body_module<'a>(
    _scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let target = match bundle.require("s") {
        Ok(obj) => obj,
        Err(e) => return err(e),
    };
    let field_name = match read_field_name(&bundle) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    let m = match target.as_module() {
        Some(m) => m,
        None => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "s".to_string(),
                expected: "Module".to_string(),
                got: target.ktype().name(),
            }))
        }
    };
    access_module_member(m, &field_name)
}

fn read_field_name<'a>(bundle: &ArgumentBundle<'a>) -> Result<String, KError> {
    match bundle.get("field") {
        Some(KObject::KString(s)) => Ok(s.clone()),
        Some(KObject::KTypeValue(t)) => Ok(t.name()),
        Some(KObject::TypeNameRef(t)) => Ok(t.render()),
        Some(other) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: "field".to_string(),
            expected: "Identifier".to_string(),
            got: other.ktype().name().to_string(),
        })),
        None => Err(KError::new(KErrorKind::MissingArg("field".to_string()))),
    }
}

fn access_field<'a>(scope: &'a Scope<'a>, target: &KObject<'a>, field: &str) -> BodyResult<'a> {
    match target {
        KObject::KTypeValue(KType::Module { module: m, .. }) => access_module_member(m, field),
        // ATTR over a first-class signature value — reverse-lookup against the decl scope.
        KObject::KTypeValue(KType::Signature { sig: s, .. }) => {
            let scope = s.decl_scope();
            if let Some(Resolution::Value(obj)) = scope.bindings().lookup_value(field, None) {
                return BodyResult::Value(obj);
            }
            if let Some(kt) = scope.resolve_type(field) {
                return BodyResult::Value(
                    scope.arena.alloc_object(KObject::KTypeValue(kt.clone())),
                );
            }
            err(KError::new(KErrorKind::ShapeError(format!(
                "signature `{}` has no member `{}`",
                s.path, field
            ))))
        }
        // ATTR over an opaque-ascription abstract type — project against the source module.
        // A `Sig`-rooted abstract type has no module to project off, so it falls through to
        // the `other` TypeMismatch arm.
        KObject::KTypeValue(KType::AbstractType {
            source: AbstractSource::Module(m),
            ..
        }) => access_module_member(m, field),
        // NEWTYPE fall-through. A record-repr newtype (an ex-struct) wraps a
        // `KObject::Record`; read the field straight off it, naming the nominal type in the
        // miss diagnostic so `b.z` on a `Point` still reports `Point`. `Wrapped.inner` is
        // invariantly not a `Wrapped` (the construction-time collapse rule peels any
        // `Wrapped` before re-wrapping), so a scalar inner (a NEWTYPE-over-`Number`, which
        // has no fields) falls to the `other` arm.
        KObject::Wrapped { inner, type_id } => match inner.get() {
            KObject::Record(values, _) => match values.get(field) {
                Some(value) => BodyResult::Value(scope.arena.alloc_object(value.deep_clone())),
                None => err(KError::new(KErrorKind::ShapeError(format!(
                    "`{}` has no field `{}`",
                    type_id.name(),
                    field
                )))),
            },
            inner => access_field(scope, inner, field),
        },
        other => err(KError::new(KErrorKind::TypeMismatch {
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
/// The re-tag carrier (and its `type_id`) is alloc'd in the *module*'s arena, not the
/// read-site `scope`'s: `Wrapped::deep_clone` is shallow (the NEWTYPE invariant that
/// `type_id` is a declaration-stable `&'a KType`), so the `type_id` must outlive any
/// lift/deep-clone of the read value — e.g. a functor body's `(Er.zero)` whose read-site
/// scope is a per-call arena. The module and its `slot_type_tags` are declaration-stable,
/// so the module arena is the right home; both `inner` (the slot value) and `type_id`
/// (the abstract tag, which references the module) then live there together.
fn access_module_member<'a>(m: &'a Module<'a>, field: &str) -> BodyResult<'a> {
    let module_scope = m.child_scope();
    if let Some(kt) = m.type_members.borrow().get(field).cloned() {
        return BodyResult::Value(module_scope.arena.alloc_object(KObject::KTypeValue(kt)));
    }
    if let Some(Resolution::Value(obj)) = module_scope.bindings().lookup_value(field, None) {
        if let Some(tag) = m.slot_type_tags.borrow().get(field).cloned() {
            let type_id = module_scope.arena.alloc_ktype(tag);
            return BodyResult::Value(module_scope.arena.alloc_object(KObject::Wrapped {
                inner: NonWrappedRef::peel(obj),
                type_id,
            }));
        }
        return BodyResult::Value(obj);
    }
    if let Some(kt) = module_scope.resolve_type(field) {
        return BodyResult::Value(
            module_scope
                .arena
                .alloc_object(KObject::KTypeValue(kt.clone())),
        );
    }
    err(KError::new(KErrorKind::ShapeError(format!(
        "module `{}` has no member `{}`",
        m.path, field
    ))))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let module_ty = KType::AnyModule;
    let newtype_ty = KType::AnyUserType {
        kind: NominalKind::Newtype,
    };

    register_builtin(
        scope,
        "ATTR",
        sig(
            KType::Any,
            vec![
                kw("ATTR"),
                arg("s", KType::Identifier),
                arg("field", KType::Identifier),
            ],
        ),
        body_identifier,
    );
    register_builtin(
        scope,
        "ATTR",
        sig(
            KType::Any,
            vec![
                kw("ATTR"),
                arg("s", module_ty.clone()),
                arg("field", KType::Identifier),
            ],
        ),
        body_module,
    );
    // NEWTYPE fall-through, including ex-structs. The `AnyUserType { kind: Newtype }` slot
    // admits any `KObject::Wrapped` (the wildcard keys on `NominalKind`, never the repr).
    // `access_field`'s `Wrapped` arm reads a record repr's field directly and recurses one
    // level for any other inner.
    register_builtin(
        scope,
        "ATTR",
        sig(
            KType::Any,
            vec![
                kw("ATTR"),
                arg("s", newtype_ty),
                arg("field", KType::Identifier),
            ],
        ),
        body_newtype,
    );
    register_builtin(
        scope,
        "ATTR",
        sig(
            KType::Any,
            vec![
                kw("ATTR"),
                arg("s", KType::TypeExprRef),
                arg("field", KType::Identifier),
            ],
        ),
        body_type_lhs,
    );
    register_builtin(
        scope,
        "ATTR",
        sig(
            KType::Any,
            vec![
                kw("ATTR"),
                arg("s", KType::TypeExprRef),
                arg("field", KType::TypeExprRef),
            ],
        ),
        body_type_lhs,
    );
    // Module lhs with a Type-classed field (e.g. the `Outer.Inner` step in `Outer.Inner.x`).
    register_builtin(
        scope,
        "ATTR",
        sig(
            KType::Any,
            vec![
                kw("ATTR"),
                arg("s", module_ty),
                arg("field", KType::TypeExprRef),
            ],
        ),
        body_module,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::machine::model::KObject;
    use crate::machine::{KErrorKind, RuntimeArena};

    #[test]
    fn attr_reads_field_from_named_struct() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "NEWTYPE Point = :{x :Number, y :Number}\nLET p = (Point {x = 3, y = 4})",
        );
        let result = run_one(scope, parse_one("p.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 3.0));
    }

    #[test]
    fn attr_reads_each_field_independently() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "NEWTYPE Point = :{x :Number, y :Number}\nLET p = (Point {x = 3, y = 4})",
        );
        assert!(matches!(run_one(scope, parse_one("p.x")), KObject::Number(n) if *n == 3.0));
        assert!(matches!(run_one(scope, parse_one("p.y")), KObject::Number(n) if *n == 4.0));
    }

    #[test]
    fn attr_chained_through_nested_struct() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("ghost.x"));
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(name) if name == "ghost"),
            "expected UnboundName(\"ghost\"), got {err}",
        );
    }

    #[test]
    fn attr_on_non_struct_value_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
    /// `IntOrdView.zero` reads as the abstract `Type` (`ktype().name() == "Type"`), not the
    /// underlying `Number`, so a deferred return `Er.Type` accepts the body.
    #[test]
    fn opaque_view_slot_read_re_tags_with_abstract_type() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG WithZero = ((LET Type = Number) (VAL zero :Type))\n\
             MODULE IntOrd = ((LET Type = Number) (LET zero = 0))\n\
             LET IntOrdView = (IntOrd :| WithZero)",
        );
        let result = run_one(scope, parse_one("IntOrdView.zero"));
        assert_eq!(
            result.ktype().name(),
            "Type",
            "opaque-view slot read must carry the abstract `Type` identity, got {:?}",
            result.ktype(),
        );
    }

    /// Transparent (`:!`) views leave `slot_type_tags` empty, so the slot read stays
    /// concrete: `IntOrdView.zero` reads as the underlying `Number`, not the abstract `Type`.
    #[test]
    fn transparent_view_slot_read_stays_concrete() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG WithZero = ((LET Type = Number) (VAL zero :Type))\n\
             MODULE IntOrd = ((LET Type = Number) (LET zero = 0))\n\
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
