//! Integration coverage for the cost-driven-copy bind seam's **pin** path over the two
//! identity-carrying composites, `Tagged` and `Wrapped` (`Scope::copy_delivered_substrate`). A
//! tagged / wrapped value whose payload holds a closure captured in its producer frame borrows its
//! home region, so the cost chooser pins it into the binding rather than rebuilding it: the bound
//! value shares the producer-resident payload substrate, kept live by the binding's stored reach.
//! Reading the value back after the producer frame retires proves the pin is sound — the
//! tagged/wrapped analog of `dict_bind_pin.rs`, and a tagged/wrapped-escape case in the
//! seam-equivalence battery (identical output under `seam-force-copy` / `seam-force-pin`).

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

/// `MAKE` returns a `Some` variant carrying a closure over `n`. The closure captures `n` from
/// `MAKE`'s frame, so the tagged value borrows its home region — the cost chooser selects `Pin` at
/// the `LET r` bind seam (exact via the borrows-home bit). The tagged value rides `MAKE`'s producer
/// region by hold; `PRINT r` reads it back after `MAKE` returned, exercising the pinned payload
/// substrate through the binding's `Kept`-minted stored reach.
#[test]
fn bound_tagged_holding_a_home_closure_pins_and_reads_back() {
    let out = run_capturing(concat!(
        "UNION Maybe = (Some :Any None :Null)\n",
        "FN (MAKE n :Number) -> :(Maybe) = (Maybe (Some (FN :{} -> Number = (n))))\n",
        "LET r = (MAKE 5)\n",
        "PRINT r",
    ))
    .expect("the program evaluates without error");
    assert_eq!(
        out, "Some(fn())\n",
        "the pinned tagged value reads back with its captured closure intact"
    );
}

/// A plain-data tagged value (owned scalar payload, no home borrow) that escapes via return and bind
/// takes the **copy** verb rather than the pin — the total rebuild homes the payload substrate at the
/// binding and frees the producer. The read-back output is identical to the pin case's shape, so the
/// two verbs are semantically invisible (the equivalence battery asserts this hardcoded output under
/// both forced policies).
#[test]
fn bound_plain_data_tagged_copies_and_reads_back() {
    let out = run_capturing(concat!(
        "UNION Maybe = (Some :Number None :Null)\n",
        "FN (MAKE n :Number) -> :(Maybe) = (Maybe (Some n))\n",
        "LET r = (MAKE 5)\n",
        "PRINT r",
    ))
    .expect("the program evaluates without error");
    assert_eq!(
        out, "Some(5)\n",
        "the copied tagged value reads back with its scalar payload intact"
    );
}

/// `MAKE` returns a `Holder` newtype wrapping a closure over `n`. The closure captures `n`, so the
/// wrapped value borrows its home region and the chooser selects `Pin` at the `LET r` bind seam. The
/// wrapped value rides `MAKE`'s producer region; `PRINT r` reads it back after `MAKE` returned.
#[test]
fn bound_wrapped_holding_a_home_closure_pins_and_reads_back() {
    let out = run_capturing(concat!(
        "NEWTYPE Holder = :Any\n",
        "FN (MAKE n :Number) -> :(Holder) = (Holder (FN :{} -> Number = (n)))\n",
        "LET r = (MAKE 5)\n",
        "PRINT r",
    ))
    .expect("the program evaluates without error");
    assert_eq!(
        out, "Holder(fn())\n",
        "the pinned wrapped value reads back with its captured closure intact"
    );
}

/// A plain-data wrapped value (owned scalar payload) escapes via return and bind and takes the
/// **copy** verb — the total rebuild homes the payload substrate at the binding. Identical output
/// shape to the pin case, so the two verbs are semantically invisible.
#[test]
fn bound_plain_data_wrapped_copies_and_reads_back() {
    let out = run_capturing(concat!(
        "NEWTYPE Holder = :Number\n",
        "FN (MAKE n :Number) -> :(Holder) = (Holder n)\n",
        "LET r = (MAKE 5)\n",
        "PRINT r",
    ))
    .expect("the program evaluates without error");
    assert_eq!(
        out, "Holder(5)\n",
        "the copied wrapped value reads back with its scalar payload intact"
    );
}
