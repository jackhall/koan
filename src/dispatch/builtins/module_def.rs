//! `MODULE <name:TypeExprRef> = <body:KExpression>` — declare a structure (a bundle of
//! type definitions, values, and functions). See
//! [design/module-system.md](../../../design/module-system.md) for the surface design.
//!
//! Construction shape: the body is a parens-wrapped KExpression. Each top-level
//! `Expression` part inside the body is dispatched as an independent statement against a
//! fresh child scope. After all statements complete, the child scope is captured into a
//! [`Module`] value (`name`, `child_scope`, `type_members` initially empty), which is
//! allocated in the parent scope's arena and bound under the module's name in the parent's
//! `data`. Members reachable as `Foo.<member>` go through ATTR's `KModule` overload (see
//! `attr.rs`), which looks `<member>` up in the captured `child_scope.data`.
//!
//! Statements are dispatched via a fresh inner `Scheduler` so the surrounding caller's
//! scheduler doesn't get tangled with the module's body. The inner scheduler runs to
//! completion before MODULE returns; any error short-circuits and surfaces as a
//! `BodyResult::Err`.

use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::dispatch::runtime::{KError, KErrorKind, Scope};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::values::{KObject, Module};

use crate::parse::kexpression::KExpression;

use super::helpers::{extract_bare_type_name, extract_kexpression, run_body_statements};
use super::{err, register_builtin_with_pre_run};

pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // The name slot is `KType::TypeExprRef` — module names use the type-token shape
    // (`MODULE Foo`, `MODULE OrderedSig` would be a SIG, not a MODULE; the ascription
    // result is what's `OrderedSig`). Reject parameterized forms — module names are bare
    // leaves until functors land in stage 2.
    let name = match extract_bare_type_name(&bundle, "name", "MODULE") {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    let body_expr = match extract_kexpression(&mut bundle, "body") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "MODULE body slot must be a parenthesized expression".to_string(),
            )));
        }
    };

    let arena = scope.arena;
    let child_scope = arena.alloc_scope(Scope::child_under_named(
        scope,
        format!("MODULE {}", name),
    ));

    // Run each top-level statement in `body_expr` against the child scope. The body's parts
    // are typically a list of `Expression(stmt)` parts (one per indented line); single-
    // statement bodies parse to a flat KExpression that we dispatch as one piece.
    if let Err(e) = run_body_statements(child_scope, body_expr) {
        return BodyResult::Err(e);
    }

    let module: &'a Module<'a> = arena.alloc_module(Module::new(name.clone(), child_scope));
    let module_obj: &'a KObject<'a> = arena.alloc_object(KObject::KModule(module));
    if let Err(e) = scope.bind_value(name, module_obj) {
        return err(e);
    }
    BodyResult::Value(module_obj)
}

/// Dispatch-time placeholder extractor for MODULE. `parts[1]` is the `Type(t)` token of the
/// module's name slot. Same shape as STRUCT / SIG / named UNION.
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    super::helpers::binder_name_from_type_part(expr)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin_with_pre_run(
        scope,
        "MODULE",
        ExpressionSignature {
            return_type: KType::Module,
            elements: vec![
                SignatureElement::Keyword("MODULE".into()),
                SignatureElement::Argument(Argument {
                    name: "name".into(),
                    ktype: KType::TypeExprRef,
                }),
                SignatureElement::Keyword("=".into()),
                SignatureElement::Argument(Argument {
                    name: "body".into(),
                    ktype: KType::KExpression,
                }),
            ],
        },
        body,
        Some(pre_run),
    );
}

#[cfg(test)]
mod tests {
    use crate::dispatch::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::dispatch::runtime::{KErrorKind, RuntimeArena};
    use crate::dispatch::values::KObject;

    /// Smoke test for MODULE's pre_run extractor: structural extraction of the `Type(_)`
    /// token at `parts[1]`.
    #[test]
    fn pre_run_extracts_module_name() {
        let expr = parse_one("MODULE Foo = (LET x = 1)");
        let name = super::pre_run(&expr);
        assert_eq!(name.as_deref(), Some("Foo"));
    }

    #[test]
    fn module_binds_under_name_in_scope() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "MODULE Foo = (LET x = 1)");
        let data = scope.data.borrow();
        assert!(matches!(data.get("Foo"), Some(KObject::KModule(_))));
    }

    #[test]
    fn module_member_access_via_attr() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "MODULE Foo = (LET x = 1)");
        let result = run_one(scope, parse_one("Foo.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 1.0));
    }

    #[test]
    fn module_with_multiple_statements_in_parens() {
        // Multi-statement bodies use parens with statements separated by commas (which the
        // whitespace pass collapses to whitespace inside expression frames). The parser
        // wraps each statement in an Expression sub-part of the body slot, and MODULE's
        // body-runner dispatches each Expression in the child scope.
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE Foo = ((LET x = 1) (LET y = 2))",
        );
        assert!(matches!(run_one(scope, parse_one("Foo.x")), KObject::Number(n) if *n == 1.0));
        assert!(matches!(run_one(scope, parse_one("Foo.y")), KObject::Number(n) if *n == 2.0));
    }

    #[test]
    fn module_member_function_via_let_fn() {
        // Per the plan §1: module member functions must use `LET <name> = (FN ...)` to bind
        // under a clean identifier. Bare FN inside a MODULE body lands under the FN's
        // signature key, not under an identifier — accessible only via dispatch from inside
        // the module body, not via `Foo.<name>`.
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE Foo = (LET double = (FN (DOUBLE x: Number) -> Number = (x)))",
        );
        let data = scope.data.borrow();
        let foo = match data.get("Foo") {
            Some(KObject::KModule(m)) => *m,
            _ => panic!("Foo should be a module"),
        };
        assert!(foo.child_scope().data.borrow().contains_key("double"));
    }

    #[test]
    fn module_unknown_member_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "MODULE Foo = (LET x = 1)");
        let err = run_one_err(scope, parse_one("Foo.bogus"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Foo") && msg.contains("`bogus`")),
            "expected ShapeError naming Foo and bogus, got {err}",
        );
    }

    #[test]
    fn nested_module_accessible_via_chained_attr() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE Outer =\n  MODULE Inner = (LET x = 7)",
        );
        let result = run_one(scope, parse_one("Outer.Inner.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }
}
