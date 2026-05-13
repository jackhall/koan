//! `ATTR <s> <field:Identifier>` — struct or module field access. Surface syntax is the `.`
//! infix operator from `operators::build_attr` (private to `crate::parse`), which compiles
//! `p.x` into `[Keyword("ATTR"), Identifier("p"), Identifier("x")]`. Three overloads share
//! the bucket `[Keyword, Slot, Slot]` and pick by lhs shape:
//!
//! - [`body_identifier`] — `p.x` form. The lhs is still an `Identifier`, so this body
//!   does the scope lookup itself, mirroring [`value_lookup`](super::value_lookup), and
//!   then dispatches to either struct-field or module-member access based on what the
//!   identifier resolved to.
//! - [`body_struct`] — chained access like `p.x.y` for structs. The inner `[ATTR p x]`
//!   evaluates first and arrives here as `Future(KObject::Struct{..})`.
//! - [`body_module`] — chained access like `M.SubModule.foo`. The inner `[ATTR M SubModule]`
//!   evaluates first and arrives here as `Future(KObject::KModule(_))`. Module-system
//!   stage 1.
//!
//! The slot types are disjoint (`KType::Identifier` only matches `ExpressionPart::Identifier`;
//! `KType::Struct` and `KType::Module` only match the corresponding `Future` variants), so
//! dispatch picks unambiguously without a specificity tiebreaker.

use crate::runtime::model::{Argument, ExpressionSignature, KObject, KType, SignatureElement};
use crate::runtime::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};

use super::{err, register_builtin};

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
    let target = match scope.lookup(&s_name) {
        Some(obj) => obj,
        None => return err(KError::new(KErrorKind::UnboundName(s_name))),
    };
    access_field(scope, target, &field_name)
}

/// `ATTR <s:TypeExprRef> <field:_>` — module-system entry point. Module names are
/// Type-classed tokens (`Foo`, `IntOrd`, `OrderedSig`) per the [token classes in
/// design/type-system.md](../../../design/type-system.md#token-classes--the-parser-level-foundation),
/// so `Foo.x` parses as
/// `[ATTR Type(Foo) Identifier(x)]` rather than the `Identifier`-lhs the struct path uses.
/// `Foo.SubModule` parses as `[ATTR Type(Foo) Type(SubModule)]` — the Type-Type overload
/// shares this body so chained module access (`Outer.Inner.x`) works regardless of whether
/// the field at each step is a module name or a regular member. Resolves the type name in
/// the surrounding scope and dispatches to `access_field` (which routes to module-member
/// access when the resolved value is a module).
pub fn body_type_lhs<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let s_name = match bundle.get("s") {
        // Post-`KTypeValue` migration: the lhs's surface name is `KType::name()`. For a
        // user-typed `Foo.x`, the parser-side `resolve_for` lifted `Foo` to
        // `KTypeValue(KType::ModuleType { name: "Foo", .. })` or a similarly leaf-named
        // variant; `name()` returns the user-facing identifier in either case.
        Some(KObject::KTypeValue(t)) => t.name(),
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
    let target = match scope.lookup(&s_name) {
        Some(obj) => obj,
        None => return err(KError::new(KErrorKind::UnboundName(s_name))),
    };
    access_field(scope, target, &field_name)
}

pub fn body_struct<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let target = match bundle.get("s") {
        Some(obj) => obj,
        None => return err(KError::new(KErrorKind::MissingArg("s".to_string()))),
    };
    let field_name = match read_field_name(&bundle) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    access_field(scope, target, &field_name)
}

/// Module-member access. The lhs already resolved to a `KObject::KModule`; look `field` up
/// in the module's child scope's `data` map. Module-system stage 1.
pub fn body_module<'a>(
    _scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let target = match bundle.get("s") {
        Some(obj) => obj,
        None => return err(KError::new(KErrorKind::MissingArg("s".to_string()))),
    };
    let field_name = match read_field_name(&bundle) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    access_module_member(target, &field_name)
}

fn read_field_name<'a>(bundle: &ArgumentBundle<'a>) -> Result<String, KError> {
    match bundle.get("field") {
        Some(KObject::KString(s)) => Ok(s.clone()),
        // Module-system stage 1: a Type-classed field (e.g. `Foo.SubModule.x`) lands here as
        // a `KTypeValue`. `name()` returns the bare leaf identifier — same shape as the
        // Identifier path.
        Some(KObject::KTypeValue(t)) => Ok(t.name()),
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
        KObject::Struct { type_name, fields } => match fields.get(field) {
            Some(value) => BodyResult::Value(scope.arena.alloc_object(value.deep_clone())),
            None => err(KError::new(KErrorKind::ShapeError(format!(
                "struct `{}` has no field `{}`",
                type_name, field
            )))),
        },
        // The identifier resolved to a module — `IntOrd.compare`, `OrderedSig.Type`, etc.
        // Module-system stage 1.
        KObject::KModule(_, _) => access_module_member(target, field),
        other => err(KError::new(KErrorKind::TypeMismatch {
            arg: "s".to_string(),
            expected: "Struct".to_string(),
            got: other.ktype().name().to_string(),
        })),
    }
}

/// Look `field` up inside a [`KObject::KModule`]'s child scope. Tries the module's
/// `type_members` table first (so `Foo.Type` resolves to a `KType` value when present), then
/// the child scope's `data` (LET/FN bindings under the module body). Returns a clean
/// `ShapeError` naming the module's path and the missing member when neither finds anything.
fn access_module_member<'a>(target: &KObject<'a>, field: &str) -> BodyResult<'a> {
    let Some(m) = target.as_module() else {
        return err(KError::new(KErrorKind::TypeMismatch {
            arg: "s".to_string(),
            expected: "Module".to_string(),
            got: target.ktype().name().to_string(),
        }));
    };
    // Type-position fallback: opaque ascription's `type_members` map (e.g., `IntOrd.Type`
    // resolves to a `KType::ModuleType`). The stored `KType` is the abstract type's actual
    // identity — return it directly as a `KTypeValue` so identity comparisons downstream
    // see the per-module `{ scope_id, name }` rather than a freshly elaborated leaf.
    if let Some(kt) = m.type_members.borrow().get(field).cloned() {
        return BodyResult::Value(
            m.child_scope().arena.alloc_object(KObject::KTypeValue(kt)),
        );
    }
    let scope = m.child_scope();
    if let Some(obj) = scope.bindings().data().get(field).copied() {
        return BodyResult::Value(obj);
    }
    err(KError::new(KErrorKind::ShapeError(format!(
        "module `{}` has no member `{}`",
        m.path, field
    ))))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "ATTR",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("ATTR".into()),
                SignatureElement::Argument(Argument { name: "s".into(),     ktype: KType::Identifier }),
                SignatureElement::Argument(Argument { name: "field".into(), ktype: KType::Identifier }),
            ],
        },
        body_identifier,
    );
    register_builtin(
        scope,
        "ATTR",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("ATTR".into()),
                SignatureElement::Argument(Argument { name: "s".into(),     ktype: KType::Struct }),
                SignatureElement::Argument(Argument { name: "field".into(), ktype: KType::Identifier }),
            ],
        },
        body_struct,
    );
    register_builtin(
        scope,
        "ATTR",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("ATTR".into()),
                SignatureElement::Argument(Argument { name: "s".into(),     ktype: KType::Module }),
                SignatureElement::Argument(Argument { name: "field".into(), ktype: KType::Identifier }),
            ],
        },
        body_module,
    );
    register_builtin(
        scope,
        "ATTR",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("ATTR".into()),
                SignatureElement::Argument(Argument { name: "s".into(),     ktype: KType::TypeExprRef }),
                SignatureElement::Argument(Argument { name: "field".into(), ktype: KType::Identifier }),
            ],
        },
        body_type_lhs,
    );
    register_builtin(
        scope,
        "ATTR",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("ATTR".into()),
                SignatureElement::Argument(Argument { name: "s".into(),     ktype: KType::TypeExprRef }),
                SignatureElement::Argument(Argument { name: "field".into(), ktype: KType::TypeExprRef }),
            ],
        },
        body_type_lhs,
    );
    // Chained access where the lhs is a module value (`Outer.Inner.x` after the inner
    // resolves) and the field is itself a Type token (`Outer.Inner` step in `Outer.Inner.x`).
    register_builtin(
        scope,
        "ATTR",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("ATTR".into()),
                SignatureElement::Argument(Argument { name: "s".into(),     ktype: KType::Module }),
                SignatureElement::Argument(Argument { name: "field".into(), ktype: KType::TypeExprRef }),
            ],
        },
        body_module,
    );
}

#[cfg(test)]
mod tests {
    use crate::runtime::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::runtime::model::KObject;
    use crate::runtime::machine::{KErrorKind, RuntimeArena};

    #[test]
    fn attr_reads_field_from_named_struct() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "STRUCT Point = (x: Number, y: Number)\nLET p = (Point (x: 3, y: 4))",
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
            "STRUCT Point = (x: Number, y: Number)\nLET p = (Point (x: 3, y: 4))",
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
            "STRUCT Point = (x: Number, y: Number)\n\
             STRUCT Line = (start: Struct, finish: Struct)\n\
             LET origin = (Point (x: 0, y: 0))\n\
             LET tip = (Point (x: 3, y: 4))\n\
             LET seg = (Line (start: origin, finish: tip))",
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
            "STRUCT Point = (x: Number, y: Number)\nLET p = (Point (x: 3, y: 4))",
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
            "STRUCT Point = (x: Number, y: Number)\n\
             STRUCT Line = (start: Struct, finish: Struct)\n\
             LET origin = (Point (x: 0, y: 0))\n\
             LET tip = (Point (x: 3, y: 4))\n\
             LET seg = (Line (start: origin, finish: tip))",
        );
        let err = run_one_err(scope, parse_one("seg.start.bogus"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Point") && msg.contains("`bogus`")),
            "expected ShapeError naming Point and bogus on chained access, got {err}",
        );
    }
}
