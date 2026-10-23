//! The load reads each fact of a keyworded use's candidates once per use, and peeks each member
//! read's operand, and reads whether it was received, once.

use super::load_counts;
use super::trees::nested_signatures;

/// A keyworded use over `n` candidates reads each one's registered shape and solving slots once.
/// Every candidate solves `Elt` from its second slot, so the argument there contributes and each
/// judged candidate's solve is read.
#[test]
fn a_use_reads_each_candidate_once() {
    let n = 12;
    let mut declarations = String::new();
    for at in 1..n {
        declarations += &format!(
            "NEWTYPE Box{at} = :{{v :Number}}\n\
             EXPR FOR ALL #[Elt] #(PROBE x :Box{at} WITH y :Elt) -> Number = #(1)\n"
        );
    }
    declarations += "EXPR FOR ALL #[Elt] #(PROBE x :Number WITH y :Elt) -> Number = #(1)\n";
    let without = load_counts(&format!("{declarations}LET r = 1\n"));
    let with = load_counts(&format!("{declarations}LET r = (PROBE 1 WITH 2)\n"));
    assert_eq!(
        with.candidate_reads - without.candidate_reads,
        n,
        "one per candidate"
    );
    assert_eq!(
        with.solving_reads - without.solving_reads,
        n,
        "one per candidate"
    );
}

/// A chain of `n` member reads peeks each read's operand once.
#[test]
fn a_member_chain_peeks_each_operand_once() {
    let n = 12;
    let chain = |reads: usize| {
        format!(
            "{}LET g = (FN :{{p :Sig{n}}} -> Any = #(p{}))\n",
            nested_signatures(n),
            ".a".repeat(reads)
        )
    };
    let peeks = |reads| load_counts(&chain(reads)).peeks;
    assert_eq!(peeks(n) - peeks(0), n, "one per read");
}

/// A chain of `n` member reads asks once of each read's operand whether the body received it,
/// however many reads past it ask again.
#[test]
fn a_member_chain_reads_each_receipt_once() {
    let n = 12;
    let chain = |reads: usize| {
        format!(
            "{}LET g = (FN :{{p :Sig{n}}} -> Any = #(p{}))\n",
            nested_signatures(n),
            ".a".repeat(reads)
        )
    };
    let receipts = |reads| load_counts(&chain(reads)).receipts;
    assert_eq!(receipts(n) - receipts(0), n, "one per read");
}
