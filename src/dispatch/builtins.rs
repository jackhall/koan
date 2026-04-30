use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;

use super::kfunction::{Argument, ArgumentBundle, ExpressionSignature, KFunction, KType, SignatureElement};
use super::kobject::KObject;
use super::scope::Scope;

fn null<'a>() -> &'a KObject<'a> {
    Box::leak(Box::new(KObject::Null))
}

/// `LET <name:Identifier> = <value:Any>` — copies the bound value (scalars only) into a
/// `Box::leak`'d `KObject` so it satisfies `Scope::add`'s `&'a KObject<'a>` signature, inserts
/// it under `name`, and returns that same leaked reference. Non-scalar values are silently
/// dropped and produce a freshly leaked `KObject::Null`.
pub fn builtin_let<'a>(scope: &mut Scope<'a>, bundle: ArgumentBundle<'a>) -> &'a KObject<'a> {
    let name = match bundle.get("name") {
        Some(KObject::KString(s)) => s.clone(),
        _ => return null(),
    };
    let cloned = match bundle.get("value") {
        Some(KObject::Number(n)) => KObject::Number(*n),
        Some(KObject::KString(s)) => KObject::KString(s.clone()),
        Some(KObject::Bool(b)) => KObject::Bool(*b),
        Some(KObject::Null) => KObject::Null,
        _ => return null(),
    };
    let leaked: &'a KObject<'a> = Box::leak(Box::new(cloned));
    scope.add(name, leaked);
    leaked
}

/// `<v:Identifier>` — single-part expression containing one name token. Looks `v` up in
/// `scope.data` and returns the bound `KObject`, or `Null` if unbound. Lets a parens-wrapped
/// name like `(some_var)` dispatch and resolve to its current value.
pub fn builtin_value_lookup<'a>(scope: &mut Scope<'a>, bundle: ArgumentBundle<'a>) -> &'a KObject<'a> {
    let name = match bundle.get("v") {
        Some(KObject::KString(s)) => s.clone(),
        _ => return null(),
    };
    scope.data.get(&name).copied().unwrap_or_else(null)
}

/// `<v:Any>` — single-part expression containing a literal (or a previously-evaluated future).
/// Returns the value as a fresh leaked `KObject`. Combined with `builtin_value_lookup`
/// (registered first) this lets parens-wrapped atoms — `(99)`, `("x")`, `(some_var)` —
/// dispatch through the regular pipeline.
pub fn builtin_value_pass<'a>(_scope: &mut Scope<'a>, bundle: ArgumentBundle<'a>) -> &'a KObject<'a> {
    let cloned = match bundle.get("v") {
        Some(KObject::Number(n)) => KObject::Number(*n),
        Some(KObject::KString(s)) => KObject::KString(s.clone()),
        Some(KObject::Bool(b)) => KObject::Bool(*b),
        Some(KObject::Null) => KObject::Null,
        _ => return null(),
    };
    Box::leak(Box::new(cloned))
}

/// `PRINT <msg:Str>` — writes the bound `KString` to `scope.out`, followed by a newline.
pub fn builtin_print<'a>(scope: &mut Scope<'a>, bundle: ArgumentBundle<'a>) -> &'a KObject<'a> {
    if let Some(KObject::KString(s)) = bundle.get("msg") {
        let _ = writeln!(scope.out, "{s}");
    }
    null()
}

/// `IF <predicate:Bool> THEN <value:KExpression>` — the lazy form. When `predicate` is false,
/// the captured `value` expression is never touched. When true, dispatches the captured
/// expression against `scope` and returns the produced `KObject`. Bare atoms inside parens
/// (e.g. `(99)`, `(some_var)`) dispatch through the `value_lookup`/`value_pass` builtins.
pub fn builtin_if_then_lazy<'a>(scope: &mut Scope<'a>, mut bundle: ArgumentBundle<'a>) -> &'a KObject<'a> {
    let predicate = match bundle.get("predicate") {
        Some(KObject::Bool(b)) => *b,
        _ => return null(),
    };
    if !predicate {
        return null();
    }
    let value_rc = match bundle.args.remove("value") {
        Some(rc) => rc,
        None => return null(),
    };
    let expr = match Rc::try_unwrap(value_rc) {
        Ok(KObject::KExpression(e)) => e,
        Ok(_) => return null(),
        Err(rc) => match &*rc {
            KObject::KExpression(e) => e.clone(),
            _ => return null(),
        },
    };
    let future = match scope.dispatch(expr) {
        Ok(f) => f,
        Err(_) => return null(),
    };
    let body = future.function.body;
    body(scope, future.bundle)
}

/// Build a fresh root scope populated with the language's builtin `KFunction`s. Each call
/// `Box::leak`s its own function and object boxes, so the returned scope is `'static` and child
/// scopes can chain off it via `Scope.outer` to inherit the builtins.
pub fn default_scope() -> Scope<'static> {
    let mut scope = Scope {
        outer: None,
        data: HashMap::new(),
        functions: Vec::new(),
        out: Box::new(std::io::stdout()),
    };

    let let_fn: &'static KFunction<'static> = Box::leak(Box::new(KFunction::new(
        None,
        ExpressionSignature {
            return_type: KType::Null,
            elements: vec![
                SignatureElement::Token("LET".into()),
                SignatureElement::Argument(Argument { name: "name".into(),  ktype: KType::Identifier, variadic: false }),
                SignatureElement::Token("=".into()),
                SignatureElement::Argument(Argument { name: "value".into(), ktype: KType::Any,        variadic: false }),
            ],
        },
        builtin_let,
    )));
    let let_obj: &'static KObject<'static> = Box::leak(Box::new(KObject::KFunction(let_fn)));
    scope.add("LET".into(), let_obj);

    let print_fn: &'static KFunction<'static> = Box::leak(Box::new(KFunction::new(
        None,
        ExpressionSignature {
            return_type: KType::Null,
            elements: vec![
                SignatureElement::Token("PRINT".into()),
                SignatureElement::Argument(Argument { name: "msg".into(), ktype: KType::Str, variadic: false }),
            ],
        },
        builtin_print,
    )));
    let print_obj: &'static KObject<'static> = Box::leak(Box::new(KObject::KFunction(print_fn)));
    scope.add("PRINT".into(), print_obj);

    // `value_lookup` (Identifier) before `value_pass` (Any). Both have one-element signatures;
    // Identifier is more selective so registering it first lets a single-token expression
    // resolve to its binding, while a single literal falls through to pass.
    let value_lookup_fn: &'static KFunction<'static> = Box::leak(Box::new(KFunction::new(
        None,
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Identifier, variadic: false }),
            ],
        },
        builtin_value_lookup,
    )));
    let value_lookup_obj: &'static KObject<'static> = Box::leak(Box::new(KObject::KFunction(value_lookup_fn)));
    scope.add("value_lookup".into(), value_lookup_obj);

    let value_pass_fn: &'static KFunction<'static> = Box::leak(Box::new(KFunction::new(
        None,
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Any, variadic: false }),
            ],
        },
        builtin_value_pass,
    )));
    let value_pass_obj: &'static KObject<'static> = Box::leak(Box::new(KObject::KFunction(value_pass_fn)));
    scope.add("value_pass".into(), value_pass_obj);

    // Single lazy IF/THEN. The THEN side must be a parens-wrapped expression — bare literals
    // dispatch through `value_pass` once captured (e.g. `IF true THEN (99)`), bare names
    // through `value_lookup` (e.g. `IF true THEN (x)`).
    let if_then_fn: &'static KFunction<'static> = Box::leak(Box::new(KFunction::new(
        None,
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Token("IF".into()),
                SignatureElement::Argument(Argument { name: "predicate".into(), ktype: KType::Bool,        variadic: false }),
                SignatureElement::Token("THEN".into()),
                SignatureElement::Argument(Argument { name: "value".into(),     ktype: KType::KExpression, variadic: false }),
            ],
        },
        builtin_if_then_lazy,
    )));
    let if_then_obj: &'static KObject<'static> = Box::leak(Box::new(KObject::KFunction(if_then_fn)));
    scope.add("if_then".into(), if_then_obj);

    scope
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::rc::Rc;

    use super::{builtin_let, builtin_value_lookup, builtin_value_pass, default_scope, Scope};
    use crate::dispatch::kfunction::ArgumentBundle;
    use crate::dispatch::kobject::KObject;
    use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

    #[test]
    fn let_inserts_binding_into_scope() {
        let mut scope = Scope {
            outer: None,
            data: HashMap::new(),
            functions: Vec::new(),
            out: Box::new(std::io::sink()),
        };
        let mut args = HashMap::new();
        args.insert("name".to_string(), Rc::new(KObject::KString("x".into())));
        args.insert("value".to_string(), Rc::new(KObject::Number(42.0)));

        let result = builtin_let(&mut scope, ArgumentBundle { args });

        assert!(matches!(result, KObject::Number(n) if *n == 42.0));
        let entry = scope.data.get("x").expect("expected binding 'x'");
        assert!(matches!(entry, KObject::Number(n) if *n == 42.0));
    }

    #[test]
    fn value_pass_returns_literal() {
        let mut scope = Scope {
            outer: None,
            data: HashMap::new(),
            functions: Vec::new(),
            out: Box::new(std::io::sink()),
        };
        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::Number(7.0)));

        let result = builtin_value_pass(&mut scope, ArgumentBundle { args });

        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    #[test]
    fn value_lookup_returns_binding() {
        let bound: &'static KObject<'static> = Box::leak(Box::new(KObject::Number(42.0)));
        let mut scope = Scope {
            outer: None,
            data: HashMap::new(),
            functions: Vec::new(),
            out: Box::new(std::io::sink()),
        };
        scope.data.insert("foo".to_string(), bound);

        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::KString("foo".into())));

        let result = builtin_value_lookup(&mut scope, ArgumentBundle { args });

        assert!(matches!(result, KObject::Number(n) if *n == 42.0));
    }

    #[test]
    fn value_lookup_unbound_returns_null() {
        let mut scope = Scope {
            outer: None,
            data: HashMap::new(),
            functions: Vec::new(),
            out: Box::new(std::io::sink()),
        };
        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::KString("missing".into())));

        let result = builtin_value_lookup(&mut scope, ArgumentBundle { args });

        assert!(matches!(result, KObject::Null));
    }

    #[test]
    fn dispatch_if_then_expression() {
        let mut scope = default_scope();
        // IF true THEN (99) — value side parens-wrapped so it's an Expression that the
        // lazy if_then captures and then dispatches via `value_pass`.
        let inner = KExpression {
            parts: vec![ExpressionPart::Literal(KLiteral::Number(99.0))],
        };
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Token("IF".into()),
                ExpressionPart::Literal(KLiteral::Boolean(true)),
                ExpressionPart::Token("THEN".into()),
                ExpressionPart::Expression(Box::new(inner)),
            ],
        };

        let future = scope.dispatch(expr).expect("dispatch should match `if_then`");
        let body = future.function.body;
        let result = body(&mut scope, future.bundle);

        assert!(matches!(result, KObject::Number(n) if *n == 99.0));
    }

    #[test]
    fn dispatch_lazy_if_then_captures_expression_as_data() {
        let mut scope = default_scope();
        let inner = KExpression {
            parts: vec![
                ExpressionPart::Token("LET".into()),
                ExpressionPart::Token("z".into()),
                ExpressionPart::Token("=".into()),
                ExpressionPart::Literal(KLiteral::Number(11.0)),
            ],
        };
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Token("IF".into()),
                ExpressionPart::Literal(KLiteral::Boolean(true)),
                ExpressionPart::Token("THEN".into()),
                ExpressionPart::Expression(Box::new(inner)),
            ],
        };

        let future = scope.dispatch(expr).expect("dispatch should match lazy if_then");
        // The bundle's `value` arg is captured as a KExpression, not eagerly resolved.
        assert!(matches!(
            future.bundle.get("value"),
            Some(KObject::KExpression(_))
        ));

        let body = future.function.body;
        let result = body(&mut scope, future.bundle);
        // Lazy body dispatched at runtime: LET ran, returned 11, and bound z.
        assert!(matches!(result, KObject::Number(n) if *n == 11.0));
        assert!(matches!(scope.data.get("z"), Some(KObject::Number(n)) if *n == 11.0));
    }

    #[test]
    fn dispatch_lazy_if_then_false_skips_expression() {
        let mut scope = default_scope();
        let inner = KExpression {
            parts: vec![
                ExpressionPart::Token("LET".into()),
                ExpressionPart::Token("skipped".into()),
                ExpressionPart::Token("=".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Token("IF".into()),
                ExpressionPart::Literal(KLiteral::Boolean(false)),
                ExpressionPart::Token("THEN".into()),
                ExpressionPart::Expression(Box::new(inner)),
            ],
        };

        let future = scope.dispatch(expr).expect("dispatch should match lazy if_then");
        let body = future.function.body;
        let result = body(&mut scope, future.bundle);

        assert!(matches!(result, KObject::Null));
        assert!(scope.data.get("skipped").is_none());
    }

    #[test]
    fn dispatch_let_expression() {
        let mut scope = default_scope();
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Token("LET".into()),
                ExpressionPart::Token("x".into()),
                ExpressionPart::Token("=".into()),
                ExpressionPart::Literal(KLiteral::Number(42.0)),
            ],
        };

        let future = scope.dispatch(expr).expect("dispatch should match `LET`");
        let body = future.function.body;
        let bundle = future.bundle;
        let result = body(&mut scope, bundle);

        assert!(matches!(result, KObject::Number(n) if *n == 42.0));
        let entry = scope.data.get("x").expect("expected binding 'x'");
        assert!(matches!(entry, KObject::Number(n) if *n == 42.0));
    }
}
