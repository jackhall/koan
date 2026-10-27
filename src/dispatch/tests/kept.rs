//! What a frame keeps: a carrier at the true home of each argument and of each binder whose
//! evaluation delivered a value pinned at a home of its own, so a frame that hands such a value
//! back — or a field of one — hands it on unchanged rather than building it again.

use std::cell::Cell;

use crate::scheduler::PASSED;
use crate::values::{BuildBudget, COPIED};

/// `SAME` returns its parameter, `ONWARD` a field of it, and `BOUND` a binder bound bare to it;
/// `MAKE` returns a record literal nesting another; `BIG` returns a top-level name, and `DATA` a
/// module's member.
const PASSING: &str = "LET big = [1 2 3]\n\
                       MODULE store = (LET data = [1 2 3])\n\
                       EXPR #(BIG x :Number) -> Any = #(big)\n\
                       EXPR #(DATA x :Number) -> Any = #(ATTR store data)\n\
                       EXPR #(SAME link :Link) -> Link = #(link)\n\
                       EXPR #(ONWARD link :Link) -> Any = #(ATTR link next)\n\
                       EXPR #(BOUND link :Link) -> Link = #((LET held = link) (held))\n\
                       EXPR #(MAKE x :Number) -> Any = #({count = x, inner = {deep = x}})\n";

/// What `main` printed over a chain of 3 links, how many finishes handed a kept value on
/// unchanged, and how many bytes copies laid.
fn ran(main: &str) -> (String, usize, usize) {
    PASSED.with(|passed| passed.set(0));
    COPIED.with(|copied| copied.set(0));
    let source = format!("{PASSING}{main}");
    let ran = super::caching::ran_under(3, &source, BuildBudget::default());
    (ran.printed, PASSED.with(Cell::get), COPIED.with(Cell::get))
}

#[test]
fn a_frame_hands_its_parameter_back_unchanged() {
    let (printed, passed, _) = ran("PRINT (ATTR (SAME chain) next)\n");
    let (expected, ..) = ran("PRINT (ATTR chain next)\n");
    assert_eq!(printed, expected);
    assert!(passed > 0, "`SAME` hands `link` on at its home");
}

#[test]
fn a_frame_hands_a_field_of_its_parameter_back_unchanged() {
    let (printed, passed, copied) = ran("PRINT (ONWARD (ONWARD chain))\n");
    let (expected, _, baseline) = ran("PRINT (ATTR (ATTR chain next) next)\n");
    assert_eq!(printed, expected);
    assert!(passed >= 2, "each `ONWARD` hands its field on at its home");
    assert_eq!(copied, baseline, "nothing of the chain is copied");
}

#[test]
fn a_binder_bound_bare_to_a_parameter_keeps_the_parameters_home() {
    let (printed, passed, _) = ran("PRINT (ATTR (BOUND chain) next)\n");
    let (expected, ..) = ran("PRINT (ATTR chain next)\n");
    assert_eq!(printed, expected);
    assert!(
        passed >= 2,
        "the binding and the frame's value both hand `link` on"
    );
}

#[test]
fn a_record_a_call_returns_is_built_once_in_its_callers_home() {
    let (printed, _, copied) = ran("PRINT (ATTR (ATTR (MAKE 1) inner) deep)\n");
    let (expected, _, baseline) =
        ran("PRINT (ATTR (ATTR {count = 1, inner = {deep = 1}} inner) deep)\n");
    assert_eq!(printed, expected);
    assert_eq!(
        copied, baseline,
        "neither record is copied out of the frame's evaluation"
    );
}

#[test]
fn a_frame_hands_a_top_level_name_back_unchanged() {
    let (printed, passed, copied) = ran("PRINT (BIG 1)\n");
    let (expected, _, baseline) = ran("PRINT big\n");
    assert_eq!(printed, expected);
    assert!(passed > 0, "`BIG` hands `big` on at its home");
    assert_eq!(copied, baseline, "nothing of `big` is copied");
}

#[test]
fn a_frame_hands_a_module_member_back_unchanged() {
    let (printed, passed, copied) = ran("PRINT (DATA 1)\n");
    let (expected, _, baseline) = ran("PRINT (ATTR store data)\n");
    assert_eq!(printed, expected);
    assert!(passed > 0, "`DATA` hands `data` on at its home");
    assert_eq!(copied, baseline, "nothing of `data` is copied");
}
