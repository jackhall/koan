//! Whole programs under the drain: unit order, where a value is built, the slab holding the root
//! alone, calls and recursion, components, eager parts and module bodies.

use crate::program::body::SUPPLIED_WAKES;

use super::evaluator::{recorded, reset};
use super::{loaded, read_back, run_and_read};

/// The depth a recursion runs to: past the slab cap, and small under Miri.
const DEPTH: u32 = if cfg!(miri) { 8 } else { 300 };

/// How many top-level statements the slab-cap test writes.
const STATEMENTS: usize = if cfg!(miri) { 8 } else { 200 };

/// The address a described list sits at.
fn address(described: &str) -> &str {
    described
        .rsplit('@')
        .next()
        .expect("a list is described with its address")
}

#[test]
fn independent_statements_run_in_source_order_without_interleaving() {
    let mut substrate = loaded("LET a = 1\n(5 MINUS 4)\nLET b = 3\n(9)", 2);
    reset();
    substrate.with(|running| running.run().expect("the program runs"));
    assert_eq!(
        recorded(),
        [
            "literal 1",
            "literal 5",
            "literal 4",
            "literal 3",
            "literal 9"
        ]
    );
}

#[test]
fn a_forward_capture_runs_its_binder_first() {
    let mut substrate = loaded(
        "LET f = (FN :{x :Number} -> Number = (g))\nLET g = 5\nLET r = (f 0)",
        2,
    );
    assert_eq!(run_and_read(&mut substrate, &["r", "g"]), ["5", "5"]);
}

#[test]
fn more_statements_than_the_slab_cap_run_to_completion() {
    let source: Vec<String> = (0..STATEMENTS)
        .map(|at| format!("LET v{at} = {at}"))
        .collect();
    // One cell: the root is the slab's only slot.
    let mut substrate = loaded(&source.join("\n"), 1);
    let name = format!("v{}", STATEMENTS - 1);
    assert_eq!(
        run_and_read(&mut substrate, &[&name]),
        [format!("{}", STATEMENTS - 1)]
    );
}

#[test]
fn a_top_level_binding_is_built_in_the_root_from_the_start() {
    // A fresh list is built in the evaluation's home, which is the root; so is a call's value,
    // through a frame whose last statement forwards it there.
    let mut substrate = loaded(
        "LET xs = [1 2 3]\nLET f = (FN :{n :Number} -> Any = ([4 5]))\nLET ys = (f 0)",
        2,
    );
    reset();
    substrate.with(|running| running.run().expect("the program runs"));
    let built: Vec<String> = recorded()
        .into_iter()
        .filter_map(|seen| seen.strip_prefix("built ").map(str::to_string))
        .collect();
    let read = read_back(&mut substrate, &["xs", "ys"]);
    assert_eq!(address(&read[0]), built[0], "`xs` is where it was built");
    assert_eq!(
        address(&read[1]),
        built[1],
        "`ys` is where the frame built it"
    );
}

#[test]
fn a_result_built_in_its_own_region_crosses_into_the_root_at_the_verdicts_price() {
    // Each frame's value is its last binding, built in the frame's region beside the other one:
    // a short list beside a long one copies into the root, and the frame's region is reclaimed; a
    // long one pins, and the region splices into the root.
    let long = vec!["n"; 256].join(" ");
    let source = format!(
        "LET f = (FN :{{n :Any}} -> Any = ((LET big = [{long}]) (LET small = [n n])))\n\
         LET g = (FN :{{n :Any}} -> Any = ((LET small = [n n]) (LET big = [{long}])))\n\
         LET copied = (f 1)\nLET pinned = (g 1)"
    );
    let mut substrate = loaded(&source, 2);
    reset();
    substrate.with(|running| running.run().expect("the program runs"));
    let built: Vec<String> = recorded()
        .into_iter()
        .filter_map(|seen| seen.strip_prefix("built ").map(str::to_string))
        .collect();
    assert_eq!(built.len(), 4, "{built:?}");
    let read = read_back(&mut substrate, &["copied", "pinned"]);
    assert_ne!(
        address(&read[0]),
        built[1],
        "the short list copies into the root"
    );
    assert_eq!(
        address(&read[1]),
        built[3],
        "the long list pins where it was built"
    );
}

#[test]
fn a_called_bodys_evaluations_are_tenants_of_the_frame() {
    // In a frame, `xs` is built by a tenant, at the frame's own brand, and the read after it finds
    // it where it lies.
    let mut substrate = loaded(
        "LET f = (FN :{n :Number} -> Any = ((LET xs = [n n]) (xs)))\nLET r = (f 7)",
        2,
    );
    reset();
    substrate.with(|running| running.run().expect("the program runs"));
    let seen = recorded();
    let built = seen
        .iter()
        .find_map(|seen| seen.strip_prefix("built "))
        .expect("the frame built `xs`");
    let read = seen
        .iter()
        .find_map(|seen| seen.strip_prefix("read "))
        .expect("the frame read `xs`");
    assert_eq!(built, read, "the value never travels to its reader");
}

#[test]
fn a_component_is_one_unit_bound_from_one_knot() {
    let mut substrate = loaded(
        "LET even = (FN :{n :Number} -> Number = (odd))\n\
         LET odd = (FN :{n :Number} -> Number = (even))",
        2,
    );
    let read = run_and_read(&mut substrate, &["even", "odd"]);
    assert!(read[0].starts_with("fn in "));
    assert_eq!(read[0], read[1], "both members sit in one knot");
}

#[test]
fn an_eager_part_is_supplied_by_site_in_one_wake() {
    let mut substrate = loaded(
        "LET g = (FN :{x :Number} -> Number = (x))\n\
         LET a = [(g 1) f (g 2)]\n\
         LET f = (FN :{x :Number} -> Any = (a))",
        2,
    );
    SUPPLIED_WAKES.with(|wakes| wakes.set(0));
    let read = run_and_read(&mut substrate, &["a"]);
    assert_eq!(SUPPLIED_WAKES.with(|wakes| wakes.get()), 1);
    assert!(
        read[0].starts_with("node in "),
        "`a` is a data node: {read:?}"
    );
}

#[test]
fn a_recursion_deeper_than_the_slab_cap_runs_on_tree_cells() {
    let source = format!(
        "LET count = (FN :{{n :Number}} -> Number = (WHEN n THEN (count (n MINUS 1)) ELSE 0))\n\
         LET r = (count {DEPTH})"
    );
    let mut substrate = loaded(&source, 1);
    assert_eq!(run_and_read(&mut substrate, &["r"]), ["0"]);
}

#[test]
fn a_module_binder_runs_its_body_inline_then_ties() {
    let mut substrate = loaded("MODULE m = ((LET a = 1) (LET b = [a a]))\nLET c = 2", 2);
    let read = run_and_read(&mut substrate, &["m", "c"]);
    // A module's members are its body's slots in layout order.
    let members = read[0]
        .strip_prefix("module(")
        .and_then(|members| members.strip_suffix(')'))
        .expect("`m` is a module");
    let mut members: Vec<&str> = members.split(", ").collect();
    members.sort_unstable();
    assert_eq!(members.len(), 2, "{read:?}");
    assert_eq!(members[0], "1");
    assert!(members[1].starts_with("[1 1]@"), "{read:?}");
    assert_eq!(read[1], "2");
}

#[test]
fn a_whole_program() {
    let source = format!(
        "LET count = (FN :{{n :Number}} -> Number = (WHEN n THEN (count (n MINUS 1)) ELSE n))\n\
         LET g = (FN :{{x :Number}} -> Number = (x))\n\
         LET a = [(g 1) f (g 2)]\n\
         LET f = (FN :{{x :Number}} -> Any = (a))\n\
         MODULE m = ((LET inner = (g 3)))\n\
         LET deep = (count {DEPTH})\n\
         LET last = (f 0)"
    );
    let mut substrate = loaded(&source, 1);
    let read = run_and_read(&mut substrate, &["deep", "m", "last", "a"]);
    assert_eq!(read[0], "0");
    assert_eq!(read[1], "module(3)");
    assert_eq!(
        read[2], read[3],
        "`f` returns `a`, the data node it closes over"
    );
}

#[test]
fn a_quantified_lambda_is_called_by_name() {
    let mut substrate = loaded(
        "LET id = (FN FOR ALL (Elt) :{x :Elt} -> Elt = (x))\nLET r = (id 7)",
        2,
    );
    let read = run_and_read(&mut substrate, &["r"]);
    assert_eq!(read[0], "7");
}

#[test]
fn a_call_binds_each_type_parameter_to_its_solution() {
    // The frame solves the callee's group against the arguments' carried types and binds `Elt` to
    // what it solved, so the body reads the argument's own type.
    let mut substrate = loaded(
        "LET which = (FN FOR ALL (Elt) :{x :Elt} -> Elt = (Elt))\n\
         LET n = (which 7)\nLET s = (which \"a\")",
        2,
    );
    let read = run_and_read(&mut substrate, &["n", "s"]);
    assert_eq!(read[0], "Number");
    assert_eq!(read[1], "Str");
}

/// A quantified return is no scalar, so a call through one shares its frame rather than placing
/// its result fresh — which is what [`a_quantified_return_shares_its_frame`] observes end to end.
#[test]
fn each_type_parameter_is_bound_by_name_not_by_slot_order() {
    // A callee's type-parameter slots reach its frame **symbol-sorted**, which is BLAKE3 order and
    // so unrelated to what was written. These two callees are the same type — canonical form drops
    // the unused name from both — and differ only in the order their groups were written, so a
    // frame that read the map positionally would hand one of them the other's answer. Both must
    // read `Unused` as `Any` whichever way the two symbols happen to sort.
    let mut substrate = loaded(
        "LET ab = (FN FOR ALL (Held Unused) :{x :(LIST OF Held)} -> Held = (Unused))\n\
         LET ba = (FN FOR ALL (Unused Held) :{x :(LIST OF Held)} -> Held = (Unused))\n\
         LET one = (ab [1 2])\nLET two = (ba [1 2])",
        2,
    );
    let read = run_and_read(&mut substrate, &["one", "two"]);
    assert_eq!(read[0], "Any", "`Unused` is dropped by canonical form");
    assert_eq!(
        read[1], read[0],
        "the written order does not change the answer"
    );
}

#[test]
fn a_call_whose_argument_cannot_solve_the_group_is_refused() {
    // `Held` is reached only under a list, so a bare number admits nowhere and the walk refuses
    // before the frame binds anything.
    let mut substrate = loaded(
        "LET first = (FN FOR ALL (Held) :{x :(LIST OF Held)} -> Held = (x))\nLET r = (first 7)",
        2,
    );
    let outcome = substrate.with(|running| running.run());
    assert!(
        outcome.is_err(),
        "a group the argument cannot solve refuses"
    );
}

#[test]
fn a_quantified_return_places_as_shares() {
    let arena = crate::memory::Bump::new();
    let types = crate::type_lattice::TypeRegistry::in_region(&arena);
    assert_eq!(
        crate::program::placement_of(types.quantified(0, crate::type_lattice::KType::ANY)),
        crate::scheduler::Placement::Shares
    );
}

#[test]
fn a_quantified_return_shares_its_frame() {
    // A quantified return places as `Shares`, so the frame forwards the argument rather than
    // copying it: `r` reads back at the very address `xs` was built at.
    let mut substrate = loaded(
        "LET id = (FN FOR ALL (Elt) :{x :Elt} -> Elt = (x))\nLET xs = [1 2]\nLET r = (id xs)",
        2,
    );
    let read = run_and_read(&mut substrate, &["xs", "r"]);
    assert_eq!(address(&read[0]), address(&read[1]), "{read:?}");
}
