use crate::dispatch::arena::RuntimeArena;
use crate::dispatch::builtins::default_scope;
use crate::dispatch::kerror::{KError, KErrorKind};
use crate::execute::scheduler::Scheduler;
use crate::parse::expression_tree::parse;

/// Parse Koan source and run it. Each call constructs a fresh `RuntimeArena` and a per-run
/// scope tree (the default scope with builtins registered, allocated in that arena); every
/// value the program allocates lives in that arena and is dropped when this function returns.
/// The scheduler walks the AST itself — every top-level expression goes in as a single
/// `Dispatch` node bound to the run-root scope; the scheduler then handles nested
/// sub-expressions, list literals, and lazy slots dynamically as nodes execute.
///
/// Returns `Err(KError)` for parse failures (wrapped as `KError::ParseError`) and runtime
/// errors that bubble up to a top-level dispatch.
pub fn interpret(source: &str) -> Result<(), KError> {
    interpret_with_writer(source, Box::new(std::io::stdout()))
}

/// Same as `interpret` but lets the caller supply a writer for `PRINT` output. Tests use this
/// to capture `PRINT` into a buffer; the CLI uses the default-stdout `interpret`. Constructs
/// a fresh arena local to this call; everything the program allocates dies when this function
/// returns.
pub fn interpret_with_writer(
    source: &str,
    out: Box<dyn std::io::Write>,
) -> Result<(), KError> {
    let exprs = parse(source).map_err(|e| KError::new(KErrorKind::ParseError(e)))?;
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, out);
    let mut scheduler = Scheduler::new();
    let mut top_level: Vec<crate::dispatch::kfunction::NodeId> = Vec::with_capacity(exprs.len());
    for expr in exprs {
        top_level.push(scheduler.add_dispatch(expr, root));
    }
    scheduler.execute()?;
    // After execute, scan the top-level dispatches for the first errored result and surface
    // it. Top-level dispatches share the run-root scope, so an error in expression N doesn't
    // prevent expression N+1's dispatch from being scheduled — but per the design, the
    // first error short-circuits the program's reported outcome.
    for id in top_level {
        if let Err(e) = scheduler.read_result(id) {
            return Err(e.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    use super::*;
    use crate::dispatch::kerror::KErrorKind;
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

    // --- Error-handling tests added by the KError pass ---

    /// A bare unbound name at the top level surfaces as `KError::UnboundName` rather than
    /// the prior silent `KObject::Null` swallow.
    #[test]
    fn unbound_name_at_top_level_returns_error() {
        let result = interpret_with_writer("foo", Box::new(std::io::sink()));
        match result {
            Err(e) => assert!(
                matches!(&e.kind, KErrorKind::UnboundName(name) if name == "foo"),
                "expected UnboundName(\"foo\"), got {e}",
            ),
            Ok(()) => panic!("expected UnboundName error, got Ok"),
        }
    }

    /// An error inside a user-fn body carries at least one `Frame` whose function field
    /// names the user-fn — proving the call-stack trace works through user invocation.
    #[test]
    fn error_inside_user_fn_body_carries_frame() {
        let result = interpret_with_writer(
            "FN (BAD) = (undefined_thing)\nBAD",
            Box::new(std::io::sink()),
        );
        match result {
            Err(e) => {
                assert!(
                    matches!(&e.kind, KErrorKind::UnboundName(name) if name == "undefined_thing"),
                    "expected UnboundName(\"undefined_thing\"), got {e}",
                );
                assert!(
                    e.frames.iter().any(|f| f.function.contains("BAD")),
                    "expected a frame mentioning BAD, got frames: {:?}",
                    e.frames.iter().map(|f| &f.function).collect::<Vec<_>>(),
                );
            }
            Ok(()) => panic!("expected error from undefined name in user-fn body"),
        }
    }

    /// The first errored top-level expression short-circuits the program's reported
    /// outcome; subsequent top-level dispatches still run (Scheduler::execute keeps
    /// draining the queue), but interpret returns the first error and any later bindings
    /// are observable side-effects rather than program-level "success."
    #[test]
    fn error_short_circuits_program_outcome() {
        let result = interpret_with_writer("undefined\nLET y = 5", Box::new(std::io::sink()));
        match result {
            Err(e) => assert!(
                matches!(&e.kind, KErrorKind::UnboundName(name) if name == "undefined"),
                "expected UnboundName(\"undefined\") to be the surfaced error, got {e}",
            ),
            Ok(()) => panic!("expected first-line error to short-circuit interpret's outcome"),
        }
    }

    /// A made-up function call with no matching signature surfaces as
    /// `KError::DispatchFailed`. (`WAT THIS IS NOT A FUNCTION` parses as a multi-token
    /// expression with all-uppercase tokens, so dispatch fails to find a match.)
    #[test]
    fn dispatch_failure_surfaces_as_kerror() {
        let result = interpret_with_writer(
            "WAT THIS IS NOT A FUNCTION",
            Box::new(std::io::sink()),
        );
        match result {
            Err(e) => assert!(
                matches!(&e.kind, KErrorKind::DispatchFailed { .. }),
                "expected DispatchFailed, got {e}",
            ),
            Ok(()) => panic!("expected dispatch failure for unmatched expression"),
        }
    }

    /// A type-mismatched argument that fits the bucket shape but fails dispatch's
    /// per-slot type check surfaces as `KError::DispatchFailed` (no overload matches).
    /// `IF` requires `predicate: Bool` and `value: KExpression`; passing a string
    /// predicate doesn't match the only IF signature, so dispatch finds zero candidates.
    /// Type mismatches that DO reach `bind` (only possible with an overload set richer
    /// than today's) would surface as `TypeMismatch` from the bind step.
    #[test]
    fn type_mismatch_at_dispatch_surfaces_as_dispatch_failed() {
        let result = interpret_with_writer(
            "IF \"not_a_bool\" THEN (\"x\")",
            Box::new(std::io::sink()),
        );
        match result {
            Err(e) => assert!(
                matches!(&e.kind, KErrorKind::DispatchFailed { .. }),
                "expected DispatchFailed for unmatchable IF call, got {e}",
            ),
            Ok(()) => panic!("expected dispatch failure on IF with non-Bool predicate"),
        }
    }

    /// Sanity check the intentional-vs-error split: `IF false THEN ("nope")` returns Null
    /// (intentional skip), not an error. `PRINT "x"` similarly returns null and exits 0.
    /// These are the two surviving `null()` call sites.
    #[test]
    fn intentional_null_paths_do_not_surface_as_errors() {
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let buf = SharedBuf(captured.clone());
        let result = interpret_with_writer(
            "IF false THEN (\"nope\")\nPRINT \"hello\"",
            Box::new(buf),
        );
        assert!(result.is_ok(), "intentional-null paths should not error: {result:?}");
        assert_eq!(captured.borrow().as_slice(), b"hello\n");
    }

    /// Frame chain walks user-fn calls: an error in INNER, called from OUTER (via a
    /// non-tail position so OUTER's frame survives), surfaces with frames listing both
    /// function names. OUTER's body wraps INNER's call inside a `LET xx = (INNER)` so
    /// the body has 4 parts (not a single-Expression wrapper that the parser would peel)
    /// and INNER becomes a sub-Dispatch within OUTER's body — OUTER's slot then becomes
    /// a Forward holding OUTER's frame, and finalize appends OUTER's frame as the
    /// terminal Err propagates up. Direct `((INNER))` would peel to `INNER` and tail-call
    /// into INNER, causing TCO to replace OUTER's frame with INNER's.
    #[test]
    fn frame_chain_walks_nested_user_fn_calls() {
        let result = interpret_with_writer(
            "FN (INNER) = (undefined)\n\
             FN (OUTER) = (LET xx = (INNER))\n\
             OUTER",
            Box::new(std::io::sink()),
        );
        match result {
            Err(e) => {
                let frame_names: Vec<String> =
                    e.frames.iter().map(|f| f.function.clone()).collect();
                assert!(
                    frame_names.iter().any(|n| n.contains("INNER")),
                    "expected a frame mentioning INNER, got {:?} (full error: {})",
                    frame_names,
                    e,
                );
                assert!(
                    frame_names.iter().any(|n| n.contains("OUTER")),
                    "expected a frame mentioning OUTER, got {:?} (full error: {})",
                    frame_names,
                    e,
                );
            }
            Ok(()) => panic!("expected error from undefined name in INNER"),
        }
    }
}
