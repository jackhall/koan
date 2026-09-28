//! The depth a node stores, and the parse's refusal of syntax nested past
//! [`MAX_SYNTAX_DEPTH`].

use crate::memory::program_storage;
use crate::parse::{MAX_SYNTAX_DEPTH, parse};
use crate::symbols::SymbolInterner;

/// The stored depth of each top-level expression `input` parses to, or the parse error.
fn depths(input: &str) -> Result<Vec<usize>, String> {
    let program = program_storage();
    let symbols = SymbolInterner::new();
    parse(program.brand(), &symbols, input)
        .map(|expressions| expressions.iter().map(|e| e.depth()).collect())
        .map_err(|e| e.to_string())
}

#[test]
fn a_node_is_one_deeper_than_its_deepest_part() {
    assert_eq!(depths("PRINT 1"), Ok(vec![1]));
    assert_eq!(depths("PRINT ((1))"), Ok(vec![3]));
    assert_eq!(depths("PRINT [[1]]"), Ok(vec![3]));
    assert_eq!(depths("PRINT r.a.a"), Ok(vec![3]));
    assert_eq!(depths("PRINT #((1))"), Ok(vec![3]));
    assert_eq!(depths("PRINT {1: [2]}"), Ok(vec![3]));
    assert_eq!(depths("PRINT {a = [2]}"), Ok(vec![3]));
}

#[test]
fn an_operator_run_counts_as_the_nesting_its_rewrite_builds() {
    // One level for `PRINT`, and two operators plus three for the run.
    assert_eq!(depths("PRINT (1 + 2 + 3)"), Ok(vec![6]));
    // A lone `!=` becomes `NOT (a == b)`.
    assert_eq!(depths("a != b"), Ok(vec![2]));
    assert_eq!(depths("a == b"), Ok(vec![1]));
}

#[test]
fn a_dotted_chain_past_the_limit_is_refused() {
    let chain = |links: usize| format!("PRINT r{}", ".a".repeat(links));
    assert_eq!(
        depths(&chain(MAX_SYNTAX_DEPTH - 1)),
        Ok(vec![MAX_SYNTAX_DEPTH])
    );
    let error = depths(&chain(MAX_SYNTAX_DEPTH)).expect_err("too deep");
    assert!(error.contains(&MAX_SYNTAX_DEPTH.to_string()), "{error}");
}

#[test]
fn an_operator_run_past_the_limit_is_refused() {
    // `PRINT` and the run's three levels leave room for `MAX_SYNTAX_DEPTH - 4` operators.
    let run = |operators: usize| format!("PRINT (1{})", " + 1".repeat(operators));
    assert_eq!(
        depths(&run(MAX_SYNTAX_DEPTH - 4)),
        Ok(vec![MAX_SYNTAX_DEPTH])
    );
    let error = depths(&run(MAX_SYNTAX_DEPTH - 3)).expect_err("too deep");
    assert!(error.contains(&MAX_SYNTAX_DEPTH.to_string()), "{error}");
}
