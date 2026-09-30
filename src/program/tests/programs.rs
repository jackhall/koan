//! Whole programs under the drain: unit order, where a value is built, the slab holding the root
//! alone, calls and recursion, components, eager parts, module bodies, lambdas — born where they
//! are written, returned from a frame, and held in a knot — quotes run by `EVAL`, error values,
//! the contract a frame ends under, and the tail a frame hands its last statement to.

use crate::program::body::SUPPLIED_WAKES;
use crate::program::{CellSubstrate, Outcome};

use super::evaluator::{Mini, recorded, reset};
use super::{compare_back, loaded, output, read_back, run_and_read, type_back, written};

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
        "LET f = (FN :{x :Number} -> Number = #(g))\nLET g = 5\nLET r = (f 0)",
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
        "LET xs = [1 2 3]\nLET f = (FN :{n :Number} -> Any = #([4 5]))\nLET ys = (f 0)",
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
        "LET f = (FN :{{n :Any}} -> Any = #((LET big = [{long}]) (LET small = [n n])))\n\
         LET g = (FN :{{n :Any}} -> Any = #((LET small = [n n]) (LET big = [{long}])))\n\
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
        "LET f = (FN :{n :Number} -> Any = #((LET xs = [n n]) (xs)))\nLET r = (f 7)",
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
        "LET even = (FN :{n :Number} -> Number = #(odd))\n\
         LET odd = (FN :{n :Number} -> Number = #(even))",
        2,
    );
    let read = run_and_read(&mut substrate, &["even", "odd"]);
    assert!(read[0].starts_with("fn in "));
    assert_eq!(read[0], read[1], "both members sit in one knot");
}

#[test]
fn an_eager_part_is_supplied_by_site_in_one_wake() {
    let mut substrate = loaded(
        "LET g = (FN :{x :Number} -> Number = #(x))\n\
         LET a = [(g 1) f (g 2)]\n\
         LET f = (FN :{x :Number} -> Any = #(a))",
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
        "LET count = (FN :{{n :Number}} -> Number = #(WHEN n THEN (count (n MINUS 1)) ELSE 0))\n\
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
        "LET count = (FN :{{n :Number}} -> Number = #(WHEN n THEN (count (n MINUS 1)) ELSE n))\n\
         LET g = (FN :{{x :Number}} -> Number = #(x))\n\
         LET a = [(g 1) f (g 2)]\n\
         LET f = (FN :{{x :Number}} -> Any = #(a))\n\
         MODULE m = ((LET inner = (g 3)))\n\
         LET deep = (count {DEPTH})\n\
         LET last = (f 0)"
    );
    let mut substrate = loaded(&source, 1);
    let read = run_and_read(&mut substrate, &["deep", "m"]);
    assert_eq!(read[0], "0");
    assert_eq!(read[1], "module(3)");
    assert_eq!(
        compare_back(&mut substrate, "last", "a"),
        "Ok(true)",
        "`f` returns `a`, the data node it closes over"
    );
}

#[test]
fn a_quantified_lambda_is_called_by_name() {
    let mut substrate = loaded(
        "LET id = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))\nLET r = (id 7)",
        2,
    );
    let read = run_and_read(&mut substrate, &["r"]);
    assert_eq!(read[0], "7");
}

/// Run `substrate`'s program to completion and hand back every type value its bodies read.
fn read_types(substrate: &mut CellSubstrate) -> Vec<String> {
    reset();
    assert_eq!(
        substrate.with(|running| running.run()),
        Ok(Outcome::Completed)
    );
    recorded()
        .into_iter()
        .filter(|seen| seen.starts_with("read ") && !seen.contains("0x"))
        .collect()
}

#[test]
fn a_call_binds_each_type_parameter_to_its_solution() {
    // The frame solves the callee's group against the arguments' carried types and binds `Elt` to
    // what it solved, so the body reads the argument's own type.
    let mut substrate = loaded(
        "LET which = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #((Elt) (x)))\n\
         LET n = (which 7)\nLET s = (which \"a\")",
        2,
    );
    assert_eq!(read_types(&mut substrate), ["read Number", "read Str"]);
}

#[test]
fn a_combined_quantified_expression_called_by_name_binds_its_solution() {
    // A combined definition is typed by its function type, so a call through its `LET` name solves
    // the group as a `FN FOR ALL`'s does.
    let mut substrate = loaded(
        "LET which = FN EXPR FOR ALL #[Elt] #(WHICH x :Elt) -> Elt = #((Elt) (x))\n\
         LET n = (which 7)\nLET s = (which \"a\")",
        2,
    );
    assert_eq!(read_types(&mut substrate), ["read Number", "read Str"]);
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
        "LET ab = (FN FOR ALL #[Held Unused] :{x :(LIST OF Held)} -> Any = #(Unused))\n\
         LET ba = (FN FOR ALL #[Unused Held] :{x :(LIST OF Held)} -> Any = #(Unused))\n\
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
fn a_lowercase_name_holds_a_value_a_type_or_code() {
    // An `Any` parameter binds a type argument and a quote without a refusal at bind.
    let mut substrate = loaded(
        "LET f = (FN :{x :Any} -> Any = #(x))\n\
         LET t = (f Number)\nLET q = (f #(1))\nLET u = Number",
        2,
    );
    let read = run_and_read(&mut substrate, &["t", "q", "u"]);
    assert_eq!(read[0], "Number");
    assert!(
        read[1].starts_with("#(") && read[1].contains('1'),
        "{read:?}"
    );
    assert_eq!(read[2], "Number");
}

#[test]
fn a_generic_function_carries_types_and_code() {
    // A `FOR ALL` parameter's default bound is `Any`, so it stands for a type or a quote too.
    let mut substrate = loaded(
        "LET id = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))\n\
         LET t = (id Number)\nLET q = (id #(1))",
        2,
    );
    let read = run_and_read(&mut substrate, &["t", "q"]);
    assert_eq!(read[0], "Number");
    assert!(
        read[1].starts_with("#(") && read[1].contains('1'),
        "{read:?}"
    );
}

#[test]
fn a_capitalized_name_holds_only_a_type() {
    // Whether the refusal comes at load or at run is not this test's business.
    let refused = |source: &str| match CellSubstrate::load::<Mini>(source, "<test>", 2, output()) {
        Err(_) => true,
        Ok(mut substrate) => substrate.with(|running| running.run()) != Ok(Outcome::Completed),
    };
    assert!(!refused("LET Foo = Number"), "a type under a type name");
    assert!(refused("LET Foo = 1"), "a number under a type name");
    assert!(
        refused("LET t = Number\nLET Foo = t"),
        "a type read from a value name under a type name"
    );
}

#[test]
fn a_call_whose_argument_does_not_fit_its_parameter_is_refused() {
    // `Held` is reached only under a list, so a bare number admits nowhere and the walk refuses
    // before the frame binds anything.
    let mut substrate = loaded(
        "LET first = (FN FOR ALL #[Held] :{x :(LIST OF Held)} -> Held = #(x))\nLET r = (first 7)",
        2,
    );
    assert_eq!(
        substrate.with(|running| running.run()),
        Ok(Outcome::Uncaught),
        "an argument its parameter does not admit raises an error value"
    );
    let [error] = &written()[..] else {
        panic!("one error written");
    };
    assert!(
        error.starts_with("error: :(FN FOR ALL #[Held]")
            && error.ends_with("cannot be called with :{x :Number}"),
        "{error}"
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
        "LET id = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))\nLET xs = [1 2]\nLET r = (id xs)",
        2,
    );
    let read = run_and_read(&mut substrate, &["xs", "r"]);
    assert_eq!(address(&read[0]), address(&read[1]), "{read:?}");
}

#[test]
fn a_bounded_type_parameter_refuses_an_argument_outside_its_bound() {
    let mut substrate = loaded(
        "LET num = (FN FOR ALL #{Elt: Number} :{x :Elt} -> Elt = #(x))\nLET r = (num 7)",
        2,
    );
    assert_eq!(run_and_read(&mut substrate, &["r"]), ["7"]);
    for source in [
        "LET num = (FN FOR ALL #{Elt: Number} :{x :Elt} -> Elt = #(x))\nLET r = (num \"a\")",
        // A quote is code, not a value.
        "LET v = (FN FOR ALL #{Elt: Value} :{x :Elt} -> Elt = #(x))\nLET q = (v #(1))",
    ] {
        let mut substrate = loaded(source, 2);
        assert_eq!(
            substrate.with(|running| running.run()),
            Ok(Outcome::Uncaught),
            "`{source}` refuses"
        );
    }
}

#[test]
fn a_type_parameter_canonical_form_dropped_reads_as_its_bound() {
    let mut substrate = loaded(
        "LET which = (FN FOR ALL #{Unused: Value, Held: Any} :{x :(LIST OF Held)} -> Any = #(Unused))\n\
         LET t = (which [1 2])",
        2,
    );
    assert_eq!(run_and_read(&mut substrate, &["t"]), ["Value"]);
}

#[test]
fn a_lambda_reads_a_later_binding_when_it_is_born() {
    let mut substrate = loaded("(FN :{y :Number} -> Number = #(later))\nLET later = 5", 2);
    reset();
    substrate.with(|running| running.run().expect("the program runs"));
    let seen = recorded();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0], "literal 5", "the unit binding `later` runs first");
    assert!(
        seen[1].starts_with("born fn in ") && seen[1].ends_with(" capturing 5"),
        "{seen:?}"
    );
}

#[test]
fn a_lambda_returned_from_a_frame_keeps_its_captures() {
    // Each lambda is born as its frame's last statement, in its calling evaluation's region, and
    // crosses into the root when that evaluation finishes. `constantly`'s region holds only what
    // its lambda reaches, so the lambda pins and the region splices in; `wasteful`'s also holds a
    // long list the lambda never reaches, so the lambda's knot copies, its capture deep-copied, and
    // the region is reclaimed. Each is called after, reading its capture where it now lies.
    let long = vec!["x"; 256].join(" ");
    let source = format!(
        "LET constantly = (FN :{{x :Any}} -> Any = #(FN :{{y :Number}} -> Any = #(x)))\n\
         LET wasteful = (FN :{{x :Any}} -> Any = #((LET big = [{long}]) (FN :{{y :Number}} -> Any = #(x))))\n\
         LET pinned = (constantly [1 2])\nLET copied = (wasteful [3 4])\n\
         LET r = (pinned 0)\nLET s = (copied 0)"
    );
    let mut substrate = loaded(&source, 2);
    reset();
    substrate.with(|running| running.run().expect("the program runs"));
    let seen = recorded();
    // Each birth's knot and captured list, in birth order: `pinned`'s, then `copied`'s.
    let born: Vec<(String, String)> = seen
        .iter()
        .filter_map(|seen| {
            let (knot, capture) = seen
                .strip_prefix("born fn in ")?
                .split_once(" capturing ")?;
            Some((knot.to_string(), address(capture).to_string()))
        })
        .collect();
    assert_eq!(born.len(), 2, "{seen:?}");
    // Where each call's body found the list it reads: `r`'s call, then `s`'s.
    let reads: Vec<&str> = seen
        .iter()
        .filter_map(|seen| seen.strip_prefix("read "))
        .collect();
    assert_eq!(reads.len(), 2, "{seen:?}");
    assert_eq!(
        reads[0], born[0].1,
        "the pinned lambda reads its capture in place"
    );
    assert_ne!(
        reads[1], born[1].1,
        "the copied lambda reads its capture's copy"
    );

    let read = read_back(&mut substrate, &["pinned", "copied", "r", "s"]);
    let knot = |described: &str| {
        described
            .strip_prefix("fn in ")
            .expect("a lambda reads back as a function")
            .to_string()
    };
    assert_eq!(
        knot(&read[0]),
        born[0].0,
        "the pinned lambda's region splices into the root"
    );
    assert_ne!(
        knot(&read[1]),
        born[1].0,
        "the copied lambda's knot is re-tied in the root"
    );
    assert!(read[2].starts_with("[1 2]@"), "{read:?}");
    assert!(read[3].starts_with("[3 4]@"), "{read:?}");
}

#[test]
fn a_lambda_in_a_knot_reads_its_fellow_through_an_edge() {
    let mut substrate = loaded(
        "LET a = [(FN :{y :Number} -> Any = #(a))]\nLET g = (FIRST a)\nLET r = (g 0)",
        2,
    );
    reset();
    substrate.with(|running| running.run().expect("the program runs"));
    assert!(
        !recorded().iter().any(|seen| seen.starts_with("born ")),
        "the tie never asks for the `FN`"
    );
    let read = read_back(&mut substrate, &["a", "g", "r"]);
    let knot = read[0]
        .strip_prefix("node in ")
        .expect("`a` is a data node");
    assert_eq!(read[1], format!("fn in {knot}"), "`g` sits in `a`'s knot");
    assert_eq!(
        read[2], read[0],
        "the call reads `a` through its capture's edge"
    );
}

#[test]
fn a_lambda_part_is_supplied_to_a_tie() {
    let mut substrate = loaded(
        "LET k = 7\nLET a = [(FN :{} -> Number = #(k)) f]\nLET f = (FN :{} -> Any = #(a))",
        2,
    );
    SUPPLIED_WAKES.with(|wakes| wakes.set(0));
    reset();
    substrate.with(|running| running.run().expect("the program runs"));
    assert_eq!(SUPPLIED_WAKES.with(|wakes| wakes.get()), 1);
    let born: Vec<String> = recorded()
        .into_iter()
        .filter(|seen| seen.starts_with("born "))
        .collect();
    assert_eq!(born.len(), 1, "{born:?}");
    assert!(born[0].ends_with(" capturing 7"), "{born:?}");
    let read = read_back(&mut substrate, &["a", "f"]);
    assert!(read[0].starts_with("node in "), "{read:?}");
    assert!(read[1].starts_with("fn in "), "{read:?}");
}

const TWICE: &str = "LET x = 7\nLET twice = (FN :{body :Expression} -> Any = #(EVAL body))";

#[test]
fn eval_runs_code_whose_dollar_name_binds_where_it_is_written() {
    let mut substrate = loaded(&format!("{TWICE}\nLET r = (twice #($x MINUS 1))"), 4);
    assert_eq!(run_and_read(&mut substrate, &["r"]), ["6"]);
}

#[test]
fn a_hole_is_unbound_when_eval_runs_whatever_the_callee_declares() {
    for twice in [
        TWICE.to_string(),
        TWICE.replace("#(EVAL body)", "#((LET x = 3) (EVAL body))"),
    ] {
        let mut substrate = loaded(&format!("{twice}\nLET r = (twice #(x MINUS 1))"), 4);
        reset();
        let outcome = substrate.with(|running| running.run());
        assert_eq!(outcome, Ok(Outcome::Uncaught), "{twice}");
        assert_eq!(
            written(),
            ["error: unbound name 'x'"],
            "the refusal is an error value the top level reports"
        );
    }
}

/// A keyworded hole a use of the code selects from alone refuses the `EVAL` while unfilled; one
/// beside a builtin overload holds no function, so `x MINUS 1` above runs.
#[test]
fn a_required_keyworded_hole_is_unbound_when_eval_runs() {
    let mut substrate = loaded(&format!("{TWICE}\nLET r = (twice #(NOPE 1))"), 4);
    assert_eq!(
        substrate.with(|running| running.run()),
        Ok(Outcome::Uncaught)
    );
    assert_eq!(written(), ["error: unbound key (NOPE _)"]);
}

#[test]
fn a_parameter_needing_a_name_is_offered_it_where_eval_is_written() {
    let mut substrate = loaded(
        "LET twice = (FN :{body :(Expression NEEDING #[it])} -> Any = \
         #((LET it = 5) (EVAL body)))\n\
         LET r = (twice #(\\it MINUS 1))",
        4,
    );
    assert_eq!(run_and_read(&mut substrate, &["r"]), ["4"]);
}

#[test]
fn a_function_built_from_code_carries_its_bindings_as_captures() {
    let mut substrate = loaded(
        "LET make = (FN :{v :Number} -> Any = #(EVAL #(FN :{} -> Number = #($v))))\n\
         LET a = (make 1)\nLET b = (make 1)\nLET c = (make 2)",
        4,
    );
    substrate.with(|running| running.run().expect("the program runs"));
    assert_eq!(compare_back(&mut substrate, "a", "b"), "Ok(true)");
    assert_eq!(compare_back(&mut substrate, "a", "c"), "Ok(false)");
}

#[test]
fn a_quote_reading_its_own_binder_is_a_one_node_knot() {
    let mut substrate = loaded("LET echo = #(PRINT $echo)", 2);
    assert_eq!(
        run_and_read(&mut substrate, &["echo"]),
        ["#(PRINT $echo) in 1 binding [echo=self]"]
    );
    assert_eq!(compare_back(&mut substrate, "echo", "echo"), "Ok(true)");
}

#[test]
fn a_binder_capturing_an_eval_statement_declared_after_it_runs_after_it() {
    // `f` is born once `y` is bound, and `EVAL` waits on no binder declared before it.
    let mut substrate = loaded(
        "LET f = (FN :{n :Number} -> Number = #(y))\n\
         LET y = (EVAL #(7 MINUS 2))\n\
         LET r = (f 0)",
        4,
    );
    assert_eq!(run_and_read(&mut substrate, &["r", "y"]), ["5", "5"]);
}

#[test]
fn a_malformed_quote_loads_and_its_error_is_reported_when_eval_runs_it() {
    let mut substrate = loaded("LET r = (EVAL #((LET x = 1) (LET x = 2) (PRINT x)))", 4);
    assert_eq!(
        substrate.with(|running| running.run()),
        Ok(Outcome::Uncaught)
    );
    let [error] = &written()[..] else {
        panic!("one error written");
    };
    assert!(error.contains("is bound twice"), "{error}");
}

#[test]
fn a_marked_type_name_in_a_signature_the_code_writes_binds_where_the_quote_is_written() {
    let mut substrate = loaded(
        "LET Alias = Number\n\
         LET g = (EVAL #(FN :{v :($Alias)} -> Number = #(v)))\n\
         LET r = (g 4)",
        4,
    );
    assert_eq!(run_and_read(&mut substrate, &["r"]), ["4"]);
}

#[test]
fn a_refused_tie_is_an_error_value_that_ends_the_program_uncaught() {
    let mut substrate = loaded("LET before = 1\nLET a = [b]\nLET b = [a]\nLET after = 2", 2);
    assert_eq!(
        substrate.with(|running| running.run()),
        Ok(Outcome::Uncaught)
    );
    let [error] = &written()[..] else {
        panic!("one error written");
    };
    // The members are named in component order, which is by symbol.
    assert!(
        error == "error: these bindings build values of no finite type: `a` `b`"
            || error == "error: these bindings build values of no finite type: `b` `a`",
        "{error}"
    );
    assert_eq!(
        read_back(&mut substrate, &["before"]),
        ["1"],
        "what the top level bound before the error stays readable"
    );
}

#[test]
fn a_frame_s_value_is_retyped_to_its_declared_return() {
    // The first body's last statement binds nothing, so the frame tails into its evaluation, which
    // owes the contract; the second's is a `LET`, which the frame holds to it as it ends.
    for body in ["#([1 2])", "#((LET xs = [1 2]))"] {
        let mut substrate = loaded(
            &format!("LET f = (FN :{{n :Number}} -> (LIST OF Any) = {body})\nLET r = (f 0)"),
            2,
        );
        run_and_read(&mut substrate, &[]);
        assert_eq!(type_back(&mut substrate, "r"), ":(LIST OF Any)", "{body}");
    }
}

#[test]
fn a_frame_retypes_each_argument_to_its_declared_type() {
    // A declared return of `Any` keeps the value's own type, so what `r` carries is the argument's.
    let mut substrate = loaded(
        "LET f = (FN :{xs :(LIST OF Any)} -> Any = #(xs))\nLET r = (f [1 2])",
        2,
    );
    run_and_read(&mut substrate, &[]);
    assert_eq!(type_back(&mut substrate, "r"), ":(LIST OF Any)");
}

#[test]
fn a_tail_chain_returns_at_the_outermost_contract() {
    // `outer`'s last statement calls `inner`, whose return lies within `outer`'s, so `outer`'s
    // frame tails into `inner`'s, which is checked against its own return and retyped to `outer`'s.
    let mut substrate = loaded(
        "LET inner = (FN :{n :Number} -> (LIST OF Number) = #([1 2]))\n\
         LET outer = (FN :{n :Number} -> (LIST OF Any) = #(inner n))\n\
         LET r = (outer 0)",
        3,
    );
    run_and_read(&mut substrate, &[]);
    assert_eq!(type_back(&mut substrate, "r"), ":(LIST OF Any)");
}

#[test]
fn a_return_that_misses_its_declared_type_is_an_error_value() {
    for body in ["#(n)", "#((LET m = n))"] {
        let mut substrate = loaded(
            &format!("LET f = (FN :{{n :Number}} -> Str = {body})\nLET r = (f 1)"),
            2,
        );
        assert_eq!(
            substrate.with(|running| running.run()),
            Ok(Outcome::Uncaught),
            "{body}"
        );
        assert_eq!(
            written(),
            ["error: :(FN :{n :Number} -> Str) returned Number, which does not satisfy Str"],
            "{body}"
        );
    }
}

/// The most cells live at once while `(count depth)` runs, where `count`'s last statement is a
/// `WHEN` whose branch calls `count` again.
fn peak_counting_down(depth: u32) -> usize {
    let mut substrate = loaded(
        &format!(
            "LET count = (FN :{{n :Number}} -> Number = #(WHEN n THEN (count (n MINUS 1)) ELSE n))\n\
             LET r = (count {depth})"
        ),
        2,
    );
    substrate.with(|running| {
        let work = crate::scheduler::Work {
            step: crate::program::run,
            state: crate::program::KBirth::Program {
                program: running.program(),
            },
        };
        let root = running.root();
        let mut scheduler = running.scheduler();
        scheduler
            .run(work, root, crate::scheduler::Placement::Shares)
            .expect("the program runs");
        scheduler.peak_live_cells()
    })
}

#[test]
fn a_self_call_in_tail_position_holds_its_cells_constant_however_deep() {
    // The frame tails its `WHEN` into the evaluator under its contract, the `WHEN` tails its branch,
    // and the call — whose callee returns `Number`, which the contract asks — tails into the next
    // frame, so no frame waits on the one after it.
    let deep = if cfg!(miri) { 64 } else { 10_000 };
    assert_eq!(peak_counting_down(deep), peak_counting_down(4));
}
