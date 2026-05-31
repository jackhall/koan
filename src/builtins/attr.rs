//! `ATTR <s> <field:Identifier>` — struct, module, or NEWTYPE field access. Surface
//! syntax is the `.` infix operator. Overloads share the bucket
//! `[Keyword, Slot, Slot]` and pick by lhs shape: [`body_identifier`] for `p.x` where
//! the lhs is still an `Identifier`, [`body_struct`] for chained struct access and the
//! NEWTYPE fall-through, [`body_module`] for chained module access.
//!
//! The slot types are disjoint (`KType::Identifier` only matches `ExpressionPart::Identifier`;
//! the three `AnyUserType { kind: Struct | Module | Newtype { .. } }` slots each admit a
//! distinct `KObject` family via the manual `UserTypeKind::PartialEq`), so dispatch picks
//! unambiguously without a specificity tiebreaker.

use crate::machine::model::{KObject, KType};
use crate::machine::model::types::UserTypeKind;
use crate::machine::model::values::Module;
use crate::machine::execute::coerce_type_token_value;
use crate::machine::{
    ArgumentBundle, BodyResult, KError, KErrorKind, Resolution, Scope, SchedulerHandle,
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
            KType::AbstractType { source_module, .. } => {
                return access_module_member(source_module, &field_name);
            }
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
        Some(KObject::TypeNameRef(t)) => t.name.clone(),
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
    // The Type-classed lhs is a nominal type-side binding. Modules / signatures keep
    // a value-side carrier (`KTypeValue(Module/Signature)`), which `coerce_type_token_value`
    // recovers; struct / union names are type-only now, so it synthesizes a
    // `KTypeValue(UserType)` that `access_field` rejects with the same TypeMismatch a
    // static struct field access has always produced.
    let leaf = crate::machine::model::ast::TypeExpr::leaf(s_name);
    let target = match coerce_type_token_value(scope, &leaf, None) {
        Ok(obj) => obj,
        Err(e) => return err(e),
    };
    access_field(scope, target, &field_name)
}

pub fn body_struct<'a>(
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
        None => return err(KError::new(KErrorKind::TypeMismatch {
            arg: "s".to_string(),
            expected: "Module".to_string(),
            got: target.ktype().name(),
        })),
    };
    access_module_member(m, &field_name)
}

fn read_field_name<'a>(bundle: &ArgumentBundle<'a>) -> Result<String, KError> {
    match bundle.get("field") {
        Some(KObject::KString(s)) => Ok(s.clone()),
        Some(KObject::KTypeValue(t)) => Ok(t.name()),
        Some(KObject::TypeNameRef(t)) => Ok(t.name.clone()),
        Some(other) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: "field".to_string(),
            expected: "Identifier".to_string(),
            got: other.ktype().name().to_string(),
        })),
        None => Err(KError::new(KErrorKind::MissingArg("field".to_string()))),
    }
}

fn access_field<'a>(
    scope: &'a Scope<'a>,
    target: &KObject<'a>,
    field: &str,
) -> BodyResult<'a> {
    match target {
        KObject::Struct { name: type_name, fields, .. } => match fields.get(field) {
            Some(value) => BodyResult::Value(scope.arena.alloc(value.deep_clone())),
            None => err(KError::new(KErrorKind::ShapeError(format!(
                "struct `{}` has no field `{}`",
                type_name, field
            )))),
        },
        KObject::KTypeValue(KType::Module { module: m, .. }) => access_module_member(m, field),
        // ATTR over a first-class signature value — reverse-lookup against the decl scope.
        KObject::KTypeValue(KType::Signature(s)) => {
            let scope = s.decl_scope();
            if let Some(Resolution::Value(obj)) = scope.bindings().lookup_value(field, None) {
                return BodyResult::Value(obj);
            }
            if let Some(kt) = scope.resolve_type(field) {
                return BodyResult::Value(
                    scope.arena.alloc(KObject::KTypeValue(kt.clone())),
                );
            }
            err(KError::new(KErrorKind::ShapeError(format!(
                "signature `{}` has no member `{}`",
                s.path, field
            ))))
        }
        // ATTR over an opaque-ascription abstract type — project against the source module.
        KObject::KTypeValue(KType::AbstractType { source_module, .. }) => {
            access_module_member(source_module, field)
        }
        // NEWTYPE fall-through. `Wrapped.inner` is invariantly not a `Wrapped` (the
        // construction-time collapse rule in `super::newtype_def::newtype_construct`
        // peels any `Wrapped` before re-wrapping), so this recurses exactly one level.
        KObject::Wrapped { inner, .. } => access_field(scope, inner.get(), field),
        other => err(KError::new(KErrorKind::TypeMismatch {
            arg: "s".to_string(),
            expected: "Struct".to_string(),
            got: other.ktype().name().to_string(),
        })),
    }
}

/// Look `field` up inside a [`Module`]'s child scope: opaque-ascription `type_members`,
/// then value-side `data`, then type-side `bindings.types`.
///
/// Preferring `data` over `bindings.types` matters for nominal binders like
/// `MODULE Sub = (...)` and `STRUCT P = (...)`, which install into both: chained access
/// `Outer.Inner.X` needs the inner *module value* from `data`, not its type identity,
/// so the next ATTR step can recurse into the inner module's child scope.
fn access_module_member<'a>(m: &'a Module<'a>, field: &str) -> BodyResult<'a> {
    if let Some(kt) = m.type_members.borrow().get(field).cloned() {
        return BodyResult::Value(
            m.child_scope().arena.alloc(KObject::KTypeValue(kt)),
        );
    }
    let scope = m.child_scope();
    if let Some(Resolution::Value(obj)) = scope.bindings().lookup_value(field, None) {
        return BodyResult::Value(obj);
    }
    if let Some(kt) = scope.resolve_type(field) {
        return BodyResult::Value(
            scope.arena.alloc(KObject::KTypeValue(kt.clone())),
        );
    }
    err(KError::new(KErrorKind::ShapeError(format!(
        "module `{}` has no member `{}`",
        m.path, field
    ))))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let struct_ty = KType::AnyUserType { kind: UserTypeKind::struct_sentinel() };
    let module_ty = KType::AnyModule;
    let newtype_ty = KType::AnyUserType {
        kind: UserTypeKind::Newtype { repr: Box::new(KType::Any) },
    };

    register_builtin(scope, "ATTR",
        sig(KType::Any, vec![kw("ATTR"), arg("s", KType::Identifier), arg("field", KType::Identifier)]),
        body_identifier);
    register_builtin(scope, "ATTR",
        sig(KType::Any, vec![kw("ATTR"), arg("s", struct_ty), arg("field", KType::Identifier)]),
        body_struct);
    register_builtin(scope, "ATTR",
        sig(KType::Any, vec![kw("ATTR"), arg("s", module_ty.clone()), arg("field", KType::Identifier)]),
        body_module);
    // NEWTYPE fall-through. The wildcard `Newtype { repr: Any }` slot admits any
    // `KObject::Wrapped` (the manual `UserTypeKind::PartialEq` ignores `repr`). Reuses
    // `body_struct` because `access_field` dispatches on the lhs shape — its `Wrapped`
    // arm recurses one level into `inner`.
    register_builtin(scope, "ATTR",
        sig(KType::Any, vec![kw("ATTR"), arg("s", newtype_ty), arg("field", KType::Identifier)]),
        body_struct);
    register_builtin(scope, "ATTR",
        sig(KType::Any, vec![kw("ATTR"), arg("s", KType::TypeExprRef), arg("field", KType::Identifier)]),
        body_type_lhs);
    register_builtin(scope, "ATTR",
        sig(KType::Any, vec![kw("ATTR"), arg("s", KType::TypeExprRef), arg("field", KType::TypeExprRef)]),
        body_type_lhs);
    // Module lhs with a Type-classed field (e.g. the `Outer.Inner` step in `Outer.Inner.x`).
    register_builtin(scope, "ATTR",
        sig(KType::Any, vec![kw("ATTR"), arg("s", module_ty), arg("field", KType::TypeExprRef)]),
        body_module);
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{
        parse_one, run, run_one, run_one_err, run_root_silent,
    };
    use crate::machine::model::KObject;
    use crate::machine::{KErrorKind, RuntimeArena};

    #[test]
    fn attr_reads_field_from_named_struct() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "STRUCT Point = (x :Number, y :Number)\nLET p = (Point (x = 3, y = 4))",
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
            "STRUCT Point = (x :Number, y :Number)\nLET p = (Point (x = 3, y = 4))",
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
            "STRUCT Point = (x :Number, y :Number)\n\
             STRUCT Line = (start :Struct, finish :Struct)\n\
             LET origin = (Point (x = 0, y = 0))\n\
             LET tip = (Point (x = 3, y = 4))\n\
             LET seg = (Line (start = origin, finish = tip))",
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
                assert_eq!(expected, "Struct");
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
            "STRUCT Point = (x :Number, y :Number)\nLET p = (Point (x = 3, y = 4))",
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
            "STRUCT Point = (x :Number, y :Number)\n\
             STRUCT Line = (start :Struct, finish :Struct)\n\
             LET origin = (Point (x = 0, y = 0))\n\
             LET tip = (Point (x = 3, y = 4))\n\
             LET seg = (Line (start = origin, finish = tip))",
        );
        let err = run_one_err(scope, parse_one("seg.start.bogus"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Point") && msg.contains("`bogus`")),
            "expected ShapeError naming Point and bogus on chained access, got {err}",
        );
    }

    /// `b.x` on a NEWTYPE-wrapped struct reads through to the underlying field.
    #[test]
    fn access_field_falls_through_wrapped_struct() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "STRUCT Point = (x :Number, y :Number)\n\
             NEWTYPE Boxed = Point\n\
             LET p = (Point (x = 1, y = 2))\n\
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
                assert_eq!(expected, "Struct");
                assert_eq!(got, "Number");
            }
            _ => panic!("expected TypeMismatch on NEWTYPE-over-Number field access, got {err}"),
        }
    }

    /// The fall-through preserves inner-struct error attribution: a missing field on a
    /// wrapped `Point` names `Point` in the `ShapeError`, not the wrapper.
    #[test]
    fn access_field_falls_through_wrapped_with_missing_field() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "STRUCT Point = (x :Number, y :Number)\n\
             NEWTYPE Boxed = Point\n\
             LET p = (Point (x = 1, y = 2))\n\
             LET b = (Boxed (p))",
        );
        let err = run_one_err(scope, parse_one("b.z"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Point") && msg.contains("`z`")),
            "expected ShapeError naming Point and z on Wrapped fall-through, got {err}",
        );
    }
}
