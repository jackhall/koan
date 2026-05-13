//! Type-constructor builtins — `LIST_OF`, `DICT_OF`, `FUNCTION_OF`, `MODULE_TYPE_OF`. These
//! are ordinary scheduled `KFunction`s whose inputs are `TypeExprRef`-typed slots (resolved
//! to `KObject::KTypeValue(kt)`) and whose outputs are also `KObject::KTypeValue(kt)`
//! carrying the elaborated `KType` directly. Dispatching them through the same `Dispatch`
//! / `Bind` machinery values use means a parameterized type can be assembled by
//! sub-expression evaluation: `(LIST_OF (MODULE_TYPE_OF M Type))` wakes the outer slot
//! only after the inner sub-dispatch resolves to a concrete `KType` value.
//!
//! **Why builtins rather than a parallel registration table.** The plan in
//! [roadmap/module-system-2-scheduler.md](../../../roadmap/module-system-2-scheduler.md)
//! reduces type-expression evaluation to ordinary dispatch: the same scope-lookup chain,
//! the same `Bind`-waits-for-subs refinement, the same `lift_kobject` rules. No new node
//! kind, no `KType::TypeVar`, no second registration table — a `TypeExprRef`-typed binding
//! lives in `Scope::data` like any other value.
//!
//! The output of these builtins is the elaborated `KType` directly — no `TypeExpr`
//! intermediate. Consumers reach the `KType` through `KObject::as_ktype()` /
//! `extract_ktype()` and operate on the structural shape rather than the surface form.

use crate::runtime::model::{Argument, ExpressionSignature, KObject, KType, SignatureElement};
use crate::runtime::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};
use crate::runtime::model::values::resolve_module;

use super::{err, register_builtin};

/// Pull a `KObject::KTypeValue`'s inner `KType` out of an arg slot. The slot is declared
/// `KType::TypeExprRef`, so by `Argument::matches` shape-time it must be either an
/// `ExpressionPart::Type(_)` (lowered into `KTypeValue` by `resolve_for`) or a
/// `Future(KObject::KTypeValue(_))` lifted from a previous sub-dispatch. Anything else
/// reaching here is a `TypeMismatch` from the dispatcher's perspective.
fn read_ktype<'a>(bundle: &ArgumentBundle<'a>, name: &str) -> Result<KType, KError> {
    let Some(obj) = bundle.get(name) else {
        return Err(KError::new(KErrorKind::MissingArg(name.to_string())));
    };
    if let Some(kt) = obj.as_ktype() {
        return Ok(kt.clone());
    }
    Err(KError::new(KErrorKind::TypeMismatch {
        arg: name.to_string(),
        expected: "TypeExprRef".to_string(),
        got: obj.ktype().name(),
    }))
}

/// `LIST_OF <elem:TypeExprRef>` → `TypeExprRef` carrying `List<elem>`.
pub fn body_list_of<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let elem = match read_ktype(&bundle, "elem") {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    BodyResult::Value(
        scope
            .arena
            .alloc_object(KObject::KTypeValue(KType::List(Box::new(elem)))),
    )
}

/// `DICT_OF <key:TypeExprRef> <value:TypeExprRef>` → `TypeExprRef` carrying
/// `Dict<key, value>`.
pub fn body_dict_of<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let key = match read_ktype(&bundle, "key") {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    let value = match read_ktype(&bundle, "value") {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    BodyResult::Value(
        scope
            .arena
            .alloc_object(KObject::KTypeValue(KType::Dict(Box::new(key), Box::new(value)))),
    )
}

/// `FUNCTION_OF <args:KExpression> -> <ret:TypeExprRef>` → `TypeExprRef` carrying
/// `Function<(args) -> ret>`. The `args` slot is captured raw as a `KExpression` whose
/// parts are bare `Type(_)` tokens; we re-extract and elaborate each into a `KType`.
/// Parameterized inner args (`List<Number>`) come through as `Future(KTypeValue(kt))` from
/// a prior sub-dispatch; leaf `Type(t)` tokens go through the resolver-free
/// [`KType::from_type_expr`] (builtin-table only) to handle nested-parameter shapes.
pub fn body_function_of<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    use crate::ast::ExpressionPart;
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
    let ret = match read_ktype(&bundle, "ret") {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    let mut args: Vec<KType> = Vec::with_capacity(args_expr.parts.len());
    for part in &args_expr.parts {
        match part {
            ExpressionPart::Type(t) => match KType::from_type_expr(t) {
                Ok(kt) => args.push(kt),
                Err(msg) => {
                    return err(KError::new(KErrorKind::ShapeError(format!(
                        "FUNCTION_OF args: {msg}"
                    ))));
                }
            },
            ExpressionPart::Future(KObject::KTypeValue(kt)) => args.push(kt.clone()),
            other => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "FUNCTION_OF args must be type names, got `{}`",
                    other.summarize()
                ))));
            }
        }
    }
    BodyResult::Value(
        scope.arena.alloc_object(KObject::KTypeValue(KType::KFunction {
            args,
            ret: Box::new(ret),
        })),
    )
}

/// `MODULE_TYPE_OF <m:Module> <name>` → `TypeExprRef` carrying the abstract type bound
/// under `name` in `m`'s `type_members` table. Surface analogue of `M.Type`, but reachable
/// as a scheduled call so a functor body can synthesize it from a parameter module value.
/// The `m` slot is strictly `Module`; bare Type-token operands (`MODULE_TYPE_OF Foo Type`)
/// ride the auto-wrap rails — they sub-dispatch through `value_lookup` and arrive here
/// as a `Future(KModule)`. The shared [`crate::runtime::model::values::resolve_module`] helper
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
    // classify as Type per the token-classification rules, not Identifier. The lookup uses
    // the bare leaf name from the resolved `KType`.
    let name_kt = match read_ktype(&bundle, "name") {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    let name = name_kt.name();
    // Pull the abstract type's concrete `KType::ModuleType` (or whatever the module stored)
    // out of the `type_members` table directly so the consumer downstream sees the
    // identity-bearing variant rather than a re-elaborated leaf.
    let kt = match m.type_members.borrow().get(&name).cloned() {
        Some(kt) => kt,
        None => {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "module `{}` has no abstract type member `{}`",
                m.path, name
            ))));
        }
    };
    BodyResult::Value(scope.arena.alloc_object(KObject::KTypeValue(kt)))
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
    use crate::runtime::builtins::test_support::{parse_one, run, run_one, run_root_silent};
    use crate::runtime::model::{KObject, KType};
    use crate::runtime::machine::RuntimeArena;
    use crate::runtime::machine::execute::Scheduler;

    /// `(LIST_OF Number)` dispatches and produces a `KTypeValue` carrying the elaborated
    /// `KType::List(Number)` directly — no surface-form round-trip needed.
    #[test]
    fn list_of_number_lowers_to_list_number() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("LIST_OF Number"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(*kt, KType::List(Box::new(KType::Number)));
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// `(DICT_OF Str Number)` lowers to `Dict<Str, Number>`.
    #[test]
    fn dict_of_str_number_lowers_to_dict() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("DICT_OF Str Number"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::Dict(Box::new(KType::Str), Box::new(KType::Number))
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// Nested dispatch: `(LIST_OF (LIST_OF Number))` schedules the inner LIST_OF as a
    /// sub-Dispatch and the outer Bind splices the result in. End-to-end exercises the
    /// scheduler-driven type-expression path.
    #[test]
    fn nested_list_of_dispatches_through_scheduler() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("LIST_OF (LIST_OF Number)"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::List(Box::new(KType::List(Box::new(KType::Number))))
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// `(MODULE_TYPE_OF M Type)` reads the `Type` slot from a module's `type_members`
    /// table. Sets up an opaquely-ascribed module so `Type` is bound, then verifies the
    /// builtin returns a `KTypeValue` whose `KType::ModuleType` carries the abstract
    /// type's identity.
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
        let result = run_one(scope, parse_one("MODULE_TYPE_OF Mod Type"));
        match result {
            KObject::KTypeValue(kt) => {
                // The abstract type member is recorded as `KType::ModuleType` by the
                // ascription path; surface name is `Type`.
                assert_eq!(kt.name(), "Type");
                assert!(matches!(kt, KType::ModuleType { .. }));
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
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
