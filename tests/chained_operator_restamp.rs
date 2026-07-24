//! Integration coverage for the single escape seam: a declared return re-stamps in place in its
//! producer's region rather than relocating at the Done boundary. The headline program is a
//! substrate-returning operator chained three deep through a binding — the exact shape the deleted
//! relocation channel rejected, because a relocated envelope's host drifted from the value's
//! residence and the runtime residence audit spuriously fired
//! (`borrows a region not covered by dest, the supplied evidence, or the destination scope's
//! ambient coverage`). With re-stamp-in-place, host, producer pin, and residence coincide, so the
//! chain evaluates cleanly to the rightmost record.

use std::cell::RefCell;
use std::rc::Rc;

use koan::machine::interpret_with_writer;

/// Run `source`, capturing everything it PRINTs into a string.
fn run_capturing(source: &str) -> Result<String, koan::machine::KError> {
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    struct SharedBuf(Rc<RefCell<Vec<u8>>>);
    impl std::io::Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    interpret_with_writer(source, Box::new(SharedBuf(captured.clone())))?;
    let bytes = captured.borrow().clone();
    Ok(String::from_utf8(bytes).unwrap())
}

/// The `&` operator declares a `:{a :Number}` return and yields `right`, so `r1 & r2 & r3` is a
/// substrate-returning declared return chained three deep and bound as `chained`. Under the deleted
/// Done-boundary relocation channel this rejected with a residence-audit failure; re-stamp-in-place
/// keeps the value in its producer region so the chain evaluates without error and `chained` is the
/// rightmost record `{a = 3}`.
#[test]
fn chained_substrate_operator_restamps_in_place_without_residence_reject() {
    let out = run_capturing(concat!(
        "LET r1 = {a = 1}\n",
        "LET r2 = {a = 2}\n",
        "LET r3 = {a = 3}\n",
        "MODULE recs = ((OP #(&) OVER :{a :Number} = (right)) ",
        "(LET chained = (r1 & r2 & r3)) (PRINT chained))",
    ))
    .expect("the chained-operator program evaluates without a residence-audit rejection");
    assert_eq!(
        out, "{a = 3}\n",
        "the three-deep operator chain re-stamps in place and yields the rightmost record",
    );
}
