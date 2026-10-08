//! **Content digests**: what identifies a value for the life of the loaded program, by what it
//! holds and never by where it sits.
//!
//! A [`ContentDigest`] is the low 128 bits of a BLAKE3 hash. The recipe is one rule throughout: a
//! digest is a domain [`Tag`], then the value's own scalar payload, then its parts' digests.
//!
//! A value digests as what [the door](super::surface) shows of it at the type it is seen at,
//! exactly as the [deep copy](super::crossing) lays it down, so a copy digests as its source and a
//! cell its type hides counts for nothing:
//!
//! - A scalar digests from its payload: a number from its bits, a bool, `null`, a string
//!   length-prefixed, a type value from its handle's bits.
//! - A list, dict, record and tagged value, and a data node seen at a type other than its memo,
//!   digest as its kind's tag, its seen type's handle and its **contents**: each part the door
//!   shows, at its type there — a list its elements in order, a dict each key's digest then each
//!   cell's, in key order, a record each shown field's name and cell in symbol order, so field
//!   order is blind, and a tagged value its payload at its representation.
//! - A knot member seen at its own memo digests as its knot's, beside its index there: a knot
//!   digests its node count and each node's content in index order — a data node read through the
//!   door at its memo, an edge to a sibling hashed as its index — so values that reach one another
//!   digest as the one knot they are tied into. Its layer decides what a node's content is: the
//!   member [lists](super::Knotted::held) the values its knot holds and
//!   [hashes](super::Knotted::digest_held) the knot over their digests.
//!
//! The seen type is part of the recipe, so a retype changes the digest. Inside a knot each node is
//! digested at its own memo, so a cell a tagged node's representation hides still counts there, as
//! a copy of the knot keeps it. That over-distinction is sound: a digest keys what is equal by
//! content, and keying two equal things apart costs only a key.
//!
//! A digest is computed on demand and never stored on a value: only a view asks for one, so a value
//! no module captures never pays for it. A demand walks what the value reaches through one
//! [`Digests`] memo over the demand's scratch, so a part shared many times over, and a knot met
//! through any of its members, is digested once per demand. The walk runs over an explicit stack,
//! as the copy does: a composite is a frame over the parts its surface shows, and a knot member a
//! frame over the values its knot holds, so neither a value's depth nor a chain of knots grows the
//! call stack.

use std::hash::{Hash, Hasher};

use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, Edge, bump_table};
use crate::type_lattice::{DeclaredType, KType, TypeRegistry};

use super::circular::{Circular, Resolved};
use super::surface::Parts;
use super::{Knotted, Nothing, Seen, Surface, Value};

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
/// digest even with identical payloads. Never reorder or reuse one. The scalar tags also open a
/// scalar literal inside a body's code digest, which hashes a literal as syntax.
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
fn contents(parts: impl ExactSizeIterator<Item = ContentDigest>) -> ContentDigest {
    let mut hasher = DigestHasher::new(Tag::Contents);
    hasher.count(parts.len());
    for part in parts {
        hasher.digest(part);
    }
    hasher.finished()
}

/// A composite's digest: its kind's tag, its seen type's handle, and the contents it shows there.
fn composite(tag: Tag, ktype: KType, contents: ContentDigest) -> ContentDigest {
    DigestHasher::new(tag)
        .feed(ktype)
        .digest(contents)
        .finished()
}

/// One demand's memo, beside the registry its walk reads types in and the scratch it stages over:
/// the digest of each composite a walk has already met, keyed by the shape it digests as, the
/// address of the resident it sits in, and the type it is seen at, since one node seen at two types
/// shows two surfaces; and the digest of each knot it has met, keyed by the knot's root member.
/// Nothing a walk reads is freed or moved while it runs, so an address names one part of a shape
/// for the walk's life, and the shape keeps two kinds of resident that could share an address
/// apart.
pub struct Digests<'a, 'run, X> {
    types: &'a TypeRegistry<'run>,
    scratch: BumpAllocator<'a>,
    composites: BumpBackedMap<'a, MemoKey, ContentDigest>,
    knots: BumpBackedMap<'a, X, ContentDigest>,
}

/// What a composite's memo entry is keyed on: the shape's tag, the resident's address, the seen
/// type.
type MemoKey = (u8, usize, DeclaredType<KType>);

/// One pending frame of a digest walk: how many of its parts have been started, and where its
/// finished parts begin on the walk's `done` stack.
#[derive(Clone, Copy)]
enum Frame<'x, 'cell, X> {
    /// A list, dict, record or tagged value, or a data node seen at a type other than its memo,
    /// opened at the type it is seen at, beside its memo key.
    Composite {
        surface: Surface<'x, 'cell, X>,
        key: MemoKey,
        next: usize,
        base: usize,
    },
    /// A knot member seen at its own memo whose knot this demand has not digested: the values its
    /// knot holds sit on the walk's `held` stack from `held` to `end`.
    Knot {
        member: X,
        held: usize,
        end: usize,
        next: usize,
        base: usize,
    },
}

/// One walk's stacks: its pending frames, the finished digests of their parts, and the values each
/// pending knot holds, as its member listed them.
struct Stacks<'a, 'cell, X> {
    frames: BumpVec<'a, Frame<'a, 'cell, X>>,
    done: BumpVec<'a, ContentDigest>,
    held: BumpVec<'a, Seen<'cell, X>>,
}

impl<'a, 'run, X: Knotted> Digests<'a, 'run, X> {
    /// A demand's memo, reading types in `types` and staging its walks and its tables over
    /// `scratch`.
    pub fn new(types: &'a TypeRegistry<'run>, scratch: BumpAllocator<'a>) -> Self {
        Digests {
            types,
            scratch,
            composites: bump_table(scratch),
            knots: bump_table(scratch),
        }
    }

    /// The digest of `top`, read through the door at the type it is seen at. A composite is a
    /// frame over the parts its surface shows, and a knot member seen at its own memo a frame over
    /// the values its knot [holds](Knotted::held), so neither a value's depth nor a chain of knots
    /// grows the call stack.
    pub(super) fn seen<'cell>(&mut self, top: Seen<'cell, X>) -> ContentDigest
    where
        X: 'cell,
    {
        let mut stacks = Stacks {
            frames: BumpVec::new_in(self.scratch),
            done: BumpVec::new_in(self.scratch),
            held: BumpVec::new_in(self.scratch),
        };
        if let Some(leaf) = self.start(top, &mut stacks) {
            return leaf;
        }
        while let Some(&frame) = stacks.frames.last() {
            match self.next_part(frame, &stacks) {
                Some(part) => {
                    if let Some(Frame::Composite { next, .. } | Frame::Knot { next, .. }) =
                        stacks.frames.last_mut()
                    {
                        *next += 1;
                    }
                    if let Some(leaf) = self.start(part, &mut stacks) {
                        stacks.done.push(leaf);
                    }
                }
                None => {
                    stacks.frames.pop();
                    let finished = self.finish(frame, &mut stacks);
                    if stacks.frames.is_empty() {
                        return finished;
                    }
                    stacks.done.push(finished);
                }
            }
        }
        unreachable!("a started composite or knot pushes a frame")
    }

    /// Begin digesting `seen`: a leaf, or a composite or knot this demand already digested, is
    /// finished at once; any other composite or knot member pushes its frame. A data node seen at a
    /// type other than its memo digests as the plain value of its kind a copy lays down, so it
    /// pushes a composite frame, not its knot's.
    fn start<'cell>(
        &mut self,
        seen: Seen<'cell, X>,
        stacks: &mut Stacks<'a, 'cell, X>,
    ) -> Option<ContentDigest>
    where
        X: 'cell,
    {
        let base = stacks.done.len();
        let value = seen.value();
        match value {
            Value::List(_) | Value::Dict(_) | Value::Record(_) | Value::Tagged(_) => {}
            Value::Knotted(member) if member.ktype() != seen.ktype() => {}
            Value::Knotted(member) => {
                if let Some(knot) = self.knots.get(&member.root()) {
                    return Some(beside_index(*knot, member));
                }
                let held = stacks.held.len();
                member.held(self.types, self.scratch, &mut |part| stacks.held.push(part));
                stacks.frames.push(Frame::Knot {
                    member,
                    held,
                    end: stacks.held.len(),
                    next: 0,
                    base,
                });
                return None;
            }
            leaf => return Some(scalar(leaf)),
        }
        let (tag, address) = resident(value);
        let key = (tag as u8, address, seen.ktype());
        if let Some(digest) = self.composites.get(&key) {
            return Some(*digest);
        }
        let surface = seen
            .surface(self.types, self.scratch)
            .expect("a container, a tagged value or a data node opens");
        stacks.frames.push(Frame::Composite {
            surface,
            key,
            next: 0,
            base,
        });
        None
    }

    /// The next part `frame` has not started, if any.
    fn next_part<'cell>(
        &self,
        frame: Frame<'a, 'cell, X>,
        stacks: &Stacks<'a, 'cell, X>,
    ) -> Option<Seen<'cell, X>>
    where
        X: 'cell,
    {
        match frame {
            Frame::Composite { surface, next, .. } => {
                (next < surface.len()).then(|| surface.child(next, self.types, self.scratch))
            }
            Frame::Knot {
                held, end, next, ..
            } => (held + next < end).then(|| stacks.held[held + next]),
        }
    }

    /// `frame`'s digest over its finished parts, memoized, and its parts popped: a composite's
    /// kind, seen type and contents; a knot member's knot, from its member's
    /// [`digest_held`](Knotted::digest_held), beside its index there.
    fn finish<'cell>(
        &mut self,
        frame: Frame<'a, 'cell, X>,
        stacks: &mut Stacks<'a, 'cell, X>,
    ) -> ContentDigest
    where
        X: 'cell,
    {
        let (base, finished) = match frame {
            Frame::Composite {
                surface, key, base, ..
            } => {
                let digest = composed(&surface, &stacks.done[base..]);
                self.composites.insert(key, digest);
                (base, digest)
            }
            Frame::Knot {
                member, held, base, ..
            } => {
                let mut parts = stacks.done[base..].iter().copied();
                let knot = member.digest_held(self.types, self.scratch, &mut || {
                    parts
                        .next()
                        .expect("digest_held asks for each held value once")
                });
                debug_assert!(
                    parts.next().is_none(),
                    "digest_held asks for every held value"
                );
                self.knots.insert(member.root(), knot);
                stacks.held.truncate(held);
                (base, beside_index(knot, member))
            }
        };
        stacks.done.truncate(base);
        finished
    }
}

/// A composite's digest over its finished `parts`: its kind, its seen type and its contents.
fn composed<X: Knotted>(surface: &Surface<'_, '_, X>, parts: &[ContentDigest]) -> ContentDigest {
    let (tag, contents) = match surface.parts() {
        Parts::List { .. } => (Tag::List, contents(parts.iter().copied())),
        Parts::Dict { .. } => {
            let mut hasher = DigestHasher::new(Tag::Contents);
            hasher.count(parts.len());
            for (at, part) in parts.iter().enumerate() {
                hasher
                    .digest(scalar(surface.key(at).value::<Nothing>()))
                    .digest(*part);
            }
            (Tag::Dict, hasher.finished())
        }
        Parts::Record { .. } => {
            let mut hasher = DigestHasher::new(Tag::Contents);
            hasher.count(parts.len());
            for (at, part) in parts.iter().enumerate() {
                hasher.feed(surface.name(at)).digest(*part);
            }
            (Tag::Record, hasher.finished())
        }
        Parts::Tagged { .. } => (Tag::Tagged, parts[0]),
    };
    composite(tag, surface.ktype(), contents)
}

/// A knot member's digest: its knot's, beside its index there.
fn beside_index<X: Knotted>(knot: ContentDigest, member: X) -> ContentDigest {
    DigestHasher::new(Tag::Member)
        .digest(knot)
        .count(member.index().index() as usize)
        .finished()
}

/// An edge's digest inside its knot's: its index, since it names a sibling the knot's digest
/// covers.
pub fn edge(edge: Edge) -> ContentDigest {
    DigestHasher::new(Tag::Edge)
        .count(edge.index() as usize)
        .finished()
}

/// A leaf's digest, from its payload. Only a scalar, a string or a type value is one.
pub(super) fn scalar<X>(value: Value<'_, X>) -> ContentDigest {
    match value {
        Value::Number(number) => DigestHasher::new(Tag::Number)
            .feed(number.to_bits())
            .finished(),
        Value::Bool(flag) => DigestHasher::new(Tag::Bool).feed(flag).finished(),
        Value::Null => DigestHasher::new(Tag::Null).finished(),
        Value::Str(text) => DigestHasher::new(Tag::Str).text(text.as_bytes()).finished(),
        Value::Type(value) => DigestHasher::new(Tag::Type).feed(value.handle()).finished(),
        _ => unreachable!("a composite or a knot member is no leaf"),
    }
}

/// The kind's tag and the address of the resident `value` opens as: a plain composite's own, or
/// its data node's.
fn resident<X: Knotted>(value: Value<'_, X>) -> (Tag, usize) {
    fn at<T>(resident: &T) -> usize {
        std::ptr::from_ref(resident).addr()
    }
    match value {
        Value::List(list) => (Tag::List, at(list)),
        Value::Dict(dict) => (Tag::Dict, at(dict)),
        Value::Record(record) => (Tag::Record, at(record)),
        Value::Tagged(tagged) => (Tag::Tagged, at(tagged)),
        Value::Knotted(member) => match member.resolve() {
            Resolved::Circular(Circular::List(list)) => (Tag::List, at(list)),
            Resolved::Circular(Circular::Dict(dict)) => (Tag::Dict, at(dict)),
            Resolved::Circular(Circular::Record(record)) => (Tag::Record, at(record)),
            Resolved::Circular(Circular::Tagged(tagged)) => (Tag::Tagged, at(tagged)),
            _ => unreachable!("only a data node opens"),
        },
        _ => unreachable!("a leaf opens no resident"),
    }
}
