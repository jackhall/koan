//! The narrowing law: the load's narrowing of a keyworded use, and its refusal of one, stand for
//! what selection over the use's full candidate list does at the call. Each drawn
//! [`Dispatched`](super::generate::Dispatched) program is run as loaded and again under
//! [`unnarrowed`], which leaves every use whole and refuses none.

use proptest::prelude::*;

use super::super::statics::unnarrowed;
use super::generate::{TIED, dispatched};
use super::run;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: crate::tests::case_share(1, 4),
        ..ProptestConfig::default()
    })]

    /// Static narrowing is transparent: a call over a narrowed candidate list runs what selection
    /// over the full list would, and a use the load refuses faults at every call within its
    /// arguments' static types.
    #[test]
    fn static_narrowing_is_transparent(program in dispatched()) {
        let rendered = program.render();
        let narrowed = run(&rendered.source);
        // A registration whose union ties is refused where it is declared: no use to compare.
        prop_assume!(!narrowed.contains(TIED), "{}", narrowed);
        let whole = unnarrowed(|| run(&rendered.source));
        if !narrowed.starts_with("load: ") {
            prop_assert_eq!(narrowed, whole, "in\n{}", rendered.source);
        } else {
            prop_assert!(
                narrowed.starts_with(&format!("load: {}: ", rendered.use_at)),
                "refused away from the use: {}\nin\n{}",
                narrowed,
                rendered.source
            );
            prop_assert!(
                whole.starts_with("error: ") && !whole.contains('\n'),
                "{} unnarrowed ran {:?}\nin\n{}",
                narrowed,
                whole,
                rendered.source
            );
        }
    }
}
