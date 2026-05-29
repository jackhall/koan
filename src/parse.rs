//! Parse — turns Koan source text into a sequence of `KExpression`s. The pipeline runs
//! `mask_quotes → collapse_whitespace → build_tree`; token classification, operator
//! desugaring, dict-pair state, and `<...>` type-param folding live in private submodules.
//!
//! The `pub use` block below is the entire public surface: the [`parse`] entry point and
//! the shared `<name>: <slot>` triple walker. The AST node types live in [`crate::machine::model::ast`].
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
mod whitespace;

pub use expression_tree::{parse, parse_with_path, parse_with_source};
pub use triple_list::{parse_keyword_triple_list, parse_pair_list};
