//! Type-constructor builtins — `LIST_OF`, `DICT_OF`, `FUNCTION_OF`, `MODULE_TYPE_OF`. These
//! are ordinary scheduled `KFunction`s whose inputs are `TypeExprRef`-typed slots and whose
//! outputs are `KObject::TypeExprValue` carrying a structured `TypeExpr`. Dispatching them
//! through the same `Dispatch` / `Bind` machinery values use means a parameterized type can
//! be assembled by sub-expression evaluation: `(LIST_OF (MODULE_TYPE_OF M Type))` wakes the
//! outer slot only after the inner sub-dispatch resolves to a concrete `Type` value.
//!
//! **Why builtins rather than a parallel registration table.** The plan in
//! [roadmap/module-system-2-scheduler.md](../../../roadmap/module-system-2-scheduler.md)
//! reduces type-expression evaluation to ordinary dispatch: the same scope-lookup chain,
//! the same `Bind`-waits-for-subs refinement, the same `lift_kobject` rules. No new node
//! kind, no `KType::TypeVar`, no second registration table — a `TypeExprRef`-typed binding
//! lives in `Scope::data` like any other value.
//!
//! The output of these builtins is `KObject::TypeExprValue` (carrying the surface `TypeExpr`)
//! rather than a `KType`. Consumers that need a concrete `KType` lower the structured value
//! via `KType::from_type_expr` at the point they need to dispatch on it.

use crate::dispatch::{
    Argument, ArgumentBundle, BodyResult, ExpressionSignature, KError, KErrorKind, KObject, KType,
    Scope, SchedulerHandle, SignatureElement,
};
use crate::dispatch::values::resolve_module;
use crate::parse::{TypeExpr, TypeParams};

use super::{err, register_builtin};

/// Pull a `KObject::TypeExprValue`'s inner `TypeExpr` out of an arg slot. The slot is
/// declared `KType::TypeExprRef`, so by `Argument::matches` shape-time it must be either
/// an `ExpressionPart::Type(_)` (resolved into `TypeExprValue` by `resolve_for`) or — once
/// scheduled type-builtins are in flight — a `Future(KObject::TypeExprValue(_))` lifted
/// from a previous dispatch. Anything else reaching here is a `TypeMismatch` from the
/// dispatcher's perspective; surface that as a clean error.
fn read_type_expr<'a>(bundle: &ArgumentBundle<'a>, name: &str) -> Result<TypeExpr, KError> {
    let Some(obj) = bundle.get(name) else {
        return Err(KError::new(KErrorKind::MissingArg(name.to_string())));
    };
    if let Some(t) = obj.as_type_expr() {
        return Ok(t.clone());
    }
    Err(KError::new(KErrorKind::TypeMismatch {
        arg: name.to_string(),
        expected: "TypeExprRef".to_string(),
        got: obj.ktype().name(),
    }))
}

/// `LIST_OF <elem:TypeExprRef>` → `TypeExprRef` carrying `List<elem>`. The output has its
/// `params` field set to `TypeParams::List([elem])`, so a downstream `KType::from_type_expr`
/// produces `KType::List(Box<inner>)`.
pub fn body_list_of<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let elem = match read_type_expr(&bundle, "elem") {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    let result = TypeExpr {
        name: "List".to_string(),
        params: TypeParams::List(vec![elem]),
    };
    BodyResult::Value(scope.arena.alloc_object(KObject::TypeExprValue(result)))
}

/// `DICT_OF <key:TypeExprRef> <value:TypeExprRef>` → `TypeExprRef` carrying `Dict<key, value>`.
pub fn body_dict_of<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let key = match read_type_expr(&bundle, "key") {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    let value = match read_type_expr(&bundle, "value") {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    let result = TypeExpr {
        name: "Dict".to_string(),
        params: TypeParams::List(vec![key, value]),
    };
    BodyResult::Value(scope.arena.alloc_object(KObject::TypeExprValue(result)))
}

/// `FUNCTION_OF <args:KExpression> -> <ret:TypeExprRef>` → `TypeExprRef` carrying
/// `Function<(args) -> ret>`. The `args` slot is captured raw as a `KExpression` whose
/// parts are bare `Type(_)` tokens; we re-extract them here. This keeps the surface of
/// `FUNCTION_OF` matching the `Function<(...)-> R>` user syntax — args parenthesized,
/// return type after the arrow.
pub fn body_function_of<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    use crate::parse::ExpressionPart;
    let args_expr = match bundle.get("args") {
        Some(obj) => match obj.as_kexpression() {
            Some(e) => e.clone(),
            None => {
                return err(KError::new(KErrorKind::TypeMismatch {
                    arg: "args".to_string(),
                    expected: "KExpression".to_string(),
                    got: obj.ktype().name(),
                }));
            }
        },
        None => return err(KError::new(KErrorKind::MissingArg("args".to_string()))),
    };
    let ret = match read_type_expr(&bundle, "ret") {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    let mut args: Vec<TypeExpr> = Vec::with_capacity(args_expr.parts.len());
    for part in &args_expr.parts {
        match part {
            ExpressionPart::Type(t) => args.push(t.clone()),
            ExpressionPart::Future(KObject::TypeExprValue(t)) => args.push(t.clone()),
            other => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "FUNCTION_OF args must be type names, got `{}`",
                    other.summarize()
                ))));
            }
        }
    }
    let result = TypeExpr {
        name: "Function".to_string(),
        params: TypeParams::Function {
            args,
            ret: Box::new(ret),
        },
    };
    BodyResult::Value(scope.arena.alloc_object(KObject::TypeExprValue(result)))
}

/// `MODULE_TYPE_OF <m:Module> <name>` → `TypeExprRef` carrying the abstract type bound
/// under `name` in `m`'s `type_members` table. Surface analogue of `M.Type`, but reachable
/// as a scheduled call so a functor body can synthesize it from a parameter module value.
/// The `m` slot is strictly `Module`; bare Type-token operands (`MODULE_TYPE_OF Foo Type`)
/// ride the auto-wrap rails — they sub-dispatch through `value_lookup` and arrive here
/// as a `Future(KModule)`. The shared [`crate::dispatch::values::resolve_module`] helper
/// covers both the direct `KModule` path and the `(KModule, frame)` lifted form.
pub fn body_module_type_of<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let m = match bundle.get("m") {
        Some(obj) => match resolve_module(obj, "m") {
            Ok(m) => m,
            Err(e) => return err(e),
        },
        None => return err(KError::new(KErrorKind::MissingArg("m".to_string()))),
    };
    // The `name` slot accepts a Type token (e.g. `Type`, `Elt`) — abstract type names
    // classify as Type per the token-classification rules, not Identifier. The lookup uses the bare
    // leaf name from the resolved `TypeExpr`.
    let name = match read_type_expr(&bundle, "name") {
        Ok(t) => t.name,
        Err(e) => return err(e),
    };
    if !m.type_members.borrow().contains_key(&name) {
        return err(KError::new(KErrorKind::ShapeError(format!(
            "module `{}` has no abstract type member `{}`",
            m.path, name
        ))));
    }
    // Surface the abstract type as a leaf TypeExpr carrying `name`. Consumers that need the
    // underlying `KType::ModuleType { scope_id, name }` look it up against the module's
    // `type_members` table — same behavior as ATTR's type-position fallback.
    let result = TypeExpr {
        name,
        params: TypeParams::None,
    };
    BodyResult::Value(scope.arena.alloc_object(KObject::TypeExprValue(result)))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "LIST_OF",
        ExpressionSignature {
            return_type: KType::TypeExprRef,
            elements: vec![
                SignatureElement::Keyword("LIST_OF".into()),
                SignatureElement::Argument(Argument { name: "elem".into(), ktype: KType::TypeExprRef }),
            ],
        },
        body_list_of,
    );
    register_builtin(
        scope,
        "DICT_OF",
        ExpressionSignature {
            return_type: KType::TypeExprRef,
            elements: vec![
                SignatureElement::Keyword("DICT_OF".into()),
                SignatureElement::Argument(Argument { name: "key".into(),   ktype: KType::TypeExprRef }),
                SignatureElement::Argument(Argument { name: "value".into(), ktype: KType::TypeExprRef }),
            ],
        },
        body_dict_of,
    );
    register_builtin(
        scope,
        "FUNCTION_OF",
        ExpressionSignature {
            return_type: KType::TypeExprRef,
            elements: vec![
                SignatureElement::Keyword("FUNCTION_OF".into()),
                SignatureElement::Argument(Argument { name: "args".into(), ktype: KType::KExpression }),
                SignatureElement::Keyword("->".into()),
                SignatureElement::Argument(Argument { name: "ret".into(),  ktype: KType::TypeExprRef }),
            ],
        },
        body_function_of,
    );
    // Single overload: the `m` slot is `Module`. Bare Type-token operands
    // (`MODULE_TYPE_OF Foo Type`) ride the unified auto-wrap path and resolve through the
    // `value_lookup`-TypeExprRef overload to a `Future(KModule)`, which then matches this
    // slot strictly. Same shape as the ascription operators — no parallel TypeExprRef-lhs
    // overload needed.
    register_builtin(
        scope,
        "MODULE_TYPE_OF",
        ExpressionSignature {
            return_type: KType::TypeExprRef,
            elements: vec![
                SignatureElement::Keyword("MODULE_TYPE_OF".into()),
                SignatureElement::Argument(Argument { name: "m".into(),    ktype: KType::Module }),
                SignatureElement::Argument(Argument { name: "name".into(), ktype: KType::TypeExprRef }),
            ],
        },
        body_module_type_of,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
    use crate::dispatch::{KObject, KType, RuntimeArena};
    use crate::dispatch::types::NoopResolver;
    use crate::execute::Scheduler;

    /// `(LIST_OF Number)` dispatches and produces a `TypeExprValue` whose lowered `KType`
    /// is `List<Number>`. Round-trips the structured form through `from_type_expr`.
    #[test]
    fn list_of_number_lowers_to_list_number() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("LIST_OF Number"));
        let te = match result {
            KObject::TypeExprValue(t) => t.clone(),
            other => panic!("expected TypeExprValue, got {:?}", other.ktype()),
        };
        let kt = KType::from_type_expr(&te, &NoopResolver).expect("lowering should succeed");
        assert_eq!(kt, KType::List(Box::new(KType::Number)));
    }

    /// `(DICT_OF Str Number)` lowers to `Dict<Str, Number>`.
    #[test]
    fn dict_of_str_number_lowers_to_dict() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("DICT_OF Str Number"));
        let te = match result {
            KObject::TypeExprValue(t) => t.clone(),
            other => panic!("expected TypeExprValue, got {:?}", other.ktype()),
        };
        let kt = KType::from_type_expr(&te, &NoopResolver).expect("lowering should succeed");
        assert_eq!(
            kt,
            KType::Dict(Box::new(KType::Str), Box::new(KType::Number))
        );
    }

    /// Nested dispatch: `(LIST_OF (LIST_OF Number))` schedules the inner LIST_OF as a
    /// sub-Dispatch and the outer Bind splices the result in. End-to-end exercises the
    /// scheduler-driven type-expression path.
    #[test]
    fn nested_list_of_dispatches_through_scheduler() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("LIST_OF (LIST_OF Number)"));
        let te = match result {
            KObject::TypeExprValue(t) => t.clone(),
            other => panic!("expected TypeExprValue, got {:?}", other.ktype()),
        };
        let kt = KType::from_type_expr(&te, &NoopResolver).expect("lowering should succeed");
        assert_eq!(
            kt,
            KType::List(Box::new(KType::List(Box::new(KType::Number))))
        );
    }

    /// `(MODULE_TYPE_OF M Type)` reads the `Type` slot from a module's `type_members`
    /// table. Sets up an opaquely-ascribed module so `Type` is bound, then verifies the
    /// builtin returns a `TypeExprValue` carrying the `Type` name.
    #[test]
    fn module_type_of_resolves_via_module_member() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = ((LET Type = Number) (LET compare = 0))\n\
             SIG OrderedSig = ((LET Type = Number) (LET compare = 0))\n\
             LET Mod = (IntOrd :| OrderedSig)",
        );
        // `Mod` classifies as a Type token (uppercase first + lowercase rest); the
        // `MODULE_TYPE_OF` overload taking a `TypeExprRef` lhs handles the lookup itself,
        // mirroring how ascribe accepts `IntOrd :| OrderedSig` with both sides as Types.
        let result = run_one(scope, parse_one("MODULE_TYPE_OF Mod Type"));
        match result {
            KObject::TypeExprValue(t) => assert_eq!(t.name, "Type"),
            other => panic!("expected TypeExprValue, got {:?}", other.ktype()),
        }
    }

    /// MODULE_TYPE_OF on a module without that abstract member produces a clean
    /// `ShapeError` naming the module and the missing member.
    #[test]
    fn module_type_of_unknown_member_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "MODULE Foo = (LET x = 1)");
        // `Foo` is a Type token; the TypeExprRef-lhs overload looks it up against the
        // surrounding scope. `Bogus` is also a Type token naming a nonexistent abstract
        // member.
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("MODULE_TYPE_OF Foo Bogus"), scope);
        sched.execute().expect("scheduler runs to completion");
        let res = sched.read_result(id);
        assert!(res.is_err(), "expected MODULE_TYPE_OF on missing member to err");
    }
}
