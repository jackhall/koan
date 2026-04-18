//! Egg-based rewriter sketch.
//!
//! Gated behind the `egg` feature so the default build doesn't need the dep.
//! To try it:
//!   1. Add to Cargo.toml:
//!         [features]
//!         egg = ["dep:egg"]
//!         [dependencies]
//!         egg = { version = "0.9", optional = true }
//!   2. cargo test --features egg
//!
//! Approach: define Koan as an egg `Language` (the IR egg rewrites over),
//! write rules in egg's pattern DSL, run equality saturation, extract the
//! lowest-cost form.
//!
//! Strengths: rules are one-liners in a real pattern language; equality
//! saturation applies *all* rules exhaustively and picks the best form by
//! a cost function — useful later for optimization passes.
//! Weaknesses: need a conversion layer between `KExpression` and
//! `RecExpr<Koan>`; adds a non-trivial dependency; debugging a rule that
//! doesn't fire means reading about e-graphs.

#![cfg(feature = "egg")]

use egg::{define_language, rewrite, AstSize, Extractor, Id, RecExpr, Rewrite, Runner, Symbol};

define_language! {
    pub enum Koan {
        Num(i64),
        "+"   = Plus([Id; 2]),
        "add" = Add([Id; 2]),
        "for" = For([Id; 3]),   // (for <var> <coll> <body>)
        "in"  = In,              // syntactic marker; optional depending on parser
        "map" = Map([Id; 2]),
        "fn"  = Fn([Id; 2]),
        Sym(Symbol),
    }
}

fn rules() -> Vec<Rewrite<Koan, ()>> {
    vec![
        rewrite!("infix-add";
            "(+ ?a ?b)" => "(add ?a ?b)"),
        rewrite!("for-to-map";
            "(for ?x ?coll ?body)" => "(map (fn ?x ?body) ?coll)"),
    ]
}

/// Take an s-expression string in Koan's prefix form, saturate with the
/// rules above, extract the smallest equivalent tree.
///
/// A real integration would translate `KExpression` <-> `RecExpr<Koan>`
/// instead of going through strings.
pub fn rewrite_str(input: &str) -> String {
    let expr: RecExpr<Koan> = input.parse().expect("parse");
    let runner = Runner::default().with_expr(&expr).run(&rules());
    let extractor = Extractor::new(&runner.egraph, AstSize);
    let (_cost, best) = extractor.find_best(runner.roots[0]);
    best.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infix_add_rewrites() {
        assert_eq!(rewrite_str("(+ 2 3)"), "(add 2 3)");
    }

    #[test]
    fn for_rewrites_to_map() {
        assert_eq!(
            rewrite_str("(for x coll (print x))"),
            "(map (fn x (print x)) coll)"
        );
    }
}
