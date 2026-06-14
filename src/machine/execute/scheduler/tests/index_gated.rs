//! End-to-end tests for the index-gated resolution rule.
//!
//! Shared expectation: a binding at index `i` is visible to a consumer at index `c` iff
//! `i < c` — type binders included, with no source-order exemption. `chain.index_for(scope)
//! = None` (a "complete" scope) makes every entry visible.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use crate::builtins::default_scope;
use crate::machine::execute::KoanHarness;
use crate::machine::model::{KObject, KType, Parseable};
use crate::machine::{KError, KErrorKind, RuntimeArena, Scope};
use crate::parse::parse;

struct Sink;
impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct SharedBuf(Rc<RefCell<Vec<u8>>>);
impl Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn run_scope<'run>(arena: &'run RuntimeArena, source: &str) -> &'run Scope<'run> {
    let scope = default_scope(arena, Box::new(Sink));
    let exprs = parse(source).expect("parse should succeed");
    let mut sched = KoanHarness::new();
    sched.enter_block(scope.id, exprs, scope);
    let _ = sched.execute();
    scope
}

fn run_collect_err(source: &str) -> Option<KError> {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(Sink));
    let exprs = parse(source).expect("parse should succeed");
    let mut sched = KoanHarness::new();
    let ids: Vec<_> = sched.enter_block(scope.id, exprs, scope);
    if let Err(e) = sched.execute() {
        return Some(e);
    }
    for id in ids {
        if let Err(e) = sched.read_result(id) {
            return Some(e.clone());
        }
    }
    None
}

#[test]
fn forward_value_let_at_same_level_is_unbound() {
    let err = run_collect_err("LET y = z\nLET z = 1").expect("forward LET should error");
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(n) if n == "z"),
        "expected UnboundName('z'), got {err}",
    );
}

#[test]
fn later_sibling_reference_is_unbound_name() {
    let err = run_collect_err("LET y = late\nLET other = 7\nLET late = 11")
        .expect("forward ref to a late sibling should error");
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(n) if n == "late"),
        "expected UnboundName('late'), got {err}",
    );
}

#[test]
fn backward_value_let_resolves() {
    let arena = RuntimeArena::new();
    let scope = run_scope(&arena, "LET z = 1\nLET y = z");
    assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 1.0));
}

/// A MODULE body's child scope is "complete" from the top-level call site's chain
/// (`index_for → None`), so a later top-level reference reads through the surfaced
/// members regardless of their inner indices.
#[test]
fn returned_block_locals_visible_from_outer_chain() {
    let arena = RuntimeArena::new();
    let scope = run_scope(
        &arena,
        "MODULE Mo = ((LET inside = 7) (LET also = 9))\n\
         LET result = Mo.inside",
    );
    assert!(
        matches!(scope.lookup("result"), Some(KObject::Number(n)) if *n == 7.0),
        "expected result = 7 via Mo.inside; got {:?}",
        scope.lookup("result").map(|o| o.summarize()),
    );
}

/// Per-scope index per-frame: a module body statement's chain has its own scope id
/// and its own index, so an inner backward ref resolves against the inner producer.
#[test]
fn nested_block_cutoff_is_per_scope() {
    let arena = RuntimeArena::new();
    let scope = run_scope(
        &arena,
        "LET top = 1\n\
         MODULE Mo = ((LET a = 2) (LET b = a))",
    );
    let m = match scope.resolve_type("Mo") {
        Some(KType::Module { module, frame: _ }) => *module,
        _ => panic!("Mo should be a module identity in types"),
    };
    let data = m.child_scope().bindings().data();
    assert!(
        matches!(data.get("b").map(|(o, _)| *o), Some(KObject::Number(n)) if *n == 2.0),
        "inner backward ref `b = a` should resolve a from the same module body",
    );
}

/// Mutual recursion across sibling FNs resolves because each FN body's chain is
/// assembled from the call site's chain at the FN's outer scope, not the FN's own
/// def index. UNION-tagged termination predicate bounds the recursion.
#[test]
fn mutual_recursion_across_sibling_fns_resolves_via_body_chain() {
    let arena = RuntimeArena::new();
    let buf = Rc::new(RefCell::new(Vec::new()));
    let scope = default_scope(&arena, Box::new(SharedBuf(buf.clone())));
    let exprs = parse(
        "UNION Tick = (More :Null Done :Null)\n\
         FN (PING n :Number c :Any) -> Number = (MATCH (c) -> :Number WITH (\
            More -> (PONG (n) (Tick (Done null)))\
            Done -> (n)\
         ))\n\
         FN (PONG n :Number c :Any) -> Number = (MATCH (c) -> :Number WITH (\
            More -> (PING (n) (Tick (Done null)))\
            Done -> (n)\
         ))\n\
         LET out = (PING 42 (Tick (More null)))",
    )
    .expect("parse should succeed");
    let mut sched = KoanHarness::new();
    for e in exprs {
        sched.add_dispatch(e, scope);
    }
    sched
        .execute()
        .expect("mutual FN recursion via body chain should succeed");
    assert!(
        matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 42.0),
        "expected out = 42 via mutual PING/PONG; got {:?}",
        scope.lookup("out").map(|o| o.summarize()),
    );
}

/// `USING Mo SCOPE (...)` opens a transparent window onto Mo's child bindings;
/// references to Mo's members inside the block resolve.
#[test]
fn using_block_post_reference_visible() {
    let arena = RuntimeArena::new();
    let scope = run_scope(
        &arena,
        "MODULE Mo = ((LET hidden = 99))\n\
         LET visible = (USING Mo SCOPE (hidden))",
    );
    assert!(
        matches!(scope.lookup("visible"), Some(KObject::Number(n)) if *n == 99.0),
        "expected visible = 99 via USING Mo SCOPE; got {:?}",
        scope.lookup("visible").map(|o| o.summarize()),
    );
}

/// `OverloadBucket::pick`'s per-overload visibility filter hides the later sibling
/// overload from a consumer between two overloads.
#[test]
fn overload_pre_filter_hides_later_sibling_overload() {
    let arena = RuntimeArena::new();
    let scope = run_scope(
        &arena,
        "FN (DESCRIBE xs :(LIST OF Number)) -> Str = (\"numbers\")\n\
         LET xs = [1 2 3]\n\
         LET result = (DESCRIBE xs)\n\
         FN (DESCRIBE xs :(LIST OF Str)) -> Str = (\"strings\")",
    );
    assert!(
        matches!(scope.lookup("result"), Some(KObject::KString(s)) if s == "numbers"),
        "expected result = 'numbers' (only earlier overload visible); got {:?}",
        scope.lookup("result").map(|o| o.summarize()),
    );
}

/// A FN parameter type that forward-references a STRUCT declared later is a position error:
/// type names obey source order, with no nominal-binder carve-out.
#[test]
fn struct_forward_reference_in_fn_param_is_position_error() {
    let err = run_collect_err(
        "FN (TAKES p :Pt) -> Number = (p.x)\n\
         NEWTYPE Pt = :{x :Number, y :Number}",
    )
    .expect("a forward STRUCT reference in a FN signature should error");
    assert!(
        format!("{err}").contains("Pt"),
        "expected the error to name the forward type `Pt`, got {err}",
    );
}

/// A forward `MODULE` reference is a position error too — a MODULE name obeys source order
/// like any other type name.
#[test]
fn forward_module_reference_is_position_error() {
    let err = run_collect_err(
        "LET inner = MyMod.x\n\
         MODULE MyMod = ((LET x = 11))",
    )
    .expect("a forward MODULE reference should error");
    assert!(
        format!("{err}").contains("MyMod"),
        "expected the error to name the forward module `MyMod`, got {err}",
    );
}

/// The nominal-binder carve-out does not apply to LET: a value-LET defined after
/// a reference at the same lexical level is `UnboundName`.
#[test]
fn value_let_after_reference_is_unbound_not_carved_out() {
    let err = run_collect_err("LET sees_later = later_name\nLET later_name = 42")
        .expect("value LET forward should error");
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(n) if n == "later_name"),
        "expected UnboundName('later_name'), got {err}",
    );
}

/// A FN return type that forward-references a STRUCT declared later is a position error,
/// just like a forward reference in a parameter type or a struct field.
#[test]
fn fn_return_type_forward_reference_is_position_error() {
    let err = run_collect_err(
        "FN (FOO x :Number) -> Later = (x)\n\
         NEWTYPE Later = :{n :Number}",
    )
    .expect("a forward STRUCT reference in a FN return type should error");
    assert!(
        format!("{err}").contains("Later"),
        "expected the error to name the forward type `Later`, got {err}",
    );
}

/// A backward return-type reference (the type is declared earlier) still resolves.
#[test]
fn fn_return_type_backward_reference_resolves() {
    let arena = RuntimeArena::new();
    let scope = run_scope(
        &arena,
        "NEWTYPE Early = :{n :Number}\n\
         FN (FOO x :Number) -> Early = (Early {n = x})\n\
         LET out = (FOO 5)",
    );
    assert!(
        scope.lookup("out").is_some(),
        "a backward return-type reference must resolve; got {:?}",
        scope.lookup("out").map(|o| o.summarize()),
    );
}
