//! Two surface rules a parse enforces on the run as written: a brace value is one part unless
//! parenthesized, and a sigil takes the group after it rather than the token before it.

use super::tree;

#[test]
fn multi_part_value_without_parens_errors() {
    // Dict values are single-token unless parenthesized, mirroring list elements.
    assert!(tree("{a: foo bar}").is_err());
}

#[test]
fn sigil_glued_to_token_errors() {
    assert!(tree("foo#(x)").is_err());
}
