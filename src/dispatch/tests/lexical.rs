//! The lexical-variable law: a site naming a quantified variable by a type resolves it to one type
//! through whatever lies between it and the variable's home. Each drawn
//! [`Lexical`](super::generate::Lexical) program is run with its contexts and without.

use proptest::prelude::*;

use super::generate::lexical;
use super::run;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: crate::tests::case_share(1, 4),
        ..ProptestConfig::default()
    })]

    /// A lexical variable reads the same through any context: a contribution or instance site
    /// naming one by a type resolves to one type whether a called `FN`, a hoisting block, a
    /// returned closure, a module or a quote's code lies between it and the variable's home.
    #[test]
    fn a_lexical_variable_reads_the_same_through_any_context(case in lexical()) {
        let bare = run(&case.render(&[]));
        prop_assert!(
            !bare.starts_with("load: ") && !bare.contains("error: ") && !bare.contains('\n'),
            "the bare site ran {:?}\nin\n{}",
            bare,
            case.render(&[])
        );
        let wrapped = case.render(&case.contexts);
        prop_assert_eq!(run(&wrapped), bare, "in\n{}", wrapped);
    }
}
