//! A resumption's cost is its callee's body and binding, not the program around them: a miss and
//! an activation rooted at one body, run over programs of size n and 4n, read the same number of
//! standing units ([build/README.md § Resumption](../../build/README.md#resumption)). A standing
//! body sizes no table, and a chain's path table opens empty, so neither reads the program around
//! them.
//!
//! Each case grows the program with one kind of statement, written before or after the callee:
//! plain `LET`s, callables, type paths through a view, type paths through an enclosing callable's
//! own module parameter, and type paths beside a block at the top level that reads a class
//! through a view.

use std::cell::Cell;

use crate::memory::{Bump, BumpVec};
use crate::program::CellSubstrate;
use crate::scheduler::{TIERS, Tiers};
use crate::scope::{Address, Binding, Bound, ShapeKind, Unknown};
use crate::symbols::{BinderSymbol, TypeSymbol};
use crate::type_lattice::KType;
use crate::values::tests::reading;

use super::super::Koan;
use super::super::walk::{LoadCounts, counted};
use super::caching::{PRELUDE, SMALLEST};
use super::{WRITTEN, output};

/// One kind of statement a case grows the program by, the `at`th of them.
#[derive(Clone, Copy, Debug)]
enum Growth {
    Plain,
    Callable,
    Path,
}

impl Growth {
    fn statement(self, at: usize) -> String {
        match self {
            Growth::Plain => format!("LET q{at} = {at}\n"),
            Growth::Callable => format!("LET q{at} = (FN :{{x :Number}} -> Number = #(x))\n"),
            Growth::Path => {
                format!("LET v{at} = (ints :| Counter)\nLET q{at} :(v{at}.Carrier) = v{at}.zero\n")
            }
        }
    }
}

/// `SWAP`, which hops down a three-link chain swapping two modules and runs `step` over the first
/// at each link: under the smallest budget each hop decides its facts in its activation, and a
/// `TOUCH` in `step` misses. `n` statements of `growth` stand before the definitions where `early`,
/// else after them.
fn swapped(growth: Growth, n: usize, early: bool, step: &str) -> String {
    let defined = format!(
        "EXPR #(TOUCH m :Counter) -> Str = #((LET v = (m :| Counter)) (\"x\"))\n\
         EXPR #(SWAP a :Counter b :Counter link :Link) -> Str = \
         #({step} (SWAP b a (ATTR link next)))\n\
         EXPR #(SWAP a :Counter b :Counter end :Null) -> Str = #(\"done\")\n"
    );
    let grown: String = (0..n).map(|at| growth.statement(at)).collect();
    let (before, after) = match early {
        true => (defined, grown),
        false => (grown, defined),
    };
    format!(
        "{PRELUDE}{before}{after}LET link0 = null\nLET link1 = (Link {{next = link0}})\n\
         LET link2 = (Link {{next = link1}})\nLET link3 = (Link {{next = link2}})\n\
         PRINT (SWAP ints words link3)"
    )
}

/// `OUTER`, whose body reads `n` paths through views of its module parameter before or after a
/// callable `g`, which reads a path of the same class, and picks a `PICKY` overload by its own
/// module's carrier, and is called over two modules, so each call misses and resumes rooted at
/// `g`, `OUTER` standing around it. `OUTER` is called over `ints`,
/// which the load traces, so its own settlement is laid at the load.
fn enclosed(n: usize, early: bool) -> String {
    let grown: String = (0..n)
        .map(|at| {
            format!("(LET v{at} = (m :| Counter)) (LET a{at} :(v{at}.Carrier) = v{at}.zero) ")
        })
        .collect();
    let callee = "(LET w = (m :| Counter)) \
                  (LET g = (FN :{k :Counter} -> Str = #((LET u :(w.Carrier) = w.zero) \
                  (LET y = (k :| Counter)) (PICKY k.zero)))) ";
    let (before, after) = match early {
        true => (callee.to_string(), grown),
        false => (grown, callee.to_string()),
    };
    format!(
        "{PRELUDE}EXPR #(PICKY x :Number) -> Str = #(\"n\")\n\
         EXPR #(PICKY x :Str) -> Str = #(\"s\")\n\
         EXPR #(OUTER m :Counter) -> Str = \
         #({before}{after}(g {{k = (EVAL #($ints) -> Counter)}}) \
         (g {{k = (EVAL #($words) -> Counter)}}))\n\
         PRINT (OUTER ints)"
    )
}

/// A `USING … SCOPE` block at the top level over a module the load does not see, whose `SAME`
/// solves its variable from a binder typed by a path through a view of it and from a value
/// retyped to the block's surfaced `Carrier`: `n` views and paths of `ints` stand before or after
/// the block.
fn top_block(n: usize, early: bool) -> String {
    let grown: String = (0..n).map(|at| Growth::Path.statement(at)).collect();
    let block = "PRINT\n  USING (mm :! Counter) SCOPE (\n    (LET s = (SAME px AND (pv.zero :! Carrier)))\n    \
                 \"x\")\n";
    let (before, after) = match early {
        true => (block.to_string(), grown),
        false => (grown, block.to_string()),
    };
    format!(
        "{PRELUDE}EXPR FOR ALL #[Elt] #(SAME a :Elt AND b :Elt) -> Elt = #(a)\n\
         LET mm = (EVAL #($ints) -> Counter)\nLET pv = (mm :| Counter)\n\
         LET px :(pv.Carrier) = pv.zero\n{before}{after}"
    )
}

/// What a resumption rooted at the top-level block of `source`, binding its surfaced `Carrier` to
/// `Number`, counted: the block's statement wakes, the program standing around it.
fn block_counts(source: &str) -> LoadCounts {
    let mut substrate = CellSubstrate::load::<Koan>(source, "<test>", 2, SMALLEST, output())
        .unwrap_or_else(|error| panic!("the program loads: {error}"));
    substrate.with(|running| {
        let bump = Bump::new();
        let types = reading(running.program().tiers(), &bump);
        let root = running.program().shapes().root();
        let block = (root.nested_shapes().iter())
            .find(|(_, nested)| nested.kind() == ShapeKind::Block)
            .map(|(_, block)| block.segment())
            .expect("the program runs a block");
        let shape = running.program().shapes().shape_at(block);
        let carrier = TypeSymbol::declared("Carrier", running.symbols()).expect("a Type token");
        let (slot, _) = (shape.slot(BinderSymbol::Type(carrier))).expect("the block surfaces it");
        let unknown = Unknown {
            segment: block,
            at: Address::Slot(slot),
        };
        let mut bound = BumpVec::new_in(&bump);
        bound.push((unknown, Bound::Type(KType::NUMBER)));
        let binding = Binding::laid(&bump, &mut bound);
        let (resumed, counts) = counted(|| {
            running
                .resume_at::<Koan>(&types, binding, block, &bump)
                .is_ok()
        });
        assert!(resumed, "the block resumes");
        counts
    })
}

/// What the run of `source`, not its load, counted, beside its tiers.
fn run_counts(source: &str) -> (LoadCounts, Tiers) {
    WRITTEN.with(|written| written.borrow_mut().clear());
    let mut substrate = CellSubstrate::load::<Koan>(source, "<test>", 2, SMALLEST, output())
        .unwrap_or_else(|error| panic!("the program loads: {error}"));
    TIERS.with(|tiers| tiers.set(Tiers::default()));
    let ((), counts) = counted(|| {
        substrate
            .with(|running| running.ran())
            .expect("the program runs");
    });
    let printed = WRITTEN.with(|written| written.borrow().join("\n"));
    assert!(!printed.starts_with("load:"), "{printed}");
    (counts, TIERS.with(Cell::get))
}

/// Assert that a run of `case(n)` and of `case(4n)` read as many standing units, and settle and
/// evict alike.
fn assert_independent(what: &str, case: impl Fn(usize) -> String) {
    let [(small, small_tiers), (large, large_tiers)] = [8, 32].map(|n| run_counts(&case(n)));
    assert_eq!(
        (small_tiers.settled, small_tiers.evicted),
        (large_tiers.settled, large_tiers.evicted),
        "{what}: the runs resume alike"
    );
    assert!(small.stood > 0, "{what}: the run resumes rooted");

    assert_eq!(small.stood, large.stood, "{what}: standing units read");
}

#[test]
fn a_resumption_costs_its_body_and_binding_however_large_the_program() {
    let steps = [
        ("activation", "(LET r = (a :! Counter))"),
        ("miss", "(LET r = (TOUCH (a :! Counter)))"),
    ];
    for growth in [Growth::Plain, Growth::Callable, Growth::Path] {
        for early in [true, false] {
            for (name, step) in steps {
                let what = format!("{name}, {growth:?} growth, callee early: {early}");
                assert_independent(&what, |n| swapped(growth, n, early, step));
            }
        }
    }
    for early in [true, false] {
        let what = format!("enclosed paths, callee early: {early}");
        assert_independent(&what, |n| enclosed(n, early));
        let [small, large] = [8, 32].map(|n| block_counts(&top_block(n, early)));
        assert!(
            small.stood > 0,
            "a top-level block reads the program standing"
        );
        assert_eq!(
            small.stood, large.stood,
            "a top-level block, block early: {early}: standing units read"
        );
    }
}
