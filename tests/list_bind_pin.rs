//! Integration coverage for the cost-driven-copy bind seam's **pin** path over a **list**
//! (`Scope::copy_delivered_substrate`). A bare list holding a closure captured in its producer frame
//! borrows its home region, so the cost chooser pins it into the binding rather than rebuilding it:
//! the bound value shares the producer-resident element substrate, kept live by the binding's stored
//! reach. Reading the list back after the producer frame retires proves the pin is sound and the
//! element values are correct — the list analog of `record_bind_pin.rs`, and a list-escape case in
//! the seam-equivalence battery (identical output under `seam-force-copy` / `seam-force-pin`).

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

/// `MAKE` returns a bare list `[n, <closure over n>]`. The closure captures `n` from `MAKE`'s frame,
/// so the list borrows its home region — the cost chooser selects `Pin` at the `LET r` bind seam
/// (exact via the borrows-home bit). The list rides `MAKE`'s producer region by hold; `PRINT r` reads
/// it back with `5` intact after `MAKE` returned, exercising the pinned substrate through the
/// binding's `Kept`-minted stored reach.
#[test]
fn bound_bare_list_holding_a_home_closure_pins_and_reads_back() {
    let out = run_capturing(concat!(
        "FN (MAKE n :Number) -> :(LIST OF Any) = ([n (FN :{} -> Number = (n))])\n",
        "LET r = (MAKE 5)\n",
        "PRINT r",
    ))
    .expect("the program evaluates without error");
    assert_eq!(
        out, "[5, fn()]\n",
        "the pinned list reads back with its scalar element and captured closure intact"
    );
}

/// A plain-data list (owned scalars, no home borrow) that escapes via return and bind takes the
/// **copy** verb rather than the pin — the total rebuild homes the element substrate at the binding
/// and frees the producer. The read-back output is identical to the pin case's shape, so the two
/// verbs are semantically invisible (the equivalence battery asserts this hardcoded output under both
/// forced policies).
#[test]
fn bound_plain_data_list_copies_and_reads_back() {
    let out = run_capturing(concat!(
        "FN (MAKE n :Number) -> :(LIST OF Number) = ([n, n])\n",
        "LET r = (MAKE 5)\n",
        "PRINT r",
    ))
    .expect("the program evaluates without error");
    assert_eq!(
        out, "[5, 5]\n",
        "the copied list reads back with its scalar elements intact"
    );
}
