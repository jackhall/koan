//! The spelling rule an identifier is rejected by. Which class a token classifies into is
//! [`properties`](super::properties)' third law.

use super::tree;

#[test]
fn identifier_token_with_invalid_char_errors() {
    // Identifiers reject everything except letters, digits, and `_`.
    assert!(tree("a+b").is_err());
    assert!(tree("foo@bar").is_err());
}
