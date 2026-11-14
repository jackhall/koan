//! Crate-wide test scaffolding: the property-case share every law module states its depth by.

/// The case count for one property-test module, as a share of the proptest default.
///
/// `ProptestConfig::default().cases` reads `PROPTEST_CASES`, so a verification tier sets that one
/// variable and every module moves with it. A module states its share of the default rather than a
/// literal — `case_share(1, 1)` for the component walk's law — so their relative depths hold at
/// whatever the tier asks for. No module drops below 64 cases: a sweep shallower than that is not
/// worth the build it rides on.
pub(crate) fn case_share(numerator: u32, denominator: u32) -> u32 {
    (proptest::prelude::ProptestConfig::default().cases * numerator / denominator).max(64)
}
