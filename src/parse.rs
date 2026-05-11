//! Parse — turns Koan source text into a sequence of `KExpression`s. The pipeline runs
//! `mask_quotes → collapse_whitespace → build_tree`; token classification, operator
//! desugaring, dict-pair state, and `<...>` type-param folding live in private submodules.
//!
//! The `pub use` block below is the entire public surface: the [`parse`] entry point, the
//! AST node types (`KExpression`, `ExpressionPart`, `KLiteral`, `TypeExpr`, `TypeParams`),
//! and the shared `<name>: <slot>` triple walker. Submodules are private — all callers go
//! through this surface.
//!
//! See [design/expressions-and-parsing.md](../design/expressions-and-parsing.md).

mod expression_tree;
mod kexpression;
mod dict_literal;
mod operators;
mod quotes;
mod tokens;
mod triple_list;
mod type_frame;
mod whitespace;

pub use expression_tree::parse;
pub use kexpression::{ExpressionPart, KExpression, KLiteral, TypeExpr, TypeParams};
pub use triple_list::parse_triple_list;

#[cfg(test)]
mod expression_tree_tests;
