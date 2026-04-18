//! Rewriting expression trees into canonical function-call form.
//!
//! Two sketches live here for comparison:
//!   - `match_based`: hand-rolled rules as Rust functions over `ExpressionPart`.
//!   - `egg_based`:   pattern DSL + equality saturation via the `egg` crate
//!                    (gated behind the `egg` feature).

pub mod match_based;
pub mod egg_based;
