//! Caching bodies at run: a caching body's types live in a scratch tier, so a loop over one holds
//! the lattice flat across hops, and what leaves the body reads after its scratch resets.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use crate::program::CellSubstrate;
use crate::scheduler::{LAID, TIERS, Tiers};
use crate::scope::{BodyShape, Mint, ShapeKind, Slot};

use super::super::Koan;
use super::classes::tutorial_snippets;
use super::statics::loaded;
use super::{WRITTEN, output};
use crate::values::SpecBounds;

/// `SIG Counter`, whose head parameter an opaque view hides, `MODULE ints` and `MODULE words`,
/// which fit it at different carriers, `Link`, a chain a loop walks to the `null` at its end, and
/// `MODULE empty`, which a `USING … SCOPE` enters to run a block.
pub(super) const PRELUDE: &str = "SIG Counter FOR ALL #[Carrier] = \
                       #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                       MODULE ints = ((LET zero = 0) \
                       (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n\
                       MODULE words = ((LET zero = \"\") \
                       (LET succ = (FN :{x :Str} -> Str = #(x))))\n\
                       SIG Holder FOR ALL #[Elt] = #[(VAL held :Elt)]\n\
                       NEWTYPE Link = :{next :Any}\n\
                       MODULE empty = (LET unused = 0)\n";

/// What one run left: the most any step left in each tier, how many nodes the run added to the
/// root, what it printed, and how many specializations were live at its end.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Ran {
    pub(super) tiers: Tiers,
    root: usize,
    pub(super) printed: String,
    pub(super) live: usize,
}

/// [`PRELUDE`], a chain of `depth` links bound to `chain`, and `source`.
pub(super) fn program(depth: u32, source: &str) -> String {
    let mut program = format!("{PRELUDE}LET link0 = null\n");
    for at in 1..=depth {
        program.push_str(&format!(
            "LET link{at} = (Link {{next = link{}}})\n",
            at - 1
        ));
    }
    program.push_str(&format!("LET chain = link{depth}\n{source}"));
    program
}

/// Run [`program`] at `depth` over `source`.
fn ran(depth: u32, source: &str) -> Ran {
    ran_under(depth, source, SpecBounds::default())
}

/// Run [`program`] at `depth` over `source`, its specialization tables `bounds` long.
pub(super) fn ran_under(depth: u32, source: &str, bounds: SpecBounds) -> Ran {
    let program = program(depth, source);
    WRITTEN.with(|written| written.borrow_mut().clear());
    let mut substrate = CellSubstrate::load::<Koan>(&program, "<test>", 2, bounds, output())
        .unwrap_or_else(|error| panic!("the program loads: {error}"));
    let before = substrate.with(|running| running.program().tiers().frozen.len());
    TIERS.with(|tiers| tiers.set(Tiers::default()));
    substrate
        .with(|running| running.run())
        .expect("the program runs");
    let after = substrate.with(|running| running.program().tiers().frozen.len());
    let live = substrate.with(|running| running.scheduler().graph().live_specializations());
    Ran {
        tiers: TIERS.with(Cell::get),
        root: after - before,
        printed: WRITTEN.with(|written| written.borrow().join("\n")),
        live,
    }
}

/// How many frame bodies [`PRELUDE`] and `source` write cache: what a test reads to see it runs
/// the bodies it means to.
fn caching(source: &str) -> usize {
    fn count(shape: &BodyShape<'_>) -> usize {
        (shape.nested_shapes().iter())
            .map(|(_, nested)| {
                let frame = matches!(nested.kind(), ShapeKind::Callable | ShapeKind::Code);
                usize::from(frame && nested.caches()) + count(nested)
            })
            .sum()
    }
    loaded(
        &format!("{PRELUDE}LET chain = (Link {{next = null}})\n{source}"),
        |program| count(program.shape()),
    )
}

/// The smallest depths that show a trend, evenly spaced, so the hops between the first two number
/// those between the last two.
const DEPTHS: [u32; 3] = [4, 10, 16];

/// The least and the greatest of [`DEPTHS`], for a test that compares two runs.
const ENDS: [u32; 2] = [DEPTHS[0], DEPTHS[2]];

/// `source` run at both [`ENDS`].
fn at_ends(source: &str) -> [Ran; 2] {
    ENDS.map(|depth| ran(depth, source))
}

/// Assert that `counts`, one counter of `what` at each of [`DEPTHS`], grows no faster in depth: the
/// later hops cost no more than the earlier, whatever the operator run's fixed part.
fn assert_flat(what: &str, [few, middle, many]: [usize; 3]) {
    let grew = |from: usize, to: usize| {
        to.checked_sub(from)
            .unwrap_or_else(|| panic!("{what} fell from {from} to {to} with depth"))
    };
    let (early, late) = (grew(few, middle), grew(middle, many));
    assert!(
        late <= early,
        "{what} grew {early} over depths {} to {} and {late} over {} to {}",
        DEPTHS[0],
        DEPTHS[1],
        DEPTHS[1],
        DEPTHS[2]
    );
}

/// A Fresh loop over `ints`, passed on bare: its view's site is invariant.
const INVARIANT: &str = "EXPR #(COUNTDOWN m :Counter link :Link) -> Number = \
                         #((LET v = (m :| Counter)) (COUNTDOWN m (ATTR link next)))\n\
                         EXPR #(COUNTDOWN m :Counter end :Null) -> Number = #(7)\n";

/// `PEEK`: a `Shares` caching helper viewing the module it is handed.
const PEEK: &str = "EXPR #(PEEK m :Holder) -> Str = #((LET v = (m :| Holder)) (\"x\"))\n";

/// `DRIVE`, a `Shares` loop that mints nothing itself, so every hop is a tenant of one host, whose
/// region would take each `PEEK`'s view were `PEEK` not caching.
fn hosted_peeks() -> String {
    format!(
        "{PEEK}EXPR #(DRIVE link :Link) -> Str = \
         #((MODULE m OVER #[link] = (LET held = link)) (LET seen = (PEEK m)) \
         (DRIVE (ATTR link next)))\n\
         EXPR #(DRIVE end :Null) -> Str = #(\"done\")\n\
         PRINT (DRIVE chain)"
    )
}

/// `WRAP`, a loop viewing the module it passes on and handing [`PEEK`] a module built over each
/// link: sealed, since the module captures only what the loop received.
fn wrapped_peeks() -> String {
    format!(
        "{PEEK}EXPR #(WRAP m :Counter link :Link) -> Str = \
         #((LET v = (m :| Counter)) (MODULE h OVER #[link] = (LET held = link)) \
         (LET seen = (PEEK h)) (WRAP m (ATTR link next)))\n\
         EXPR #(WRAP m :Counter end :Null) -> Str = #(\"done\")\n\
         PRINT (WRAP ints chain)"
    )
}

/// [`INVARIANT`] over `ints`.
fn invariant_loop() -> String {
    format!("{INVARIANT}PRINT (COUNTDOWN ints chain)")
}

/// `PASS`, a `Shares` loop caching on its host's scratch and parking once a hop on `TOUCH`.
const PARKING_LOOP: &str = "EXPR #(TOUCH m :Counter) -> Number = #(1)\n\
                            EXPR #(PASS m :Counter link :Link) -> Str = \
                            #((LET v = (m :| Counter)) (LET t = (TOUCH m)) \
                            (PASS m (ATTR link next)))\n\
                            EXPR #(PASS m :Counter end :Null) -> Str = #(\"done\")\n\
                            PRINT (PASS ints chain)";

/// `name`, a `Fresh` loop viewing a new module over each link, holding `held`, written over `link`.
fn varying(name: &str, held: &str) -> String {
    format!(
        "EXPR #({name} link :Link) -> Number = \
         #((MODULE m OVER #[link] = (LET held = {held})) \
         (LET seen = (m :| Holder)) ({name} (ATTR link next)))\n\
         EXPR #({name} end :Null) -> Number = #(7)\n"
    )
}

/// [`varying`] over each link itself.
fn varying_loop() -> String {
    format!("{}PRINT (COUNTDOWN chain)", varying("COUNTDOWN", "link"))
}

/// A `Fresh` loop whose hops each call `PEEK`, a `Shares` caching helper viewing a module over the
/// link it is handed.
const HELPER_LOOP: &str = "EXPR #(PEEK link :Link) -> Str = \
                           #((MODULE m OVER #[link] = (LET held = link)) \
                           (LET v = (m :| Holder)) (\"x\"))\n\
                           EXPR #(COUNTDOWN link :Link) -> Number = \
                           #((MODULE m OVER #[link] = (LET held = link)) \
                           (LET viewed = (m :| Holder)) (LET seen = (PEEK link)) \
                           (COUNTDOWN (ATTR link next)))\n\
                           EXPR #(COUNTDOWN end :Null) -> Number = #(7)\n\
                           PRINT (COUNTDOWN chain)";

/// `START`, `Shares`, hopping into the `Fresh` [`INVARIANT`] loop.
const START: &str = "EXPR #(START m :Counter link :Link) -> :(Number | Str) = \
                     #((LET v = (m :| Counter)) (COUNTDOWN m link))\n";

/// [`START`] over `ints`.
fn mixed_chain() -> String {
    format!("{INVARIANT}{START}PRINT (START ints chain)")
}

/// [`wrapped_peeks`] with both bodies `Fresh`, `PEEK` viewing at an invariant site.
const FRESH_WRAPPED: &str = "EXPR #(PEEK m :Holder) -> Number = #((LET v = (m :| Holder)) (1))\n\
                             EXPR #(WRAP m :Counter link :Link) -> Number = \
                             #((LET v = (m :| Counter)) (MODULE h OVER #[link] = (LET held = link)) \
                             (LET seen = (PEEK h)) (WRAP m (ATTR link next)))\n\
                             EXPR #(WRAP m :Counter end :Null) -> Number = #(7)\n\
                             PRINT (WRAP ints chain)";

/// `PASS`, a loop handing `peek` a member of a record of `length` numbers it received, the member
/// reached through `GETA`.
fn member_via_call(length: usize) -> String {
    let items = vec!["1"; length].join(", ");
    format!(
        "LET peek = (FN :{{xs :(LIST OF Number)}} -> Number = #(1))\n\
         EXPR #(GETA p :{{a :(LIST OF Number)}}) -> :(LIST OF Number) = #(ATTR p a)\n\
         EXPR #(PASS m :Counter p :{{a :(LIST OF Number)}} link :Link) -> Number = \
         #((LET v = (m :| Counter)) (LET l = (GETA p)) (LET t = (peek {{xs = l}})) \
         (PASS m p (ATTR link next)))\n\
         EXPR #(PASS m :Counter p :{{a :(LIST OF Number)}} end :Null) -> Number = #(7)\n\
         PRINT (PASS ints {{a = [{items}]}} chain)"
    )
}

/// `WALK`, a loop handing the rest of the chain on through a call by name and a keyworded tail
/// call.
const BY_NAME_WALK: &str = "LET peek = (FN :{link :Any} -> Number = #(1))\n\
                            EXPR #(WALK m :Counter link :Link) -> Number = \
                            #((LET v = (m :| Counter)) (LET t = (peek {link = (ATTR link next)})) \
                            (WALK m (ATTR link next)))\n\
                            EXPR #(WALK m :Counter end :Null) -> Number = #(7)\n\
                            PRINT (WALK ints chain)";

/// `PASS`, a loop passing on a list of `length` numbers it received.
fn received_list(length: usize) -> String {
    let items = vec!["1"; length].join(", ");
    format!(
        "EXPR #(PASS m :Counter xs :Any link :Link) -> Str = \
         #((LET v = (m :| Counter)) (PASS m xs (ATTR link next)))\n\
         EXPR #(PASS m :Counter xs :Any end :Null) -> Str = #(\"done\")\n\
         PRINT (PASS ints [{items}] chain)"
    )
}

/// What one run's adoptions counted: the lattice's, then the walk's.
#[derive(Clone, Copy)]
struct Adoptions {
    /// Handles the lattice was asked to make resolve.
    asked: usize,
    /// Of those, the ones that took a lookup.
    probes: usize,
    /// Lookups the chain answered at once.
    hits: usize,
    /// Lookups that missed, each laying a node.
    laid: usize,
    /// Handles the walks handed over.
    adopted: usize,
    /// Walks into a region, at an exit.
    exits: usize,
    /// Walks into a warm tier, at an invariant site.
    warm: usize,
    /// Parts the walks entered.
    entered: usize,
    /// Parts the walks skipped on their bits.
    skipped: usize,
}

impl Adoptions {
    /// Zero every counter.
    fn reset() {
        use crate::type_lattice::{ADOPT_ASKED, ADOPT_HITS, ADOPT_LAID, ADOPT_PROBES};
        use crate::values::{ADOPTED, EXIT_WALKS, PARTS_ENTERED, PARTS_SKIPPED, WARM_WALKS};
        let counters = [
            &ADOPT_ASKED,
            &ADOPT_PROBES,
            &ADOPT_HITS,
            &ADOPT_LAID,
            &ADOPTED,
            &EXIT_WALKS,
            &WARM_WALKS,
            &PARTS_ENTERED,
            &PARTS_SKIPPED,
        ];
        for counter in counters {
            counter.set(0);
        }
    }

    /// Every counter as it stands.
    fn read() -> Self {
        use crate::type_lattice::{ADOPT_ASKED, ADOPT_HITS, ADOPT_LAID, ADOPT_PROBES};
        use crate::values::{ADOPTED, EXIT_WALKS, PARTS_ENTERED, PARTS_SKIPPED, WARM_WALKS};
        Adoptions {
            asked: ADOPT_ASKED.get(),
            probes: ADOPT_PROBES.get(),
            hits: ADOPT_HITS.get(),
            laid: ADOPT_LAID.get(),
            adopted: ADOPTED.get(),
            exits: EXIT_WALKS.get(),
            warm: WARM_WALKS.get(),
            entered: PARTS_ENTERED.get(),
            skipped: PARTS_SKIPPED.get(),
        }
    }

    /// The counters in table order, each named.
    fn named(self) -> [(&'static str, usize); 9] {
        [
            ("asked", self.asked),
            ("probes", self.probes),
            ("hits", self.hits),
            ("laid", self.laid),
            ("adopted", self.adopted),
            ("exits", self.exits),
            ("warm", self.warm),
            ("entered", self.entered),
            ("skipped", self.skipped),
        ]
    }
}

/// Run [`program`] at `depth` over `source`, counting its adoptions.
fn adopting(depth: u32, source: &str) -> (Ran, Adoptions) {
    Adoptions::reset();
    let ran = ran(depth, source);
    (ran, Adoptions::read())
}

/// What one run's content digests counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Digested {
    /// Parts the digest walks hashed, memo hits excluded.
    visits: usize,
    /// Demands opened, one per memo.
    demands: usize,
}

impl Digested {
    /// Zero both counters.
    fn reset() {
        use crate::values::{DIGEST_DEMANDS, DIGEST_VISITS};
        DIGEST_VISITS.set(0);
        DIGEST_DEMANDS.set(0);
    }

    /// Both counters as they stand.
    fn read() -> Self {
        use crate::values::{DIGEST_DEMANDS, DIGEST_VISITS};
        Digested {
            visits: DIGEST_VISITS.get(),
            demands: DIGEST_DEMANDS.get(),
        }
    }
}

/// Run [`program`] at `depth` over `source`, counting its content digests.
fn digesting(depth: u32, source: &str) -> (Ran, Digested) {
    Digested::reset();
    let ran = ran(depth, source);
    (ran, Digested::read())
}

/// Across the caching suite's programs, what a loop hop's adoptions cost, printed as a table of each
/// counter per hop: flat in depth, a root handle adopted at no lookup, no node laid where no hop
/// builds a specialization, and every hop's walks stopping at a part on its bits.
#[test]
fn a_hops_adoptions_hold_flat_in_depth() {
    let programs = [
        ("invariant_loop", invariant_loop()),
        ("wrapped_peeks", wrapped_peeks()),
        ("FRESH_WRAPPED", FRESH_WRAPPED.to_string()),
        ("hosted_peeks", hosted_peeks()),
        ("varying_loop", varying_loop()),
        ("HELPER_LOOP", HELPER_LOOP.to_string()),
        ("PARKING_LOOP", PARKING_LOOP.to_string()),
        ("mixed_chain", mixed_chain()),
        ("member_via_call", member_via_call(8)),
        ("BY_NAME_WALK", BY_NAME_WALK.to_string()),
        ("received_list", received_list(8)),
    ];
    let hops = (ENDS[1] - ENDS[0]) as usize;
    let per_hop = |delta: usize| format!("{:.1}", delta as f64 / hops as f64);
    let header = Adoptions::read().named().map(|(name, _)| name).join(" | ");
    eprintln!("| program | {header} | specialized |");
    for (name, source) in programs {
        let runs = DEPTHS.map(|depth| adopting(depth, &source));
        let counts = runs.each_ref().map(|(_, adoptions)| adoptions.named());
        for at in 0..counts[0].len() {
            let counter = counts[0][at].0;
            assert_flat(
                &format!("{name}: {counter}"),
                counts.map(|named| named[at].1),
            );
        }
        let specialized = runs.each_ref().map(|(ran, _)| ran.tiers.specialized);
        assert_flat(&format!("{name}: specialized"), specialized);
        let deltas = (counts[0].into_iter().zip(counts[2]))
            .map(|((counter, few), (_, many))| (counter, many - few))
            .collect::<BTreeMap<_, _>>();
        let specialized = specialized[2] - specialized[0];
        let row = (counts[0].iter())
            .map(|(counter, _)| per_hop(deltas[counter]))
            .collect::<Vec<_>>()
            .join(" | ");
        eprintln!("| {name} | {row} | {} |", per_hop(specialized));
        assert!(
            deltas["probes"] < deltas["asked"],
            "{name}: each hop adopts root handles at no lookup, {} probes of {}",
            deltas["probes"],
            deltas["asked"]
        );
        if specialized == 0 {
            assert_eq!(
                deltas["laid"], 0,
                "{name}: a hop borrowing nothing new lays no node"
            );
            assert_eq!(
                deltas["hits"], deltas["probes"],
                "{name}: a hop borrowing nothing new hits at every probe"
            );
        }
        assert!(
            deltas["skipped"] > 0,
            "{name}: each hop's walks stop at a part on its bits"
        );
    }
}

/// A list of `length` numbers, as a koan literal.
fn numbers(length: usize) -> String {
    format!("[{}]", vec!["1"; length].join(", "))
}

/// `PAIR`, viewing two modules over the list it is handed, the second from inside a block, and
/// `SINGLE`, viewing one.
const PAIR: &str = "EXPR #(PAIR xs :(LIST OF Number)) -> Number = \
                    #((MODULE a OVER #[xs] = (LET held = xs)) (MODULE b OVER #[xs] = (LET held = xs)) \
                    (LET u = (a :| Holder)) (USING a SCOPE ((LET w = (b :| Holder)))) (1))\n\
                    EXPR #(SINGLE xs :(LIST OF Number)) -> Number = \
                    #((MODULE a OVER #[xs] = (LET held = xs)) (LET u = (a :| Holder)) (1))\n";

/// `RUN`, a loop viewing a module over the record it passes on bare, over a record of `length`
/// numbers.
fn record_loop(length: usize) -> String {
    format!(
        "EXPR #(RUN cfg :{{items :(LIST OF Number)}} link :Link) -> Number = \
         #((MODULE ops OVER #[cfg] = (LET held = cfg)) (LET v = (ops :| Holder)) \
         (RUN cfg (ATTR link next)))\n\
         EXPR #(RUN cfg :{{items :(LIST OF Number)}} end :Null) -> Number = #(7)\n\
         PRINT (RUN {{items = {}}} chain)",
        numbers(length)
    )
}

#[test]
fn two_views_over_modules_capturing_one_list_digest_it_once() {
    let visits = |call: &str, length: usize| {
        let (ran, digested) = digesting(1, &format!("{PAIR}PRINT ({call} {})", numbers(length)));
        assert_eq!(ran.printed, "1");
        digested.visits
    };
    let pair = visits("PAIR", 64) - visits("PAIR", 1);
    let single = visits("SINGLE", 64) - visits("SINGLE", 1);
    assert!(single > 0, "a view digests the list its module captures");
    assert_eq!(pair, single, "the second view reads the first's memo");
}

#[test]
fn a_loop_passing_a_record_on_bare_digests_it_once_per_loop() {
    let growth = DEPTHS.map(|depth| {
        let visits = |length| {
            let (ran, digested) = digesting(depth, &record_loop(length));
            assert_eq!(ran.printed, "7");
            digested.visits
        };
        visits(64) - visits(1)
    });
    assert!(growth[0] > 0, "the first hop's view digests the record");
    assert_eq!(
        growth, [growth[0]; 3],
        "each later hop reads the memo the tail call hands it"
    );
}

#[test]
fn only_a_use_that_may_call_a_function_hands_a_memo_on() {
    // `RUN`'s `cfg` keeps a memo; `ATTR` can select only a builtin, which runs no frame to keep it,
    // while the tail call may select `RUN`.
    let source = "EXPR #(RUN cfg :{items :(LIST OF Number)} link :Link) -> Number = \
                  #((MODULE ops OVER #[cfg] = (LET held = cfg)) (LET v = (ops :| Holder)) \
                  (LET n = (ATTR cfg items)) (RUN cfg (ATTR link next)))\n";
    let handed = loaded(&format!("{PRELUDE}{source}"), |program| {
        let mut shapes = Vec::new();
        every_shape(program.shape(), &mut shapes);
        (shapes.into_iter())
            .filter_map(|shape| shape.statics())
            .map(|statics| statics.handoffs.len())
            .sum::<usize>()
    });
    assert_eq!(handed, 1, "the tail call alone hands `cfg`'s memo on");
}

#[test]
fn a_memo_holds_the_digest_of_the_value_its_slot_holds() {
    use crate::knot::module::view::CHECK_SLOT_MEMOS;
    // `OUTER`'s `cfg :Any` is handed bare to `INNER`'s `cfg`, which binds it at a type of its own.
    let restamped = "EXPR #(INNER cfg :{items :(LIST OF Number)}) -> Number = \
                     #((MODULE b OVER #[cfg] = (LET held = cfg)) (LET w = (b :| Holder)) (1))\n\
                     EXPR #(OUTER cfg :Any) -> Number = \
                     #((MODULE a OVER #[cfg] = (LET held = cfg)) (LET v = (a :| Holder)) \
                     (INNER cfg))\n\
                     PRINT (OUTER {items = [1, 2]})";
    let sources = [
        format!("{PAIR}PRINT (PAIR [1, 2])"),
        record_loop(2),
        HELPER_LOOP.to_string(),
        restamped.to_string(),
    ];
    CHECK_SLOT_MEMOS.set(true);
    let printed = sources
        .each_ref()
        .map(|source| ran(DEPTHS[0], source).printed);
    CHECK_SLOT_MEMOS.set(false);
    assert_eq!(printed, ["1", "7", "7", "1"]);
}

/// `PROBE`, viewing one module over the whole chain: one walk of it.
const PROBE: &str = "EXPR #(PROBE link :Link) -> Number = \
                     #((MODULE m OVER #[link] = (LET held = link)) (LET v = (m :| Holder)) (1))\n\
                     PRINT (PROBE chain)";

/// Across the caching suite's programs, what a loop hop's content digests cost, printed as a table
/// of each counter per hop: the demands flat in depth, and the visits flat but where a key or a
/// view reaches a new module over the rest of the chain each hop, which pays one walk of it.
#[test]
fn a_hops_digest_visits_hold_flat_or_pay_for_the_module_they_view() {
    // Each program beside how many modules over the rest of the chain a hop digests, and who.
    let programs = [
        ("invariant_loop", invariant_loop(), 0, "-"),
        ("wrapped_peeks", wrapped_peeks(), 1, "`h`, at `PEEK`'s seek"),
        (
            "FRESH_WRAPPED",
            FRESH_WRAPPED.to_string(),
            1,
            "`h`, at `PEEK`'s seek",
        ),
        ("hosted_peeks", hosted_peeks(), 1, "`m`, at `PEEK`'s seek"),
        ("varying_loop", varying_loop(), 1, "`m`, at its `:|`"),
        (
            "HELPER_LOOP",
            HELPER_LOOP.to_string(),
            1,
            "`COUNTDOWN`'s `m`, at its `:|`",
        ),
        ("PARKING_LOOP", PARKING_LOOP.to_string(), 0, "-"),
        ("mixed_chain", mixed_chain(), 0, "-"),
        ("member_via_call", member_via_call(8), 0, "-"),
        ("BY_NAME_WALK", BY_NAME_WALK.to_string(), 0, "-"),
        ("received_list", received_list(8), 0, "-"),
    ];
    let probes = DEPTHS.map(|depth| digesting(depth, PROBE).1.visits);
    let span = (DEPTHS[1] - DEPTHS[0]) as usize;
    let per_link = (probes[1] - probes[0]) / span;
    assert_eq!(
        probes[2] - probes[1],
        per_link * span,
        "a walk is linear in the chain"
    );
    // The second difference over `DEPTHS` of `d (d + 1) / 2`: what one walk of the rest of the
    // chain per hop adds up to, in links.
    let walked = DEPTHS.map(|depth| (depth * (depth + 1) / 2) as usize);
    let quadratic = (walked[2] - walked[1]) - (walked[1] - walked[0]);
    let hops = (ENDS[1] - ENDS[0]) as usize;
    let per_hop = |delta: usize| format!("{:.1}", delta as f64 / hops as f64);
    eprintln!("| program | visits | demands | pays |");
    for (name, source, pays, payer) in programs {
        let counts = DEPTHS.map(|depth| digesting(depth, &source).1);
        let visits = counts.map(|digested| digested.visits);
        let demands = counts.map(|digested| digested.demands);
        assert_flat(&format!("{name}: demands"), demands);
        let growth = (visits[2] - visits[1]) - (visits[1] - visits[0]);
        if pays == 0 {
            assert_flat(&format!("{name}: visits"), visits);
        } else {
            assert_eq!(
                growth,
                pays * per_link * quadratic,
                "{name}: each hop walks the rest of the chain {pays} time(s), through {payer}"
            );
        }
        eprintln!(
            "| {name} | {} | {} | {payer} |",
            per_hop(visits[2] - visits[0]),
            per_hop(demands[2] - demands[0])
        );
    }
}

#[test]
fn a_module_surfaced_read_or_passed_on_digests_nothing() {
    let source = "LET pass = (FN :{m :Any} -> Number = #(1))\n\
                  EXPR #(SURFACE link :Link) -> Number = \
                  #((MODULE m OVER #[link] = (LET held = link)) (LET h = m.held) \
                  (USING m SCOPE (LET x = held)) (LET t = (pass {m = m})) \
                  (SURFACE (ATTR link next)))\n\
                  EXPR #(SURFACE end :Null) -> Number = #(7)\n\
                  PRINT (SURFACE chain)";
    for depth in ENDS {
        let (ran, digested) = digesting(depth, source);
        assert_eq!(ran.printed, "7");
        assert_eq!(
            digested,
            Digested {
                visits: 0,
                demands: 0
            }
        );
    }
}

#[test]
fn a_shares_call_with_new_content_each_call_holds_its_hosts_region_flat() {
    let source = hosted_peeks();
    assert_eq!(caching(&source), 1, "`PEEK` caches");
    let [few, many] = at_ends(&source);
    assert_eq!((few.printed.as_str(), few.root), ("done", 0));
    // The host's scratch grows with the loop, as its bytes do; its region does not.
    assert_eq!(few.tiers.region, many.tiers.region);
}

#[test]
fn a_shares_loop_handing_a_keyed_shares_helper_a_module_over_its_link_holds_adoptions_flat() {
    let source = wrapped_peeks();
    assert_eq!(caching(&source), 2, "`WRAP` and `PEEK` cache");
    let ran_adopting = |depth: u32| {
        crate::values::ADOPTED.with(|adopted| adopted.set(0));
        let ran = ran(depth, &source);
        (ran, crate::values::ADOPTED.with(Cell::get))
    };
    let runs = DEPTHS.map(ran_adopting);
    let [(few, few_adopted), _, (many, _)] = &runs;
    assert_eq!((few.printed.as_str(), few.root), ("done", 0));
    // `PEEK` keys on the module it is handed, so it borrows a specialization of its own each hop,
    // and `WRAP` one across the loop; the region and each lent branch hold flat.
    for ((ran, _), depth) in runs.iter().zip(DEPTHS) {
        assert_eq!(ran.tiers.specialized, depth as usize + 1);
    }
    assert_eq!(
        (few.tiers.region, few.tiers.lent),
        (many.tiers.region, many.tiers.lent)
    );
    assert!(*few_adopted > 0, "the loop's exits adopt");
    assert_flat("adoptions", runs.each_ref().map(|(_, adopted)| *adopted));
}

#[test]
fn a_fresh_loop_handing_a_fresh_helper_a_module_over_its_link_holds_adoptions_flat() {
    // `PEEK` adopts the module it views into the lent branch at an invariant site, which stops at
    // the link the module holds, settled by the exit that handed it on.
    let source = FRESH_WRAPPED;
    assert_eq!(caching(source), 2, "`WRAP` and `PEEK` cache");
    let ran_adopting = |depth: u32| {
        crate::values::ADOPTED.with(|adopted| adopted.set(0));
        let ran = ran(depth, source);
        (ran, crate::values::ADOPTED.with(Cell::get))
    };
    let runs = DEPTHS.map(ran_adopting);
    let (few, few_adopted) = &runs[0];
    assert_eq!(few.printed, "7");
    assert!(*few_adopted > 0, "the loop's exits adopt");
    assert_flat("adoptions", runs.each_ref().map(|(_, adopted)| *adopted));
}

#[test]
fn a_member_reached_through_a_call_is_not_entered() {
    // `GETA`'s result is a member of the record the loop received, reached by a route the load
    // records nothing for: the exit handing it to `peek` stops at it, whatever its length.
    let adopted = |length: usize| {
        let source = member_via_call(length);
        assert_eq!(caching(&source), 1);
        crate::values::ADOPTED.with(|adopted| adopted.set(0));
        assert_eq!(ran(DEPTHS[0], &source).printed, "7");
        crate::values::ADOPTED.with(Cell::get)
    };
    assert_eq!(adopted(8), adopted(64));
}

#[test]
fn a_fresh_loop_over_an_invariant_module_holds_its_lent_branch_flat() {
    let source = invariant_loop();
    assert_eq!(caching(&source), 1);
    let [few, many] = at_ends(&source);
    assert_eq!(few.printed, "7");
    assert!(few.tiers.lent > 0, "the view's types are adopted warm");
    assert!(
        few.tiers.lent_verdicts > 0,
        "the view's verdicts are recorded warm, once across hops"
    );
    assert_eq!(few, many);
}

#[test]
fn a_shares_loop_parking_each_hop_holds_its_hosts_scratch_bytes_flat() {
    // The parking keeps the host's scratch named across the loop: each hop reuses the receipt run
    // the last released.
    let source = PARKING_LOOP;
    assert_eq!(caching(source), 1);
    let [few, many] = at_ends(source);
    assert_eq!(few.printed, "done");
    assert!(few.tiers.scratch_bytes > 0);
    assert_eq!(few, many);
}

#[test]
fn a_fresh_loop_over_varying_content_holds_the_lattice_flat() {
    let source = varying_loop();
    assert_eq!(caching(&source), 1);
    let [few, many] = at_ends(&source);
    assert_eq!((few.printed.as_str(), few.root), ("7", 0));
    assert_eq!(few, many);
}

#[test]
fn a_fresh_loop_calling_a_helper_over_its_link_keys_nothing_and_holds_flat() {
    // The helper's parameter is a record, whose digest costs its size, so it keys nothing: its
    // view routes cold, and no hop seeks or builds a specialization.
    let source = HELPER_LOOP;
    assert_eq!(caching(source), 2);
    let [few, many] = at_ends(source);
    assert_eq!(few.printed, "7");
    for ran in [&few, &many] {
        assert_eq!(
            (ran.tiers.sought, ran.tiers.specialized, ran.live),
            (0, 0, 0)
        );
    }
    assert_eq!(few, many);
}

#[test]
fn a_mixed_placement_tail_chain_over_invariant_content_holds_flat() {
    let source = mixed_chain();
    assert_eq!(caching(&source), 2);
    let [few, many] = at_ends(&source);
    assert_eq!(few.printed, "7");
    assert_eq!(few, many);
}

#[test]
fn a_loops_scratch_resets_once_its_value_is_delivered() {
    // Two `Shares` loops over different content, each asked for by a `Fresh` frame's statement:
    // the second starts on the scratch the first's delivery reset, so the two leave what one
    // leaves.
    let driving = |calls: &str| {
        format!(
            "{PEEK}EXPR #(DRIVE k :Number link :Link) -> Str = \
             #((MODULE m OVER #[link, k] = (LET held = [link, k])) (LET seen = (PEEK m)) \
             (DRIVE k (ATTR link next)))\n\
             EXPR #(DRIVE k :Number end :Null) -> Str = #(\"done\")\n\
             LET twice = (FN :{{n :Number}} -> Number = #({calls} (n)))\n\
             PRINT (twice {{n = 0}})"
        )
    };
    assert_eq!(caching(&driving("(LET first = (DRIVE 1 chain))")), 1);
    let once = ran(DEPTHS[0], &driving("(LET first = (DRIVE 1 chain))"));
    let twice = ran(
        DEPTHS[0],
        &driving("(LET first = (DRIVE 1 chain)) (LET second = (DRIVE 2 chain))"),
    );
    assert_eq!(twice.printed, "0");
    assert_eq!(once.tiers.scratch, twice.tiers.scratch);
}

#[test]
fn two_loops_a_frame_asks_for_in_turn_leave_what_the_larger_leaves() {
    // A `Fresh` frame asks for two loops in turn, over content of different types: the second
    // starts on the branch the first's delivery reset or, where it keys a specialization, on a
    // specialization of its own binding, so the two leave what the larger leaves.
    let cases = [
        (
            INVARIANT.to_string(),
            "COUNTDOWN ints chain",
            "COUNTDOWN words chain",
        ),
        (
            format!(
                "{}{}",
                varying("COUNTDOWN", "link"),
                varying("RECOUNT", "[link]")
            ),
            "COUNTDOWN chain",
            "RECOUNT chain",
        ),
        (
            format!("{INVARIANT}{START}"),
            "START ints chain",
            "START words chain",
        ),
    ];
    for (loops, first, second) in &cases {
        let asked = |calls: &str| {
            format!(
                "{loops}LET twice = (FN :{{n :Number}} -> Number = #({calls} (n)))\n\
                 PRINT (twice {{n = 0}})"
            )
        };
        let alone = |call: &str| ran(DEPTHS[0], &asked(&format!("(LET first = ({call}))")));
        let (one, other) = (alone(first), alone(second));
        let both = ran(
            DEPTHS[0],
            &asked(&format!(
                "(LET first = ({first})) (LET second = ({second}))"
            )),
        );
        assert_eq!(both.printed, "0");
        assert_eq!(
            (both.tiers.lent, both.tiers.scratch),
            (
                one.tiers.lent.max(other.tiers.lent),
                one.tiers.scratch.max(other.tiers.scratch)
            ),
            "{first}, then {second}"
        );
    }
}

#[test]
fn a_value_retyped_in_a_caching_body_reads_after_the_reset() {
    let source = "EXPR #(MK m :Counter) -> :(Number | (LIST OF (LIST OF Number))) = \
                  #((LET v = (m :| Counter)) ([[1, 2]] :! :(Number | (LIST OF (LIST OF Number)))))\n\
                  LET twice = (FN :{n :Number} -> Any = #((LET r = (MK ints)) (LET s = (MK ints)) (r)))\n\
                  PRINT (twice {n = 0})";
    assert_eq!(caching(source), 1);
    assert_eq!(ran(1, source).printed, "[[1, 2]]");
}

#[test]
fn a_closure_a_quote_and_a_member_leave_a_caching_body_and_run_after_the_reset() {
    let source = "EXPR #(MKCLOSURE m :Counter) -> :(FN :{x :Any} -> Number) = \
                  #((LET v = (m :| Counter)) (FN :{x :Any} -> Number = #((LET w = [v.zero]) (1))))\n\
                  EXPR #(MKQUOTE m :Counter) -> :Code = \
                  #((LET v = (m :| Counter)) (#((LET w = [$v.zero]) (2))))\n\
                  EXPR #(MKTIED m :Counter) -> :(FN :{x :Any} -> Number) = \
                  #((LET v = (m :| Counter)) \
                  (LET even = (FN :{x :Any} -> Number = #((LET w = [v.zero]) (odd {x = x})))) \
                  (LET odd = (FN :{x :Any} -> Number = #(3))) (even))\n\
                  LET run = (FN :{n :Number} -> Any = #(\
                  (LET f = (MKCLOSURE ints)) (LET q = (MKQUOTE ints)) (LET g = (MKTIED ints)) \
                  (LET again = (MKCLOSURE ints)) \
                  ([(f {x = 0}), (EVAL q -> Number), (g {x = 0})])))\n\
                  PRINT (run {n = 0})";
    assert_eq!(caching(source), 3);
    assert_eq!(ran(1, source).printed, "[1, 2, 3]");
}

#[test]
fn a_held_frame_hands_on_what_its_last_statement_reads() {
    // The last statement reads `k`, built over a view of a local, inside a block: the read runs
    // after the successor's first step, once the frame's scratch is gone.
    let source = "EXPR #(SIZE x :Any) -> Number = #(1)\n\
                  EXPR #(HAND m :Counter) -> Number = \
                  #((LET w = m) (LET v = (w :| Counter)) (LET k = [v.zero]) \
                  (SIZE (USING empty SCOPE ((LET z = k) (1)))))\n\
                  PRINT (HAND ints)";
    assert_eq!(caching(source), 1);
    assert_eq!(ran(1, source).printed, "1");
}

#[test]
fn a_received_list_passed_on_is_not_entered() {
    let adopted = |length: usize| {
        let source = received_list(length);
        assert_eq!(caching(&source), 1);
        crate::values::ADOPTED.with(|adopted| adopted.set(0));
        assert_eq!(ran(DEPTHS[0], &source).printed, "done");
        crate::values::ADOPTED.with(Cell::get)
    };
    assert_eq!(adopted(8), adopted(64));
}

#[test]
fn a_member_of_a_received_value_passed_on_adopts_a_hop_at_a_time() {
    // Each hop passes on `(ATTR link next)`, the rest of the chain, through a keyworded tail call
    // and, in the second program, a call by name too: an exit adopting it whole would walk the
    // chain each hop, so the operator run's adoptions would grow with the square of its depth.
    for source in [invariant_loop(), BY_NAME_WALK.to_string()] {
        assert_eq!(caching(&source), 1);
        let adopted = |depth: u32| {
            crate::values::ADOPTED.with(|adopted| adopted.set(0));
            assert_eq!(ran(depth, &source).printed, "7");
            crate::values::ADOPTED.with(Cell::get)
        };
        let counts = DEPTHS.map(adopted);
        assert!(counts[0] > 0, "the loop's exits adopt");
        assert_flat("adoptions", counts);
    }
}

#[test]
fn a_barrier_called_caching_frame_keeps_its_blocks_value() {
    let source = "SIG Maker = #[(VAL mk :(FN :{m :Counter} -> Any))]\n\
                  MODULE makers = (LET mk = (FN :{m :Counter} -> Any = \
                  #((LET v = (m :| Counter)) (USING empty SCOPE ([4])))))\n\
                  LET seen = (makers :| Maker)\n\
                  LET twice = (FN :{n :Number} -> Any = \
                  #((LET r = (seen.mk {m = ints})) (LET s = (seen.mk {m = ints})) (r)))\n\
                  PRINT (twice {n = 0})";
    assert_eq!(caching(source), 1);
    assert_eq!(ran(1, source).printed, "[4]");
}

/// The nodes each caching body's steps laid through its chains over one run of `program`, by the
/// body's kind: in the scratch branch, then in the lent branch. Empty where `program` refuses at
/// load.
fn laid(program: &str) -> Vec<(&'static str, usize, usize)> {
    let Ok(mut substrate) =
        CellSubstrate::load::<Koan>(program, "<measured>", 2, SpecBounds::default(), output())
    else {
        return Vec::new();
    };
    let kinds: BTreeMap<usize, &'static str> = substrate.with(|running| {
        let (mut kinds, mut stack) = (BTreeMap::new(), vec![running.program().shape()]);
        while let Some(shape) = stack.pop() {
            let kind = match shape.kind() {
                ShapeKind::Program => "program",
                ShapeKind::Callable => "callable",
                ShapeKind::Code => "code",
                ShapeKind::Block => "block",
                ShapeKind::Module => "module",
            };
            if kinds
                .insert(std::ptr::from_ref(shape).addr(), kind)
                .is_none()
            {
                stack.extend(shape.nested_shapes().iter().map(|(_, nested)| *nested));
            }
        }
        kinds
    });
    LAID.with(|laid| laid.borrow_mut().clear());
    let _ = substrate.with(|running| running.run());
    LAID.with(|laid| {
        (laid.borrow().iter())
            .map(|(shape, &(scratch, warm))| {
                (kinds.get(shape).copied().unwrap_or("?"), scratch, warm)
            })
            .collect()
    })
}

/// The measurement the type-tiers work reads: the nodes laid through caching chains over the
/// tutorial's runnable programs, then per caching body over the flat-loop programs at each of
/// [`DEPTHS`]. Run with `cargo test laid_through_caching_chains -- --ignored --nocapture`.
#[test]
#[ignore]
fn laid_through_caching_chains() {
    let tutorial: usize = (tutorial_snippets().iter())
        .flat_map(|(_, code)| laid(code))
        .map(|(_, scratch, warm)| scratch + warm)
        .sum();
    println!("tutorial programs: {tutorial} nodes laid through caching chains\n");
    println!("| program | body | depth | scratch nodes | warm nodes |\n|---|---|---|---|---|");
    let programs = [
        ("hosted peeks", hosted_peeks()),
        ("wrapped peeks", wrapped_peeks()),
        ("invariant loop", invariant_loop()),
        ("parking loop", PARKING_LOOP.to_string()),
        ("varying loop", varying_loop()),
        ("helper loop", HELPER_LOOP.to_string()),
        ("mixed chain", mixed_chain()),
    ];
    for (name, source) in &programs {
        for depth in DEPTHS {
            for (kind, scratch, warm) in laid(&program(depth, source)) {
                println!("| {name} | {kind} | {depth} | {scratch} | {warm} |");
            }
        }
    }
}

/// Every shape under `shape`, itself included, depth first.
pub(super) fn every_shape<'graph>(
    shape: &'graph BodyShape<'graph>,
    out: &mut Vec<&'graph BodyShape<'graph>>,
) {
    out.push(shape);
    for (_, nested) in shape.nested_shapes() {
        every_shape(nested, out);
    }
}

/// Bodies whose invariant sites read a parameter, captures alone, a block's site, and nothing.
const KEYED: &str = "EXPR #(OUTER m :Counter) -> Number = \
                     #((LET g = (FN :{x :Number} -> Number = #((LET v = (m :| Counter)) (x)))) (g 1))\n\
                     EXPR #(BLOCKED m :Counter k :Number) -> Number = \
                     #(USING empty SCOPE ((LET v = (m :| Counter)) (k)))\n\
                     LET plain = (FN :{x :Number} -> Number = #(x))\n";

/// Each shape's slot names, beside its kind and the parameters its record keys a
/// specialization on, by name.
fn keyed(source: &str) -> Vec<(ShapeKind, Vec<String>, Option<Vec<String>>)> {
    loaded(source, |program| {
        let mut shapes = Vec::new();
        every_shape(program.shape(), &mut shapes);
        let name = |shape: &BodyShape<'_>, slot: Slot| {
            let symbol = shape.slot_name(slot).symbol();
            program.symbols().display(symbol).to_string()
        };
        (shapes.into_iter())
            .map(|shape| {
                let names = (0..shape.slots())
                    .map(|index| name(shape, Slot(index as u32)))
                    .collect();
                let invariant = (shape.caching().and_then(|caching| caching.invariant))
                    .map(|slots| slots.iter().map(|slot| name(shape, *slot)).collect());
                (shape.kind(), names, invariant)
            })
            .collect()
    })
}

#[test]
fn a_caching_frame_keys_on_the_parameters_its_invariant_sites_read() {
    let shapes = keyed(&format!("{PRELUDE}{INVARIANT}{KEYED}"));
    let of = |names: &[&str]| {
        let found = (shapes.iter()).find(|(_, each, _)| each == names);
        let (kind, _, invariant) = found.unwrap_or_else(|| panic!("a shape binds {names:?}"));
        (*kind, invariant.clone())
    };
    let some = |names: &[&str]| Some(names.iter().map(|name| name.to_string()).collect());
    assert_eq!(
        of(&["link", "v", "m"]),
        (ShapeKind::Callable, some(&["m"])),
        "a loop viewing the module it passes on keys on it"
    );
    assert_eq!(
        of(&["m", "end"]),
        (ShapeKind::Callable, None),
        "a body with no invariant site keys on nothing"
    );
    assert_eq!(
        of(&["x", "v"]),
        (ShapeKind::Callable, some(&[])),
        "a body viewing a capture alone keys on no parameter"
    );
    assert_eq!(of(&["g", "m"]), (ShapeKind::Callable, None));
    assert_eq!(
        of(&["m", "k"]),
        (ShapeKind::Callable, some(&["m"])),
        "a frame keys on what its blocks' invariant sites read"
    );
    assert_eq!(
        of(&["unused", "v"]),
        (ShapeKind::Block, None),
        "a block's own record keys on nothing"
    );
    assert!(
        (shapes.iter())
            .filter(|(kind, _, _)| *kind != ShapeKind::Callable)
            .all(|(_, _, invariant)| invariant.is_none()),
        "only a frame body's record keys a specialization"
    );
}

/// Each shape's slot names, beside the classes of its own minting sites in site order.
pub(super) fn minted(source: &str) -> Vec<(Vec<String>, Vec<Mint>)> {
    loaded(source, |program| {
        let mut shapes = Vec::new();
        every_shape(program.shape(), &mut shapes);
        (shapes.into_iter())
            .map(|shape| {
                let names = (0..shape.slots())
                    .map(|index| {
                        let symbol = shape.slot_name(Slot(index as u32)).symbol();
                        program.symbols().display(symbol).to_string()
                    })
                    .collect();
                let mints = (shape.caching().into_iter())
                    .flat_map(|caching| caching.mints.iter().map(|(_, mint)| *mint))
                    .collect();
                (names, mints)
            })
            .collect()
    })
}

#[test]
fn a_parameter_keys_only_where_its_digest_is_constant_cost() {
    // A list's, a record's or an `Any`'s digest costs its size: a parameter of such a type keys
    // nothing, and the view of a module built over it routes cold.
    for parameter in ["xs :(LIST OF Number)", "xs :{a :Number}", "xs :Any"] {
        let source = format!(
            "{PRELUDE}EXPR #(KEEP {parameter}) -> Number = \
             #((MODULE h OVER #[xs] = (LET held = xs)) (LET v = (h :| Holder)) (1))\n"
        );
        let keep = ["xs", "v", "h"].map(str::to_string).to_vec();
        let invariant = (keyed(&source).into_iter())
            .find(|(_, names, _)| *names == keep)
            .map(|(_, _, invariant)| invariant);
        assert_eq!(invariant, Some(None), "`{parameter}` keys nothing");
        let mints = (minted(&source).into_iter()).find(|(names, _)| *names == keep);
        assert_eq!(
            mints,
            Some((keep, vec![Mint::Varying])),
            "`{parameter}`'s view routes cold"
        );
    }
    // A type value's digest hashes its handle: a type binder keys, and a nominal over it is warm.
    let source = format!(
        "{PRELUDE}EXPR #(BOXED Elt :Type) -> Number = \
                          #((NEWTYPE Box = :{{v :Elt}}) (1))\n"
    );
    let boxed = |names: &[(Vec<String>, Vec<Mint>)]| {
        (names.iter())
            .find(|(names, _)| names.first().is_some_and(|name| name == "Elt"))
            .map(|(_, mints)| mints.clone())
    };
    assert_eq!(boxed(&minted(&source)), Some(vec![Mint::Invariant]));
    let invariant = (keyed(&source).into_iter())
        .find(|(_, names, _)| names.first().is_some_and(|name| name == "Elt"))
        .map(|(_, _, invariant)| invariant);
    assert_eq!(invariant, Some(Some(vec!["Elt".to_string()])));
}

#[test]
fn every_shape_of_a_load_has_its_own_segment_and_every_load_the_same() {
    let source = format!("{PRELUDE}{INVARIANT}{KEYED}LET q = #(1)\n");
    let segments = || {
        loaded(&source, |program| {
            let mut shapes = Vec::new();
            every_shape(program.shape(), &mut shapes);
            (shapes.iter())
                .map(|shape| (shape.kind(), shape.segment()))
                .collect::<Vec<_>>()
        })
    };
    let first = segments();
    let distinct: std::collections::BTreeSet<_> =
        first.iter().map(|(_, segment)| *segment).collect();
    assert_eq!(distinct.len(), first.len(), "no two shapes share a segment");
    for kind in [
        ShapeKind::Callable,
        ShapeKind::Block,
        ShapeKind::Module,
        ShapeKind::Code,
    ] {
        assert!(
            first.iter().any(|(each, _)| *each == kind),
            "the program holds a {kind:?} shape"
        );
    }
    assert_eq!(segments(), first, "a second load assigns the same segments");
}

/// What one load of `source` at depth 1 leaves after each run: the tiers, what the build root's
/// table holds, and what it printed; `between` runs on the loaded program between the runs.
pub(super) fn runs(
    source: &str,
    count: usize,
    between: impl Fn(&mut crate::program::Running<'_>),
) -> Vec<(Tiers, Option<usize>, String)> {
    let program = program(1, source);
    let mut substrate =
        CellSubstrate::load::<Koan>(&program, "<test>", 2, SpecBounds::default(), output())
            .unwrap_or_else(|error| panic!("the program loads: {error}"));
    (0..count)
        .map(|_| {
            WRITTEN.with(|written| written.borrow_mut().clear());
            TIERS.with(|tiers| tiers.set(Tiers::default()));
            substrate
                .with(|running| running.run())
                .expect("the program runs");
            let table = substrate.with(|running| {
                let build = running.roots().build;
                let table = running.scheduler().graph().table_len(build);
                between(running);
                table.expect("the build root is live")
            });
            let printed = WRITTEN.with(|written| written.borrow().join("\n"));
            (TIERS.with(Cell::get), table, printed)
        })
        .collect()
}

#[test]
fn a_loop_seeks_its_specialization_once_and_a_second_caller_finds_it_warm() {
    let once = format!("{INVARIANT}PRINT (COUNTDOWN ints chain)\n");
    let [(alone, ..)] = runs(&once, 1, |_| {})[..] else {
        unreachable!()
    };
    assert_eq!(
        (alone.sought, alone.specialized),
        (1, 1),
        "one seek for the whole loop"
    );
    let twice = format!("{once}PRINT (COUNTDOWN ints chain)");
    let [(both, table, ref printed)] = runs(&twice, 1, |_| {})[..] else {
        unreachable!()
    };
    assert_eq!(printed, "7\n7");
    assert_eq!(
        (both.sought, both.specialized),
        (2, 1),
        "the second caller builds nothing"
    );
    assert_eq!(table, Some(1));
    let warm = |source: &str| -> usize {
        laid(&program(1, source))
            .iter()
            .map(|(_, _, warm)| warm)
            .sum()
    };
    let warm_once = warm(&once);
    assert!(warm_once > 0, "the loop's view is adopted warm");
    assert_eq!(
        warm(&twice),
        warm_once,
        "the second caller finds every warm type laid"
    );
    let [(apart, table, _)] = runs(&format!("{once}PRINT (COUNTDOWN words chain)"), 1, |_| {})[..]
    else {
        unreachable!()
    };
    assert_eq!(apart.specialized, 2, "two bindings, two specializations");
    assert_eq!(table, Some(2));
}

#[test]
fn a_callable_born_under_a_specialization_keys_its_own_beneath_it() {
    let inner =
        "(LET g = (FN :{x :Number} -> Number = #((LET v = (m :| Counter)) (x)))) (g {x = 1})";
    for body in [inner.to_string(), format!("(USING empty SCOPE ({inner}))")] {
        let source = format!(
            "EXPR #(OUTER m :Counter) -> Number = #((LET w = (m :| Counter)) {body})\n\
             PRINT (OUTER ints)"
        );
        let [(tiers, table, ref printed)] = runs(&source, 1, |_| {})[..] else {
            unreachable!()
        };
        assert_eq!(printed, "1", "{body}");
        assert_eq!(
            (tiers.specialized, table),
            (2, Some(1)),
            "`g`'s specialization is a child of `OUTER`'s, not of the root: {body}"
        );
    }
}

#[test]
fn an_evicted_specialization_is_rebuilt_on_the_next_miss_with_the_same_answer() {
    let source = format!(
        "{INVARIANT}PRINT (COUNTDOWN ints chain)\nPRINT (COUNTDOWN words chain)\n\
         PRINT (COUNTDOWN ints chain)"
    );
    let one = NonZeroUsize::MIN;
    let tight = ran_under(
        DEPTHS[0],
        &source,
        SpecBounds {
            root: one,
            nested: one,
        },
    );
    assert_eq!(tight.printed, "7\n7\n7");
    assert_eq!(
        (tight.tiers.specialized, tight.tiers.evicted),
        (3, 2),
        "`words` evicts `ints`, and `ints` again evicts `words`"
    );
    let roomy = ran(DEPTHS[0], &source);
    assert_eq!(roomy.printed, tight.printed);
    assert_eq!((roomy.tiers.specialized, roomy.tiers.evicted), (2, 0));
}

/// Bounds of `root` and `nested`.
pub(super) fn bounds(root: usize, nested: usize) -> SpecBounds {
    SpecBounds {
        root: NonZeroUsize::new(root).expect("a bound is not zero"),
        nested: NonZeroUsize::new(nested).expect("a bound is not zero"),
    }
}

#[test]
fn a_loop_handing_a_keyed_helper_a_new_module_each_hop_holds_its_specializations_flat() {
    // `PEEK` keys on a new module each hop and `WRAP` on `ints` across the loop, both under the
    // root: each `PEEK` miss evicts the last `PEEK`, never `WRAP`, whose hops are newer births.
    let source = wrapped_peeks();
    let [few, many] = ENDS.map(|depth| ran_under(depth, &source, bounds(2, 2)));
    for (ran, depth) in [(&few, ENDS[0]), (&many, ENDS[1])] {
        let depth = depth as usize;
        assert_eq!(ran.printed, "done");
        assert_eq!(
            (
                ran.tiers.specialized,
                ran.tiers.evicted,
                ran.tiers.cold_births
            ),
            (depth + 1, depth - 1, 0),
            "every `WRAP` hop runs warm"
        );
        assert_eq!(ran.live, 2);
    }
    assert_eq!(
        (few.tiers.region, few.tiers.lent),
        (many.tiers.region, many.tiers.lent)
    );
    // `ENDS` stops at the root's default bound, so an operator run past it shows the bound biting.
    let bound = SpecBounds::default().root.get();
    let depth = bound + 4;
    let past = ran(depth as u32, &source);
    assert_eq!(past.printed, "done");
    assert_eq!((past.tiers.specialized, past.live), (depth + 1, bound));
}

#[test]
fn a_loop_whose_keyed_frame_calls_a_keyed_callable_holds_its_specializations_flat() {
    let source = "EXPR #(OUTER m :Holder) -> Number = \
                  #((LET w = (m :| Holder)) \
                  (LET g = (FN :{x :Number} -> Number = #((LET v = (m :| Holder)) (x)))) \
                  (g {x = 1}))\n\
                  EXPR #(DRIVE link :Link) -> Number = \
                  #((MODULE m OVER #[link] = (LET held = link)) (LET seen = (OUTER m)) \
                  (DRIVE (ATTR link next)))\n\
                  EXPR #(DRIVE end :Null) -> Number = #(7)\n\
                  PRINT (DRIVE chain)";
    let shapes = keyed(&format!(
        "{PRELUDE}LET chain = (Link {{next = null}})\n{source}"
    ));
    let invariant = |names: &[&str]| {
        let found = (shapes.iter()).find(|(_, each, _)| each == names);
        found
            .unwrap_or_else(|| panic!("a shape binds {names:?}"))
            .2
            .clone()
    };
    assert_eq!(invariant(&["g", "w", "m"]), Some(vec!["m".to_string()]));
    assert_eq!(
        invariant(&["x", "v"]),
        Some(vec![]),
        "`g` keys on its capture"
    );
    assert_eq!(
        invariant(&["link", "m", "seen"]),
        None,
        "`DRIVE` keys on nothing"
    );
    // Each hop builds an `OUTER` under the root and a `g` beneath it; a miss on the root's full
    // table releases the oldest `OUTER` with its `g`.
    let [few, many] = ENDS.map(|depth| ran_under(depth, source, bounds(2, 2)));
    for (ran, depth) in [(&few, ENDS[0]), (&many, ENDS[1])] {
        let depth = depth as usize;
        assert_eq!(ran.printed, "7");
        assert_eq!(
            (ran.tiers.specialized, ran.tiers.evicted, ran.live),
            (2 * depth, 2 * depth - 4, 4)
        );
    }
    assert_eq!(
        (few.tiers.region, few.tiers.lent),
        (many.tiers.region, many.tiers.lent)
    );
}

#[test]
fn a_frame_under_an_evicted_specialization_is_born_cold_with_the_same_answer() {
    // `SWEEP`'s first `PEEK` miss evicts `OUTER`'s specialization while `OUTER` runs, so `g`, born
    // under it, is born cold.
    let source = format!(
        "{PEEK}EXPR #(SWEEP link :Link) -> Str = \
         #((MODULE h OVER #[link] = (LET held = link)) (LET seen = (PEEK h)) \
         (SWEEP (ATTR link next)))\n\
         EXPR #(SWEEP end :Null) -> Str = #(\"done\")\n\
         EXPR #(OUTER m :Counter) -> Number = #((LET w = (m :| Counter)) \
         (LET g = (FN :{{x :Number}} -> Number = #((LET v = (m :| Counter)) (x)))) \
         (LET swept = (SWEEP chain)) (g {{x = 1}}))\n\
         PRINT (OUTER ints)"
    );
    let tight = ran_under(2, &source, bounds(1, 1));
    assert_eq!(tight.printed, "1");
    assert_eq!(tight.tiers.cold_births, 1, "`g` alone is born cold");
    let roomy = ran(2, &source);
    assert_eq!(roomy.printed, "1");
    assert_eq!(roomy.tiers.cold_births, 0);
}

/// The allocations one run of `source` makes.
fn allocations(source: &str) -> u64 {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 2, SpecBounds::default(), output())
            .unwrap_or_else(|error| panic!("the program loads: {error}"));
    let before = crate::tests::allocation_count();
    substrate
        .with(|running| running.run())
        .expect("the program runs");
    crate::tests::allocation_count() - before
}

#[test]
fn a_call_over_a_list_keys_nothing_and_its_birth_costs_below_the_lists_length() {
    // `TAKE`'s parameter is `Any`, so it keys nothing: no call seeks a specialization, and the view
    // of the module born over the list routes cold.
    let source = |list: &str, count: usize| {
        let mut source = format!(
            "{PRELUDE}EXPR #(TAKE xs :Any) -> Number = \
             #((MODULE h OVER #[xs] = (LET held = xs)) (LET seen = (h :| Holder)) (1))\n\
             LET l = [{list}]\n"
        );
        source.push_str(&"PRINT (TAKE l)\n".repeat(count));
        source
    };
    let shapes = keyed(&source("1", 1));
    assert!(
        (shapes.iter()).any(|(_, names, invariant)| names[0] == "xs" && invariant.is_none()),
        "`TAKE` keys nothing: {shapes:?}"
    );
    let per_birth = |length: usize| {
        let list = (0..length)
            .map(|at| at.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        TIERS.with(|tiers| tiers.set(Tiers::default()));
        let cost = (allocations(&source(&list, 4)) - allocations(&source(&list, 1))) / 3;
        let tiers = TIERS.with(Cell::get);
        assert_eq!((tiers.sought, tiers.specialized), (0, 0));
        cost
    };
    let (short, long) = (per_birth(1), per_birth(64));
    assert!(
        long - short < 64,
        "a birth over a 64-element list costs {long} allocations against {short} over one"
    );
}

#[test]
fn a_shares_callees_callable_keys_under_its_own_chain_not_its_hosts() {
    // `SHARE` returns no scalar, so it is a tenant of `OUTER`'s frame, and keys `m`, which `g`'s
    // view waits on; the callable it births was born under `SHARE`'s specialization, a child of
    // the root as `SHARE` itself was, so its specialization lies beneath `SHARE`'s, not `OUTER`'s.
    let source = "EXPR #(SHARE m :Counter) -> Any = \
                  #((LET g = (FN :{x :Number} -> Number = #((LET v = (m :| Counter)) (x)))) \
                  (g {x = 1}))\n\
                  EXPR #(OUTER m :Counter) -> Number = #((LET w = (m :| Counter)) (SHARE m) (2))\n\
                  PRINT (OUTER ints)";
    let [(tiers, table, ref printed)] = runs(source, 1, |_| {})[..] else {
        unreachable!()
    };
    assert_eq!(printed, "2");
    assert_eq!((tiers.specialized, table), (3, Some(2)));
}

/// [`INVARIANT`] at `Shares`: the same loop, ending in a string.
const SHARES_INVARIANT: &str = "EXPR #(COUNTDOWN m :Counter link :Link) -> Str = \
                                #((LET v = (m :| Counter)) (COUNTDOWN m (ATTR link next)))\n\
                                EXPR #(COUNTDOWN m :Counter end :Null) -> Str = #(\"seven\")\n";

#[test]
fn a_shares_loop_seeks_its_specialization_once_and_a_second_park_finds_it_warm() {
    let once = format!("{SHARES_INVARIANT}PRINT (COUNTDOWN ints chain)\n");
    let [(alone, ..)] = runs(&once, 1, |_| {})[..] else {
        unreachable!()
    };
    assert_eq!(
        (alone.sought, alone.specialized),
        (1, 1),
        "one seek for the whole loop"
    );
    let twice = format!("{once}PRINT (COUNTDOWN ints chain)");
    let [(both, table, ref printed)] = runs(&twice, 1, |_| {})[..] else {
        unreachable!()
    };
    assert_eq!(printed, "seven\nseven");
    assert_eq!(
        (both.sought, both.specialized),
        (2, 1),
        "the second park builds nothing"
    );
    assert_eq!(table, Some(1));
    let warm = |source: &str| -> usize {
        laid(&program(1, source))
            .iter()
            .map(|(_, _, warm)| warm)
            .sum()
    };
    let warm_once = warm(&once);
    assert!(warm_once > 0, "the loop's view is adopted warm");
    assert_eq!(
        warm(&twice),
        warm_once,
        "the second park finds every warm type laid"
    );
    assert!(
        alone.lent_verdicts > 0,
        "the view's verdicts are recorded warm"
    );
    assert_eq!(
        both.lent_verdicts, alone.lent_verdicts,
        "the second park records no verdict anew"
    );
}

#[test]
fn a_callable_born_in_a_shares_frame_keys_its_own_beneath_it() {
    let inner =
        "(LET g = (FN :{x :Number} -> Number = #((LET v = (m :| Counter)) (x)))) (g {x = 1})";
    for body in [inner.to_string(), format!("(USING empty SCOPE ({inner}))")] {
        let source = format!(
            "EXPR #(OUTER m :Counter) -> Any = #((LET w = (m :| Counter)) {body})\n\
             PRINT (OUTER ints)"
        );
        let [(tiers, table, ref printed)] = runs(&source, 1, |_| {})[..] else {
            unreachable!()
        };
        assert_eq!(printed, "1", "{body}");
        assert_eq!(
            (tiers.specialized, table),
            (2, Some(1)),
            "`g`'s specialization is a child of `OUTER`'s, not of the root: {body}"
        );
    }
}

#[test]
fn an_unkeyed_shares_callee_in_a_keyed_frame_keys_its_callables_under_its_own_birth() {
    // `OUTER` is a keyed tenant; `SHARE`, whose `m :Any` keys nothing, borrows nothing, so the
    // callable it births keys under the root `SHARE` was born under, not under `OUTER`'s
    // specialization.
    let source = "EXPR #(SHARE m :Any) -> Any = \
                  #((LET g = (FN :{x :Number} -> Number = #((LET v = (m :| Counter)) (x)))) \
                  (g {x = 1}))\n\
                  EXPR #(OUTER m :Counter) -> Any = #((LET w = (m :| Counter)) (SHARE m) (2))\n\
                  PRINT (OUTER ints)";
    let [(tiers, table, ref printed)] = runs(source, 1, |_| {})[..] else {
        unreachable!()
    };
    assert_eq!(printed, "2");
    assert_eq!((tiers.specialized, table), (2, Some(2)));
}

#[test]
fn a_keyed_shares_call_links_a_host_scratch_laid_before_it() {
    // `VARY` caches but keys nothing, so it lays its host's scratch branch through the `Shares`
    // chain, unlinked; the keyed loop asked for at the same park reads through it next.
    let vary = "EXPR #(VARY link :Link) -> Str = \
                #((MODULE m OVER #[link] = (LET held = link)) \
                (LET seen = (m :| Holder)) (VARY (ATTR link next)))\n\
                EXPR #(VARY end :Null) -> Str = #(\"done\")\n";
    let shapes = keyed(&format!("{PRELUDE}{vary}"));
    assert!(
        (shapes.iter()).any(|(_, names, invariant)| names[0] == "link" && invariant.is_none()),
        "`VARY` keys nothing: {shapes:?}"
    );
    let printed = |calls: &str| {
        let [(_, _, ref printed)] =
            runs(&format!("{SHARES_INVARIANT}{vary}PRINT {calls}"), 1, |_| {})[..]
        else {
            unreachable!()
        };
        printed.clone()
    };
    let both = printed("{first = (VARY chain), second = (COUNTDOWN ints chain)}");
    assert_eq!(both, "{first = done, second = seven}");
    assert_eq!(
        (printed("(VARY chain)"), printed("(COUNTDOWN ints chain)")),
        ("done".to_string(), "seven".to_string())
    );
}

/// `BOXED`, keyed on a type it composes a nominal over. `ASK` composes a record type at run and
/// hands it to `BOXED` by a child; `HOPPING` composes it and hands it by a child to `HOP`, which
/// passes it on to `BOXED` by a tail hop. A hop keys only a type it passes through unchanged, so
/// the type is composed one call up.
const BOXED: &str = "EXPR #(BOXED Elt :Type) -> Number = #((NEWTYPE Box = :{v :Elt}) (1))\n\
                     EXPR #(HOP Elt :Type) -> Number = #(BOXED Elt)\n\
                     EXPR #(ASK Elt :Type) -> Number = \
                     #((LET r = (BOXED :{a :Elt b :Elt})) (r))\n\
                     EXPR #(HOPPING Elt :Type) -> Number = \
                     #((LET r = (HOP :{a :Elt b :Elt})) (r))\n";

#[test]
fn a_type_composed_at_run_reaches_the_specialization_it_keys_by_value() {
    // Each asker keys its own `Elt`, which its statement's call reads, and `HOP` keys the
    // record it is handed: one specialization per keyed frame, and the record relayed into each
    // one keyed on it.
    for (asker, keyed, relayed) in [("ASK", 2, 1), ("HOPPING", 3, 2)] {
        let once = ran(DEPTHS[0], &format!("{BOXED}PRINT ({asker} Number)\n"));
        let twice = ran(
            DEPTHS[0],
            &format!("{BOXED}LET first = ({asker} Number)\nPRINT ({asker} Number)\n"),
        );
        assert_eq!((once.printed.as_str(), twice.printed.as_str()), ("1", "1"));
        assert_eq!(
            (
                once.tiers.sought,
                once.tiers.specialized,
                once.tiers.relayed
            ),
            (keyed, keyed, relayed),
            "`{asker}`'s composed record is laid by value"
        );
        assert_eq!(
            (
                twice.tiers.sought,
                twice.tiers.specialized,
                twice.tiers.relayed
            ),
            (2 * keyed, keyed, relayed),
            "`{asker}`'s second call hits, and a hit relays nothing"
        );
    }
    assert_eq!(
        ran(DEPTHS[0], &invariant_loop()).tiers.relayed,
        0,
        "a written module's types are the root's"
    );
}
