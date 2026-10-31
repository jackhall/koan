//! A keyworded self-call in tail position hops to the callee's frame: however deep the recursion,
//! the cells live at once stay constant, through `MATCH` arms in tail position too. A call by name,
//! in tail position or not, binds each parameter to the field of its argument record the parameter
//! names.

use std::cell::Cell;

use crate::program::{CellSubstrate, KBirth, KBundle, KState, run as body};
use crate::scheduler::{Action, Placement, Step, StepError, Work};

use super::super::Koan;
use super::output;
use crate::values::BuildBudget;

/// A program whose `WALK` follows a chain of `depth` links, each a newtype over a record naming the
/// next, to the `null` at its end — selecting its overload by the link's type, and calling itself
/// by keyword as its last statement.
fn chain(depth: u32) -> String {
    let mut source = String::from(
        "NEWTYPE Link = :{next :Any}\n\
         EXPR #(WALK link :Link) -> Str = #(WALK (ATTR link next))\n\
         EXPR #(WALK end :Null) -> Str = #(\"done\")\n\
         LET link0 = null\n",
    );
    for at in 1..=depth {
        source.push_str(&format!(
            "LET link{at} = (Link {{next = link{}}})\n",
            at - 1
        ));
    }
    source.push_str(&format!("PRINT (WALK link{depth})"));
    source
}

/// The most cells live at once while the walk of a `depth`-link chain runs.
fn peak_walking(depth: u32) -> usize {
    peak_of(&chain(depth))
}

/// The most cells live at once while `source` runs.
fn peak_of(source: &str) -> usize {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 2, BuildBudget::default(), output())
            .unwrap_or_else(|error| panic!("the program loads: {error}"));
    substrate.with(|running| {
        let work = Work {
            step: body,
            state: KBirth::Program {
                program: running.program(),
            },
        };
        let root = running.root();
        let mut scheduler = running.scheduler();
        scheduler
            .run(work, root, Placement::Shares)
            .expect("the program runs");
        scheduler.peak_live_cells()
    })
}

#[test]
fn a_keyworded_self_call_in_tail_position_holds_its_cells_constant_however_deep() {
    // The frame tails its last statement into the evaluator under its contract; the evaluator
    // selects `WALK` again, whose declared `Str` the contract asks, and hops to its frame.
    // Under Miri one hop past the shallow walk is enough to exercise the hop's crossing.
    let (deep, shallow) = if cfg!(miri) { (3, 2) } else { (10_000, 4) };
    assert_eq!(peak_walking(deep), peak_walking(shallow));
}

/// What each hop of a [`countdown`] runs before it hops: a module over the link it is handed, viewed opaquely,
/// so each hop holds different content and mints its view's carrier.
const VIEW: &str = "(MODULE m OVER #[link] = (LET held = link)) (LET seen = (m :| Holder))";

/// [`VIEW`]'s module, viewed at a signature with no head parameter: the same work, its content's
/// digest included, less the minting.
const PLAIN: &str = "(MODULE m OVER #[link] = (LET held = link)) (LET seen = (m :| Plain))";

/// A program whose `COUNTDOWN` follows a chain of `depth` links as `WALK` does, running `body`
/// before each hop. `call` false leaves the chain unwalked, so the difference prices the walk.
fn countdown(depth: u32, body: &str, call: bool) -> String {
    let mut source = format!(
        "SIG Holder FOR ALL #[Elt] = #[(VAL held :Elt)]\n\
         SIG Plain = #[(VAL held :Any)]\n\
         NEWTYPE Link = :{{next :Any}}\n\
         EXPR #(COUNTDOWN link :Link) -> Str = #({body} (COUNTDOWN (ATTR link next)))\n\
         EXPR #(COUNTDOWN end :Null) -> Str = #(\"done\")\n\
         LET link0 = null\n"
    );
    for at in 1..=depth {
        source.push_str(&format!(
            "LET link{at} = (Link {{next = link{}}})\n",
            at - 1
        ));
    }
    if call {
        source.push_str(&format!("PRINT (COUNTDOWN link{depth})"));
    }
    source
}

thread_local! {
    /// What [`branch_length`] read.
    static BRANCH: Cell<usize> = const { Cell::new(0) };
}

/// A root work over the top level's resting view: it records the root cell's branch length.
fn branch_length<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>,
) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let KState::Born(birth @ KBirth::Inspect { .. }) = state else {
        return step.failed(StepError::Refused);
    };
    BRANCH.with(|branch| {
        branch.set(
            step.header()
                .get()
                .and_then(|header| header.branch())
                .map_or(0, |branch| branch.len()),
        )
    });
    step.leave(birth)
}

/// What one walk of a countdown leaves: the most cells live at once, the heap allocations the
/// walk made, the root's length before and after it, and the root cell's branch length after.
struct Walk {
    peak: usize,
    allocations: u64,
    root: [usize; 2],
    branch: usize,
}

/// The heap allocations `running.run()` makes over `source`.
fn run_allocations(source: &str) -> u64 {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 2, BuildBudget::default(), output())
            .unwrap_or_else(|error| panic!("the countdown loads: {error}"));
    let before = crate::tests::allocation_count();
    substrate
        .with(|running| running.run())
        .expect("the countdown runs");
    crate::tests::allocation_count() - before
}

/// One walk of a `depth`-link countdown running `body`, its allocations net of the same program
/// run without the walk.
fn walking(depth: u32, body: &str) -> Walk {
    let source = countdown(depth, body, true);
    let peak = peak_of(&source);
    let mut substrate =
        CellSubstrate::load::<Koan>(&source, "<test>", 2, BuildBudget::default(), output())
            .unwrap_or_else(|error| panic!("the countdown loads: {error}"));
    let before = substrate.with(|running| running.program().tiers().frozen.len());
    substrate
        .with(|running| running.run())
        .expect("the countdown runs");
    let after = substrate.with(|running| running.program().tiers().frozen.len());
    substrate
        .with(|running| running.inspect(branch_length))
        .expect("the inspection runs");
    let allocations = run_allocations(&source) - run_allocations(&countdown(depth, body, false));
    Walk {
        peak,
        allocations,
        root: [before, after],
        branch: BRANCH.with(Cell::get),
    }
}

#[test]
fn a_fresh_loop_viewing_a_new_module_each_hop_holds_its_types_flat() {
    // Each hop's frame is a cell of its own, so the carrier it mints lives in that frame's branch
    // and dies with it. A koan hop costs the heap a few allocations whatever it does, and a view's
    // digest of the module more as the chain it captures deepens, so the minting is priced as the
    // view's cost over a view minting nothing: the same per hop at both depths.
    let (few, many) = if cfg!(miri) { (2, 4) } else { (64, 512) };
    let [viewed_few, viewed_many] = [few, many].map(|depth| walking(depth, VIEW));
    let [plain_few, plain_many] = [few, many].map(|depth| walking(depth, PLAIN));
    assert_eq!(
        viewed_many.peak, viewed_few.peak,
        "a hop releases its predecessor"
    );
    assert_eq!(
        viewed_few.root[0], viewed_few.root[1],
        "the run writes no root"
    );
    assert_eq!(
        viewed_many.root, viewed_few.root,
        "the load's types are the depth's"
    );
    assert_eq!(
        [viewed_few.branch, viewed_many.branch],
        [0, 0],
        "no carrier reaches the root cell's branch"
    );
    let minting = |viewed: &Walk, plain: &Walk| viewed.allocations - plain.allocations;
    let (per_few, per_many) = (
        minting(&viewed_few, &plain_few),
        minting(&viewed_many, &plain_many),
    );
    // A bump claims chunks as its bytes double, so the two depths may differ by a handful of
    // allocations that grow with the log of the depth, never with it.
    let slack = u64::from(many.ilog2()) * u64::from(few);
    assert!(
        per_many * u64::from(few) <= per_few * u64::from(many) + slack,
        "minting costs {per_many} over {many} hops against {per_few} over {few}"
    );
}

/// `RUNS`, keyed on `k` — its `PICK` reads `k`'s carrier — which hops into itself down the chain,
/// and `GO`, which tails into `RUNS` by name.
const RUNS: &str = "EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                    EXPR #(PICK x :Str) -> Number = #(1)\n\
                    EXPR #(RUNS k :Counter link :Link) -> Number = \
                    #((PRINT (PICK k.zero)) (RUNS k (ATTR link next)))\n\
                    EXPR #(RUNS k :Counter end :Null) -> Number = #(7)\n\
                    LET go = (FN :{k :Counter} -> Number = #(RUNS k chain))\n";

/// Run `source` over a chain of `depth` links: what it printed, how many tail calls decided at the
/// call whether they keep their frame's contract, and how many hopped under their frame's own
/// settlement.
fn routed(depth: u32, source: &str) -> (String, usize, usize) {
    use crate::dispatch::evaluate::{DERIVED, HOPPED};
    DERIVED.with(|derived| derived.set(0));
    HOPPED.with(|hopped| hopped.set(0));
    let ran = super::caching::ran_under(depth, source, BuildBudget::default());
    (ran.printed, DERIVED.with(Cell::get), HOPPED.with(Cell::get))
}

#[test]
fn a_tail_call_reads_its_route_and_derives_none() {
    // `GO`'s call by name and each `RUNS` hop read whether they keep the contract off their route,
    // and each hop of `RUNS` into itself stays under its frame's settlement: three, the last
    // link's into the `end` overload's body.
    let (printed, derived, hopped) = routed(4, &format!("{RUNS}PRINT (go {{k = ints}})\n"));
    assert_eq!(printed, format!("{}7", "n\n".repeat(4)));
    assert_eq!((derived, hopped), (0, 3));
    // A call of a function the load does not know decides at the call.
    let unknown = "EXPR #(VIA f :Any) -> Number = #(f {k = ints})\nPRINT (VIA go)\n";
    let (printed, derived, _) = routed(4, &format!("{RUNS}{unknown}"));
    assert_eq!(printed, format!("{}7", "n\n".repeat(4)));
    assert_eq!(derived, 1);
}

#[test]
fn a_call_by_name_binds_each_parameter_to_the_field_it_names_past_a_hidden_one() {
    // Each argument record stores a field its type hides, sorting before a field it names.
    let source = "LET f = (FN :{x :Number} -> Any = #(x))\n\
                  PRINT (f ({x = 1, y = 3} :! :{x :Number}))\n\
                  PRINT (f ({a = 5, x = 7} :! :{x :Number}))\n\
                  LET g = (FN :{y :Number} -> Any = #(y))\n\
                  PRINT (g ({x = 5, y = 7} :! :{y :Number}))\n";
    assert_eq!(super::run(source), "1\n7\n7");
    // In tail position the call hops, and its successor binds by name too.
    let tail = "LET h = (FN :{x :Number, z :Number} -> Any = #(z))\n\
                LET k = (FN :{r :Any} -> Any = #(h r))\n\
                PRINT (k {r = ({a = 5, x = 7, y = 1, z = 9} :! :{x :Number, z :Number})})\n\
                LET f = (FN :{x :Number} -> Any = #(x))\n\
                LET g = (FN :{r :Any} -> Any = #(f r))\n\
                PRINT (g {r = ({a = 5, x = 7} :! :{x :Number})})\n";
    assert_eq!(super::run(tail), "9\n7");
}

/// A program whose `DOWN` counts down a `Nat` chain of `depth` links through `body`'s `MATCH` arms.
fn nat_chain(depth: u32, body: &str) -> String {
    let mut source = format!(
        "UNION Nat = #{{Zero: Null, Succ: Nat}}\n\
         EXPR #(DOWN n :Nat) -> Str = #({body})\n\
         LET n0 = (Nat.Zero null)\n"
    );
    for at in 1..=depth {
        source.push_str(&format!("LET n{at} = (Nat.Succ n{})\n", at - 1));
    }
    source.push_str(&format!("PRINT (DOWN n{depth})"));
    source
}

#[test]
fn a_recursion_through_tail_arms_holds_its_cells_constant_however_deep() {
    // The frame hands its `MATCH` off under its contract, the `MATCH` its arm, and the arm its
    // call, which hops to `DOWN`'s next frame.
    let (deep, shallow) = if cfg!(miri) { (3, 2) } else { (256, 8) };
    for body in [
        "MATCH n UNDER Nat WITH #{Zero: (\"done\"), Succ: (DOWN it)}",
        "MATCH n UNDER Nat WITH #{Zero: (\"done\"), \
         Succ: (MATCH it UNDER Nat WITH #{Zero: (\"one\"), Succ: (DOWN it)})}",
    ] {
        assert_eq!(
            peak_of(&nat_chain(deep, body)),
            peak_of(&nat_chain(shallow, body)),
            "{body}"
        );
    }
    // An arm whose last statement binds waits on its call, so each round holds a frame.
    let bound = "MATCH n UNDER Nat WITH #{Zero: (\"done\"), Succ: ((LET r = (DOWN it)) (r))}";
    assert!(peak_of(&nat_chain(deep, bound)) > peak_of(&nat_chain(shallow, bound)));
}

#[test]
fn an_arm_owes_its_frames_contract_whether_or_not_it_tails() {
    // An arm whose last statement binds runs as a child, and the `MATCH` holds its value to the
    // contract; a tail arm owes the contract itself.
    for arms in [
        "#{Number: (LET r = it), _: (LET r = \"s\")}",
        "#{Number: (it), _: (\"s\")}",
    ] {
        let f = format!("EXPR #(PIN x :Any) -> Number = #(MATCH x WITH {arms})\n");
        assert_eq!(super::run(&format!("{f}PRINT (PIN 1)")), "1");
        assert_eq!(
            super::run(&format!("{f}PRINT (PIN \"t\")")),
            "error: :(FN :{x :Any} -> Number) returned Str, which does not satisfy Number"
        );
    }
}

#[test]
fn a_tail_arms_call_reads_its_route_and_derives_none() {
    // Each hop of `RUNS` into itself is a tail arm's call, which reads its route and stays under
    // its frame's settlement: four, one per link.
    let runs = "EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                EXPR #(RUNS k :Counter link :Any) -> Number = #(MATCH link WITH \
                #{Link: ((PRINT (PICK k.zero)) (RUNS k (ATTR it next))), Null: (7)})\n\
                LET go = (FN :{k :Counter} -> Number = #(RUNS k chain))\n";
    let (printed, derived, hopped) = routed(4, &format!("{runs}PRINT (go {{k = ints}})\n"));
    assert_eq!(printed, format!("{}7", "n\n".repeat(4)));
    assert_eq!((derived, hopped), (0, 4));
}

#[test]
fn a_match_in_a_frame_called_through_a_barrier_runs_its_arm_as_a_child() {
    // The frame does not tail, so its value crosses the barrier, which hides `y`.
    let source = "SIG Shower = #[(VAL show :(FN :{} -> :{x :Number}))]\n\
                  MODULE m = (LET show = (FN :{} -> :{x :Number, y :Str} = \
                  #(MATCH 1 WITH #{Number: ({x = 1, y = \"leak\"})})))\n\
                  LET v = (m :| Shower)\nPRINT (v.show {})";
    assert_eq!(super::run(source), "{x = 1}");
}
