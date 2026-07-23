//! Integration coverage for the cost-driven-copy bind seam's **pin** path
//! (`Scope::copy_delivered_record`). A bare record holding a closure captured in its producer frame
//! borrows its home region, so the cost chooser pins it into the binding rather than rebuilding it:
//! the bound value shares the producer-resident substrate, kept live by the binding's stored reach.
//! Reading the record back after the producer frame retires proves the pin is sound and the field
//! values are correct.

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

/// `MAKE` returns a bare record `{v = n, get = <closure over n>}`. The `get` closure captures `n`
/// from `MAKE`'s frame, so the record borrows its home region — the cost chooser selects `Pin` at
/// the `LET r` bind seam (exact via the borrows-home bit, independent of the size ratio). The record
/// rides `MAKE`'s producer region by hold; `PRINT r` reads it back with `v = 5` intact after `MAKE`
/// returned, exercising the pinned substrate through the binding's `Kept`-minted stored reach.
#[test]
fn bound_bare_record_holding_a_home_closure_pins_and_reads_back() {
    let out = run_capturing(concat!(
        "FN (MAKE n :Number) -> :{v :Number} = ({v = n, get = (FN :{} -> Number = (n))})\n",
        "LET r = (MAKE 5)\n",
        "PRINT r",
    ))
    .expect("the program evaluates without error");
    assert_eq!(
        out, "{v = 5, get = fn()}\n",
        "the pinned record reads back with its field value and captured closure intact"
    );
}
