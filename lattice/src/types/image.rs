//! A **generated image**: a frozen tier written as Rust `static`s and compiled into the binary, so
//! no load builds what it holds. The lattice's own [`seed`] holds every fixed handle's node and
//! rests on nothing; koan's builtin image rests on the seed, and a load's root rests on koan's
//! ([identity.md § Storage](identity.md#storage-a-root-and-its-branches)).
//!
//! Each type node is its own `static StoredNode<'static>`, its children references to other
//! statics, which the linker relocates; a sealed component is a `static Component`, and each text
//! a `static StaticText`. The tier's slot run is a `static` array copied from a run
//! [`FrozenTable::placed`] lays, so a probe of the image is a probe of any frozen table. A
//! generated file names the stored layout through this module's re-exports and the
//! `#[doc(hidden)]` `imaged` constructors beside each type, which check at compile time what a
//! digest cannot: that a node lies in the root, that a text classifies as its class.
//!
//! The generator ([image/generator.rs](image/generator.rs)) is compiled for tests alone — the
//! lattice's, and koan's through the `generator` feature. `emit` writes a file from a tier a
//! recipe built through the chain, so every digest and every canonical shape is the chain's. It
//! matches [`Content`] exhaustively — the fourth conversion table beside `Content::lay`,
//! `StoredNode::view_as` and `Content::transfer` — writing only the variants a builtin image holds
//! and panicking on the rest. `check` recomputes each node's digest from its static content, and
//! `check_texts` each text's symbol, since BLAKE3 is not `const`. Each image's golden test
//! regenerates its file and fails when the checked-in one is stale.
//!
//! **Regenerating.** A change to a digest recipe, a flag or a builtin type leaves both generated
//! files compiling: run the fixed-handle golden first if the recipe moved, then
//!
//! ```sh
//! UPDATE_IMAGE=1 cargo test -p lattice the_seed_image_is_fresh
//! UPDATE_IMAGE=1 cargo test -p koan --lib the_builtin_image_is_fresh
//! ```
//!
//! A change to the layout a generated file spells — a field of a `Content` variant, an `imaged`
//! constructor, a re-export's name — stops it compiling. Update `emit`, fix a stub by hand if it
//! no longer compiles, then regenerate through the stubs, the seed first, with
//! `--features bootstrap-image` on each command above. The feature compiles each image's
//! hand-written stub ([`seed_stub`] here) in its generated file's place.

pub use crate::bump::{FrozenTable, Slot};
pub use crate::symbols::{
    BinderSymbol, KeywordSymbol, StaticText, StaticTexts, TypeSymbol, ValueSymbol,
};

pub use super::component::Component;
pub use super::digest::{ComponentMember, TypeDigest};
pub use super::handle::{KType, Parametric};
pub use super::kind::KKind;
pub use super::node::NodeSchema;
pub use super::record::Record;
pub use super::run::{Elements, Run};
pub use super::schema::{SigOrigin, SigSchema};
pub use super::shape::StoredElement;
pub use super::stored::{Content, Flags, StoredNode};
pub use super::tiers::Frozen;

#[cfg(not(feature = "bootstrap-image"))]
#[rustfmt::skip]
pub mod seed;
#[cfg(feature = "bootstrap-image")]
pub use self::seed_stub as seed;
#[doc(hidden)]
pub mod seed_stub;

#[cfg(any(test, feature = "generator"))]
mod generator;
#[cfg(any(test, feature = "generator"))]
pub use generator::{check, check_texts, compare_or_update, emit, static_name};
