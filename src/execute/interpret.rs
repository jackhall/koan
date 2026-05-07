use crate::dispatch::runtime::RuntimeArena;
use crate::dispatch::builtins::default_scope;
use crate::dispatch::runtime::{KError, KErrorKind};
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
    use crate::dispatch::runtime::KErrorKind;
    use crate::dispatch::values::KObject;
    use crate::dispatch::runtime::Scope;

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
    fn interprets_match_via_print() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        run(
            r#"PRINT (MATCH true WITH (true -> ("yes") false -> ("no")))"#,
            &arena,
            captured.clone(),
        );
        assert_eq!(captured.borrow().as_slice(), b"yes\n");
    }

    #[test]
    fn match_branch_resolves_outer_name() {
        // The branch body's lazy slot evaluates in the surrounding scope, so a name bound
        // before the MATCH (`greeting`) resolves through the outer chain at branch-dispatch
        // time. Integration-level coverage of the lazy-slot/closure-capture machinery from
        // a koan program (the `match_case` unit tests exercise it via test scaffolding).
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        run(
            "LET greeting = \"hi\"\nPRINT (MATCH true WITH (true -> (greeting) false -> (\"no\")))\n",
            &arena,
            captured.clone(),
        );
        assert_eq!(captured.borrow().as_slice(), b"hi\n");
    }

    #[test]
    fn match_unmatched_branch_skips_let_side_effect() {
        // The unmatched branch's body is never dispatched, so its `LET y = 1` must not
        // execute and `y` must remain unbound. Verifies the lazy-slot guarantee end-to-end:
        // unmatched branches are inert.
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run(
            "MATCH false WITH (true -> (LET y = 1) false -> (null))\nPRINT \"after\"\n",
            &arena,
            captured.clone(),
        );
        assert!(scope.data.borrow().get("y").is_none(), "unmatched branch's LET must not have bound y");
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

    // --- Dict literal integration tests ---

    use crate::dispatch::values::KKey;
    use crate::dispatch::types::Serializable;

    fn lookup_string_key<'a, 'b>(
        d: &'b std::collections::HashMap<Box<dyn Serializable + 'a>, KObject<'a>>,
        key: &str,
    ) -> Option<&'b KObject<'a>> {
        let probe: Box<dyn Serializable> = Box::new(KKey::String(key.to_string()));
        d.get(&probe)
    }

    fn lookup_number_key<'a, 'b>(
        d: &'b std::collections::HashMap<Box<dyn Serializable + 'a>, KObject<'a>>,
        key: f64,
    ) -> Option<&'b KObject<'a>> {
        let probe: Box<dyn Serializable> = Box::new(KKey::Number(key));
        d.get(&probe)
    }

    fn lookup_bool_key<'a, 'b>(
        d: &'b std::collections::HashMap<Box<dyn Serializable + 'a>, KObject<'a>>,
        key: bool,
    ) -> Option<&'b KObject<'a>> {
        let probe: Box<dyn Serializable> = Box::new(KKey::Bool(key));
        d.get(&probe)
    }

    #[test]
    fn let_binds_an_empty_dict_literal() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run("LET d = {}\n", &arena, captured);
        let data = scope.data.borrow();
        match data.get("d") {
            Some(KObject::Dict(entries)) => assert!(entries.is_empty()),
            _ => panic!("expected `d` bound to an empty Dict"),
        }
    }

    #[test]
    fn let_binds_a_dict_with_string_keys() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run(r#"LET d = {"a": 1, "b": 2}"#, &arena, captured);
        let data = scope.data.borrow();
        match data.get("d") {
            Some(KObject::Dict(entries)) => {
                assert_eq!(entries.len(), 2);
                assert!(matches!(lookup_string_key(entries, "a"), Some(KObject::Number(n)) if *n == 1.0));
                assert!(matches!(lookup_string_key(entries, "b"), Some(KObject::Number(n)) if *n == 2.0));
            }
            _ => panic!("expected `d` bound to a Dict"),
        }
    }

    #[test]
    fn let_binds_a_dict_with_number_keys() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run(r#"LET d = {1: "a", 2: "b"}"#, &arena, captured);
        let data = scope.data.borrow();
        match data.get("d") {
            Some(KObject::Dict(entries)) => {
                assert_eq!(entries.len(), 2);
                assert!(matches!(lookup_number_key(entries, 1.0), Some(KObject::KString(s)) if s == "a"));
                assert!(matches!(lookup_number_key(entries, 2.0), Some(KObject::KString(s)) if s == "b"));
            }
            _ => panic!("expected `d` bound to a Dict"),
        }
    }

    #[test]
    fn let_binds_a_dict_with_bool_keys() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run("LET d = {true: 1, false: 0}\n", &arena, captured);
        let data = scope.data.borrow();
        match data.get("d") {
            Some(KObject::Dict(entries)) => {
                assert_eq!(entries.len(), 2);
                assert!(matches!(lookup_bool_key(entries, true), Some(KObject::Number(n)) if *n == 1.0));
                assert!(matches!(lookup_bool_key(entries, false), Some(KObject::Number(n)) if *n == 0.0));
            }
            _ => panic!("expected `d` bound to a Dict"),
        }
    }

    #[test]
    fn bare_identifier_key_is_looked_up() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run(
            "LET name = \"alice\"\nLET d = {name: 1}\n",
            &arena,
            captured,
        );
        let data = scope.data.borrow();
        match data.get("d") {
            Some(KObject::Dict(entries)) => {
                assert_eq!(entries.len(), 1);
                // The key should be the looked-up value of `name`, not the literal "name".
                assert!(matches!(lookup_string_key(entries, "alice"), Some(KObject::Number(n)) if *n == 1.0));
                assert!(lookup_string_key(entries, "name").is_none());
            }
            _ => panic!("expected `d` bound to a Dict"),
        }
    }

    #[test]
    fn sub_expression_as_value_evaluates_eagerly() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run(r#"LET d = {"a": (LET y = 7)}"#, &arena, captured);
        let data = scope.data.borrow();
        match data.get("d") {
            Some(KObject::Dict(entries)) => {
                assert!(matches!(lookup_string_key(entries, "a"), Some(KObject::Number(n)) if *n == 7.0));
            }
            _ => panic!("expected `d` bound to a Dict"),
        }
        assert!(matches!(data.get("y"), Some(KObject::Number(n)) if *n == 7.0));
    }

    #[test]
    fn sub_expression_as_key_evaluates() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run(
            "LET k = \"x\"\nLET d = {(k): 1}\n",
            &arena,
            captured,
        );
        let data = scope.data.borrow();
        match data.get("d") {
            Some(KObject::Dict(entries)) => {
                assert!(matches!(lookup_string_key(entries, "x"), Some(KObject::Number(n)) if *n == 1.0));
            }
            _ => panic!("expected `d` bound to a Dict"),
        }
    }

    #[test]
    fn multiline_dict_binds_correctly() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run(
            "LET d = {\n  \"a\": 1\n  \"b\": 2\n}\n",
            &arena,
            captured,
        );
        let data = scope.data.borrow();
        match data.get("d") {
            Some(KObject::Dict(entries)) => {
                assert_eq!(entries.len(), 2);
                assert!(matches!(lookup_string_key(entries, "a"), Some(KObject::Number(n)) if *n == 1.0));
                assert!(matches!(lookup_string_key(entries, "b"), Some(KObject::Number(n)) if *n == 2.0));
            }
            _ => panic!("expected `d` bound to a Dict"),
        }
    }

    #[test]
    fn nested_dict_in_list_binds_correctly() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = run(r#"LET xs = [{"a": 1} {"b": 2}]"#, &arena, captured);
        let data = scope.data.borrow();
        match data.get("xs") {
            Some(KObject::List(outer)) => {
                assert_eq!(outer.len(), 2);
                match &outer[0] {
                    KObject::Dict(d) => assert!(matches!(
                        lookup_string_key(d, "a"),
                        Some(KObject::Number(n)) if *n == 1.0,
                    )),
                    _ => panic!("outer[0] should be a Dict"),
                }
            }
            _ => panic!("expected `xs` bound to a List"),
        }
    }

    #[test]
    fn non_scalar_key_returns_shape_error() {
        // Bind a variable to a list, then use it as a dict key via lookup. The list reaches
        // `KKey::try_from_kobject` at materialization time and is rejected.
        let result = interpret_with_writer(
            "LET k = [1 2]\nLET d = {(k): 1}",
            Box::new(std::io::sink()),
        );
        match result {
            Err(e) => assert!(
                matches!(&e.kind, KErrorKind::ShapeError(msg) if msg.contains("dict key")),
                "expected ShapeError mentioning dict key, got {e}",
            ),
            Ok(()) => panic!("expected ShapeError for non-scalar dict key"),
        }
    }

    #[test]
    fn unbound_identifier_key_returns_unbound_name() {
        let result = interpret_with_writer("LET d = {missing: 1}", Box::new(std::io::sink()));
        match result {
            Err(e) => assert!(
                matches!(&e.kind, KErrorKind::UnboundName(name) if name == "missing"),
                "expected UnboundName(\"missing\"), got {e}",
            ),
            Ok(()) => panic!("expected UnboundName for missing identifier key"),
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
            "FN (BAD) -> Any = (undefined_thing)\nBAD",
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
    /// `KError::DispatchFailed`. (`WAT THIS IS NOT FUNC` parses as a multi-token expression
    /// with ≥2-uppercase keyword tokens, so dispatch fails to find a match.)
    #[test]
    fn dispatch_failure_surfaces_as_kerror() {
        let result = interpret_with_writer(
            "WAT THIS IS NOT FUNC",
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
    /// `MATCH` requires `branches: KExpression`; passing a string literal in that slot
    /// fits the bucket shape (4 parts: `MATCH _ WITH _`) but fails the slot-type check,
    /// so dispatch finds zero candidates. Type mismatches that DO reach `bind` (only
    /// possible with an overload set richer than today's) would surface as
    /// `TypeMismatch` from the bind step.
    #[test]
    fn type_mismatch_at_dispatch_surfaces_as_dispatch_failed() {
        let result = interpret_with_writer(
            "MATCH true WITH \"not_an_expression\"",
            Box::new(std::io::sink()),
        );
        match result {
            Err(e) => assert!(
                matches!(&e.kind, KErrorKind::DispatchFailed { .. }),
                "expected DispatchFailed for unmatchable MATCH call, got {e}",
            ),
            Ok(()) => panic!("expected dispatch failure on MATCH with non-KExpression branches"),
        }
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
            "FN (INNER) -> Any = (undefined)\n\
             FN (OUTER) -> Any = (LET xx = (INNER))\n\
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

    #[test]
    fn tagged_union_full_program_via_type_token() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        run(
            "UNION Result = (ok: Str err: Str)\n\
             LET r = (Result (ok \"all good\"))\n\
             MATCH (r) WITH (ok -> (PRINT it) err -> (PRINT \"failed\"))",
            &arena,
            captured.clone(),
        );
        assert_eq!(captured.borrow().as_slice(), b"all good\n");
    }

    #[test]
    fn tagged_union_full_program_via_let_bound_type() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        run(
            "LET result = (UNION (ok: Str err: Str))\n\
             LET r = (result (err \"oops\"))\n\
             MATCH (r) WITH (ok -> (PRINT \"good\") err -> (PRINT it))",
            &arena,
            captured.clone(),
        );
        assert_eq!(captured.borrow().as_slice(), b"oops\n");
    }

    #[test]
    fn tagged_union_none_branch_runs() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        run(
            "UNION Maybe = (some: Number none: Null)\n\
             LET m = (Maybe (none null))\n\
             MATCH (m) WITH (some -> (PRINT \"some-branch\") none -> (PRINT \"none-branch\"))",
            &arena,
            captured.clone(),
        );
        assert_eq!(captured.borrow().as_slice(), b"none-branch\n");
    }
}
