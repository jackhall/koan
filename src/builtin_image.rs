//! The **builtin image**: every type the builtin table names, generated once and compiled into the
//! binary as a frozen tier resting on the lattice's seed, so a load's root rests on it and builds
//! no builtin type. It holds the shape type of each overload the table registers — each native
//! overload's, and each dispatched [`BUILTIN_SHAPES`](crate::parse::BUILTIN_SHAPES) entry's — and
//! the builtin `Result` and `Error`, each canonicalized and sealed once, at generation, with the
//! texts of the names they hold. It carries no verdict. See
//! [identity.md § Storage](../lattice/src/types/identity.md#storage-a-root-and-its-branches).
//!
//! The load reads the handles it needs from the constants the generated file names — [`RESULT`],
//! [`ERROR`], [`ERROR_VARIANTS`] in [`FaultKind::ALL`](crate::scope::FaultKind::ALL)'s order,
//! [`OVERLOAD_TYPES`] in the table's order and [`SHAPE_TYPES`] by
//! [`BuiltinShapeId`](crate::parse::BuiltinShapeId) — and runs no recipe: a recipe digests its
//! content before it can probe, and `Error`'s digest needs its union canonicalized. The recipes,
//! `elaborate`'s `builtin_result`, `builtin_error` and `builtin_shape_types` and dispatch's
//! `overload_types`, are compiled for tests alone, where the generator calls them.
//!
//! The generator is `the_builtin_image_is_fresh` in this module's tests, which builds the tier
//! through the recipes over the seed and writes it through the lattice's emitter. A change to a
//! builtin type, a fault kind or an overload leaves the generated file compiling; regenerate it
//! with `UPDATE_IMAGE=1 cargo test -p koan --lib the_builtin_image_is_fresh`. A change to the
//! stored layout stops it compiling: regenerate the seed first, then this image with
//! `--features bootstrap-image` added, which compiles the hand-written [`stub`] in the generated
//! file's place ([the lattice's image module](../lattice/src/types/image.rs) § Regenerating).

#[cfg(test)]
mod tests;

#[cfg(not(feature = "bootstrap-image"))]
#[rustfmt::skip]
mod generated;
#[cfg(not(feature = "bootstrap-image"))]
pub use generated::{ERROR, ERROR_VARIANTS, IMAGE, OVERLOAD_TYPES, RESULT, SHAPE_TYPES, SYMBOLS};
#[cfg(feature = "bootstrap-image")]
pub use stub::{ERROR, ERROR_VARIANTS, IMAGE, OVERLOAD_TYPES, RESULT, SHAPE_TYPES, SYMBOLS};
#[doc(hidden)]
pub mod stub;

// The load indexes the variants by fault kind and the shape types by shape id.
#[cfg(not(feature = "bootstrap-image"))]
const _: () = {
    assert!(ERROR_VARIANTS.len() == crate::scope::FaultKind::ALL.len());
    assert!(SHAPE_TYPES.len() == crate::parse::BUILTIN_SHAPES.len());
};
