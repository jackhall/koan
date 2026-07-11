//! `compile_fail` guards for the fold-closure capture-provenance discipline: an
//! ambient-lifetime reference cannot reach a fold's tied placement sink, and a `FoldingBrand`
//! cannot be minted outside a fold combinator. The fixtures under `tests/compile_fail/`
//! compile as external crates (seeing only koan's public API through `koan::fold_fixture`), so
//! each pins the *compile* error the discipline rests on. The committed `.stderr` files are the
//! ground truth — regenerate with `TRYBUILD=overwrite cargo test --test compile_fail`.

#[test]
fn fold_provenance_guards() {
    let t = trybuild::TestCases::new();
    // The AC's pin: an ambient `KType` fed to the tied type sink inside a fold closure.
    t.compile_fail("tests/compile_fail/fold_ambient_type.rs");
    // Object twin of the same tie.
    t.compile_fail("tests/compile_fail/fold_ambient_object.rs");
    // The positional half: no `FoldToken`, so `FoldingBrand`'s sole constructor is unreachable.
    t.compile_fail("tests/compile_fail/fold_brand_forge.rs");
}

/// The compiling twin of `fold_ambient_type`: an **operand-derived** `KType` cloned at the brand
/// (from a dep the combinator folds) satisfies the tied sink and seals cleanly.
#[test]
fn operand_derived_clone_at_brand_compiles() {
    use koan::fold_fixture::{deliver_type, drive_type_fold, store_folded_type, Carried};
    let dep = deliver_type();
    drive_type_fold(&[&dep], |brand, views| match views[0] {
        Carried::Type(kt) => store_folded_type(brand, kt.clone()),
        Carried::Object(_) => unreachable!("deliver_type yields a type terminal"),
    });
}
