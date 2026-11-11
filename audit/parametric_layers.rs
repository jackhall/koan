//! What a resumption re-runs, lays and decides per hop, walked closed and witnessed
//! ([build/README.md § Resumption](../src/build/README.md#resumption)).
//!
//! Four tail loops in the decision memo's style ([`ran_under`]), each over a chain of links:
//!
//! - *one type per hop*: `PAIR` hands `LOOP` the same run-composed record every hop, its body
//!   holding units over `xs :(LIST OF Elt)` and one reading the bare `Elt` parameter `x`;
//! - *a novel type per hop*: that body, passing `{a = x}` on, so each hop binds a deeper record;
//! - *nothing held*: a body reading only `x`, each hop binding a deeper record;
//! - *a deepening entry under a statement*: that loop, whose statement `r` alternates between two
//!   modules.
//!
//! Each is run at two depths, so the load and the loop's fixed part cancel, walked closed
//! ([`UNWITNESSED`]) and witnessed. The table prints, per hop: the units re-run by the kind of open
//! mark each reaches, in witnessing walks and in closing walks (`unmarked` where it reaches none);
//! the binding events that walked, the decisions kept and the binding layers laid; the questions
//! the doors recorded; the share of re-runs that are a closing walk's; the closed types laid; the
//! bytes written in regions; and the bytes allocated. Declared in `src/dispatch/tests.rs`.

use std::cell::Cell;

use super::super::walk::{QUESTIONS, RERUNS, UNMARKED, WITNESSED};
use crate::program::activation_store::{DECIDED, KEPT};
use crate::program::resume::{LAYERS_LAID, UNWITNESSED};
use crate::scheduler::REGION_WRITTEN;
use crate::scope::{CLOSED_LAID, OpenKind};
use crate::values::BuildBudget;

use super::caching::ran_under;
use super::memo::{MAKE, alternating_deepening, deepening};

/// `PEEK`, whose overloads tell a string from anything else.
const PEEK: &str = "EXPR #(PEEK x :Any) -> Str = #(\"a\")\n\
                    EXPR #(PEEK x :Str) -> Number = #(1)\n";

/// `LOOP`'s overloads over `next`, what each hop hands the next as `x` and `xs`, and the call
/// that starts it.
fn looped(next: &str, start: &str) -> String {
    format!(
        "{PEEK}EXPR FOR ALL #[Elt] #(LOOP x :Elt xs :(LIST OF Elt) link :Link) -> Number = \
         #((LET ys = xs) (LET zs = ys) (PRINT (PEEK x)) (LOOP {next} (ATTR link next)))\n\
         EXPR FOR ALL #[Elt] #(LOOP x :Elt xs :(LIST OF Elt) end :Null) -> Number = #(7)\n\
         {start}"
    )
}

/// The four loops, by name.
fn programs() -> [(&'static str, String); 4] {
    [
        (
            "one type per hop",
            looped(
                "x zs",
                "EXPR FOR ALL #[Item] #(PAIR y :Item) -> Number = \
                 #(LOOP {a = y, b = y} [{a = y, b = y}] chain)\nPRINT (PAIR 1)\n",
            ),
        ),
        (
            "a novel type per hop",
            looped(
                "{a = x} [{a = x}]",
                "PRINT (LOOP {a = 1} [{a = 1}] chain)\n",
            ),
        ),
        ("nothing held", format!("{MAKE}{}", deepening())),
        (
            "a deepening entry under a statement",
            format!("{MAKE}{}", alternating_deepening()),
        ),
    ]
}

/// Re-runs by the kind of open mark each reaches, and at [`UNMARKED`] where it reaches none.
type Reruns = [usize; UNMARKED + 1];

/// What one run counted.
#[derive(Clone, Copy, Default)]
struct Counted {
    witnessing: Reruns,
    closing: Reruns,
    decided: usize,
    kept: usize,
    laid: usize,
    questions: usize,
    closed: usize,
    region: usize,
    bytes: u64,
}

/// Run `source` over `depth` links, counting.
fn counted(depth: u32, source: &str) -> Counted {
    RERUNS.with(|reruns| reruns.set([0; UNMARKED + 1]));
    WITNESSED.with(|reruns| reruns.set([0; UNMARKED + 1]));
    DECIDED.with(|decided| decided.set(0));
    KEPT.with(|kept| kept.set(0));
    LAYERS_LAID.with(|laid| laid.set(0));
    QUESTIONS.with(|questions| questions.set(0));
    CLOSED_LAID.with(|laid| laid.set(0));
    REGION_WRITTEN.with(|written| written.set(0));
    let before = crate::tests::allocation_bytes();
    let ran = ran_under(depth, source, BuildBudget::default());
    let bytes = crate::tests::allocation_bytes() - before;
    assert!(!ran.printed.contains("error"), "{}", ran.printed);
    Counted {
        witnessing: WITNESSED.with(Cell::get),
        closing: RERUNS.with(Cell::get),
        decided: DECIDED.with(Cell::get),
        kept: KEPT.with(Cell::get),
        laid: LAYERS_LAID.with(Cell::get),
        questions: QUESTIONS.with(Cell::get),
        closed: CLOSED_LAID.with(Cell::get),
        region: REGION_WRITTEN.with(Cell::get),
        bytes,
    }
}

/// The kinds a re-run is counted by, in [`RERUNS`]' order.
const KINDS: [OpenKind; UNMARKED] = [
    OpenKind::Verdict,
    OpenKind::NeverSatisfies,
    OpenKind::Fits,
    OpenKind::LowerEndOutside,
    OpenKind::Settles,
    OpenKind::Solve,
    OpenKind::Instance,
    OpenKind::EndRead,
    OpenKind::TypeNodeOfVariable,
    OpenKind::Retype,
    OpenKind::TypeExpression,
    OpenKind::Unequal,
    OpenKind::Unclosed,
    OpenKind::Overlap,
    OpenKind::Kind,
    OpenKind::ModuleBinder,
    OpenKind::TypeValue,
    OpenKind::ModuleContent,
];

/// Every re-run `reruns` counts.
fn all(reruns: &Reruns) -> f64 {
    reruns.iter().sum::<usize>() as f64
}

/// The depths each loop runs at: the smallest pair that shows the per-hop trend.
const DEPTHS: [u32; 2] = [4, 16];

/// `shallow`'s and `deep`'s re-runs, per hop, by kind: `none` where nothing re-runs.
fn per_kind(shallow: &Reruns, deep: &Reruns, hops: f64) -> String {
    let mut reruns = Vec::new();
    for at in 0..=UNMARKED {
        let count = (deep[at] as f64 - shallow[at] as f64) / hops;
        if count != 0.0 {
            let kind = KINDS
                .get(at)
                .map_or("unmarked".to_string(), |kind| format!("{kind:?}"));
            reruns.push(format!("{kind} {count:.1}"));
        }
    }
    match reruns.is_empty() {
        true => "none".to_string(),
        false => reruns.join(", "),
    }
}

#[test]
#[ignore = "a measurement, printed: run with --ignored --nocapture"]
fn parametric_layers() {
    let hops = f64::from(DEPTHS[1] - DEPTHS[0]);
    for (name, source) in programs() {
        println!(
            "{name}, per hop between depths {} and {}:",
            DEPTHS[0], DEPTHS[1]
        );
        for (column, closed) in [("before", true), ("after", false)] {
            UNWITNESSED.with(|on| on.set(closed));
            let [shallow, deep] = DEPTHS.map(|depth| counted(depth, &source));
            UNWITNESSED.with(|on| on.set(false));
            let per = |of: fn(&Counted) -> f64| (of(&deep) - of(&shallow)) / hops;
            let closing = per(|counted| all(&counted.closing));
            let witnessing = per(|counted| all(&counted.witnessing));
            let held = match closing + witnessing {
                0.0 => "-".to_string(),
                total => format!("{:.2}", closing / total),
            };
            println!(
                "  {column:6}  witnessing: {}  closing: {}  decided {:.1}  kept {:.1}  \
                 laid {:.1}  questions {:.1}  held {held}  closed types {:.1}  \
                 region bytes {:.0}  bytes {:.0}",
                per_kind(&shallow.witnessing, &deep.witnessing, hops),
                per_kind(&shallow.closing, &deep.closing, hops),
                per(|counted| counted.decided as f64),
                per(|counted| counted.kept as f64),
                per(|counted| counted.laid as f64),
                per(|counted| counted.questions as f64),
                per(|counted| counted.closed as f64),
                per(|counted| counted.region as f64),
                per(|counted| counted.bytes as f64),
            );
        }
    }
}
