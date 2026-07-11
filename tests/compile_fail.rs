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

/// `compile_fail` guard for the step-brand discipline (`scheduler-lifetime-tokens`): a Done-arm
/// `StepCarried` cannot be stashed past its construction step. The fixture compiles as an external
/// crate, seeing only `koan::step_fixture`, so it pins the *compile* error the brand rests on. The
/// committed `.stderr` is the ground truth — regenerate with
/// `TRYBUILD=overwrite cargo test --test compile_fail`.
///
/// The `StepCarried` type is nameable from an external crate (via `step_fixture`, mirroring how the
/// fold guards name `FoldingBrand`), but its constructor `born` (`pub(crate)`) and its sole exit
/// `seal_at_step` (`pub(super)`) are not — so a guard can neither forge nor unwrap the brand.
#[test]
fn step_brand_guard() {
    let t = trybuild::TestCases::new();
    // The AC's pin: stashing a step-branded carrier past its `for<'b>` step closure escapes the brand.
    t.compile_fail("tests/compile_fail/step_carrier_stash.rs");
    // The door half: a carrier obtained straight from a `StepAllocator` door — the shape a builtin
    // holds — is equally unstashable past the step.
    t.compile_fail("tests/compile_fail/step_allocator_stash.rs");
    // The unwrap half: the sole exit `seal_at_step` is `pub(super)`, unreachable from outside.
    t.compile_fail("tests/compile_fail/step_carrier_unwrap.rs");
}

/// The compiling twin of `step_carrier_stash`: using the step-branded carrier **within** its step
/// closure — never smuggling it out — satisfies the `for<'b>` bound and compiles.
#[test]
fn step_carrier_consumed_in_brand_compiles() {
    use koan::step_fixture::drive_step;
    let mut ran = 0;
    drive_step(|_carrier| {
        // Legal: the carrier stays inside its step brand; nothing escapes the closure.
        ran += 1;
    });
    assert_eq!(ran, 1);
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
