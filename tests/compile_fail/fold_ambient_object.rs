//! Object twin of `fold_ambient_type`: an ambient-lifetime `KObject` deep-cloned into the tied
//! object sink inside a `for<'b>` fold closure is the same lifetime mismatch — `KObject<'ambient>`
//! cannot coerce to the brand's `KObject<'b>`.

use koan::fold_fixture::{drive_object_fold, store_folded_object, with_ambient_object};

fn main() {
    with_ambient_object(|ambient| {
        drive_object_fold(&[], |brand, _views| {
            store_folded_object(brand, ambient.deep_clone())
        });
    });
}
