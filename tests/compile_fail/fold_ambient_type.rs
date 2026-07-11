//! The AC's pin: a fold closure cannot embed a reference captured from outside its operands.
//! `with_ambient_type` hands out a `KType` borrowed at a foreign ambient lifetime; feeding its
//! clone to the tied type sink inside a `for<'b>` fold closure is a lifetime mismatch — the
//! ambient `KType<'ambient>` cannot coerce to the brand's `KType<'b>`.

use koan::fold_fixture::{drive_type_fold, store_folded_type, with_ambient_type};

fn main() {
    with_ambient_type(|ambient| {
        drive_type_fold(&[], |brand, _views| store_folded_type(brand, ambient.clone()));
    });
}
