//! A tail loop's memory: what every region holds at each tail of a loop stays flat in its depth,
//! a hop copies only what it threads, and the host the loop delivers to holds the same bytes
//! however long the loop runs ([roadmap](../../../roadmap/rewrite/tail-loop-memory.md)).
//!
//! Three loops over [`caching::program`](super::caching::program)'s chain: a walk, which threads
//! nothing it built; a record rebuilt whole each hop; and a list consed onto each hop. Each body
//! first calls `TOUCH`, a callee whose own body tails, and each loop runs in a self-hopping form,
//! which hops under its frame's settlement, and a mutual form, which seeks one each hop. The
//! drain samples every region at each tail ([`HOPS`]); what a copy lays is counted in
//! [`COPIED`], and what a hop lays in the host's decision memo in `KEPT`. Each figure a run
//! counts outside its samples is taken against a run of the same program that prints `0` instead
//! of looping, so building the chain is not billed to the loop.

use std::cell::Cell;

use crate::program::activation_store::KEPT;
use crate::scheduler::{HOPS, HopSample};
use crate::values::{BuildBudget, COPIED};

/// The depths every loop runs at.
const DEPTHS: [u32; 3] = [16, 64, 256];

/// The samples skipped before a loop is warm: its first settlement and its first chunks.
const WARM: usize = 4;

/// What a figure may move by between two depths without growing in them: a few of a bump's first
/// chunks, which a region claims as its bytes double.
const SLACK: usize = 4096;

/// `TOUCH`, the callee each hop makes first, `ONWARD`, which hands back the link after the one it
/// is handed, and `LEN`, which counts a consed list.
const HELPERS: &str = "EXPR #(TOUCH m :Counter) -> Number = #(1)\n\
                       EXPR #(ONWARD link :Link) -> Any = #(ATTR link next)\n\
                       EXPR #(LEN n :Number xs :{head :Number, tail :Any}) -> Number = \
                       #(LEN (n + 1) (ATTR xs tail))\n\
                       EXPR #(LEN n :Number end :Null) -> Number = #(n)\n";

/// The walk: hands on the next link of the chain it was handed.
const WALK: &str = "EXPR #(NAME m :Counter link :Link) -> Str = \
                    #((LET t = (TOUCH m)) (NEXT m (ATTR link next)))\n\
                    EXPR #(NAME m :Counter end :Null) -> Str = #(\"done\")\n";

/// The record rebuild: hands on a record it builds whole from the one it was handed.
const STEP: &str = "EXPR #(NAME m :Counter r :{count :Number} link :Link) -> Number = \
                    #((LET t = (TOUCH m)) (NEXT m {count = ((ATTR r count) + 1)} (ATTR link next)))\n\
                    EXPR #(NAME m :Counter r :{count :Number} end :Null) -> Number = \
                    #(ATTR r count)\n";

/// The cons loop: hands on the list it was handed with one more cell in front.
const ACC: &str = "EXPR #(NAME m :Counter acc :Any link :Link) -> Any = \
                   #((LET t = (TOUCH m)) (NEXT m {head = 1, tail = acc} (ATTR link next)))\n\
                   EXPR #(NAME m :Counter acc :Any end :Null) -> Any = #(ATTR acc head)\n";

/// The walk through a helper that returns a part of its argument: `ONWARD` hands back the next
/// link, which the hop hands on.
const ONWARD: &str = "EXPR #(NAME m :Counter link :Link) -> Str = \
                      #((LET t = (TOUCH m)) (LET n = (ONWARD link)) (NEXT m n))\n\
                      EXPR #(NAME m :Counter end :Null) -> Str = #(\"done\")\n";

/// How a loop hops: into itself, under its frame's settlement, or between two callees, seeking
/// one each hop.
#[derive(Clone, Copy, Debug)]
enum Form {
    Selfward,
    Mutual,
}

/// `body`'s overloads named for `form`, and the name a call starts the loop at.
fn defined(body: &str, form: Form) -> (String, &'static str) {
    let named = |name: &str, next: &str| body.replace("NAME", name).replace("NEXT", next);
    match form {
        Form::Selfward => (named("LOOP", "LOOP"), "LOOP"),
        Form::Mutual => (named("PING", "PONG") + &named("PONG", "PING"), "PING"),
    }
}

/// What one run counted.
struct Measured {
    printed: String,
    /// The samples of the loop's own tails, warm-up skipped.
    hops: Vec<HopSample>,
    copied: usize,
    kept: usize,
    hopped: usize,
}

/// Run `defs` and `main` over a chain of `depth` links, counting from zero.
fn measured(depth: u32, defs: &str, main: &str) -> Measured {
    use crate::dispatch::evaluate::HOPPED;
    HOPS.with(|hops| hops.borrow_mut().clear());
    COPIED.with(|copied| copied.set(0));
    KEPT.with(|kept| kept.set(0));
    HOPPED.with(|hopped| hopped.set(0));
    let source = format!("{HELPERS}{defs}{main}");
    let printed = super::caching::ran_under(depth, &source, BuildBudget::default()).printed;
    let samples = HOPS.with(|hops| hops.take());
    // The loop's own tails are the ones its host receives: more than any callee's.
    let host = (samples.iter())
        .max_by_key(|sample| {
            samples
                .iter()
                .filter(|other| other.host == sample.host)
                .count()
        })
        .map(|sample| sample.host);
    let hops = (samples.into_iter())
        .filter(|sample| Some(sample.host) == host)
        .skip(WARM)
        .collect();
    Measured {
        printed,
        hops,
        copied: COPIED.with(Cell::get),
        kept: KEPT.with(Cell::get),
        hopped: HOPPED.with(Cell::get),
    }
}

/// [`measured`] over `body` in `form`, started by `start` (the loop's name prepended to its
/// arguments), beside the figures the same program counts printing `0` instead.
fn looped(depth: u32, body: &str, form: Form, arguments: &str) -> (Measured, Measured) {
    let (defs, name) = defined(body, form);
    let looped = measured(depth, &defs, &format!("PRINT ({name} {arguments})\n"));
    let baseline = measured(depth, &defs, "PRINT 0\n");
    (looped, baseline)
}

/// How far `live` spread over a run's samples.
fn spread(hops: &[HopSample]) -> usize {
    let live = hops.iter().map(|sample| sample.live);
    live.clone().max().unwrap_or(0) - live.min().unwrap_or(0)
}

/// Every form of `body` at every depth: what it printed checked against `printed`, and the spread
/// of its live bytes flat in the depth.
fn assert_flat(body: &str, arguments: &str, printed: impl Fn(u32) -> String) {
    for form in [Form::Selfward, Form::Mutual] {
        let spreads: Vec<usize> = (DEPTHS.iter())
            .map(|&depth| {
                let (looped, _) = looped(depth, body, form, arguments);
                assert_eq!(looped.printed, printed(depth), "{form:?} at {depth}");
                match form {
                    Form::Selfward => assert!(looped.hopped > 0, "a selfward hop stays under"),
                    Form::Mutual => assert_eq!(looped.hopped, 0, "a mutual hop seeks"),
                }
                spread(&looped.hops)
            })
            .collect();
        assert!(
            spreads[2] <= spreads[0] + SLACK,
            "{form:?}: live bytes spread {spreads:?} over depths {DEPTHS:?}"
        );
    }
}

/// The bytes a loop copied per hop at each depth, past what its program copies without looping.
fn copied_per_hop(body: &str, form: Form, arguments: &str) -> Vec<usize> {
    (DEPTHS.iter())
        .map(|&depth| {
            let (looped, baseline) = looped(depth, body, form, arguments);
            looped.copied.saturating_sub(baseline.copied) / depth as usize
        })
        .collect()
}

/// Whether two per-hop figures are the same constant, give or take what a depth's rounding moves.
fn same_per_hop(per_hop: &[usize]) -> bool {
    per_hop.iter().all(|&at| at.abs_diff(per_hop[0]) <= 8)
}

#[test]
#[ignore = "phase 6"]
fn a_chain_walk_holds_its_memory_flat_in_its_depth() {
    assert_flat(WALK, "ints chain", |_| "done".to_string());
}

#[test]
#[ignore = "phase 6"]
fn a_record_rebuilt_each_hop_holds_its_memory_flat_in_its_depth() {
    assert_flat(STEP, "ints {count = 0} chain", |depth| depth.to_string());
}

#[test]
fn a_chain_walk_copies_no_part_of_the_chain() {
    for form in [Form::Selfward, Form::Mutual] {
        let per_hop = copied_per_hop(WALK, form, "ints chain");
        assert!(
            same_per_hop(&per_hop),
            "{form:?}: {per_hop:?} bytes per hop"
        );
    }
}

#[test]
fn a_cons_loop_copies_a_constant_per_hop() {
    for form in [Form::Selfward, Form::Mutual] {
        let per_hop = copied_per_hop(ACC, form, "ints null chain");
        assert!(
            same_per_hop(&per_hop),
            "{form:?}: {per_hop:?} bytes per hop"
        );
    }
}

#[test]
#[ignore = "phase 7"]
fn a_cons_loop_holds_a_constant_times_what_it_threads() {
    for form in [Form::Selfward, Form::Mutual] {
        let (looped, _) = looped(256, ACC, form, "ints null chain");
        assert_eq!(looped.printed, "1");
        let hops = &looped.hops;
        let hop_region = hops
            .iter()
            .map(|sample| sample.hop_region)
            .max()
            .unwrap_or(0);
        for (at, sample) in hops.iter().enumerate() {
            let held = sample.live.saturating_sub(hops[0].live);
            assert!(
                held <= 4 * sample.threads + 2 * hop_region + SLACK,
                "{form:?}: at sample {at} the loop holds {held} B for {} B threaded",
                sample.threads
            );
        }
    }
}

#[test]
fn a_cons_loop_delivers_its_whole_list() {
    let body = ACC.replace("#(ATTR acc head)", "#(acc)");
    for form in [Form::Selfward, Form::Mutual] {
        let (defs, name) = defined(&body, form);
        let ran = measured(
            256,
            &defs,
            &format!("PRINT (LEN 0 ({name} ints null chain))\n"),
        );
        assert_eq!(ran.printed, "256", "{form:?}");
    }
}

#[test]
#[ignore = "phase 6"]
fn a_loops_host_holds_the_same_bytes_however_long_it_runs() {
    let loops = [
        (WALK, "ints chain"),
        (STEP, "ints {count = 0} chain"),
        (ACC, "ints null chain"),
    ];
    for (body, arguments) in loops {
        for form in [Form::Selfward, Form::Mutual] {
            let (looped, _) = looped(256, body, form, arguments);
            let (early, last) = (&looped.hops[2 * 16], looped.hops.last().unwrap());
            assert_eq!(
                (early.host_region, early.host_scratch),
                (last.host_region, last.host_scratch),
                "{form:?}: the host's region and scratch over hops 16 to 256"
            );
            let laid: Vec<usize> = (DEPTHS.iter())
                .map(|&depth| {
                    let (looped, baseline) = self::looped(depth, body, form, arguments);
                    looped.kept.saturating_sub(baseline.kept)
                })
                .collect();
            assert!(
                laid.iter().all(|&at| at == laid[0]),
                "{form:?}: memo entries laid at depths {DEPTHS:?}: {laid:?}"
            );
        }
    }
}

#[test]
#[ignore = "phase 6"]
fn a_call_returning_a_part_of_its_argument_places_nothing_new() {
    for form in [Form::Selfward, Form::Mutual] {
        let onward = copied_per_hop(ONWARD, form, "ints chain");
        let walk = copied_per_hop(WALK, form, "ints chain");
        assert_eq!(
            onward, walk,
            "{form:?}: bytes per hop through ONWARD and without"
        );
    }
    assert_flat(ONWARD, "ints chain", |_| "done".to_string());
}
