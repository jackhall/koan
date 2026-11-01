//! The builtin image's hand-written stand-in, compiled in the generated file's place under the
//! `bootstrap-image` feature so the generator runs when the generated file does not compile: an
//! empty image over the seed. A load over it fails, since no builtin type resolves; only the
//! generator is meant to run under the feature. Always compiled, so a layout change that breaks it
//! is seen at once.

use crate::type_lattice::image::*;

pub static IMAGE: Frozen<'static> = Frozen::imaged_over(FrozenTable::EMPTY, &seed::IMAGE);
pub static SYMBOLS: StaticTexts = StaticTexts::imaged(FrozenTable::EMPTY);
pub const RESULT: KType = KType::NEVER;
pub const ERROR: KType = KType::NEVER;
pub const ERROR_VARIANTS: &[KType] = &[];
pub const OVERLOAD_TYPES: &[KType] = &[];
pub const SHAPE_TYPES: &[&[KType]] = &[];
