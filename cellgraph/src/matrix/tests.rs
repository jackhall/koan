//! The holder tally against the scan it stands in for.
//!
//! Every write that can touch a bit is scripted here, and after each one the count the matrix
//! carries is compared against a fresh scan down the column. A write that forgets to count shows
//! up as a divergence at the step that made it.

use super::*;

/// The count a scan down `held`'s column reports, which is what [`Matrix::holders`] must equal
/// after every write.
fn scanned(matrix: &Matrix, cap: u32, held: u32) -> u32 {
    (0..cap).filter(|holder| matrix.test(*holder, held)).count() as u32
}

/// Compare the tally against the scan for every column at once.
fn agrees(matrix: &Matrix, cap: u32) {
    for held in 0..cap {
        assert_eq!(
            matrix.holders(held),
            scanned(matrix, cap, held),
            "column {held} disagrees with its tally"
        );
    }
}

#[test]
fn the_tally_follows_every_write() {
    let cap = 6;
    let mut matrix = Matrix::new(cap);
    agrees(&matrix, cap);

    // A set counts once, and a set that changes nothing counts nothing.
    matrix.set(0, 3);
    matrix.set(1, 3);
    matrix.set(1, 3);
    assert_eq!(matrix.holders(3), 2);
    agrees(&matrix, cap);

    // Inheriting a row counts only the bits the union newly sets: row 2 already names 3, so the
    // parent's naming of 3 adds nothing, while its naming of 4 adds one.
    matrix.set(2, 3);
    matrix.set(0, 4);
    matrix.inherit_row(2, 0);
    assert_eq!(matrix.holders(3), 3);
    assert_eq!(matrix.holders(4), 2);
    agrees(&matrix, cap);

    // A mint folds a reach mask in the same way, and drops the destination's own bit rather than
    // counting it: a cell that held itself would never reach a zero count.
    let mut reach = Mask::empty(cap);
    reach.add(4);
    reach.add(5);
    matrix.mint(5, &reach);
    assert!(!matrix.test(5, 5));
    assert_eq!(matrix.holders(4), 3);
    assert_eq!(matrix.holders(5), 0);
    agrees(&matrix, cap);

    // A clear counts down once, and a clear of a bit already gone counts nothing.
    matrix.clear(1, 3);
    matrix.clear(1, 3);
    assert_eq!(matrix.holders(3), 2);
    agrees(&matrix, cap);

    // A whole-row release drops every name the row made.
    matrix.clear_row(2);
    assert_eq!(matrix.holders(3), 1);
    assert_eq!(matrix.holders(4), 2);
    agrees(&matrix, cap);

    // And clearing the rest leaves every column at zero, which is the state a recycled slot is
    // handed back in.
    matrix.clear_row(0);
    matrix.clear_row(1);
    matrix.clear_row(5);
    for held in 0..cap {
        assert_eq!(matrix.holders(held), 0);
    }
}
