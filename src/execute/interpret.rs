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

/// Recursively schedule `expr`: every nested `ExpressionPart::Expression` becomes its own
/// scheduler node first (post-order), then the parent is added either as a pre-bound future
/// (if it has no nested children) or as a pending node carrying `(part_index, dep)`
/// substitutions. The scheduler will splice each dep's runtime result into the parent's
/// parts as a `Future` part before late-dispatching it.
fn schedule_expr<'a>(
    expr: KExpression<'a>,
    scope: &Scope<'a>,
    scheduler: &mut Scheduler<'a>,
) -> Result<NodeId, String> {
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

        interpret(r#"PRINT (IF true THEN "yes")"#, &mut scope).unwrap();

        assert_eq!(captured.borrow().as_slice(), b"yes\n");
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
