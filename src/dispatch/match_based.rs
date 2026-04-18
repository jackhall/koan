//! Hand-rolled tree rewriter.
//!
//! Each rule is a Rust function that takes the parts of an `Expression`
//! and returns `Ok(new_parts)` if it fires, or `Err(parts)` to give the
//! input back unchanged. `dispatch` walks the tree bottom-up, trying
//! every rule at each node.
//!
//! Strengths: fully typed, no deps, rules are ordinary Rust (debugger,
//! unit tests, etc.).
//! Weaknesses: no pattern DSL — each rule is written by hand; combining
//! rules that interact (rule A enables rule B) is all on you.

use std::collections::HashMap;

use crate::kexpression::{ExpressionPart, KExpression, KLiteral};
use crate::kobject::KObject;

fn empty() -> KExpression {
    KExpression {
        base: KObject { name: String::new(), remaining_args: HashMap::new() },
        parts: Vec::new(),
    }
}

fn token(s: &str) -> ExpressionPart {
    ExpressionPart::Token(s.to_string())
}

fn sub(parts: Vec<ExpressionPart>) -> ExpressionPart {
    let mut e = empty();
    e.parts = parts;
    ExpressionPart::Expression(Box::new(e))
}

/// A rule takes ownership of the parts vector. On a match it returns `Ok`
/// with the rewritten parts; otherwise it returns `Err` with the vector
/// unchanged so the next rule can try.
type Rule = fn(Vec<ExpressionPart>) -> Result<Vec<ExpressionPart>, Vec<ExpressionPart>>;

/// `(a + b)` -> `(add a b)`
fn infix_add(mut parts: Vec<ExpressionPart>) -> Result<Vec<ExpressionPart>, Vec<ExpressionPart>> {
    let is_match = parts.len() == 3
        && matches!(&parts[1], ExpressionPart::Token(op) if op == "+");
    if !is_match {
        return Err(parts);
    }
    let b = parts.pop().unwrap();
    let _op = parts.pop().unwrap();
    let a = parts.pop().unwrap();
    Ok(vec![token("add"), a, b])
}

/// `(for x in coll body)` -> `(map (fn (x) body) coll)`
fn for_to_map(mut parts: Vec<ExpressionPart>) -> Result<Vec<ExpressionPart>, Vec<ExpressionPart>> {
    let is_match = parts.len() == 5
        && matches!(&parts[0], ExpressionPart::Token(t) if t == "for")
        && matches!(&parts[1], ExpressionPart::Token(_))
        && matches!(&parts[2], ExpressionPart::Token(t) if t == "in");
    if !is_match {
        return Err(parts);
    }
    let body = parts.pop().unwrap();
    let coll = parts.pop().unwrap();
    let _in = parts.pop().unwrap();
    let var = parts.pop().unwrap();
    let _for = parts.pop().unwrap();
    Ok(vec![
        token("map"),
        sub(vec![token("fn"), sub(vec![var]), body]),
        coll,
    ])
}

const RULES: &[Rule] = &[infix_add, for_to_map];

fn rewrite_once(parts: Vec<ExpressionPart>) -> Vec<ExpressionPart> {
    let mut current = parts;
    for rule in RULES {
        match rule(current) {
            Ok(rewritten) => return rewritten,
            Err(unchanged) => current = unchanged,
        }
    }
    current
}

/// Walk the tree bottom-up, applying rules at each `Expression` node.
pub fn dispatch(mut e: KExpression) -> KExpression {
    let children_rewritten: Vec<ExpressionPart> = e
        .parts
        .into_iter()
        .map(|p| match p {
            ExpressionPart::Expression(inner) => {
                ExpressionPart::Expression(Box::new(dispatch(*inner)))
            }
            other => other,
        })
        .collect();
    e.parts = rewrite_once(children_rewritten);
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn describe(e: &KExpression) -> String {
        let parts: Vec<String> = e
            .parts
            .iter()
            .map(|p| match p {
                ExpressionPart::Token(s) => s.clone(),
                ExpressionPart::Expression(inner) => format!("({})", describe(inner)),
                ExpressionPart::Literal(KLiteral::Number(n)) => n.to_string(),
                ExpressionPart::Literal(KLiteral::String(s)) => format!("\"{}\"", s),
                ExpressionPart::Literal(KLiteral::Boolean(b)) => b.to_string(),
                ExpressionPart::Literal(KLiteral::Null) => "null".to_string(),
            })
            .collect();
        parts.join(" ")
    }

    // NB: the parser wraps every top-level line in an outer Expression
    // (see `collapse_whitespace`), so test inputs here don't include
    // the outermost parens — collapse adds them.

    #[test]
    fn infix_add_rewrites() {
        let input = crate::parse::parse("2 + 3").unwrap();
        let out = dispatch(input);
        assert_eq!(describe(&out), "(add 2 3)");
    }

    #[test]
    fn for_rewrites_to_map() {
        let input = crate::parse::parse("for x in coll (print x)").unwrap();
        let out = dispatch(input);
        assert_eq!(describe(&out), "(map (fn (x) (print x)) coll)");
    }

    #[test]
    fn nested_rewrite() {
        let input = crate::parse::parse("(1 + 2) + 3").unwrap();
        let out = dispatch(input);
        assert_eq!(describe(&out), "(add (add 1 2) 3)");
    }
}
