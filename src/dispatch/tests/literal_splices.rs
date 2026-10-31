//! Splicing under a literal: a `$..` splice in a group a list, dict or record literal's element or
//! a template's brace holds composes as one in a paren group does, as does one in a quote a builtin
//! reads as written there; and a marked use holds a splice nowhere under it. See the scope README
//! § Splicing.

use crate::parse::MAX_SYNTAX_DEPTH;

use super::reads::ok;
use super::run;
use super::splicing::{TOO_DEEP, on_its_stack};

const PARTS: &str = "LET parts = [#(1 + 2)]\n";

#[test]
fn a_splice_under_a_literal_composes_as_in_a_paren_group() {
    assert_eq!(
        run(&format!("{PARTS}EVAL #(PRINT [($..parts)]) -> Any")),
        "[3]"
    );
    assert_eq!(
        run(&format!(
            "{PARTS}PRINT (EVAL #(%\"sum {{$..parts}}\") -> Str)"
        )),
        "sum 3"
    );
    assert_eq!(
        run(&format!("{PARTS}PRINT (EVAL #({{a = ($..parts)}}) -> Any)")),
        "{a = 3}"
    );
    assert_eq!(
        run(&format!(
            "{PARTS}PRINT #(PRINT [($..parts)])\nPRINT #(%\"sum {{$..parts}}\")"
        )),
        "PRINT [(1 + 2)]\n%\"sum {1 + 2}\""
    );
}

/// A dict's key and value, a spread's operand, a kept brace and a nested literal's element.
#[test]
fn every_element_of_a_literal_composes() {
    assert_eq!(
        run(&format!(
            "{PARTS}PRINT (EVAL #({{($..parts): ($..parts)}}) -> Any)"
        )),
        "{3: 3}"
    );
    assert_eq!(
        run("LET lists = [#([1 2])]\nPRINT (EVAL #([0 ..($..lists)]) -> Any)"),
        "[0, 1, 2]"
    );
    assert_eq!(
        run(&format!(
            "{PARTS}PRINT #(%\"x %{{$..parts}}\")\nPRINT (EVAL #(%\"x %{{$..parts}}\") -> Any)"
        )),
        "%\"x %{1 + 2}\"\n{strings = [x , ], values = [3]}"
    );
    assert_eq!(
        run(&format!(
            "{PARTS}PRINT #([[($..parts)]])\nPRINT (EVAL #([[($..parts)]]) -> Any)"
        )),
        "[[(1 + 2)]]\n[[3]]"
    );
}

/// The group's own parts set its level, as in a paren group: alone in it, the splice is at
/// statement level, so its statements lay as a block, which a list's element runs as a paren
/// group runs it.
#[test]
fn a_splice_under_a_literal_takes_its_level_from_its_parts() {
    let stmts = "LET stmts = [#(LET x = 4), #(x)]\n";
    assert_eq!(
        run(&format!(
            "{stmts}PRINT #([($..stmts)])\nPRINT #(PRINT ($..stmts))"
        )),
        "[((LET x = 4) (x))]\nPRINT ((LET x = 4) (x))"
    );
    assert_eq!(
        run(&format!("{stmts}EVAL #([($..stmts)]) -> Any")),
        run(&format!("{stmts}EVAL #(PRINT ($..stmts)) -> Any")),
    );
    assert_eq!(
        run("LET args = [#(a + b), #(c)]\nPRINT #([(f $..args)])"),
        "[(f (a + b) c)]"
    );
}

#[test]
fn a_splice_under_a_literal_keeps_each_elements_bindings() {
    let parts = "LET mk = (FN :{i :Number} -> Any = #(#($i + 1)))\n\
                 LET parts = [(mk {i = 10})]\nLET i = 1\n";
    assert_eq!(
        run(&format!("{parts}EVAL #(PRINT [$i ($..parts)]) -> Any")),
        "[1, 11]"
    );
    assert_eq!(
        run(&format!(
            "{parts}PRINT (EVAL #(%\"{{$i}} and {{$..parts}}\") -> Str)"
        )),
        "1 and 11"
    );
}

#[test]
fn a_composed_literal_reads_and_compares_as_its_written_text() {
    let composed = "#(PRINT [($..parts)])";
    assert_eq!(
        run(&format!("{PARTS}PRINT ({composed} == #(PRINT [(1 + 2)]))")),
        "true"
    );
    assert_eq!(run(&format!("{PARTS}PRINT (LENGTH {composed})")), "2");
    assert_eq!(
        run(&format!("{PARTS}PRINT ({composed} 1)")),
        ok(":(UNBUILT Expression)", "[(1 + 2)]")
    );
}

/// A chain of quotes `a1` to `a{n}`, each splicing the last into a group a list holds. `a{i}`'s
/// root is a node of one list, whose one element is `a{i-1}`'s root: the group's splice is at
/// statement level, so the group is that root. Each link adds the list and its root node, so
/// `a{n}` nests `2n + 1` deep.
fn nested_in_lists(n: usize) -> String {
    let mut source = String::from("LET l0 = [#(1)]\n");
    for i in 1..=n {
        source += &format!("LET a{i} = #([($..l{})])\nLET l{i} = [a{i}]\n", i - 1);
    }
    source + "PRINT 9"
}

#[test]
fn composing_through_literals_past_the_depth_limit_is_an_error_value() {
    on_its_stack(|| {
        let most = (MAX_SYNTAX_DEPTH - 1) / 2;
        assert_eq!(run(&nested_in_lists(most)), "9");
        assert_eq!(run(&nested_in_lists(most + 1)), TOO_DEEP);
    });
}

/// A `MATCH` or `TRY` arm in `#{…}` is read as written, so a splice in it composes into the quote
/// around it. Neither evaluates yet, so the code is printed, not run.
#[test]
fn a_splice_in_a_written_quote_of_a_container_literal_composes_into_the_quote_around_it() {
    let ys = "LET ys = [#(PRINT \"got\")]\n";
    assert_eq!(
        run(&format!(
            "{ys}PRINT #(MATCH (42) -> :Str WITH #{{Number: ($..ys)}})"
        )),
        "MATCH (42) -> Str WITH {#(Number): #(PRINT \"got\")}"
    );
    assert_eq!(
        run(&format!(
            "{ys}PRINT #(TRY (42) -> :Str WITH #{{Ok: ($..ys)}})"
        )),
        "TRY (42) -> Str WITH {#(Ok): #(PRINT \"got\")}"
    );
    assert_eq!(
        run("LET xs = [1]\nLET q = #(MATCH (42) -> :Str WITH #{Number: ($..xs)})"),
        "load: <test>:2:46: `$..` splices a list of code, and this value is :(LIST OF Number), \
         which can never be one"
    );
}

/// A node holding a splice reads every quote part as written, so a quote a literal holds beside
/// one composes with the quote around it, and its `$` names bind where that quote is written.
#[test]
fn a_quote_in_a_literal_beside_a_splice_composes_with_the_quote_around_it() {
    assert_eq!(
        run("LET a = [#(1)]\nLET b = [#(2)]\nPRINT #(f $..a [#($..b)])"),
        "f 1 [#(2)]"
    );
    let hidden = "LET args = [#(1)]\nLET q = #((LET y = 2) (f $..args [#(PRINT $y)]))";
    assert_eq!(
        run(hidden),
        "load: <test>:2:43: `y` names no binding visible here"
    );
    assert_eq!(
        run(&format!("LET y = 2\n{hidden}\nPRINT \"loaded\"")),
        "loaded"
    );
}

#[test]
fn a_splice_under_a_marked_use_is_refused() {
    let greet = "EXPR #(GREET x :Any) -> Str = #(\"hi\")\nLET xs = [#(1)]\n";
    for under in ["($..xs)", "[($..xs)]"] {
        assert_eq!(
            run(&format!("{greet}PRINT #($(GREET {under}))")),
            "load: parse error at <test>:3:9: a marked use wraps one keyworded use, whose key a \
             splice would leave unknown"
        );
    }
}
