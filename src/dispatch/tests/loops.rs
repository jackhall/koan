//! A tail loop's memory: what every region holds at each tail of a loop stays flat in its depth,
//! a hop copies only what it threads, and the host the loop delivers to holds the same bytes
//! however long the loop runs ([tail hops](../../scheduler/README.md#tail-hops)).
//!
//! Four loops over [`caching::program`](super::caching::program)'s chain: a walk, which threads
//! nothing it built; a record rebuilt whole each hop; and a list consed onto each hop — and a view
//! of a large list homed above the loop, extended by one element each hop. Each body
//! first calls `TOUCH`, a callee whose own body tails, and each loop runs in a self-hopping form,
//! which hops under its frame's settlement, and a mutual form, which seeks one each hop. The
//! drain samples every region at each tail ([`HOPS`]); what a copy lays is counted in
//! [`COPIED`], and what a hop lays in the host's decision memo in `KEPT`. Each figure a run
//! counts outside its samples is taken against a run of the same program that prints `0` instead
//! of looping, so building the chain is not billed to the loop.
//!
//! The same loops through `MATCH` arms, the last tests here: a walk and a cons loop under
//! `MATCH … WITH`, a walk and tutorial 06's `DOUBLE` under `MATCH … UNDER`, a helper returning an
//! arm's `it`, and a `MATCH` over a scrutinee its frame builds.

use std::cell::Cell;

use crate::memory::{FIRST_CHUNK, SPLICES};
use crate::program::activation_store::KEPT;
use crate::scheduler::{HOPS, HopSample, SKELETONS};
use crate::values::{BuildBudget, COPIED};

/// The depths every loop runs at.
const DEPTHS: [u32; 3] = [16, 64, 256];

/// The samples skipped before a loop is warm: its first settlement and its first chunks.
const WARM: usize = 4;

/// What a figure may move by between two depths without growing in them: a few of a bump's first
/// chunks, which a region claims as its bytes double.
const SLACK: usize = FIRST_CHUNK;

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

/// The walk returning `Number`, whose hops are siblings that cross by a copy unless their
/// arguments pin from where they rest.
const WALK_NUMBER: &str = "EXPR #(NAME m :Counter link :Link) -> Number = \
                           #((LET t = (TOUCH m)) (NEXT m (ATTR link next)))\n\
                           EXPR #(NAME m :Counter end :Null) -> Number = #(0)\n";

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

/// A loop that hands on `PASSED`, which a test replaces with a value read from outside the loop,
/// unchanged each hop.
const PASS: &str = "EXPR #(NAME m :Counter xs :Any link :Link) -> Number = \
                    #((LET t = (TOUCH m)) (NEXT m PASSED (ATTR link next)))\n\
                    EXPR #(NAME m :Counter xs :Any end :Null) -> Number = #(0)\n";

/// A list of `length` numbers, written out.
fn numbers(length: usize) -> String {
    format!("[{}]", vec!["7"; length].join(" "))
}

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

/// [`measured`] over `prelude`'s top-level definitions and `body` in `form`, started by `start`
/// (the loop's name prepended to its arguments), beside the figures the same program counts
/// printing `0` instead.
fn looped(
    depth: u32,
    prelude: &str,
    body: &str,
    form: Form,
    arguments: &str,
) -> (Measured, Measured) {
    let (defined, name) = defined(body, form);
    let defs = format!("{prelude}{defined}");
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
                let (looped, _) = looped(depth, "", body, form, arguments);
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

/// The bytes a loop copied per hop between each two depths, past what its program copies without
/// looping: the marginal cost of a hop, which a fixed cost at the loop's entry does not move.
fn copied_per_hop(prelude: &str, body: &str, form: Form, arguments: &str) -> Vec<usize> {
    let copied: Vec<usize> = (DEPTHS.iter())
        .map(|&depth| {
            let (looped, baseline) = looped(depth, prelude, body, form, arguments);
            looped.copied.saturating_sub(baseline.copied)
        })
        .collect();
    (copied.windows(2).zip(DEPTHS.windows(2)))
        .map(|(copied, depths)| (copied[1] - copied[0]) / (depths[1] - depths[0]) as usize)
        .collect()
}

/// Whether the per-hop figures are one constant, give or take what a depth's rounding moves.
fn same_per_hop(per_hop: &[usize]) -> bool {
    per_hop.iter().all(|&at| at.abs_diff(per_hop[0]) <= 8)
}

#[test]
fn a_chain_walk_holds_its_memory_flat_in_its_depth() {
    assert_flat(WALK, "ints chain", |_| "done".to_string());
}

#[test]
fn a_record_rebuilt_each_hop_holds_its_memory_flat_in_its_depth() {
    assert_flat(STEP, "ints {count = 0} chain", |depth| depth.to_string());
}

#[test]
fn a_chain_walk_copies_no_part_of_the_chain() {
    for form in [Form::Selfward, Form::Mutual] {
        let per_hop = copied_per_hop("", WALK, form, "ints chain");
        assert!(
            same_per_hop(&per_hop),
            "{form:?}: {per_hop:?} bytes per hop"
        );
    }
}

#[test]
fn a_hop_hands_on_its_received_arguments_without_copying_them() {
    for form in [Form::Selfward, Form::Mutual] {
        SKELETONS.with(|count| count.set(0));
        let (looped, _) = looped(16, "", WALK_NUMBER, form, "ints chain");
        assert_eq!(looped.printed, "0");
        let skeletons = SKELETONS.with(Cell::get);
        assert!(
            skeletons >= 16,
            "{form:?}: {skeletons} hops built over a skeleton"
        );
        let per_hop = copied_per_hop("", WALK_NUMBER, form, "ints chain");
        assert!(
            same_per_hop(&per_hop),
            "{form:?}: {per_hop:?} bytes per hop"
        );
    }
}

/// No hop is pledged into the loop cell or the host: a value built in a hop that a loop threads is
/// copied out of it, so the splices a loop makes do not grow with its depth.
#[test]
fn a_loop_pledges_none_of_its_hops() {
    let loops = [(STEP, "ints {count = 0} chain"), (ACC, "ints null chain")];
    for (body, arguments) in loops {
        for form in [Form::Selfward, Form::Mutual] {
            let (defs, name) = defined(body, form);
            let splices: Vec<usize> = (DEPTHS.iter())
                .map(|&depth| {
                    let spliced = |main: &str| {
                        SPLICES.with(|splices| splices.set(0));
                        let _ = measured(depth, &defs, main);
                        SPLICES.with(Cell::get)
                    };
                    let looped = spliced(&format!("PRINT ({name} {arguments})\n"));
                    looped.saturating_sub(spliced("PRINT 0\n"))
                })
                .collect();
            assert!(
                splices.iter().all(|&at| at == splices[0]),
                "{form:?}: splices at depths {DEPTHS:?}: {splices:?}"
            );
        }
    }
}

/// Each hop builds its cell in the loop cell over the list it was handed, so it copies nothing
/// of the list; the loop cell compacts as its bytes double, so what compactions copy comes to a
/// constant per hop, amortized: no more per hop the deeper the loop runs, give or take where the
/// compactions fall.
#[test]
fn a_cons_loop_copies_a_constant_per_hop() {
    for form in [Form::Selfward, Form::Mutual] {
        let per_hop = copied_per_hop("", ACC, form, "ints null chain");
        assert!(
            // Where compactions fall moves the figure a little; one growing with the list would
            // multiply it by the depth's ratio.
            per_hop
                .windows(2)
                .all(|pair| pair[1] <= pair[0] + pair[0] / 4),
            "{form:?}: {per_hop:?} bytes per hop"
        );
    }
}

#[test]
fn a_cons_loop_holds_a_constant_times_what_it_threads() {
    for form in [Form::Selfward, Form::Mutual] {
        let (looped, _) = looped(256, "", ACC, form, "ints null chain");
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
                held <= 4 * sample.threads + hop_region + SLACK,
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
fn a_loops_host_holds_the_same_bytes_however_long_it_runs() {
    let pass_big = PASS.replace("PASSED", "big");
    let pass_member = PASS.replace("PASSED", "(ATTR store data)");
    let (big, store) = (
        format!("LET big = {}\n", numbers(4)),
        format!("MODULE store = (LET data = {})\n", numbers(4)),
    );
    // The last three re-keep, every hop, a carrier of a part homed in the host.
    let loops = [
        ("", WALK, "ints chain"),
        ("", STEP, "ints {count = 0} chain"),
        ("", ACC, "ints null chain"),
        (big.as_str(), pass_big.as_str(), "ints null chain"),
        (store.as_str(), pass_member.as_str(), "ints null chain"),
        ("", ONWARD, "ints chain"),
    ];
    for (prelude, body, arguments) in loops {
        for form in [Form::Selfward, Form::Mutual] {
            let (looped, _) = looped(256, prelude, body, form, arguments);
            let (early, last) = (&looped.hops[2 * 16], looped.hops.last().unwrap());
            assert_eq!(
                (early.host_region, early.host_scratch),
                (last.host_region, last.host_scratch),
                "{form:?}: the host's region and scratch over hops 16 to 256"
            );
            let laid: Vec<usize> = (DEPTHS.iter())
                .map(|&depth| {
                    let (looped, baseline) = self::looped(depth, prelude, body, form, arguments);
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

/// `ONWARD` hands back the field of its argument unchanged, at the home its caller built the
/// argument record in — the hop's own frame — so it places nothing new there, and the host holds
/// flat.
#[test]
fn a_call_returning_a_part_of_its_argument_places_nothing_new() {
    for form in [Form::Selfward, Form::Mutual] {
        crate::scheduler::PASSED.with(|passed| passed.set(0));
        let (looped, _) = looped(64, "", ONWARD, form, "ints chain");
        assert_eq!(looped.printed, "done");
        let passed = crate::scheduler::PASSED.with(Cell::get);
        assert!(
            passed >= 64,
            "{form:?}: {passed} values handed back unchanged"
        );
        let (early, last) = (&looped.hops[2 * 16], looped.hops.last().unwrap());
        assert_eq!(
            early.host_region, last.host_region,
            "{form:?}: the host holds flat"
        );
    }
}

/// A loop whose overloads pass on unchanged a list `xs` while they rebuild a record each hop, and
/// `ENTRY`, whose tail call into it builds `xs`, of `length` numbers.
fn kept_list(form: Form, length: usize) -> String {
    let body = "EXPR #(NAME m :Counter xs :Any r :{count :Number} link :Link) -> Number = \
                #((LET t = (TOUCH m)) (NEXT m xs {count = ((ATTR r count) + 1)} (ATTR link next)))\n\
                EXPR #(NAME m :Counter xs :Any r :{count :Number} end :Null) -> Number = \
                #(ATTR r count)\n";
    let (defs, name) = defined(body, form);
    let list = numbers(length);
    format!(
        "{defs}EXPR #(ENTRY m :Counter link :Link) -> Number = #({name} m {list} {{count = 0}} link)\n"
    )
}

/// The bytes `defs` and `main` copied per hop between 64 and 256 links, past what `defs` copies
/// printing `0`, each run checked against `printed` at its depth.
fn copied_per_hop_of(defs: &str, main: &str, printed: impl Fn(u32) -> String) -> usize {
    let copied = |depth: u32| {
        let looped = measured(depth, defs, main);
        assert_eq!(looped.printed, printed(depth));
        looped.copied - measured(depth, defs, "PRINT 0\n").copied
    };
    (copied(256) - copied(64)) / (256 - 64)
}

/// What a value the loop's first hop built and every hop after passes on unchanged costs per hop:
/// it rests in the loop cell, so a compaction carries it into the fresh one, and no hop copies it
/// into its own region. The per-hop copy does not grow with the value.
#[test]
fn a_value_passed_on_unchanged_rides_the_loop_cell_through_its_compactions() {
    for form in [Form::Selfward, Form::Mutual] {
        let per_hop = |length: usize| {
            let defs = kept_list(form, length);
            copied_per_hop_of(&defs, "PRINT (ENTRY ints chain)\n", |depth| {
                depth.to_string()
            })
        };
        // Copied every hop, 256 numbers would add some 6 KB a hop; what compactions carry adds
        // a fraction of that, amortized.
        let (short, long) = (per_hop(4), per_hop(256));
        assert!(
            long <= short + 256,
            "{form:?}: {short} B per hop with 4 numbers, {long} with 256"
        );
    }
}

/// Whether two per-hop figures are the same, give or take what a depth's rounding moves.
fn same_figures(short: &[usize], long: &[usize]) -> bool {
    short.iter().zip(long).all(|(a, b)| a.abs_diff(*b) <= 8)
}

/// `PASS` handing on `passed`, which `prelude(length)` defines over `length` numbers: the bytes
/// it copies per hop with 4 numbers and with 256.
fn passing_per_hop(
    prelude: impl Fn(usize) -> String,
    passed: &str,
    form: Form,
) -> (Vec<usize>, Vec<usize>) {
    let body = PASS.replace("PASSED", passed);
    let at = |length| copied_per_hop(&prelude(length), &body, form, "ints null chain");
    (at(4), at(256))
}

/// A capture is a part of the frame's kept callee, and a top-level name a capture: a loop hands
/// it on where it rests.
#[test]
fn a_loop_hands_on_a_top_level_name_without_copying_it() {
    for form in [Form::Selfward, Form::Mutual] {
        let big = |length| format!("LET big = {}\n", numbers(length));
        let (short, long) = passing_per_hop(big, "big", form);
        assert!(
            same_figures(&short, &long),
            "{form:?}: {short:?} B per hop with 4 numbers, {long:?} with 256"
        );
    }
}

/// A module member read is held beside the module's kept part, one member segment down.
#[test]
fn a_loop_hands_on_a_module_member_without_copying_it() {
    for form in [Form::Selfward, Form::Mutual] {
        let store = |length| format!("MODULE store = (LET data = {})\n", numbers(length));
        let (short, long) = passing_per_hop(store, "(ATTR store data)", form);
        assert!(
            same_figures(&short, &long),
            "{form:?}: {short:?} B per hop with 4 numbers, {long:?} with 256"
        );
    }
}

/// `INNER`, tied in `OUTER`'s body, captures `OUTER`'s parameter `ys` and hands it on each hop;
/// `OUTER` reaches it through `enter`, a spawn or a tail.
fn outer(enter: &str) -> String {
    format!(
        "EXPR #(OUTER m :Counter ys :Any link :Link) -> Number = #(\
         (EXPR #(INNER m :Counter zs :Any l :Link) -> Number = \
         #((LET t = (TOUCH m)) (INNER m ys (ATTR l next)))) \
         (EXPR #(INNER m :Counter zs :Any end :Null) -> Number = #(0)) {enter})\n"
    )
}

/// The bytes `defs` copies per hop, `OUTER` handed 4 numbers and 256.
fn outer_per_hop(defs: &str) -> (usize, usize) {
    let at = |length| {
        let main = format!("PRINT (OUTER ints {} chain)\n", numbers(length));
        copied_per_hop_of(defs, &main, |_| "0".to_string())
    };
    (at(4), at(256))
}

#[test]
fn a_loop_hands_on_its_enclosing_frames_capture_without_copying_it() {
    let entries = [
        ("spawned", "(LET r = (INNER m null link)) (r)"),
        ("tailed", "(INNER m null link)"),
    ];
    for (entry, enter) in entries {
        let (short, long) = outer_per_hop(&outer(enter));
        assert!(
            short.abs_diff(long) <= 8,
            "{entry}: {short} B per hop with 4 numbers, {long} with 256"
        );
    }
}

/// A helper the frame ties and tails into is its callee's knot, tied in the frame's region: the
/// tail copies it into the loop cell once, not each hop.
#[test]
fn a_tied_helper_a_frame_tails_into_is_copied_once() {
    let defs = "EXPR #(OUTER m :Counter ys :Any link :Link) -> Number = #(\
                (EXPR #(INNER m :Counter l :Link) -> Number = \
                #((LET t = (TOUCH m)) (LET u = ys) (INNER m (ATTR l next)))) \
                (EXPR #(INNER m :Counter end :Null) -> Number = #(0)) (INNER m link))\n";
    let (short, long) = outer_per_hop(defs);
    assert!(
        short.abs_diff(long) <= 8,
        "{short} B per hop with 4 numbers, {long} with 256"
    );
}

/// A mutual hop's callee is a capture of its predecessor's callee, so it is handed on where it
/// rests, as a hop into itself hands on its own.
#[test]
fn a_mutual_loop_copies_none_of_its_callees() {
    let big = format!("LET big = {}\n", numbers(4));
    let pass = PASS.replace("PASSED", "big");
    let loops = [
        ("", WALK_NUMBER, "ints chain"),
        (big.as_str(), pass.as_str(), "ints null chain"),
    ];
    for (prelude, body, arguments) in loops {
        let selfward = copied_per_hop(prelude, body, Form::Selfward, arguments);
        let mutual = copied_per_hop(prelude, body, Form::Mutual, arguments);
        assert!(
            same_figures(&selfward, &mutual),
            "{selfward:?} B per hop selfward, {mutual:?} mutual"
        );
    }
}

/// A spawned frame keeps its argument at the home its caller kept it, so `ONWARD` hands back a
/// field homed where the chain rests, and the hop hands it on without copying it.
#[test]
fn a_chain_walk_through_a_helper_copies_no_part_of_the_chain() {
    for form in [Form::Selfward, Form::Mutual] {
        let per_hop = copied_per_hop("", ONWARD, form, "ints chain");
        assert!(
            same_per_hop(&per_hop),
            "{form:?}: {per_hop:?} bytes per hop"
        );
    }
}

#[test]
fn a_chain_walk_through_a_helper_holds_its_memory_flat_in_its_depth() {
    assert_flat(ONWARD, "ints chain", |_| "done".to_string());
}

/// The view loop: hands on a view of the list it was handed with one more element after it.
const VIEW: &str = "EXPR #(NAME m :Counter v :(LIST OF Number) link :Link) -> Number = \
                    #((LET t = (TOUCH m)) (NEXT m [..v 1] (ATTR link next)))\n\
                    EXPR #(NAME m :Counter v :(LIST OF Number) end :Null) -> Number = \
                    #(LENGTH v)\n";

/// Each hop builds its view in the loop cell over the list it was handed, where the large source
/// homed above the loop pins free, so no hop copies the source's cells: what a loop copies per hop
/// is a constant, and each hop lays the few tree nodes its edit makes — a growth per hop far below
/// the source's cells, which a flat rebuild would lay again every hop.
#[test]
fn a_view_threaded_through_a_loop_copies_none_of_its_source() {
    let big = format!("LET big = {}\n", numbers(10_000));
    let source = 10_000 * size_of::<crate::values::Value<'static>>();
    for form in [Form::Selfward, Form::Mutual] {
        let spreads: Vec<usize> = (DEPTHS.iter())
            .map(|&depth| {
                let (looped, _) = looped(depth, &big, VIEW, form, "ints big chain");
                assert_eq!(looped.printed, (10_000 + depth).to_string(), "{form:?}");
                spread(&looped.hops)
            })
            .collect();
        for (pair, depths) in spreads.windows(2).zip(DEPTHS.windows(2)) {
            let per_hop = (pair[1] - pair[0]) / (depths[1] - depths[0]) as usize;
            assert!(
                per_hop < source / 64,
                "{form:?}: the loop grows {per_hop} B a hop over a {source} B source"
            );
        }
        let per_hop = copied_per_hop(&big, VIEW, form, "ints big chain");
        assert!(
            same_per_hop(&per_hop) && per_hop[0] < source / 64,
            "{form:?}: {per_hop:?} bytes copied per hop"
        );
    }
}

/// The walk through a `MATCH`: its `Link` arm's tail call hands on the next link of the link the
/// arm admitted as `it`.
const MATCH_WALK: &str = "EXPR #(NAME m :Counter link :Any) -> Str = \
                          #((LET t = (TOUCH m)) (MATCH link WITH \
                          #{Link: (NEXT m (ATTR it next)), Null: (\"done\")}))\n";

/// The cons loop through a `MATCH`: its `Link` arm hands on the list it was handed with one more
/// cell in front.
const MATCH_ACC: &str = "EXPR #(NAME m :Counter acc :Any link :Any) -> Any = \
                         #((LET t = (TOUCH m)) (MATCH link WITH \
                         #{Link: (NEXT m {head = 1, tail = acc} (ATTR it next)), \
                         Null: (ATTR acc head)}))\n";

#[test]
fn a_chain_walk_through_a_match_holds_its_memory_flat_and_copies_none_of_the_chain() {
    assert_flat(MATCH_WALK, "ints chain", |_| "done".to_string());
    for form in [Form::Selfward, Form::Mutual] {
        let per_hop = copied_per_hop("", MATCH_WALK, form, "ints chain");
        assert!(
            same_per_hop(&per_hop),
            "{form:?}: {per_hop:?} bytes per hop"
        );
    }
}

#[test]
fn a_cons_loop_through_a_match_holds_a_constant_times_what_it_threads() {
    for form in [Form::Selfward, Form::Mutual] {
        let (looped, _) = looped(256, "", MATCH_ACC, form, "ints null chain");
        assert_eq!(looped.printed, "1");
        let hops = &looped.hops;
        let hop_region = (hops.iter())
            .map(|sample| sample.hop_region)
            .max()
            .unwrap_or(0);
        for (at, sample) in hops.iter().enumerate() {
            let held = sample.live.saturating_sub(hops[0].live);
            assert!(
                held <= 4 * sample.threads + hop_region + SLACK,
                "{form:?}: at sample {at} the loop holds {held} B for {} B threaded",
                sample.threads
            );
        }
    }
}

/// `Nat`, `zero`, and a `Nat` of `depth` bound to `nat`: what a loop under `MATCH … UNDER` walks.
fn nat(depth: u32) -> String {
    let mut prelude = String::from(
        "UNION Nat = #{Zero: Null, Succ: Nat}\nLET zero = (Nat.Zero null)\nLET nat0 = zero\n",
    );
    for at in 1..=depth {
        prelude.push_str(&format!("LET nat{at} = (Nat.Succ nat{})\n", at - 1));
    }
    prelude + &format!("LET nat = nat{depth}\n")
}

/// The walk down a `Nat` under its union: its `Succ` arm hands on the payload it binds as `it`.
const NAT_WALK: &str = "EXPR #(NAME m :Counter n :Nat) -> Str = \
                        #((LET t = (TOUCH m)) (MATCH n UNDER Nat WITH \
                        #{Zero: (\"done\"), Succ: (NEXT m it)}))\n";

/// Tutorial 06's `DOUBLE`: walks `n` down, handing on an accumulator two `Succ` layers deeper.
const DOUBLE: &str = "EXPR #(NAME m :Counter n :Nat acc :Nat) -> Nat = \
                      #((LET t = (TOUCH m)) (MATCH n UNDER Nat WITH \
                      #{Zero: (acc), Succ: (NEXT m it (Nat.Succ (Nat.Succ acc)))}))\n";

/// [`DOUBLE`] with no `MATCH`: walks the chain by overload, handing on the same accumulator.
const DOUBLE_PLAIN: &str = "EXPR #(NAME m :Counter link :Link acc :Nat) -> Nat = \
                            #((LET t = (TOUCH m)) \
                            (NEXT m (ATTR link next) (Nat.Succ (Nat.Succ acc))))\n\
                            EXPR #(NAME m :Counter end :Null acc :Nat) -> Nat = #(acc)\n";

#[test]
fn a_walk_under_a_union_holds_its_memory_flat_and_copies_none_of_it() {
    for form in [Form::Selfward, Form::Mutual] {
        let spreads: Vec<usize> = (DEPTHS.iter())
            .map(|&depth| {
                let (looped, _) = looped(depth, &nat(depth), NAT_WALK, form, "ints nat");
                assert_eq!(looped.printed, "done", "{form:?} at {depth}");
                spread(&looped.hops)
            })
            .collect();
        assert!(
            spreads[2] <= spreads[0] + SLACK,
            "{form:?}: live bytes spread {spreads:?} over depths {DEPTHS:?}"
        );
        let copied: Vec<usize> = (DEPTHS.iter())
            .map(|&depth| {
                let (looped, baseline) = looped(depth, &nat(depth), NAT_WALK, form, "ints nat");
                looped.copied.saturating_sub(baseline.copied)
            })
            .collect();
        let per_hop: Vec<usize> = (copied.windows(2).zip(DEPTHS.windows(2)))
            .map(|(copied, depths)| (copied[1] - copied[0]) / (depths[1] - depths[0]) as usize)
            .collect();
        assert!(
            same_per_hop(&per_hop),
            "{form:?}: {per_hop:?} bytes per hop"
        );
    }
}

/// What `DOUBLE` holds at each hop is what the same loop holds walking its count without a
/// `MATCH`: its accumulator, and a constant more.
#[test]
fn double_holds_what_the_same_loop_holds_without_a_match() {
    let held = |looped: &Measured| -> Vec<usize> {
        let first = looped.hops[0].live;
        (looped.hops.iter())
            .map(|sample| sample.live.saturating_sub(first))
            .collect()
    };
    for form in [Form::Selfward, Form::Mutual] {
        let (matched, _) = looped(256, &nat(256), DOUBLE, form, "ints nat zero");
        let (plain, _) = looped(256, &nat(256), DOUBLE_PLAIN, form, "ints chain zero");
        assert_eq!(matched.printed, plain.printed, "{form:?}");
        assert!(matched.hops.iter().any(|sample| sample.threads > 0));
        let (matched, plain) = (held(&matched), held(&plain));
        assert_eq!(matched.len(), plain.len(), "{form:?}: one sample per hop");
        for (at, (matched, plain)) in matched.iter().zip(&plain).enumerate() {
            assert!(
                *matched <= plain + SLACK,
                "{form:?}: at sample {at} `DOUBLE` holds {matched} B against {plain} B"
            );
        }
    }
}

/// `PAYLOAD` hands back the `it` its arm binds: the payload of its argument, at the home that
/// argument rests in, so the walk through it places nothing new in the host.
#[test]
fn a_helper_returning_it_hands_back_its_scrutinees_payload_where_it_rests() {
    let helper = "EXPR #(PAYLOAD n :Nat) -> Nat = \
                  #(MATCH n UNDER Nat WITH #{Succ: (it), Zero: (n)})\n";
    let walk = "EXPR #(NAME m :Counter n :Nat) -> Str = \
                #((LET t = (TOUCH m)) (MATCH n UNDER Nat WITH \
                #{Zero: (\"done\"), Succ: ((LET p = (PAYLOAD n)) (NEXT m p))}))\n";
    for form in [Form::Selfward, Form::Mutual] {
        crate::scheduler::PASSED.with(|passed| passed.set(0));
        let prelude = format!("{}{helper}", nat(64));
        let (looped, _) = looped(64, &prelude, walk, form, "ints nat");
        assert_eq!(looped.printed, "done");
        let passed = crate::scheduler::PASSED.with(Cell::get);
        assert!(
            passed >= 64,
            "{form:?}: {passed} values handed back unchanged"
        );
        let (early, last) = (&looped.hops[16], looped.hops.last().unwrap());
        assert_eq!(
            early.host_region, last.host_region,
            "{form:?}: the host holds flat"
        );
    }
}

/// The cons loop matching the cell it builds: the arm hands on `it`, which the frame built, so the
/// frame builds its scrutinee in the loop cell, and a hop copies a constant, amortized, as the
/// plain cons loop's does, never the list it hands on.
#[test]
fn a_match_over_a_scrutinee_its_frame_builds_threads_it() {
    let body = "EXPR #(NAME m :Counter acc :Any link :Link) -> Any = \
                #((LET t = (TOUCH m)) (MATCH {head = 1, tail = acc} WITH \
                #{Any: (NEXT m it (ATTR link next))}))\n\
                EXPR #(NAME m :Counter acc :Any end :Null) -> Any = #(ATTR acc head)\n";
    for form in [Form::Selfward, Form::Mutual] {
        let (looped, _) = looped(64, "", body, form, "ints null chain");
        assert_eq!(looped.printed, "1");
        assert!(looped.hops.iter().any(|sample| sample.threads > 0));
        let per_hop = copied_per_hop("", body, form, "ints null chain");
        assert!(
            per_hop
                .windows(2)
                .all(|pair| pair[1] <= pair[0] + pair[0] / 4),
            "{form:?}: {per_hop:?} bytes per hop"
        );
    }
}
