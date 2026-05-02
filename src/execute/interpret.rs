use crate::dispatch::scope::Scope;
use crate::execute::scheduler::Scheduler;
use crate::parse::expression_tree::parse;

/// Parse Koan source and run it. The scheduler walks the AST itself — every top-level
/// expression goes in as a single `Dispatch` node; the scheduler then handles nested
/// sub-expressions, list literals, and lazy slots dynamically as nodes execute.
pub fn interpret(source: &str, scope: &mut Scope<'static>) -> Result<(), String> {
    let exprs = parse(source)?;
    let mut scheduler = Scheduler::new();
    for expr in exprs {
        scheduler.add_dispatch(expr);
    }
    scheduler.execute(scope)?;
    Ok(())
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

    #[test]
    fn let_binds_a_list_literal_of_numbers() {
        let mut scope = default_scope();
        interpret("LET xs = [1 2 3]\n", &mut scope).unwrap();
        match scope.data.get("xs") {
            Some(KObject::List(items)) => {
                assert_eq!(items.len(), 3);
                assert!(matches!(items[0], KObject::Number(n) if n == 1.0));
                assert!(matches!(items[2], KObject::Number(n) if n == 3.0));
            }
            _ => panic!("expected `xs` bound to a List"),
        }
    }

    #[test]
    fn let_binds_an_empty_list_literal() {
        let mut scope = default_scope();
        interpret("LET xs = []\n", &mut scope).unwrap();
        match scope.data.get("xs") {
            Some(KObject::List(items)) => assert!(items.is_empty()),
            _ => panic!("expected `xs` bound to an empty List"),
        }
    }

    #[test]
    fn list_literal_with_subexpression_element_evaluates_eagerly() {
        // `(LET y = 7)` evaluates as part of the list construction; afterwards `y` is bound
        // and the list contains the LET's return value (the bound number).
        let mut scope = default_scope();
        interpret("LET xs = [1 (LET y = 7) 3]\n", &mut scope).unwrap();
        match scope.data.get("xs") {
            Some(KObject::List(items)) => {
                assert_eq!(items.len(), 3);
                assert!(matches!(items[0], KObject::Number(n) if n == 1.0));
                assert!(matches!(items[1], KObject::Number(n) if n == 7.0));
                assert!(matches!(items[2], KObject::Number(n) if n == 3.0));
            }
            _ => panic!("expected `xs` bound to a List"),
        }
        assert!(matches!(scope.data.get("y"), Some(KObject::Number(n)) if *n == 7.0));
    }

    #[test]
    fn multiline_list_literal_binds_correctly() {
        // The `[` opens on line 1, elements span the next three lines, `]` closes on line 5.
        // `collapse_whitespace` is bracket-aware, so the continuation lines are appended into
        // the list span instead of being wrapped as indented children.
        let src = "LET xs = [\n  1\n  2\n  3\n]\n";
        let mut scope = default_scope();
        interpret(src, &mut scope).unwrap();
        match scope.data.get("xs") {
            Some(KObject::List(items)) => {
                assert_eq!(items.len(), 3);
                assert!(matches!(items[0], KObject::Number(n) if n == 1.0));
                assert!(matches!(items[2], KObject::Number(n) if n == 3.0));
            }
            _ => panic!("expected `xs` bound to a List"),
        }
    }

    #[test]
    fn nested_list_literal_produces_list_of_lists() {
        let mut scope = default_scope();
        interpret("LET xs = [[1 2] [3 4]]\n", &mut scope).unwrap();
        match scope.data.get("xs") {
            Some(KObject::List(outer)) => {
                assert_eq!(outer.len(), 2);
                match &outer[0] {
                    KObject::List(inner) => {
                        assert_eq!(inner.len(), 2);
                        assert!(matches!(inner[0], KObject::Number(n) if n == 1.0));
                    }
                    _ => panic!("inner[0] should be a List"),
                }
            }
            _ => panic!("expected `xs` bound to a List"),
        }
    }
}
