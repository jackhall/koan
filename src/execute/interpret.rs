use crate::dispatch::kfunction::{KType, SignatureElement};
use crate::dispatch::scope::Scope;
use crate::execute::scheduler::{NodeId, Scheduler};
use crate::parse::expression_tree::parse;
use crate::parse::kexpression::{ExpressionPart, KExpression};

/// Parse Koan source and submit each top-level expression — including its nested
/// sub-expressions — to a `Scheduler`, then run the resulting DAG against `scope`. Each
/// nested `(...)` becomes its own scheduled future; a parent expression depends on its
/// sub-expressions and is dispatched late, with `ExpressionPart::Future` slots filled in
/// from the deps' results. The caller owns the scope so output sink and post-run bindings
/// can be inspected.
pub fn interpret(source: &str, scope: &mut Scope<'static>) -> Result<(), String> {
    let exprs = parse(source)?;
    let mut scheduler = Scheduler::new();
    for expr in exprs {
        schedule_expr(expr, scope, &mut scheduler)?;
    }
    scheduler.execute(scope)?;
    Ok(())
}

/// Recursively schedule `expr`. If a "lazy candidate" function exists — one whose signature
/// matches the call shape AND has at least one `KExpression`-typed slot binding an Expression
/// part — schedule only the *eager* Expression parts (those landing on non-`KExpression`
/// slots) as deps, leaving the lazy ones as raw `Expression` parts in the parent. Otherwise
/// fall back to the post-order eager pipeline: every nested `Expression` becomes its own
/// scheduler node and the parent is added as a `Pending` with `(part_index, dep)` subs whose
/// results the scheduler splices in as `Future` parts before late dispatch.
fn schedule_expr<'a>(
    expr: KExpression<'a>,
    scope: &Scope<'a>,
    scheduler: &mut Scheduler<'a>,
) -> Result<NodeId, String> {
    if let Some(eager_indices) = lazy_candidate(scope, &expr) {
        let mut parts: Vec<ExpressionPart<'a>> = expr.parts;
        let mut subs: Vec<(usize, NodeId)> = Vec::with_capacity(eager_indices.len());
        for i in eager_indices {
            let inner = match std::mem::replace(&mut parts[i], ExpressionPart::Token(String::new())) {
                ExpressionPart::Expression(boxed) => *boxed,
                _ => unreachable!("lazy_candidate only flags Expression parts"),
            };
            let dep = schedule_expr(inner, scope, scheduler)?;
            subs.push((i, dep));
        }
        let parent = KExpression { parts };
        if subs.is_empty() {
            let future = scope.dispatch(parent)?;
            return Ok(scheduler.add(future));
        }
        return Ok(scheduler.add_pending(parent, subs));
    }

    let mut parts: Vec<ExpressionPart<'a>> = Vec::with_capacity(expr.parts.len());
    let mut subs: Vec<(usize, NodeId)> = Vec::new();
    for (i, part) in expr.parts.into_iter().enumerate() {
        match part {
            ExpressionPart::Expression(inner) => {
                let dep = schedule_expr(*inner, scope, scheduler)?;
                subs.push((i, dep));
                // Placeholder — overwritten with `Future(result)` at execute time before dispatch.
                parts.push(ExpressionPart::Token(String::new()));
            }
            other => parts.push(other),
        }
    }
    let new_expr = KExpression { parts };
    if subs.is_empty() {
        let future = scope.dispatch(new_expr)?;
        Ok(scheduler.add(future))
    } else {
        Ok(scheduler.add_pending(new_expr, subs))
    }
}

/// Look for a function in `scope` whose signature matches `expr`'s shape AND has at least one
/// Expression part landing on a `KType::KExpression` slot. Returns the indices of Expression
/// parts that sit on non-`KExpression` slots — those need eager scheduling. The caller leaves
/// Expression parts at lazy-slot positions in place so the receiving builtin gets them as
/// `KObject::KExpression` data.
///
/// The "at least one lazy slot" requirement is what disambiguates: a function with only
/// `Any`-typed slots would otherwise falsely qualify (since `Any` matches `Expression`), and
/// `bind` would deposit a `KObject::KExpression` into a bundle the body doesn't know how to
/// handle. Requiring an explicit `KExpression` slot guarantees the function opted in.
fn lazy_candidate<'a>(scope: &Scope<'a>, expr: &KExpression<'a>) -> Option<Vec<usize>> {
    if !expr.parts.iter().any(|p| matches!(p, ExpressionPart::Expression(_))) {
        return None;
    }
    for f in &scope.functions {
        let sig = &f.signature;
        if sig.elements.len() != expr.parts.len() {
            continue;
        }
        let mut eager_indices: Vec<usize> = Vec::new();
        let mut has_lazy_slot = false;
        let mut ok = true;
        for (i, (el, part)) in sig.elements.iter().zip(expr.parts.iter()).enumerate() {
            match (el, part) {
                (SignatureElement::Token(s), ExpressionPart::Token(t)) if s == t => {}
                (SignatureElement::Token(_), _) => { ok = false; break; }
                (SignatureElement::Argument(arg), part) => match (arg.ktype, part) {
                    (KType::KExpression, ExpressionPart::Expression(_)) => {
                        has_lazy_slot = true;
                    }
                    (KType::KExpression, _) => { ok = false; break; }
                    (_, ExpressionPart::Expression(_)) => {
                        // Speculative: assume the eager-evaluated result will type-match at
                        // late dispatch. If not, dispatch will fail at that point.
                        eager_indices.push(i);
                    }
                    (_, other) => {
                        if !arg.matches(other) { ok = false; break; }
                    }
                },
            }
        }
        if ok && has_lazy_slot {
            return Some(eager_indices);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    use super::*;
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::kobject::KObject;

    struct SharedBuf(Rc<RefCell<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    #[test]
    fn interprets_let_and_print() {
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let mut scope = default_scope();
        scope.out = Box::new(SharedBuf(captured.clone()));

        interpret("LET x = 42\nPRINT \"hello\"\n", &mut scope).unwrap();

        assert_eq!(captured.borrow().as_slice(), b"hello\n");
        assert!(matches!(scope.data.get("x"), Some(KObject::Number(n)) if *n == 42.0));
    }

    #[test]
    fn interprets_if_then_via_print() {
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let mut scope = default_scope();
        scope.out = Box::new(SharedBuf(captured.clone()));

        interpret(r#"PRINT (IF true THEN ("yes"))"#, &mut scope).unwrap();

        assert_eq!(captured.borrow().as_slice(), b"yes\n");
    }

    #[test]
    fn if_then_false_does_not_run_lazy_expression() {
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let mut scope = default_scope();
        scope.out = Box::new(SharedBuf(captured.clone()));

        interpret(r#"IF false THEN (PRINT "should not appear")"#, &mut scope).unwrap();

        assert!(
            captured.borrow().is_empty(),
            "lazy expression must not execute when predicate is false; got {:?}",
            String::from_utf8_lossy(&captured.borrow()),
        );
    }

    #[test]
    fn if_then_true_runs_lazy_expression() {
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let mut scope = default_scope();
        scope.out = Box::new(SharedBuf(captured.clone()));

        interpret(r#"IF true THEN (PRINT "ran")"#, &mut scope).unwrap();

        assert_eq!(captured.borrow().as_slice(), b"ran\n");
    }

    #[test]
    fn if_then_lazy_value_lookup_resolves_name() {
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let mut scope = default_scope();
        scope.out = Box::new(SharedBuf(captured.clone()));

        interpret(
            "LET greeting = \"hi\"\nPRINT (IF true THEN (greeting))\n",
            &mut scope,
        )
        .unwrap();

        // The lazy expression `(greeting)` dispatches to `value_lookup`, which finds the
        // string bound by the prior LET and returns it for PRINT to write.
        assert_eq!(captured.borrow().as_slice(), b"hi\n");
    }

    #[test]
    fn if_then_false_skips_let_side_effect() {
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let mut scope = default_scope();
        scope.out = Box::new(SharedBuf(captured.clone()));

        interpret("IF false THEN (LET y = 1)\nPRINT \"after\"\n", &mut scope).unwrap();

        assert!(scope.data.get("y").is_none(), "lazy LET must not have bound y");
        assert_eq!(captured.borrow().as_slice(), b"after\n");
    }

    #[test]
    fn interprets_nested_expression() {
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let mut scope = default_scope();
        scope.out = Box::new(SharedBuf(captured.clone()));

        interpret(r#"(PRINT (LET msg = "hello world!"))"#, &mut scope).unwrap();

        assert_eq!(captured.borrow().as_slice(), b"hello world!\n");
        assert!(matches!(scope.data.get("msg"), Some(KObject::KString(s)) if s == "hello world!"));
    }
}
