//! The positional half of the discipline: a `FoldingBrand` cannot be minted outside a fold
//! combinator. `in_fold_closure` is its sole constructor and is crate-private, so an external
//! caller can name the brand type but cannot construct one — there is no `FoldToken` to pass and
//! the constructor is unreachable.

use koan::fold_fixture::FoldingBrand;

fn main() {
    let _forge = FoldingBrand::in_fold_closure;
}
