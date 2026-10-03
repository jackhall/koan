//! The spelling laws: a call is one call however it is spelled. Each drawn
//! [`Spelled`](super::generate::Spelled) program calls one registration by keyword, by name, and by
//! name with its record's fields written in reverse; every spelling must load, fault or print
//! alike. A refusal's or a fault's message names the spelling, so only its kind is compared.

use proptest::prelude::*;

use super::generate::{Spelling, spelled_calls, spelled_calls_of};
use super::run;

/// What a program's run comes to: refused at load, faulted, or its exact output.
fn outcome(output: String) -> String {
    match output {
        refused if refused.starts_with("load: ") => "refused".to_string(),
        faulted if faulted.starts_with("error: ") => "faulted".to_string(),
        printed => printed,
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: crate::tests::case_share(1, 4),
        ..ProptestConfig::default()
    })]

    /// A call by name of a registration over one class runs as its keyworded call: the same
    /// value, the same solution, and so the same carried types.
    #[test]
    fn a_call_by_name_runs_as_its_keyworded_call(program in spelled_calls()) {
        let keyworded = program.render(Spelling::Keyworded);
        let by_name = program.render(Spelling::ByName);
        prop_assert_eq!(
            outcome(run(&keyworded)),
            outcome(run(&by_name)),
            "keyworded\n{}\nby name\n{}",
            keyworded,
            by_name
        );
    }

    /// A call by name is blind to the order its record's fields are written in.
    #[test]
    fn a_call_by_name_is_blind_to_its_field_order(program in spelled_calls_of(2)) {
        let by_name = program.render(Spelling::ByName);
        let reversed = program.render(Spelling::Reversed);
        prop_assert_eq!(
            outcome(run(&by_name)),
            outcome(run(&reversed)),
            "by name\n{}\nreversed\n{}",
            by_name,
            reversed
        );
    }
}
