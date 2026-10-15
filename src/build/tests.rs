//! The build module's tests: a specialization's key, and the import boundary.

mod boundary;

use crate::scope::Segment;
use crate::values::ContentDigest;
use crate::values::digest::{DigestHasher, Tag};

use super::key_of;

/// A digest standing in for an argument's content.
fn digest(at: usize) -> ContentDigest {
    DigestHasher::new(Tag::Number).count(at).finished()
}

#[test]
fn a_key_is_equal_for_equal_inputs_and_tells_segments_and_bindings_apart() {
    let key =
        |segment, digests: &[usize]| key_of(Segment(segment), digests.iter().map(|at| digest(*at)));
    assert_eq!(key(3, &[1, 2]), key(3, &[1, 2]));
    assert_ne!(
        key(3, &[1, 2]),
        key(4, &[1, 2]),
        "segments with equal digests differ"
    );
    assert_ne!(
        key(3, &[1, 2]),
        key(3, &[2, 1]),
        "the digests count in slot order"
    );
    assert_ne!(key(3, &[1]), key(3, &[1, 1]));
    assert_ne!(key(3, &[]), key(4, &[]));
}
