use crate::dispatch::arena::RuntimeArena;
use crate::dispatch::builtins::default_scope;
use crate::execute::scheduler::Scheduler;
use crate::parse::expression_tree::parse;

/// Parse Koan source and run it. Each call constructs a fresh `RuntimeArena` and a per-run
/// scope tree (the default scope with builtins registered, allocated in that arena); every
/// value the program allocates lives in that arena and is dropped when this function returns.
/// The scheduler walks the AST itself — every top-level expression goes in as a single
/// `Dispatch` node bound to the run-root scope; the scheduler then handles nested
/// sub-expressions, list literals, and lazy slots dynamically as nodes execute.
pub fn interpret(source: &str) -> Result<(), String> {
    interpret_with_writer(source, Box::new(std::io::stdout()))
}

/// Same as `interpret` but lets the caller supply a writer for `PRINT` output. Tests use this
/// to capture `PRINT` into a buffer; the CLI uses the default-stdout `interpret`. Constructs
/// a fresh arena local to this call; everything the program allocates dies when this function
/// returns.
pub fn interpret_with_writer(
    source: &str,
    out: Box<dyn std::io::Write>,
) -> Result<(), String> {
    let exprs = parse(source)?;
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, out);
    let mut scheduler = Scheduler::new();
    for expr in exprs {
        scheduler.add_dispatch(expr, root);
    }
    scheduler.execute()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    use super::*;
    use crate::dispatch::kobject::KObject;
    use crate::dispatch::scope::Scope;

    struct SharedBuf(Rc<RefCell<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    /// Spin up an arena + default scope, run `source`, return (captured PRINT output, arena,
    /// root scope) so the caller can inspect bindings before the arena drops at end of scope.
    /// Each test that needs both stdout and post-run state uses this rig directly.
    fn run<'a>(source: &str, arena: &'a RuntimeArena, captured: Rc<RefCell<Vec<u8>>>) -> &'a Scope<'a> {
        let exprs = parse(source).expect("parse should succeed");
        let root = default_scope(arena, Box::new(SharedBuf(captured)));
        let mut scheduler = Scheduler::new();
        for expr in exprs {
            scheduler.add_dispatch(expr, root);
        }
        scheduler.execute().expect("program should run");
        root
    }

    #[test]
    fn interprets_let_and_print() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run("LET x = 42\nPRINT \"hello\"\n", &arena, captured.clone());

        assert_eq!(captured.borrow().as_slice(), b"hello\n");
        let data = scope.data.borrow();
        assert!(matches!(data.get("x"), Some(KObject::Number(n)) if *n == 42.0));
    }

    #[test]
    fn interprets_if_then_via_print() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        run(r#"PRINT (IF true THEN ("yes"))"#, &arena, captured.clone());
        assert_eq!(captured.borrow().as_slice(), b"yes\n");
    }

    #[test]
    fn if_then_false_does_not_run_lazy_expression() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        run(r#"IF false THEN (PRINT "should not appear")"#, &arena, captured.clone());
        assert!(
            captured.borrow().is_empty(),
            "lazy expression must not execute when predicate is false; got {:?}",
            String::from_utf8_lossy(&captured.borrow()),
        );
    }

    #[test]
    fn if_then_true_runs_lazy_expression() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        run(r#"IF true THEN (PRINT "ran")"#, &arena, captured.clone());
        assert_eq!(captured.borrow().as_slice(), b"ran\n");
    }

    #[test]
    fn if_then_lazy_value_lookup_resolves_name() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        run(
            "LET greeting = \"hi\"\nPRINT (IF true THEN (greeting))\n",
            &arena,
            captured.clone(),
        );
        assert_eq!(captured.borrow().as_slice(), b"hi\n");
    }

    #[test]
    fn if_then_false_skips_let_side_effect() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run(
            "IF false THEN (LET y = 1)\nPRINT \"after\"\n",
            &arena,
            captured.clone(),
        );
        assert!(scope.data.borrow().get("y").is_none(), "lazy LET must not have bound y");
        assert_eq!(captured.borrow().as_slice(), b"after\n");
    }

    #[test]
    fn interprets_nested_expression() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run(r#"(PRINT (LET msg = "hello world!"))"#, &arena, captured.clone());

        assert_eq!(captured.borrow().as_slice(), b"hello world!\n");
        let data = scope.data.borrow();
        assert!(matches!(data.get("msg"), Some(KObject::KString(s)) if *s == "hello world!"));
    }

    #[test]
    fn let_binds_a_list_literal_of_numbers() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run("LET xs = [1 2 3]\n", &arena, captured);
        let data = scope.data.borrow();
        match data.get("xs") {
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
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run("LET xs = []\n", &arena, captured);
        let data = scope.data.borrow();
        match data.get("xs") {
            Some(KObject::List(items)) => assert!(items.is_empty()),
            _ => panic!("expected `xs` bound to an empty List"),
        }
    }

    #[test]
    fn list_literal_with_subexpression_element_evaluates_eagerly() {
        // `(LET y = 7)` evaluates as part of the list construction; afterwards `y` is bound
        // and the list contains the LET's return value (the bound number).
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run("LET xs = [1 (LET y = 7) 3]\n", &arena, captured);
        let data = scope.data.borrow();
        match data.get("xs") {
            Some(KObject::List(items)) => {
                assert_eq!(items.len(), 3);
                assert!(matches!(items[0], KObject::Number(n) if n == 1.0));
                assert!(matches!(items[1], KObject::Number(n) if n == 7.0));
                assert!(matches!(items[2], KObject::Number(n) if n == 3.0));
            }
            _ => panic!("expected `xs` bound to a List"),
        }
        assert!(matches!(data.get("y"), Some(KObject::Number(n)) if *n == 7.0));
    }

    #[test]
    fn multiline_list_literal_binds_correctly() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run("LET xs = [\n  1\n  2\n  3\n]\n", &arena, captured);
        let data = scope.data.borrow();
        match data.get("xs") {
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
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run("LET xs = [[1 2] [3 4]]\n", &arena, captured);
        let data = scope.data.borrow();
        match data.get("xs") {
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
