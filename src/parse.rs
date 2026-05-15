//! Parse — turns Koan source text into a sequence of `KExpression`s. The pipeline runs
//! `mask_quotes → collapse_whitespace → build_tree`; token classification, operator
//! desugaring, dict-pair state, and `<...>` type-param folding live in private submodules.
//!
//! The `pub use` block below is the entire public surface: the [`parse`] entry point and
//! the shared `<name>: <slot>` triple walker. The AST node types live in [`crate::runtime::machine::model::ast`].
//! Submodules are private — all callers go through this surface.
//!
//! See [design/expressions-and-parsing.md](../design/expressions-and-parsing.md).

mod dict_literal;
mod expression_tree;
mod frame;
mod operators;
mod parse_stack;
mod quotes;
mod tokens;
mod triple_list;
mod type_expr_frame;
mod whitespace;

pub use expression_tree::parse;
pub use triple_list::{parse_keyword_triple_list, parse_pair_list};

#[cfg(test)]
mod expression_tree_tests;
