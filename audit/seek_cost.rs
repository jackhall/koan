//! The per-call cost of a seek: one read off a request the load laid, one keyed from the values,
//! and a miss that walks its callee's body and evicts.
//!
//! Each case is a tail loop over [`caching::program`]'s chain of links whose body calls `TOUCH`, a
//! helper keyed on its module by a view in its body. A case is timed at two chain depths, so the
//! load and the loop's fixed part cancel, and less a base loop that hops the same way without the
//! seek. The table prints each case's tiers and how many seeks read a request or computed their
//! key, so a reader sees each case did what it claims. Declared in `src/dispatch/tests.rs`.

use std::cell::Cell;
use std::collections::HashMap;
use std::time::Instant;

use crate::program::{CellSubstrate, KBirth, REQUESTING, SEEKING};
use crate::scheduler::{TIERS, Tiers};
use crate::scope::Sought;
use crate::values::BuildBudget;

use super::super::Koan;
use super::caching::{SMALLEST, program};
use super::{WRITTEN, output};

/// `TOUCH`, keyed on `m` by its view, and `LOOP`, which hops down a chain passing `m` on bare and
/// runs `{step}` at each link: the template every case but the miss fills.
const LOOPED: &str = "EXPR #(TOUCH m :Counter) -> Str = #((LET v = (m :| Counter)) (\"x\"))\n\
                      EXPR #(LOOP m :Counter link :Link) -> Str = \
                      #({step} (LOOP m (ATTR link next)))\n\
                      EXPR #(LOOP m :Counter end :Null) -> Str = #(\"done\")\n\
                      PRINT (LOOP ints chain)";

/// `SWAP`, which hops down a chain swapping two modules at each link and runs `{step}` over the
/// first: the template of the miss and its base.
const SWAPPED: &str = "EXPR #(TOUCH m :Counter) -> Str = #((LET v = (m :| Counter)) (\"x\"))\n\
                       EXPR #(SWAP a :Counter b :Counter link :Link) -> Str = \
                       #({step} (SWAP b a (ATTR link next)))\n\
                       EXPR #(SWAP a :Counter b :Counter end :Null) -> Str = #(\"done\")\n\
                       PRINT (SWAP ints words chain)";

/// One measured case: its name, its program's template and step, the budget it runs under, and
/// the base case whose per-call time it is measured less.
struct Case {
    name: &'static str,
    template: &'static str,
    step: &'static str,
    budget: BuildBudget,
    base: Option<&'static str>,
}

/// The cases, each base before the cases measured less it.
fn cases() -> [Case; 6] {
    let roomy = BuildBudget::default();
    let case = |name, template, step, budget, base| Case {
        name,
        template,
        step,
        budget,
        base,
    };
    [
        case("base", LOOPED, "", roomy, None),
        case(
            "request",
            LOOPED,
            "(LET r = (TOUCH m))",
            roomy,
            Some("base"),
        ),
        case(
            "values-base",
            LOOPED,
            "(LET r = (m :! Counter))",
            roomy,
            None,
        ),
        case(
            "values",
            LOOPED,
            "(LET r = (TOUCH (m :! Counter)))",
            roomy,
            Some("values-base"),
        ),
        case(
            "miss-base",
            SWAPPED,
            "(LET r = (a :! Counter))",
            SMALLEST,
            None,
        ),
        case(
            "miss",
            SWAPPED,
            "(LET r = (TOUCH (a :! Counter)))",
            SMALLEST,
            Some("miss-base"),
        ),
    ]
}

thread_local! {
    /// How many seeks read a request, and how many computed their key from the values.
    static SEEKS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

fn requested(_: &KBirth<'_, '_>, _: &Sought) {
    SEEKS.with(|seeks| seeks.set((seeks.get().0 + 1, seeks.get().1)));
}

fn computed(_: &KBirth<'_, '_>, _: &Sought) {
    SEEKS.with(|seeks| seeks.set((seeks.get().0, seeks.get().1 + 1)));
}

/// One timed run: the nanoseconds the run took, not the load, beside its tiers and seeks.
#[derive(Clone, Copy)]
struct Run {
    nanos: f64,
    tiers: Tiers,
    seeks: (usize, usize),
}

/// The median of five runs of `case` over a chain of `depth` links, each on a fresh load, beside
/// the last run's tiers and seeks.
fn timed(case: &Case, depth: u32) -> Run {
    let source = program(depth, &case.template.replace("{step}", case.step));
    let mut runs: Vec<Run> = (0..5)
        .map(|_| {
            WRITTEN.with(|written| written.borrow_mut().clear());
            let mut substrate =
                CellSubstrate::load::<Koan>(&source, "<measured>", 2, case.budget, output())
                    .unwrap_or_else(|error| panic!("{}: the program loads: {error}", case.name));
            TIERS.with(|tiers| tiers.set(Tiers::default()));
            SEEKS.with(|seeks| seeks.set((0, 0)));
            REQUESTING.with(|hook| hook.set(Some(requested)));
            SEEKING.with(|hook| hook.set(Some(computed)));
            let start = Instant::now();
            substrate
                .with(|running| running.ran())
                .unwrap_or_else(|_| panic!("{}: the program runs", case.name));
            let nanos = start.elapsed().as_nanos() as f64;
            REQUESTING.with(|hook| hook.set(None));
            SEEKING.with(|hook| hook.set(None));
            let printed = WRITTEN.with(|written| written.borrow().join("\n"));
            assert_eq!(printed, "done", "{}", case.name);
            Run {
                nanos,
                tiers: TIERS.with(Cell::get),
                seeks: SEEKS.with(Cell::get),
            }
        })
        .collect();
    runs.sort_by(|left, right| left.nanos.total_cmp(&right.nanos));
    runs.swap_remove(2)
}

/// Each run [`timed`] so far, by case and depth, so a depth two figures share is run once.
#[derive(Default)]
struct Timings(HashMap<(usize, u32), Run>);

impl Timings {
    /// The run of the case at `at` over a chain of `depth` links.
    fn run(&mut self, at: usize, depth: u32) -> Run {
        *(self.0.entry((at, depth))).or_insert_with(|| timed(&cases()[at], depth))
    }

    /// The case at `at`'s per-call nanoseconds between depths `n` and `2n`, less its base's.
    fn net(&mut self, at: usize, n: u32) -> f64 {
        let raw = |timings: &mut Self, at| {
            (timings.run(at, 2 * n).nanos - timings.run(at, n).nanos) / f64::from(n)
        };
        let base = cases()[at].base.map_or(0.0, |base| {
            let base = cases().iter().position(|case| case.name == base);
            raw(self, base.expect("a base names a case"))
        });
        raw(self, at) - base
    }
}

/// Each case's per-call cost over `(n, 2n)`, at the smallest `n` from 250 at which its figures over
/// `(n, 2n)` and `(2n, 4n)` agree within 10%, doubling to 4000 at most; the tiers are its run at
/// `2n`. Each round's figures print as they come.
#[test]
#[ignore = "a measurement, printed: run with --ignored --nocapture"]
fn seek_cost() {
    let mut timings = Timings::default();
    let mut rows = Vec::new();
    for (at, case) in cases().iter().enumerate() {
        let mut n = 250;
        let figure = loop {
            let (low, high) = (timings.net(at, n), timings.net(at, 2 * n));
            println!("{} at n = {n}: {low:.0} ns, at 2n: {high:.0} ns", case.name);
            if (low - high).abs() <= 0.1 * low.abs().max(high.abs()) {
                break format!("{low:.0}");
            }
            if n >= 4000 {
                break format!("{low:.0} (unsettled)");
            }
            n *= 2;
        };
        let Tiers {
            sought,
            laid,
            settled,
            evicted,
            ..
        } = timings.run(at, 2 * n).tiers;
        let (requested, computed) = timings.run(at, 2 * n).seeks;
        rows.push(format!(
            "| {} | {figure} | {n} | {sought} | {laid} | {settled} | {evicted} | {requested} | \
             {computed} |",
            case.name
        ));
    }
    println!(
        "| case | per-call ns | n | sought | laid | settled | evicted | requested | computed |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    for row in rows {
        println!("{row}");
    }
}
