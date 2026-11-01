//! The seed image, pinned: the checked-in file is what the seed's recipe generates, every digest
//! in it is its static content's, and a root resting on it holds only what it adds.

use crate::bump::Bump;
use crate::symbols::SymbolInterner;
use crate::types::SEED;
use crate::types::chain::TypeChain;
use crate::types::handle::KType;
use crate::types::image::{check, compare_or_update, emit};
use crate::types::tiers::RootBuilder;

/// The command that regenerates the seed, which its header names.
const REGENERATE: &str = "UPDATE_IMAGE=1 cargo test -p lattice the_seed_image_is_fresh";

/// The checked-in seed.
const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/types/image/seed.rs");

/// The seed as its recipe builds it: `Never` and `Any`, then every fixed handle's node.
#[test]
fn the_seed_image_is_fresh() {
    let bump = Bump::new();
    let builder = RootBuilder::bare(&bump);
    TypeChain::building(builder).seed_constants(&bump);
    let generated = emit(
        builder.freeze(),
        &SymbolInterner::new(),
        "super",
        REGENERATE,
    );
    compare_or_update(PATH, &generated, REGENERATE);
}

#[test]
fn the_seed_image_reads_back_as_written() {
    let written = emit(&SEED, &SymbolInterner::new(), "super", REGENERATE);
    compare_or_update(PATH, &written, REGENERATE);
}

#[test]
fn every_seed_digest_is_its_content() {
    assert_eq!(check(&SEED), Vec::<String>::new());
}

#[test]
fn a_root_resting_on_the_seed_holds_only_what_it_adds() {
    let bump = Bump::new();
    let builder = RootBuilder::over(&bump, &SEED);
    let types = TypeChain::building(builder);
    assert_eq!(builder.len(), 0);
    let number = types.list(KType::NUMBER);
    assert_eq!(builder.len(), 1);
    let frozen = builder.freeze();
    assert_eq!(frozen.len(), 1);
    assert!(frozen.find(KType::NUMBER.digest()).is_some());
    assert!(frozen.find(number.digest()).is_some());
}
