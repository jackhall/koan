//! The seed's hand-written stand-in, compiled in the generated seed's place under the
//! `bootstrap-image` feature, so the seed's generator runs when the generated file does not
//! compile. It holds `Never` and `Any` alone, which every chain reads as its bottom and top; a load
//! over it fails, since nothing else resolves. Always compiled, so a layout change that breaks it
//! is seen at once. See [image.rs](../image.rs) § Regenerating.

use super::super::handle::Handle;
use super::*;

pub static NEVER: StoredNode<'static> = StoredNode::imaged(Handle::NEVER.bits(), Content::Never);

pub static ANY: StoredNode<'static> = StoredNode::imaged(Handle::ANY.bits(), Content::Any);

pub static IMAGE: Frozen<'static> = Frozen::imaged_root(FrozenTable::EMPTY, &NEVER, &ANY);
