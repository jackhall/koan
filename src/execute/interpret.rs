use crate::dispatch::scope::Scope;
use crate::execute::scheduler::Scheduler;
use crate::parse::expression_tree::parse;

/// Parse Koan source, dispatch each top-level expression into a scheduled future against
/// `scope`, and execute the resulting DAG. The caller owns the scope so output sink and
/// post-run bindings can be inspected.
pub fn interpret(source: &str, scope: &mut Scope<'static>) -> Result<(), String> {
    let exprs = parse(source)?;
    let mut scheduler = Scheduler::new();
    for expr in exprs {
        let future = scope.dispatch(expr)?;
        scheduler.add(future);
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

        interpret("let x = 42\nprint \"hello\"\n", &mut scope).unwrap();

        assert_eq!(captured.borrow().as_slice(), b"hello\n");
        assert!(matches!(scope.data.get("x"), Some(KObject::Number(n)) if *n == 42.0));
    }
}
