//! `ATTR <s> <field:Identifier>` — struct or module field access. Surface syntax is the `.`
//! infix operator from [`operators::build_attr`](crate::parse::operators), which compiles
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

use crate::dispatch::runtime::{KError, KErrorKind};
use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::values::KObject;
use crate::dispatch::runtime::Scope;

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
        Some(KObject::TypeExprValue(t)) => t.name.clone(),
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
        // a `TypeExprValue`. The bare leaf name is what we need to look up in the enclosing
        // module's scope — same as the Identifier path.
        Some(KObject::TypeExprValue(t)) => Ok(t.name.clone()),
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
        KObject::KModule(_) => access_module_member(target, field),
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
    let m = match target {
        KObject::KModule(m) => *m,
        other => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "s".to_string(),
                expected: "Module".to_string(),
                got: other.ktype().name().to_string(),
            }));
        }
    };
    // Type-position fallback: opaque ascription's `type_members` map (e.g., `IntOrd.Type`
    // resolves to a `KType::ModuleType`). Returned as a `KObject::TypeExprValue` so the
    // value flows through any "expects a type" position — same as a bare type-token.
    if let Some(_kt) = m.type_members.borrow().get(field).cloned() {
        // For now, return a TypeExprValue carrying the abstract type's surface name. The
        // ascription stage 1 path mints fresh ModuleType variants here; consumers reading
        // back the type should compare via `KType::matches_value`.
        let te = crate::parse::kexpression::TypeExpr::leaf(field.to_string());
        return BodyResult::Value(
            m.child_scope().arena.alloc_object(KObject::TypeExprValue(te)),
        );
    }
    let scope = m.child_scope();
    if let Some(obj) = scope.data.borrow().get(field).copied() {
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
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    use crate::dispatch::runtime::RuntimeArena;
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::runtime::KErrorKind;
    use crate::dispatch::values::KObject;
    use crate::dispatch::runtime::Scope;
    use crate::execute::scheduler::Scheduler;
    use crate::parse::expression_tree::parse;
    use crate::parse::kexpression::KExpression;

    struct SharedBuf(Rc<RefCell<Vec<u8>>>);
    impl Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    fn build_scope<'a>(arena: &'a RuntimeArena, captured: Rc<RefCell<Vec<u8>>>) -> &'a Scope<'a> {
        default_scope(arena, Box::new(SharedBuf(captured)))
    }

    fn parse_one(src: &str) -> KExpression<'static> {
        let mut exprs = parse(src).expect("parse should succeed");
        assert_eq!(exprs.len(), 1, "test helper expects a single expression");
        exprs.remove(0)
    }

    fn run<'a>(scope: &'a Scope<'a>, source: &str) {
        let exprs = parse(source).expect("parse should succeed");
        let mut sched = Scheduler::new();
        for expr in exprs {
            sched.add_dispatch(expr, scope);
        }
        sched.execute().expect("scheduler should succeed");
    }

    fn run_one<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> &'a KObject<'a> {
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().expect("scheduler should succeed");
        sched.read(id)
    }

    fn run_one_err<'a>(
        scope: &'a Scope<'a>,
        expr: KExpression<'a>,
    ) -> crate::dispatch::runtime::KError {
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().expect("scheduler should not surface errors directly");
        match sched.read_result(id) {
            Ok(_) => panic!("expected error"),
            Err(e) => e.clone(),
        }
    }

    #[test]
    fn attr_reads_field_from_named_struct() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
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
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
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
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
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
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let err = run_one_err(scope, parse_one("ghost.x"));
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(name) if name == "ghost"),
            "expected UnboundName(\"ghost\"), got {err}",
        );
    }

    #[test]
    fn attr_on_non_struct_value_errors() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
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
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
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
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
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
