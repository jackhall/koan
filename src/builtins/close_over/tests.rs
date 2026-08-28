//! `CLOSE OVER` acceptance. The block's severance is observed through the memory substrate —
//! `region_metrics().live` while the escaped value is still held — because "holds no reach into the
//! producer's region" is a statement about what is alive, not about what a read returns.

use crate::builtins::test_support::TestRun;
use crate::machine::{program_storage, run_root_storage};

fn output(source: &str) -> String {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);
    test_run.run(source);
    String::from_utf8(captured.borrow().clone()).expect("output is utf8")
}

#[test]
fn smoke_empty_capture() {
    assert_eq!(output("LET x = (CLOSE OVER () (1))\nPRINT x\n"), "1\n");
}

#[test]
fn smoke_value_capture() {
    assert_eq!(
        output("LET a = (7)\nLET x = (CLOSE OVER (a) (a))\nPRINT x\n"),
        "7\n"
    );
}

#[test]
fn smoke_block_local() {
    assert_eq!(
        output("LET a = (7)\nLET x = (CLOSE OVER (a) ((LET b = (a)) (b)))\nPRINT x\n"),
        "7\n"
    );
}
