//! The memo bits a weight carries: masked out of its bytes and its equality, dropped by a sum.

use crate::values::{MemoBits, Weight};

#[test]
fn a_weights_bits_are_masked_out_of_its_bytes_and_its_equality() {
    let plain = Weight::text(40);
    let marked = plain.with_bits(MemoBits::BOTH);
    assert_eq!(marked.bytes(), 40);
    assert_eq!(marked.bits(), MemoBits::BOTH);
    assert_eq!(marked, plain, "equality reads the bytes alone");
    assert_eq!(marked.cmp(&plain), std::cmp::Ordering::Equal);
    assert_eq!(marked.without_bits().bits(), MemoBits::NONE);
    let settled = MemoBits {
        root_closed: false,
        settled: true,
    };
    assert_eq!(plain.with_bits(settled).bits(), settled);
    assert!(settled.meets() && !MemoBits::NONE.meets());
}

#[test]
fn a_sum_of_weights_carries_no_bits_and_saturates_below_them() {
    let marked = Weight::text(8).with_bits(MemoBits::BOTH);
    let sum = marked.plus(marked);
    assert_eq!(sum.bytes(), 16);
    assert_eq!(sum.bits(), MemoBits::NONE);
    let full = Weight::text(usize::MAX);
    assert_eq!(
        full.bits(),
        MemoBits::NONE,
        "a saturated count reaches no bit"
    );
    assert_eq!(full.plus(full).bits(), MemoBits::NONE);
    assert_eq!(full.plus(Weight::text(1)), full);
    assert_eq!(Weight::run::<u64>(usize::MAX).bits(), MemoBits::NONE);
}
