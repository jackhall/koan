//! **Content digests**: what identifies a value for the life of the loaded program, by what it
//! holds and never by where it sits.
//!
//! A [`ContentDigest`] is the low 128 bits of a BLAKE3 hash. The recipe is one rule throughout: a
//! digest is a domain [`Tag`], then the value's own scalar payload, then its parts' digests.
//!
//! A digest is computed on demand and never stored on a value: only a view asks for one, so a value
//! no module captures never pays for it. A demand walks what the value reaches, through one
//! [`Digests`] memo, so a part shared many times over is digested once per demand.
//!
//! - A scalar digests from its payload: a number from its bits, a bool, `null`, a string
//!   length-prefixed, a type value from its handle's bits.
//! - A list, dict, record and tagged value digest as its kind's tag, its type's handle and its
//!   **contents** — a list its cells' digests in order, a dict each key's digest then each cell's,
//!   in key order, a record each field's name and cell digest in symbol order, so field order is
//!   blind, and a tagged value its payload's.
//! - A knot member's digest is its knot's, beside its index there: a knot digests its node count
//!   and each node's content in index order, an edge to a sibling hashed as its index, so values
//!   that reach one another — closures that call one another, a ring of containers — digest as the
//!   one knot they are tied into.
//!
//! The digest covers the carried type and every cell, hidden ones included, so two values no reader
//! can tell apart may digest apart. That over-distinction is sound: a digest keys what is equal by
//! content, and keying two equal things apart costs only a key.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// A value's content identity: the low 128 bits of a BLAKE3 hash of its content, held as bytes so a
/// node storing one keeps a word's alignment.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ContentDigest([u8; 16]);

impl ContentDigest {
    /// A placeholder for a digest not yet computed.
    pub const NONE: ContentDigest = ContentDigest([0; 16]);

    /// The digest's bits, for an identity keyed on content outside `values` — a carrier's.
    pub fn bits(self) -> u128 {
        u128::from_le_bytes(self.0)
    }
}

/// The domain tag every digest begins with, one per digestible shape, so no two shapes share a
/// digest even with identical payloads. Never reorder or reuse one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Tag {
    Number = 0x01,
    Bool = 0x02,
    Null = 0x03,
    Str = 0x04,
    Type = 0x05,
    List = 0x06,
    Dict = 0x07,
    Record = 0x08,
    Tagged = 0x09,
    /// A run of digests: a container's contents.
    Contents = 0x0A,
    /// An opaque view's carrier key: its source's digest and its application.
    Carrier = 0x0B,
    /// A knot: its node count and each node's content in index order.
    Knot = 0x10,
    /// One member of a knot: the knot's digest and the member's index.
    Member = 0x11,
    /// An edge inside a knot, by index.
    Edge = 0x12,
    Builtin = 0x13,
    Function = 0x14,
    Module = 0x15,
    View = 0x16,
    Barrier = 0x17,
    Code = 0x18,
    Data = 0x19,
    /// A body's code, digested as resolved, where its shape is built.
    Shape = 0x20,
    /// A name read in a body's own activation or a block it hops out of.
    Local = 0x21,
    /// A name bound at the program's top level.
    Top = 0x22,
    /// A name a body captures from outside it, short of the top level.
    Capture = 0x23,
    /// A name a quote's code leaves open, or one an `EVAL` offers.
    Open = 0x24,
    Keyword = 0x25,
    /// A keyworded use and its candidates.
    Use = 0x26,
    /// A spread candidate: a list of functions read at a coordinate.
    Spread = 0x27,
    /// A part that is a nested body, by its code digest.
    Nested = 0x28,
    /// A container literal, or a group of parts.
    Group = 0x29,
    /// A type part the load typed, by its handle.
    Typed = 0x2A,
    /// A type part the load left unknown, by its syntax.
    Untyped = 0x2B,
    /// A mark on a name or a use, by which mark.
    Mark = 0x2C,
    /// A view's operator: `:!`, `:|`, or the re-view a nested module takes at an opaque boundary.
    Transparent = 0x2D,
    Opaque = 0x2E,
    Reviewed = 0x2F,
}

/// The one hasher every digest is computed through: a tag first, then payload, then each part's
/// digest, every integer fed little-endian. It is a [`Hasher`] too, so a lattice handle or a symbol
/// feeds its own bits through its `Hash`.
pub struct DigestHasher(blake3::Hasher);

impl DigestHasher {
    /// A hasher for a digest of the shape `tag`.
    pub fn new(tag: Tag) -> DigestHasher {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&[tag as u8]);
        DigestHasher(hasher)
    }

    /// Feed a tag inside a run: the shape of the part that follows.
    pub fn tag(&mut self, tag: Tag) -> &mut Self {
        self.0.update(&[tag as u8]);
        self
    }

    /// Feed `value`'s bits through its `Hash`: a lattice handle's digest, a symbol's bits.
    pub fn feed(&mut self, value: impl Hash) -> &mut Self {
        value.hash(self);
        self
    }

    /// Feed a count or an index.
    pub fn count(&mut self, count: usize) -> &mut Self {
        self.0.update(&(count as u64).to_le_bytes());
        self
    }

    /// Feed a part's digest.
    pub fn digest(&mut self, digest: ContentDigest) -> &mut Self {
        self.0.update(&digest.0);
        self
    }

    /// Feed a run of bytes, length-prefixed.
    pub fn text(&mut self, text: &[u8]) -> &mut Self {
        self.count(text.len());
        self.0.update(text);
        self
    }

    /// The digest of everything fed.
    pub fn finished(&self) -> ContentDigest {
        let bytes = self.0.finalize();
        let mut low = [0u8; 16];
        low.copy_from_slice(&bytes.as_bytes()[..16]);
        ContentDigest(low)
    }
}

impl Hasher for DigestHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    /// The low 64 bits of the digest, for a reader that asks a `Hasher` for one.
    fn finish(&self) -> u64 {
        self.finished().bits() as u64
    }
}

/// The contents a run of part digests makes: their count, then each in order.
pub fn contents(parts: impl ExactSizeIterator<Item = ContentDigest>) -> ContentDigest {
    let mut hasher = DigestHasher::new(Tag::Contents);
    hasher.count(parts.len());
    for part in parts {
        hasher.digest(part);
    }
    hasher.finished()
}

/// A composite's digest: its kind's tag, its type's handle, and its stored contents.
pub fn composite(
    tag: Tag,
    ktype: crate::type_lattice::KType,
    contents: ContentDigest,
) -> ContentDigest {
    DigestHasher::new(tag)
        .feed(ktype)
        .digest(contents)
        .finished()
}

/// One demand's memo: the digest of each part a walk has already met, keyed by the shape it
/// digests as and the address of the resident it sits in. Nothing a walk reads is freed or moved
/// while it runs, so an address names one part of a shape for the walk's life, and the shape
/// keeps two kinds of resident that could share an address apart.
#[derive(Default)]
pub struct Digests(HashMap<(u8, usize), ContentDigest>);

impl Digests {
    /// The digest of the part resident at `at`, digested as `tag`: `compute`'s answer, the first
    /// time this demand meets it.
    pub fn memo<T>(
        &mut self,
        tag: Tag,
        at: &T,
        compute: impl FnOnce(&mut Digests) -> ContentDigest,
    ) -> ContentDigest {
        let key = (tag as u8, std::ptr::from_ref(at).addr());
        if let Some(digest) = self.0.get(&key) {
            return *digest;
        }
        let digest = compute(self);
        self.0.insert(key, digest);
        digest
    }
}
