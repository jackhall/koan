//! Caching bodies at run: a caching body's types live in a scratch tier, so a loop over one holds
//! the lattice flat across hops, and what leaves the body reads after its scratch resets.

use std::cell::Cell;
use std::collections::BTreeMap;

use crate::program::CellSubstrate;
use crate::scheduler::{LAID, TIERS, Tiers};
use crate::scope::{BodyShape, ShapeKind, Slot};

use super::super::Koan;
use super::classes::tutorial_snippets;
use super::statics::loaded;
use super::{WRITTEN, output};

/// `SIG Counter`, whose head parameter an opaque view hides, `MODULE ints` and `MODULE words`,
/// which fit it at different carriers, `Link`, a chain a loop walks to the `null` at its end, and
/// `MODULE empty`, which a `USING … SCOPE` enters to run a block.
const PRELUDE: &str = "SIG Counter FOR ALL #[Carrier] = \
                       #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                       MODULE ints = ((LET zero = 0) \
                       (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n\
                       MODULE words = ((LET zero = \"\") \
                       (LET succ = (FN :{x :Str} -> Str = #(x))))\n\
                       SIG Holder FOR ALL #[Elt] = #[(VAL held :Elt)]\n\
                       NEWTYPE Link = :{next :Any}\n\
                       MODULE empty = (LET unused = 0)\n";

/// What one run left: the most any step left in each tier, how many nodes the run added to the
/// root, and what it printed.
#[derive(Debug, PartialEq, Eq)]
struct Ran {
    tiers: Tiers,
    root: usize,
    printed: String,
}

/// [`PRELUDE`], a chain of `depth` links bound to `chain`, and `source`.
fn program(depth: u32, source: &str) -> String {
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
    let program = program(depth, source);
    WRITTEN.with(|written| written.borrow_mut().clear());
    let mut substrate = CellSubstrate::load::<Koan>(&program, "<test>", 2, output())
        .unwrap_or_else(|error| panic!("the program loads: {error}"));
    let before = substrate.with(|running| running.program().root().len());
    TIERS.with(|tiers| tiers.set(Tiers::default()));
    substrate
        .with(|running| running.run())
        .expect("the program runs");
    let after = substrate.with(|running| running.program().root().len());
    Ran {
        tiers: TIERS.with(Cell::get),
        root: after - before,
        printed: WRITTEN.with(|written| written.borrow().join("\n")),
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

/// The smallest depths that show a trend.
const DEPTHS: [u32; 2] = [8, 32];

/// `source` run at both [`DEPTHS`].
fn at_depths(source: &str) -> [Ran; 2] {
    DEPTHS.map(|depth| ran(depth, source))
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

#[test]
fn a_shares_call_with_new_content_each_call_holds_its_hosts_region_flat() {
    let source = hosted_peeks();
    assert_eq!(caching(&source), 1, "`PEEK` caches");
    let [few, many] = at_depths(&source);
    assert_eq!((few.printed.as_str(), few.root), ("done", 0));
    // The host's scratch grows with the loop, as its bytes do; its region does not.
    assert_eq!(few.tiers.region, many.tiers.region);
}

#[test]
fn a_loop_passing_on_a_module_built_over_its_link_caches_and_holds_flat() {
    let source = wrapped_peeks();
    assert_eq!(caching(&source), 2, "`WRAP` and `PEEK` cache");
    let ran_adopting = |depth: u32| {
        crate::values::ADOPTED.with(|adopted| adopted.set(0));
        let ran = ran(depth, &source);
        (ran, crate::values::ADOPTED.with(Cell::get))
    };
    let [(few, few_adopted), (many, many_adopted)] = DEPTHS.map(ran_adopting);
    assert_eq!((few.printed.as_str(), few.root), ("done", 0));
    // `PEEK`'s warm tier grows with the loop, as in `hosted_peeks`; the region does not.
    assert_eq!(few.tiers.region, many.tiers.region);
    assert!(few_adopted > 0, "the loop's exits adopt");
    // A hop's adoptions at the greater depth cost no more than at the lesser.
    assert!(
        many_adopted * DEPTHS[0] as usize <= few_adopted * DEPTHS[1] as usize,
        "adoptions {few_adopted} over {} hops against {many_adopted} over {}",
        DEPTHS[0],
        DEPTHS[1]
    );
}

#[test]
fn a_fresh_loop_over_an_invariant_module_holds_its_lent_branch_flat() {
    let source = invariant_loop();
    assert_eq!(caching(&source), 1);
    let [few, many] = at_depths(&source);
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
    let [few, many] = at_depths(source);
    assert_eq!(few.printed, "done");
    assert!(few.tiers.scratch_bytes > 0);
    assert_eq!(few, many);
}

#[test]
fn a_fresh_loop_over_varying_content_holds_the_lattice_flat() {
    let source = varying_loop();
    assert_eq!(caching(&source), 1);
    let [few, many] = at_depths(&source);
    assert_eq!((few.printed.as_str(), few.root), ("7", 0));
    assert_eq!(few, many);
}

#[test]
fn a_fresh_loop_calling_a_shares_caching_helper_holds_the_lattice_flat() {
    // Each hop's frame caches and holds its scratch; the helper, viewing a module over the link it
    // is handed, is its tenant, so it runs on that frame's own scratch, never the lent branch the
    // loop's hops share.
    let source = HELPER_LOOP;
    assert_eq!(caching(source), 2);
    let [few, many] = at_depths(source);
    assert_eq!(few.printed, "7");
    assert_eq!(few, many);
}

#[test]
fn a_mixed_placement_tail_chain_over_invariant_content_holds_flat() {
    let source = mixed_chain();
    assert_eq!(caching(&source), 2);
    let [few, many] = at_depths(&source);
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
fn a_received_list_passed_on_is_adopted_by_its_memo_alone() {
    let adopted = |length: usize| {
        let items = vec!["1"; length].join(", ");
        let source = format!(
            "EXPR #(PASS m :Counter xs :Any link :Link) -> Str = \
             #((LET v = (m :| Counter)) (PASS m xs (ATTR link next)))\n\
             EXPR #(PASS m :Counter xs :Any end :Null) -> Str = #(\"done\")\n\
             PRINT (PASS ints [{items}] chain)"
        );
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
    let by_name = "LET peek = (FN :{link :Any} -> Number = #(1))\n\
                   EXPR #(WALK m :Counter link :Link) -> Number = \
                   #((LET v = (m :| Counter)) (LET t = (peek {link = (ATTR link next)})) \
                   (WALK m (ATTR link next)))\n\
                   EXPR #(WALK m :Counter end :Null) -> Number = #(7)\n\
                   PRINT (WALK ints chain)";
    for source in [invariant_loop(), by_name.to_string()] {
        assert_eq!(caching(&source), 1);
        let adopted = |depth: u32| {
            crate::values::ADOPTED.with(|adopted| adopted.set(0));
            assert_eq!(ran(depth, &source).printed, "7");
            crate::values::ADOPTED.with(Cell::get)
        };
        let (few, many) = (adopted(DEPTHS[0]), adopted(DEPTHS[1]));
        assert!(few > 0, "the loop's exits adopt");
        // A hop's adoptions at the greater depth cost no more than at the lesser.
        assert!(
            many * DEPTHS[0] as usize <= few * DEPTHS[1] as usize,
            "adoptions {few} over {} hops against {many} over {}",
            DEPTHS[0],
            DEPTHS[1]
        );
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
    let Ok(mut substrate) = CellSubstrate::load::<Koan>(program, "<measured>", 2, output()) else {
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
/// tutorial's runnable programs, then per caching body over the flat-loop programs at both
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
fn every_shape<'graph>(shape: &'graph BodyShape<'graph>, out: &mut Vec<&'graph BodyShape<'graph>>) {
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
fn runs(
    source: &str,
    count: usize,
    between: impl Fn(&mut crate::program::Running<'_>),
) -> Vec<(Tiers, Option<usize>, String)> {
    let program = program(1, source);
    let mut substrate = CellSubstrate::load::<Koan>(&program, "<test>", 2, output())
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
    let source = format!("{INVARIANT}PRINT (COUNTDOWN ints chain)");
    let evict = |running: &mut crate::program::Running<'_>| {
        let build = running.roots().build;
        let mut scheduler = running.scheduler();
        let spec = (scheduler.graph().table_entry(build)).expect("the table names one");
        let _ = scheduler.evict(spec);
    };
    let ran = runs(&source, 2, evict);
    let [
        (first, first_table, ref first_printed),
        (second, second_table, ref second_printed),
    ] = ran[..]
    else {
        unreachable!()
    };
    assert_eq!(first_printed, second_printed);
    assert_eq!(
        (first.specialized, second.specialized),
        (1, 1),
        "the miss rebuilds it"
    );
    assert_eq!(
        (first_table, second_table),
        (Some(1), Some(1)),
        "its entry is renewed"
    );
}

/// The allocations one run of `source` makes.
fn allocations(source: &str) -> u64 {
    let mut substrate = CellSubstrate::load::<Koan>(source, "<test>", 2, output())
        .unwrap_or_else(|error| panic!("the program loads: {error}"));
    let before = crate::tests::allocation_count();
    substrate
        .with(|running| running.run())
        .expect("the program runs");
    crate::tests::allocation_count() - before
}

#[test]
fn a_birth_keyed_on_a_list_digests_it_at_a_cost_below_its_length() {
    // `TAKE` views a module built over the list it is handed, so its specialization is keyed on
    // the list's digest, computed at each birth.
    let per_birth = |length: usize| {
        let list = (0..length)
            .map(|at| at.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let calls = |count: usize| {
            let mut source = format!(
                "{PRELUDE}EXPR #(TAKE xs :Any) -> Number = \
                 #((MODULE h OVER #[xs] = (LET held = xs)) (LET seen = (h :| Holder)) (1))\n\
                 LET l = [{list}]\n"
            );
            source.push_str(&"PRINT (TAKE l)\n".repeat(count));
            allocations(&source)
        };
        TIERS.with(|tiers| tiers.set(Tiers::default()));
        let cost = (calls(4) - calls(1)) / 3;
        let tiers = TIERS.with(Cell::get);
        assert_eq!(
            (tiers.sought, tiers.specialized),
            (5, 2),
            "each run builds one"
        );
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
    // `SHARE` returns no scalar, so it is a tenant of `OUTER`'s frame; the callable it births
    // was born under the root, as `SHARE` itself was, so its specialization is the root's child.
    let source = "EXPR #(SHARE m :Counter) -> Any = \
                  #((LET g = (FN :{x :Number} -> Number = #((LET v = (m :| Counter)) (x)))) \
                  (g {x = 1}))\n\
                  EXPR #(OUTER m :Counter) -> Number = #((LET w = (m :| Counter)) (SHARE m) (2))\n\
                  PRINT (OUTER ints)";
    let [(tiers, table, ref printed)] = runs(source, 1, |_| {})[..] else {
        unreachable!()
    };
    assert_eq!(printed, "2");
    assert_eq!((tiers.specialized, table), (2, Some(2)));
}
