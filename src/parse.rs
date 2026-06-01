//! Turns Koan source text into a sequence of `KExpression`s. Pipeline:
//! `mask_quotes → collapse_whitespace → build_tree`. The `pub use` block below
//! is the entire public surface; submodules are private.
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
mod whitespace;

pub use expression_tree::{parse, parse_with_path, parse_with_source};
pub use triple_list::{parse_keyword_triple_list, parse_pair_list};
