//! The retype law: a value retyped to a type lies within the static type the load gives it there,
//! at every retype site. Each drawn type and value under it is rendered once per site, after the
//! nominal types the draw may name ([`NOMINALS`]).
//!
//! The oracle is the evaluator's debug net, which checks that every evaluated node's value carries
//! a type within its static type; a release build checks only that each site runs without a fault.

use proptest::prelude::*;

use super::generate::{Desc, NOMINALS, ascribed, retyped_type, spelled, value_under};
use super::run;

/// A type a retype re-stamps values at, beside a value under it.
fn retyped() -> impl Strategy<Value = (Desc, String)> {
    retyped_type().prop_flat_map(|desc| {
        let value = value_under(&desc);
        (Just(desc), value)
    })
}

/// One program per retype site, each retyping `value` to `desc` and ending in an evaluated node
/// whose static type is the retype's: a bare name is read in place and never checked, so each is
/// wrapped in a list.
fn sites(desc: &Desc, value: &str) -> [String; 5] {
    let (spelled, ascribed) = (spelled(desc, &[]), ascribed(desc, &[]));
    [
        format!("PRINT [({value} :! {spelled})]"),
        format!("LET x {ascribed} = {value}\nPRINT [x]"),
        format!("LET f = (FN :{{x {ascribed}}} -> Any = #([x]))\nPRINT (f {{x = {value}}})"),
        // A quote's code reads a name from outside only through `$`.
        format!("LET v = {value}\nPRINT [(EVAL #($v) -> {spelled})]"),
        format!("EXPR #(GIVE x :Any) -> {spelled} = #({value})\nPRINT [(GIVE 1)]"),
    ]
    .map(|site| format!("{NOMINALS}\n{site}"))
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: crate::tests::case_share(1, 4),
        ..ProptestConfig::default()
    })]

    /// A value retyped to a type lies within the static type the load gives it there, at every
    /// retype site: the evaluator's `carried_under_static` net checks each evaluated node, and the
    /// program must print without a fault.
    #[test]
    fn a_retyped_value_lies_within_its_static_type((desc, value) in retyped()) {
        for source in sites(&desc, &value) {
            let output = run(&source);
            prop_assert!(
                !output.starts_with("load: ") && !output.contains("error: "),
                "{}\nin\n{}",
                output,
                source
            );
        }
    }
}

#[test]
fn every_retype_site_renders_as_source_that_runs() {
    let desc = Desc::Wrapped(Box::new(Desc::Dict(Box::new(Desc::Maybe))));
    let printed = "[:(Wrap {Type = :(MAP Str -> :(Some | None))})({\"a\": Some(1)})]";
    for source in sites(&desc, "(Wrap ({\"a\": (Maybe.Some 1)}))") {
        assert_eq!(run(&source), printed, "{source}");
    }
}
