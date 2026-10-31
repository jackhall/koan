//! Splicing: a quote holding `$..xs` composed when it is born, at statement level or part level,
//! each element keeping its own bindings; what the load refuses of a splice and what its birth
//! answers as an error value; and the depth limit composed code is held to.

use std::cell::Cell;

use crate::parse::MAX_SYNTAX_DEPTH;
use crate::program::CellSubstrate;
use crate::scheduler::REGION_WRITTEN;
use crate::values::BuildBudget;

use super::super::Koan;
use super::caching::program;
use super::{WRITTEN, output, run};

#[test]
fn a_spliced_block_runs_its_statements() {
    assert_eq!(
        run("LET stmts = [#(LET x = 4), #(PRINT x)]\nEVAL #($..stmts) -> Any"),
        "4"
    );
}

#[test]
fn two_quotes_binding_one_name_keep_their_bindings() {
    let source = "LET mk = (FN :{i :Number} -> Any = #(#(PRINT $i)))\n\
                  LET both = [(mk {i = 1}), (mk {i = 2})]\n\
                  LET q = #($..both)\nPRINT q\nEVAL q -> Any";
    assert_eq!(run(source), "(PRINT $i) (PRINT $i)\n1\n2");
}

#[test]
fn a_quote_spliced_alone_is_its_written_text() {
    let source = "LET k = 3\nLET q = #(PRINT $k)\nLET xs = [q]\nPRINT (#($..xs) == q)";
    assert_eq!(run(source), "true");
}

/// A splice reads its list where the quote is born, not where the code runs.
#[test]
fn a_splice_reads_its_list_where_the_quote_is_born() {
    let source = "LET xs = [#(PRINT 1)]\nLET q = #($..xs)\n\
                  LET f = (FN :{xs :Any} -> Any = #(EVAL q -> Any))\nf {xs = [#(PRINT 2)]}";
    assert_eq!(run(source), "1");
}

#[test]
fn a_dollar_spread_spreads_when_the_code_runs() {
    let source = "LET xs = [1 2]\nLET q = #([0 ..$xs])\nPRINT q\nPRINT (EVAL q -> Any)";
    assert_eq!(run(source), "[0 ..$xs]\n[0, 1, 2]");
}

/// Beside parts that are not groups, each element is one part if it is one, and a group if not.
#[test]
fn a_part_level_splice_groups_each_element_of_more_than_one_part() {
    let source = "LET args = [#(a + b), #(c)]\nPRINT #(f $..args)\n\
                  PRINT (#(f $..args) == #(f (a + b) c))";
    assert_eq!(run(source), "f (a + b) c\ntrue");
}

#[test]
fn a_block_splices_the_same_statements_on_one_line_or_several() {
    let source = "LET stmts = [#(PRINT 2)]\nLET one = #((PRINT 1) $..stmts (PRINT 3))\n\
                  LET many = #(\n  PRINT 1\n  $..stmts\n  PRINT 3\n)\n\
                  PRINT one\nPRINT (one == many)\nEVAL many -> Any";
    assert_eq!(run(source), "(PRINT 1) (PRINT 2) (PRINT 3)\ntrue\n1\n2\n3");
}

#[test]
fn an_empty_splice_adds_no_statement() {
    let source = "LET none = []\nLET t = #((PRINT 1) $..none)\nPRINT t\nEVAL t -> Any";
    assert_eq!(run(source), "PRINT 1\n1");
}

#[test]
fn a_loop_composing_code_from_composed_code_runs_each_hop() {
    let source = "LET step = (FN :{acc :Any, i :Number} -> Any = \
                  #((LET more = [#(PRINT $i)]) (#($..acc $..more))))\n\
                  LET a = #((PRINT 0))\nLET b = (step {acc = [a], i = 1})\n\
                  LET c = (step {acc = [b], i = 2})\nEVAL c -> Any\n\
                  LET nest = #((LET ys = [#(PRINT 9)]) (EVAL #($..ys) -> Any))\n\
                  LET nests = [nest]\nEVAL #($..nests) -> Any";
    assert_eq!(run(source), "0\n1\n2\n9");
}

/// R13 (c): a `$` name in a quote part beside a splice binds where the quote is written, so it
/// keeps its binding when the splice makes that part a builtin's body.
#[test]
fn a_splice_making_a_quote_part_a_builtins_body_binds_where_written() {
    let source = "LET y = 5\nLET head = [#(FN), #(:{a :Number}), #(->), #(Any), #(=)]\n\
                  LET q = #($..head #(PRINT ($y + a)))\nLET f = (EVAL q -> Any)\nf {a = 1}";
    assert_eq!(run(source), "6");
}

/// An element's own inner quote is no part beside the splice: made a builtin's body, it belongs to
/// the composed root, which binds none of its `$` names.
#[test]
fn an_elements_inner_quote_made_a_builtins_body_is_unbound() {
    let source = "LET x = 7\nLET qs = [#(#(PRINT $x))]\n\
                  LET g = #(FN :{a :Number} -> Any = $..qs)\nPRINT g\nEVAL g -> Any";
    assert_eq!(
        run(source),
        "FN :{a Number} -> Any = #(PRINT $x)\nerror: unbound name 'x'"
    );
}

/// R13 (d): a `$` name a splicing node's quote part holds, which nothing binds where the quote is
/// written, refuses the load; bound beside the quote, it loads.
#[test]
fn a_dollar_name_a_splicing_node_hides_is_refused_where_unbound() {
    let hidden = "LET args = [#(1)]\nLET q = #((LET y = 2) (f $..args #(PRINT $y)))";
    assert_eq!(
        run(hidden),
        "load: <test>:2:42: `y` names no binding visible here"
    );
    assert_eq!(
        run(&format!("LET y = 2\n{hidden}\nPRINT \"loaded\"")),
        "loaded"
    );
}

#[test]
fn the_load_refuses_a_splice_outside_a_quote_or_of_no_code() {
    assert_eq!(
        run("LET xs = [1]\nPRINT $..xs"),
        "load: <test>:2:7: a `$` or `\\` mark belongs to a quote value, and no quote value holds \
         this one"
    );
    assert_eq!(
        run("LET xs = [1 2]\nLET q = #(PRINT $..xs)"),
        "load: <test>:2:17: `$..` splices a list of code, and this value is :(LIST OF Number), \
         which can never be one"
    );
    // Under a literal's element or a template's brace, as in a paren group.
    assert_eq!(
        run("LET xs = [1 2]\nLET q = #(PRINT [($..xs)])"),
        "load: <test>:2:19: `$..` splices a list of code, and this value is :(LIST OF Number), \
         which can never be one"
    );
    assert_eq!(
        run("LET xs = [1 2]\nLET q = #(%\"{$..xs}\")"),
        "load: <test>:2:14: `$..` splices a list of code, and this value is :(LIST OF Number), \
         which can never be one"
    );
}

/// R8: a quote holding a splice is composed when it is born, so it cannot wait on its binder.
#[test]
fn a_splice_naming_a_fellow_member_is_refused() {
    assert_eq!(
        run("LET xs = [#(1)]\nLET q = #(PRINT $q $..xs)"),
        "load: <test>:2:1: a quote holding a splice is composed when it is born, so it cannot \
         name a fellow member of the knot its binder ties"
    );
}

#[test]
fn a_splice_of_no_list_or_of_no_code_is_an_error_value() {
    for splice in ["$..x", "[($..x)]"] {
        let splicer = format!("LET f = (FN :{{x :Any}} -> Any = #(#(PRINT {splice})))\n");
        assert_eq!(
            run(&format!("{splicer}PRINT (f {{x = 5}})")),
            "error: `$..` splices a list of code, and Number is none"
        );
        assert_eq!(
            run(&format!("{splicer}PRINT (f {{x = [1]}})")),
            "error: a spliced element is no code: Number"
        );
    }
}

#[test]
fn composed_code_is_typed_by_the_walk_before_it_runs() {
    let source = "LET regs = [#(OP #(+) OVER Number = #(0)), #(1 + 2)]\n\
                  PRINT \"loaded\"\nEVAL #($..regs) -> Any";
    assert_eq!(
        run(source),
        "loaded\nerror: <test>:1:14: this overload of `_ + _` takes operands the builtin \
         :(EXPR #(_ :Number + _ :Number) -> Number) already takes"
    );
}

/// A chain of quotes `a1` to `a{n}`, each splicing the last into a part beside `g`, so `a{k}`
/// nests `k` deep.
fn nested(n: usize) -> String {
    let mut source = String::from("LET l0 = [#(1)]\n");
    for i in 1..=n {
        source += &format!("LET a{i} = #(g $..l{})\nLET l{i} = [a{i}]\n", i - 1);
    }
    source + "PRINT 9"
}

/// `check` run on the stack the interpreter sizes for syntax at the depth limit.
pub(super) fn on_its_stack(check: fn()) {
    std::thread::Builder::new()
        .stack_size(crate::program::STACK_BYTES)
        .spawn(check)
        .expect("spawn")
        .join()
        .expect("runs within the stack");
}

pub(super) const TOO_DEEP: &str =
    "error: composed code nests deeper than the syntax depth limit of 1024";

#[test]
fn composing_past_the_depth_limit_is_an_error_value() {
    on_its_stack(|| {
        assert_eq!(run(&nested(MAX_SYNTAX_DEPTH)), "9");
        assert_eq!(run(&nested(MAX_SYNTAX_DEPTH + 1)), TOO_DEEP);
    });
}

/// One flat node whose operator run alone passes the limit: `0 + 1 + 1 …`, the run's rewrite
/// nesting three levels past its operators.
#[test]
fn an_operator_run_spliced_past_the_depth_limit_is_an_error_value() {
    on_its_stack(|| {
        let program = |n| {
            let ops = vec!["#(+), #(1)"; n].join(", ");
            format!("LET ops = [{ops}]\nLET q = #(0 $..ops)\nPRINT (LENGTH q)")
        };
        let most = MAX_SYNTAX_DEPTH - 3;
        assert_eq!(run(&program(most)), (2 * most + 1).to_string());
        assert_eq!(run(&program(most + 1)), TOO_DEEP);
    });
}

/// What a run of `source` printed, and the bytes its steps wrote to their regions.
fn written(source: &str) -> (String, usize) {
    WRITTEN.with(|written| written.borrow_mut().clear());
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 4, BuildBudget::default(), output())
            .unwrap_or_else(|error| panic!("the program loads: {error}"));
    REGION_WRITTEN.with(|written| written.set(0));
    substrate
        .with(|running| running.run())
        .expect("the program runs");
    let printed = WRITTEN.with(|written| written.borrow().join("\n"));
    (printed, REGION_WRITTEN.with(Cell::get))
}

/// R14: each hop splices the code so far beside one statement more, never building it, so each
/// hop lays a view's few new tree nodes, not the code's syntax again.
#[test]
fn a_loop_appending_a_statement_per_hop_lays_n_log_n_bytes() {
    let grown = |hops| {
        let (printed, bytes) = written(&program(
            hops,
            "EXPR #(GROW acc :Any FROM link :Link) -> Any = #((LET accs = [acc]) \
             (LET more = [#(PRINT 1)]) (GROW #($..accs $..more) FROM (ATTR link next)))\n\
             EXPR #(GROW acc :Any FROM end :Null) -> Any = #(acc)\n\
             PRINT (LENGTH (GROW #((PRINT 0) (PRINT 0)) FROM chain))",
        ));
        assert_eq!(printed, (hops + 2).to_string());
        bytes
    };
    let [at8, at16, at32] = [8, 16, 32].map(grown);
    let (first, second) = (at16 - at8, at32 - at16);
    assert!(
        second <= 3 * first,
        "region bytes grow faster than n log n: {at8} at 8, {at16} at 16, {at32} at 32"
    );
}

/// Spliced code is carried unbuilt: a parameter needing names admits it only once `BUILD` has
/// given it its exact type.
#[test]
fn a_needing_parameter_admits_spliced_code_only_once_built() {
    let source = "LET stmts = [#(LET x = 4), #(PRINT (x + \\y))]\nLET s = #($..stmts)\n\
                  LET take = (FN :{c :(Block NEEDING #[y])} -> Any = #((LET y = 2) (EVAL c -> Any)))\n\
                  take {c = (BUILD s)}\ntake {c = s}";
    assert_eq!(
        run(source),
        "6\nerror: :(FN :{c ::(Block NEEDING #[y])} -> Any) cannot be called with \
         :{c ::(UNBUILT Block)}"
    );
}

#[test]
fn build_of_built_code_is_itself() {
    let source = "LET q = #(PRINT 1)\nLET xs = [q]\nLET b = (BUILD #($..xs))\n\
                  PRINT ((BUILD q) == q)\nPRINT ((BUILD b) == b)";
    assert_eq!(run(source), "true\ntrue");
}

/// Code that does not type builds, and the `EVAL` that runs it reports why, as the load would.
#[test]
fn build_never_refuses() {
    let source = "LET bad = [#(LET x = 1), #(LET x = 2)]\nLET b = (BUILD #($..bad))\n\
                  PRINT \"built\"\nEVAL b -> Any";
    assert_eq!(
        run(source),
        "built\nerror: <test>:1:27: `x` is bound twice; first at <test>:1:13"
    );
}

/// An `EVAL` offers the `\` names its operand's type needs, and unbuilt code needs none.
#[test]
fn an_eval_of_unbuilt_code_offers_no_backslash_name() {
    let source = "LET w = 5\nLET xs = [#(PRINT \\w)]\nEVAL #($..xs) -> Any";
    assert_eq!(run(source), "error: unbound name 'w'");
}

/// R5: `USING` marks each hole it fills, so the fill goes wherever the code goes and no binder
/// composed beside it rebinds it.
#[test]
fn a_filled_hole_keeps_its_fill_when_spliced() {
    let source = "LET fs = [(#(PRINT x) USING {x = 1})]\nEVAL #((LET x = 2) $..fs) -> Any";
    assert_eq!(run(source), "1");
}

#[test]
fn a_filled_hole_is_never_rebound() {
    assert_eq!(
        run("EVAL ((#(PRINT x) USING {x = 1}) USING {x = 2}) -> Any"),
        "1"
    );
    assert_eq!(
        run("EVAL ((#(PRINT (x + y)) USING {x = 1}) USING {y = 2}) -> Any"),
        "3",
        "holes are filled in stages"
    );
}

/// A hole read in a body nested in the code is filled where it is read: in a callable, a filled
/// mark; in a quote value, one pinned to the root's fill.
#[test]
fn a_hole_read_in_a_nested_body_is_filled_there() {
    let source = "LET h = #(FN :{a :Number} -> Number = #(a + x))\n\
                  LET k = (EVAL (h USING {x = 100}) -> Any)\nPRINT (k {a = 1})\n\
                  LET n = #(#(PRINT $x))\nLET m = (EVAL (n USING {x = 7}) -> Any)\nEVAL m -> Any";
    assert_eq!(run(source), "101\n7");
}

#[test]
fn a_filled_mark_prints_bare_and_compares_by_its_fill() {
    let source = "LET q = #(PRINT (x + 1))\nPRINT (q USING {x = 4})\n\
                  PRINT ((q USING {x = 4}) == (q USING {x = 4}))\n\
                  PRINT ((q USING {x = 4}) == (q USING {x = 5}))";
    assert_eq!(run(source), "PRINT (x + 1)\ntrue\nfalse");
}

/// `USING` has its code's shape in hand, so what it returns is built, whatever it fills.
#[test]
fn using_returns_built_code() {
    let source = "LET stmts = [#(LET x = 4), #(PRINT (x + \\y))]\nLET s = #($..stmts)\n\
                  LET take = (FN :{c :(Block NEEDING #[y])} -> Any = #((LET y = 2) (EVAL c -> Any)))\n\
                  take {c = (s USING {z = 1})}";
    assert_eq!(run(source), "6");
}

/// A filled use stands where its use stood, held by a node of its own, so a block's statements
/// stay groups.
#[test]
fn a_filled_use_keeps_a_block_a_block() {
    let source = "MODULE greet = ((EXPR #(GREET x :Str) -> Str = #(\"hi\")) \
                  (EXPR #(WAVE x :Str) -> Str = #(\"bye\")))\n\
                  LET b = (#((PRINT (GREET \"a\")) (PRINT (WAVE \"b\"))) USING greet)\n\
                  LET blocks = (FN :{c :Block} -> Any = #(EVAL c -> Any))\n\
                  PRINT (LENGTH b)\nblocks {c = b}";
    assert_eq!(run(source), "2\nhi\nbye");
}

/// Every operand of one operator run resolves alike: a name hole fills each operand naming it, and
/// a key hole fills the run.
#[test]
fn using_fills_an_operator_run_as_a_whole() {
    assert_eq!(run("EVAL (#(PRINT (x + x + 1)) USING {x = 2}) -> Any"), "5");
    let joined = "MODULE m = (OP #(+) OVER Str = #(\"joined\"))\n\
                  LET filled = (#(PRINT (\"a\" + \"b\" + \"c\")) USING m)\n\
                  PRINT filled\nEVAL filled -> Any";
    assert_eq!(run(joined), "PRINT (\"a\" + \"b\" + \"c\")\njoined");
}

/// A chain of keyworded uses `GREET (GREET (… "a"))` nesting `n` deep, its key filled at each.
fn greeted(n: usize) -> String {
    let mut source =
        String::from("MODULE m = (EXPR #(GREET x :Any) -> Str = #(\"hi\"))\nLET l0 = [#(\"a\")]\n");
    for i in 1..=n {
        source += &format!("LET a{i} = #(GREET $..l{})\nLET l{i} = [a{i}]\n", i - 1);
    }
    source + &format!("PRINT (EVAL (a{n} USING m) -> Any)")
}

#[test]
fn a_filled_use_counts_its_holder_level() {
    on_its_stack(|| {
        // Each use is held a level deeper, so a chain half the limit deep fills to the limit.
        let half = MAX_SYNTAX_DEPTH / 2;
        assert_eq!(run(&greeted(half)), "hi");
        assert_eq!(run(&greeted(half + 1)), TOO_DEEP);
    });
}
