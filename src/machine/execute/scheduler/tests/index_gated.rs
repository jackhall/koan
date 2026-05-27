//! End-to-end tests for the index-gated resolution rule. Each test pins one of the
//! cases from `scratch/plan-index-gated-resolution.md` Phase 6.
//!
//! The shared expectation: a binding at index `i` is visible to a consumer at index
//! `c` iff `i < c` (strict less-than) **or** the binding's `BindingIndex.nominal_binder`
//! flag is set (D7 carve-out: STRUCT / named UNION / SIG / FUNCTOR / MODULE).
//! `chain.index_for(scope) = None` (a "complete" scope) makes every entry visible.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use crate::builtins::default_scope;
use crate::machine::SchedulerHandle;
use crate::machine::execute::Scheduler;
use crate::machine::model::{KObject, KType, Parseable};
use crate::machine::{KError, KErrorKind, RuntimeArena, Scope};
use crate::parse::parse;

struct Sink;
impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { Ok(b.len()) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

/// Captures program PRINT output for assertions that need it.
struct SharedBuf(Rc<RefCell<Vec<u8>>>);
impl Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

fn run_scope<'a>(arena: &'a RuntimeArena, source: &str) -> &'a Scope<'a> {
    let scope = default_scope(arena, Box::new(Sink));
    let exprs = parse(source).expect("parse should succeed");
    let mut sched = Scheduler::new();
    sched.enter_block(scope.id, exprs, scope);
    let _ = sched.execute();
    scope
}

fn run_collect_err(source: &str) -> Option<KError> {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(Sink));
    let exprs = parse(source).expect("parse should succeed");
    let mut sched = Scheduler::new();
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

/// Plan §6 case 1: forward reference resolves under submission order, fails under
/// lexical order. The "resolves under submission order" half no longer applies
/// post-gate (the gate hides the producer regardless of submission timing); only
/// the lexical-order half remains. Backward refs cover the FIFO-park rails — see
/// `backward_value_let_resolves` below.
#[test]
fn forward_value_let_at_same_level_is_unbound() {
    let err = run_collect_err("LET y = z\nLET z = 1")
        .expect("forward LET should error");
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(n) if n == "z"),
        "expected UnboundName('z'), got {err}",
    );
}

/// Plan §6 case 2: later-sibling reference is `UnboundName`. Same shape as case 1
/// expressed as a multi-statement program where the consumer can't see any of the
/// later siblings.
#[test]
fn later_sibling_reference_is_unbound_name() {
    let err = run_collect_err("LET y = late\nLET other = 7\nLET late = 11")
        .expect("forward ref to a late sibling should error");
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(n) if n == "late"),
        "expected UnboundName('late'), got {err}",
    );
}

/// Plan §6 case 3: earlier-sibling reference is `Value`.
#[test]
fn backward_value_let_resolves() {
    let arena = RuntimeArena::new();
    let scope = run_scope(&arena, "LET z = 1\nLET y = z");
    assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 1.0));
}

/// Plan §6 case 4: deferred body sees returned-block locals (`index_for → None`
/// case). A MODULE body's child scope is "complete" from the top-level call site's
/// chain (the call-site chain doesn't list the module's child scope id), so a later
/// top-level reference reads through the surfaced members regardless of their inner
/// indices. Exercised via `MODULE Mo = ((LET inside = 7))` followed by a top-level
/// read of `Mo.inside` — the inner index of `inside` would otherwise outrank the
/// top-level statement's cutoff.
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

/// Plan §6 case 5: nested-block cutoff (per-scope index per-frame). A module body
/// statement's chain has its own scope id and its own index; an inner reference's
/// cutoff is the inner statement's index, not the outer's, so an inner backward ref
/// sees the inner producer correctly.
#[test]
fn nested_block_cutoff_is_per_scope() {
    let arena = RuntimeArena::new();
    let scope = run_scope(
        &arena,
        "LET top = 1\n\
         MODULE Mo = ((LET a = 2) (LET b = a))",
    );
    let m = match scope.lookup("Mo") {
        Some(KObject::KTypeValue(KType::Module { module, frame: _ })) => *module,
        _ => panic!("Mo should be a module"),
    };
    let data = m.child_scope().bindings().data();
    assert!(
        matches!(data.get("b").map(|(o, _)| *o), Some(KObject::Number(n)) if *n == 2.0),
        "inner backward ref `b = a` should resolve a from the same module body",
    );
}

/// Plan §6 case 6: mutual recursion across sibling FNs in the same block resolves
/// because each FN body's chain is assembled from the *call site's* chain at the
/// FN's outer scope, not the FN's own def index. By the time `FOO 1` runs at a
/// top-level idx past both decls, the assembled body chain carries the call-site
/// idx; every sibling FN at a lower idx is visible from the body. Bounded with a
/// UNION-tagged termination predicate so the test doesn't loop forever.
#[test]
fn mutual_recursion_across_sibling_fns_resolves_via_body_chain() {
    let arena = RuntimeArena::new();
    let buf = Rc::new(RefCell::new(Vec::new()));
    let scope = default_scope(&arena, Box::new(SharedBuf(buf.clone())));
    let exprs = parse(
        "UNION Tick = (more :Null done :Null)\n\
         FN (PING n :Number c :Tagged) -> Number = (MATCH (c) WITH (\
            more -> (PONG (n) (Tick (done null)))\
            done -> (n)\
         ))\n\
         FN (PONG n :Number c :Tagged) -> Number = (MATCH (c) WITH (\
            more -> (PING (n) (Tick (done null)))\
            done -> (n)\
         ))\n\
         LET out = (PING 42 (Tick (more null)))",
    )
    .expect("parse should succeed");
    let mut sched = Scheduler::new();
    for e in exprs { sched.add_dispatch(e, scope); }
    sched.execute().expect("mutual FN recursion via body chain should succeed");
    assert!(
        matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 42.0),
        "expected out = 42 via mutual PING/PONG; got {:?}",
        scope.lookup("out").map(|o| o.summarize()),
    );
}

/// Plan §6 case 7: USING block — post-USING reference visible; pre-USING not.
/// `USING Mo SCOPE (...)` opens a transparent window onto Mo's child bindings.
/// Inside the block, references to Mo's members resolve; outside / before the
/// USING, the same names do not. The post-USING half is exercised here.
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

/// Plan §6 case 8: function overload pre-filter — consumer between two overloads
/// sees only the earlier. `OverloadBucket::pick`'s per-overload visibility filter
/// hides the later sibling overload from the consumer, so the consumer picks the
/// only visible candidate (the earlier one).
#[test]
fn overload_pre_filter_hides_later_sibling_overload() {
    let arena = RuntimeArena::new();
    let scope = run_scope(
        &arena,
        "FN (DESCRIBE xs :(List Number)) -> Str = (\"numbers\")\n\
         LET xs = [1 2 3]\n\
         LET result = (DESCRIBE xs)\n\
         FN (DESCRIBE xs :(List Str)) -> Str = (\"strings\")",
    );
    assert!(
        matches!(scope.lookup("result"), Some(KObject::KString(s)) if s == "numbers"),
        "expected result = 'numbers' (only earlier overload visible); got {:?}",
        scope.lookup("result").map(|o| o.summarize()),
    );
}

/// Plan §6 case 9: type-side gate (STRUCT defined after / before the reference).
/// STRUCT is a nominal-binder carve-out, so a forward reference to a later
/// STRUCT resolves; a forward reference to a value LET aliasing a type fails.
#[test]
fn type_side_gate_struct_forward_resolves() {
    let arena = RuntimeArena::new();
    let scope = run_scope(
        &arena,
        "FN (TAKES p :Pt) -> Number = (p.x)\n\
         STRUCT Pt = (x :Number, y :Number)\n\
         LET p = (Pt (x = 5, y = 6))\n\
         LET result = (TAKES p)",
    );
    assert!(
        matches!(scope.lookup("result"), Some(KObject::Number(n)) if *n == 5.0),
        "STRUCT nominal-binder carve-out should allow the forward type-ref; got {:?}",
        scope.lookup("result").map(|o| o.summarize()),
    );
}

/// Plan §6 case 10: mutual recursion across nominal binders. Two sibling structs
/// referencing each other (`STRUCT A { b: B }` next to `STRUCT B { a: A }`)
/// elaborate as a 2-member SCC — both nominal identities are visible regardless
/// of source order.
#[test]
fn mutual_recursion_across_nominal_struct_binders() {
    let arena = RuntimeArena::new();
    let scope = run_scope(
        &arena,
        "STRUCT Alpha = (b :Beta)\n\
         STRUCT Beta = (a :Alpha)",
    );
    // Both identities must be registered in `bindings.types`. SCC close
    // installs both before any carrier write completes.
    assert!(scope.resolve_type("Alpha").is_some(), "STRUCT Alpha should be registered");
    assert!(scope.resolve_type("Beta").is_some(), "STRUCT Beta should be registered");
}

/// Companion to §6 case 10 for MODULE / FUNCTOR cross-references: a forward
/// `MODULE A` referenced by an earlier sibling resolves via the same
/// nominal-binder carve-out.
#[test]
fn nominal_module_forward_reference_resolves() {
    let arena = RuntimeArena::new();
    let scope = run_scope(
        &arena,
        "LET inner = MyMod.x\n\
         MODULE MyMod = ((LET x = 11))",
    );
    assert!(
        matches!(scope.lookup("inner"), Some(KObject::Number(n)) if *n == 11.0),
        "MODULE nominal-binder carve-out should allow the forward ref; got {:?}",
        scope.lookup("inner").map(|o| o.summarize()),
    );
}

/// Plan §6 case 11: value-LET defined after a reference at the same lexical level
/// is `UnboundName` (the nominal-binder carve-out does not apply to LET).
/// Distinct from case 1 in that it pins the explicit semantic message: LET is
/// value-style gated, not nominal.
#[test]
fn value_let_after_reference_is_unbound_not_carved_out() {
    let err = run_collect_err("LET sees_later = later_name\nLET later_name = 42")
        .expect("value LET forward should error");
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(n) if n == "later_name"),
        "expected UnboundName('later_name'), got {err}",
    );
}
